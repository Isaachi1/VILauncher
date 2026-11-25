//! The pictures of the efforts on PortWatch VI: the GitHub avatar of each repository's owner,
//! or the picture the site keeps for a build with no public code.
//!
//! The window loads nothing from other sites (its CSP is `'self'`), so the launcher downloads
//! each picture here, keeps it in the cache folder for a week and hands it to the window as a
//! `data:` URL. Only small PNG, JPEG, GIF or WebP files are accepted, checked by their first
//! bytes rather than by what the server says; anything else is dropped and the window shows
//! the initials instead. A picture that can't be downloaded falls back to the cached copy, even
//! an old one.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{net, paths};

/// Where the site's own pictures (`assets/img/…` in `tracker.json`) are served from.
pub const SITE_ASSETS: &str = "https://raw.githubusercontent.com/4lwayz/portwatch-vi/main/";
const MAX_BYTES: u64 = 512 * 1024;
const FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
const PARALLEL: usize = 8;

/// One picture to fetch: an effort's id and its picture's URL.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Wanted {
    pub id: String,
    pub url: String,
}

/// The URL of an effort's picture: the site's own (a path under `assets/img/`), else the
/// GitHub avatar of the repository's owner, else none.
pub fn url_for(avatar: Option<&str>, repo: Option<&str>) -> Option<String> {
    if let Some(path) = avatar {
        let safe = path.strip_prefix("assets/img/").is_some_and(|rest| {
            !rest.is_empty() && !rest.contains("..") && rest.chars().all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c))
        });
        if safe {
            return Some(format!("{SITE_ASSETS}{path}"));
        }
    }
    let owner = repo?.split_once('/')?.0;
    let valid = !owner.is_empty() && owner.len() <= 39 && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| format!("https://avatars.githubusercontent.com/{owner}?s=88"))
}

pub fn cache_dir() -> PathBuf {
    paths::cache_dir().join("avatars")
}

/// The pictures as `data:` URLs, by effort id. Efforts whose picture couldn't be had are left out.
pub fn fetch_all(wanted: &[Wanted]) -> HashMap<String, String> {
    fetch_all_with(wanted, &cache_dir(), download)
}

pub fn fetch_all_with(wanted: &[Wanted], dir: &Path, get: impl Fn(&str) -> Option<Vec<u8>> + Sync) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for chunk in wanted.chunks(PARALLEL) {
        let got: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk.iter().map(|w| s.spawn(|| (w.id.clone(), one(&w.url, dir, &get)))).collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        out.extend(got.into_iter().filter_map(|(id, uri)| uri.map(|u| (id, u))));
    }
    out
}

fn one(url: &str, dir: &Path, get: &impl Fn(&str) -> Option<Vec<u8>>) -> Option<String> {
    let file = dir.join(hex(&Sha256::digest(url.as_bytes())));
    let cached = fs::read(&file).ok().filter(|b| mime(b).is_some());
    let fresh = fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age < FRESH);
    let bytes = match cached {
        Some(b) if fresh => b,
        cached => match get(url).filter(|b| mime(b).is_some() && b.len() as u64 <= MAX_BYTES) {
            Some(b) => {
                let _ = fs::create_dir_all(dir);
                let _ = fs::write(&file, &b);
                b
            }
            None => cached?,
        },
    };
    Some(format!("data:{};base64,{}", mime(&bytes)?, base64(&bytes)))
}

fn download(url: &str) -> Option<Vec<u8>> {
    let resp = net::agent(Duration::from_secs(10)).get(url).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_BYTES).then_some(bytes)
}

/// The image type, from the file's first bytes.
fn mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

    #[test]
    fn urls() {
        assert_eq!(url_for(Some("assets/img/efforts/suri.jpg"), None).unwrap(), format!("{SITE_ASSETS}assets/img/efforts/suri.jpg"));
        assert_eq!(url_for(None, Some("Marvisak/VIctory")).unwrap(), "https://avatars.githubusercontent.com/Marvisak?s=88");
        // Anything else in `avatar` is ignored, and the owner is used.
        assert_eq!(url_for(Some("https://evil.example/x.png"), Some("a-b/c")).unwrap(), "https://avatars.githubusercontent.com/a-b?s=88");
        assert_eq!(url_for(Some("assets/img/../../x"), None), None);
        assert_eq!(url_for(None, Some("bad owner/x")), None);
        assert_eq!(url_for(None, None), None);
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn fetches_caches_and_drops_what_is_not_an_image() {
        let t = tempfile::tempdir().unwrap();
        let wanted = [
            Wanted { id: "a".into(), url: "u/a".into() },
            Wanted { id: "html".into(), url: "u/html".into() },
            Wanted { id: "gone".into(), url: "u/gone".into() },
        ];
        let got = fetch_all_with(&wanted, t.path(), |url| match url {
            "u/a" => Some(PNG.to_vec()),
            "u/html" => Some(b"<html>".to_vec()),
            _ => None,
        });
        assert_eq!(got.len(), 1);
        assert_eq!(got["a"], format!("data:image/png;base64,{}", base64(PNG)));

        // Offline: the cached copy is used.
        let again = fetch_all_with(&wanted[..1], t.path(), |_| None);
        assert_eq!(again["a"], got["a"]);
    }

    #[test]
    fn too_big_is_dropped() {
        let t = tempfile::tempdir().unwrap();
        let mut big = PNG.to_vec();
        big.resize(MAX_BYTES as usize + 1, 0);
        let got = fetch_all_with(&[Wanted { id: "a".into(), url: "u".into() }], t.path(), |_| Some(big.clone()));
        assert!(got.is_empty());
    }
}

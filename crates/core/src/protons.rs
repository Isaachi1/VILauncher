//! Proton versions the launcher manages by itself, without Steam.
//!
//! GE-Proton comes from the releases of GloriousEggroll/proton-ge-custom on GitHub:
//! each release has `GE-Proton<N>-<M>.tar.gz` and a `.sha512sum` file next to it. The
//! archive is downloaded only from that repository's release folder, its size must be
//! the one GitHub lists and its SHA-512 the one in the checksum file, and it is
//! unpacked next to the other versions and moved in only once complete.
//!
//! Versions live in `~/.local/share/vilauncher/protons/<name>`, where
//! [`runners::find_protons`](crate::runners::find_protons) finds them along with the
//! ones Steam installed (Proton Experimental, GE-Proton in compatibilitytools.d). Only
//! the launcher's own versions can be removed here; Steam's stay Steam's.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

use crate::config::Config;
use crate::runners::{self, ProtonInstall};
use crate::{Error, Result, net, paths};

pub const GE_REPO: &str = "GloriousEggroll/proton-ge-custom";
const DOWNLOAD_PREFIX: &str = "https://github.com/GloriousEggroll/proton-ge-custom/releases/download/";
/// No GE-Proton archive is this large; a bigger download is refused.
const MAX_ARCHIVE: u64 = 2 << 30;

pub fn releases_url() -> String {
    std::env::var("VILAUNCHER_GE_PROTON_API")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("https://api.github.com/repos/{GE_REPO}/releases?per_page=15"))
}

/// Where the launcher keeps the Proton versions it downloaded.
pub fn managed_dir() -> PathBuf {
    paths::data_dir().join("protons")
}

/// A GE-Proton release that can be downloaded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Release {
    pub name: String,
    pub published_at: Option<String>,
    /// The `.tar.gz` archive.
    pub file: String,
    pub url: String,
    pub size: u64,
    /// The `.sha512sum` file next to it.
    pub checksum_url: String,
    /// Already in the launcher's folder.
    pub installed: bool,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// A name that can be a folder name: letters, digits, `.`, `_`, `-`, not starting with `.`.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// The GitHub API's list of releases, keeping the ones with an archive and a checksum
/// from the GE-Proton repository itself; newest first, as GitHub lists them.
pub fn parse_releases(text: &str, installed: &[String]) -> Result<Vec<Release>> {
    let list: Vec<GhRelease> = serde_json::from_str(text).map_err(|e| Error::Config(format!("GE-Proton releases: {e}")))?;
    let mut out = Vec::new();
    for r in list.into_iter().filter(|r| !r.draft && !r.prerelease && plain_name(&r.tag_name)) {
        let ours = |a: &&GhAsset| a.browser_download_url.starts_with(&format!("{DOWNLOAD_PREFIX}{}/", r.tag_name));
        let Some(tar) = r.assets.iter().filter(ours).find(|a| a.name.ends_with(".tar.gz") && plain_name(&a.name)) else { continue };
        let Some(sum) = r.assets.iter().filter(ours).find(|a| a.name.ends_with(".sha512sum")) else { continue };
        out.push(Release {
            installed: installed.contains(&r.tag_name),
            name: r.tag_name,
            published_at: r.published_at,
            file: tar.name.clone(),
            url: tar.browser_download_url.clone(),
            size: tar.size,
            checksum_url: sum.browser_download_url.clone(),
        });
    }
    Ok(out)
}

/// The names of the versions in the launcher's folder.
pub fn installed_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(managed_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("proton").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| plain_name(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The GE-Proton releases on GitHub, marking the ones already downloaded.
pub fn available() -> Result<Vec<Release>> {
    let url = releases_url();
    let text = net::get_text(&url, Duration::from_secs(20))?;
    parse_releases(&text, &installed_names())
}

/// The SHA-512 the checksum file names for `file` (`<hex>  <file>`, as sha512sum writes it).
pub fn parse_checksum(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next().map(|n| n.trim_start_matches('*'));
        let name = name.map(|n| n.rsplit('/').next().unwrap_or(n));
        let ok = hash.len() == 128 && hash.chars().all(|c| c.is_ascii_hexdigit()) && name.is_none_or(|n| n == file);
        ok.then(|| hash.to_ascii_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Copies `src` into `dest`, at most `size` bytes, checking the size and the SHA-512.
pub fn save_verified(mut src: impl Read, dest: &Path, size: u64, sha512: &str, progress: &mut dyn FnMut(u64)) -> Result<()> {
    if size > MAX_ARCHIVE {
        return Err(Error::Config(format!("{}: {size} bytes is too large for Proton", dest.display())));
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut out = fs::File::create(dest)?;
    let mut h = Sha512::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > size {
            let _ = fs::remove_file(dest);
            return Err(Error::Config(format!("{}: larger than the {size} bytes GitHub lists", dest.display())));
        }
        h.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        progress(done);
    }
    let got = hex(&h.finalize());
    if done != size || !got.eq_ignore_ascii_case(sha512) {
        let _ = fs::remove_file(dest);
        return Err(Error::Config(format!(
            "{}: the download does not match its checksum ({done} of {size} bytes); nothing was installed",
            dest.display()
        )));
    }
    out.sync_all()?;
    Ok(())
}

/// Unpacks a verified GE-Proton archive (one top folder holding `proton`) into
/// `root/<name>`, through a hidden staging folder. Returns the new folder.
pub fn unpack(archive: &Path, root: &Path, name: &str) -> Result<PathBuf> {
    if !plain_name(name) {
        return Err(Error::Config(format!("{name:?} is not a Proton name")));
    }
    fs::create_dir_all(root)?;
    let staging = root.join(format!(".{name}.new"));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let result = (|| {
        let gz = flate2::read::GzDecoder::new(fs::File::open(archive)?);
        let mut tar = tar::Archive::new(gz);
        // Proton's scripts and binaries must stay executable.
        tar.set_preserve_permissions(true);
        // `unpack` refuses entries that would land outside `staging` (`..`, absolute paths).
        tar.unpack(&staging).map_err(|e| Error::Config(format!("{}: {e}", archive.display())))?;
        let tops: Vec<PathBuf> = fs::read_dir(&staging)?.flatten().map(|e| e.path()).collect();
        let top = match tops.as_slice() {
            [one] if one.is_dir() && one.join("proton").is_file() => one.clone(),
            _ if staging.join("proton").is_file() => staging.clone(),
            _ => return Err(Error::Config(format!("{}: no Proton folder (with a `proton` script) inside", archive.display()))),
        };
        let dest = root.join(name);
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        fs::rename(&top, &dest)?;
        Ok(dest)
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Downloads, checks and installs a GE-Proton release. `name` is a release name
/// (`GE-Proton10-25`) or `latest`. `progress` gets (bytes done, total).
pub fn install(name: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<ProtonInstall> {
    let releases = available()?;
    let rel = if name == "latest" { releases.first() } else { releases.iter().find(|r| r.name == name) };
    let rel = rel.ok_or_else(|| Error::Config(format!("no GE-Proton release named {name} on GitHub")))?;
    install_release(rel, progress)
}

pub fn install_release(rel: &Release, progress: &mut dyn FnMut(u64, u64)) -> Result<ProtonInstall> {
    for url in [&rel.url, &rel.checksum_url] {
        if !url.starts_with(DOWNLOAD_PREFIX) {
            return Err(Error::Config(format!("{url} is not a GE-Proton release download")));
        }
    }
    let sums = net::get_text(&rel.checksum_url, Duration::from_secs(30))?;
    let sha512 = parse_checksum(&sums, &rel.file)
        .ok_or_else(|| Error::Config(format!("{}: no SHA-512 for {} in it", rel.checksum_url, rel.file)))?;
    let resp = net::agent(Duration::from_secs(1800)).get(&rel.url).call().map_err(|e| net::error(&rel.url, e))?;
    let archive = paths::cache_dir().join("downloads").join(&rel.file);
    let total = rel.size;
    let saved = save_verified(resp.into_reader(), &archive, total, &sha512, &mut |n| progress(n, total));
    let result = saved.and_then(|_| unpack(&archive, &managed_dir(), &rel.name));
    let _ = fs::remove_file(&archive);
    let path = result?;
    Ok(ProtonInstall { name: rel.name.clone(), version: runners::proton_version(&path), path, origin: runners::Origin::Launcher })
}

fn same(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => paths::normalize(a) == paths::normalize(b),
    }
}

/// Removes one of the launcher's own Proton versions. The one the configuration
/// uses is refused: choose another first.
pub fn remove(name: &str, cfg: &Config) -> Result<()> {
    remove_from(&managed_dir(), name, cfg)
}

pub fn remove_from(root: &Path, name: &str, cfg: &Config) -> Result<()> {
    if !plain_name(name) {
        return Err(Error::Config(format!("{name:?} is not a Proton name")));
    }
    let dir = root.join(name);
    if !dir.join("proton").is_file() {
        return Err(Error::Config(format!("{name} is not a Proton version VILauncher installed (only those can be removed here)")));
    }
    if cfg.runner_path().is_some_and(|p| same(&p, &dir)) {
        return Err(Error::Config(format!("{name} is the Proton in use: choose another one before removing it")));
    }
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, TempHome};

    fn release_json(tag: &str, repo: &str) -> String {
        let base = format!("https://github.com/{repo}/releases/download/{tag}");
        format!(
            r#"{{"tag_name": "{tag}", "published_at": "2026-11-20T10:00:00Z", "draft": false, "prerelease": false,
                "assets": [{{"name": "{tag}.sha512sum", "size": 150, "browser_download_url": "{base}/{tag}.sha512sum"}},
                           {{"name": "{tag}.tar.gz", "size": 500, "browser_download_url": "{base}/{tag}.tar.gz"}}]}}"#
        )
    }

    #[test]
    fn releases_come_only_from_ge_proton() {
        let text = format!(
            "[{}, {}, {}, {}]",
            release_json("GE-Proton10-25", GE_REPO),
            release_json("GE-Proton10-24", "someone/else"),
            release_json("../evil", GE_REPO),
            r#"{"tag_name": "GE-Proton10-23", "assets": []}"#
        );
        let list = parse_releases(&text, &["GE-Proton10-25".into()]).unwrap();
        assert_eq!(list.len(), 1);
        let r = &list[0];
        assert_eq!((r.name.as_str(), r.file.as_str(), r.size, r.installed), ("GE-Proton10-25", "GE-Proton10-25.tar.gz", 500, true));
        assert!(r.checksum_url.ends_with("/GE-Proton10-25/GE-Proton10-25.sha512sum"));
    }

    #[test]
    fn checksum_file_is_read() {
        let h = "a".repeat(128);
        assert_eq!(parse_checksum(&format!("{h}  GE-Proton10-25.tar.gz\n"), "GE-Proton10-25.tar.gz"), Some(h.clone()));
        assert_eq!(parse_checksum(&format!("{h} *./out/GE-Proton10-25.tar.gz"), "GE-Proton10-25.tar.gz"), Some(h.clone()));
        assert_eq!(parse_checksum(&format!("{h}  other.tar.gz"), "GE-Proton10-25.tar.gz"), None);
        assert_eq!(parse_checksum("abc  GE-Proton10-25.tar.gz", "GE-Proton10-25.tar.gz"), None);
    }

    #[test]
    fn download_must_match_size_and_sha512() {
        let t = tempfile::tempdir().unwrap();
        let data = b"proton bytes".to_vec();
        let sha = hex(&Sha512::digest(&data));
        let dest = t.path().join("d/p.tar.gz");
        save_verified(&data[..], &dest, data.len() as u64, &sha, &mut |_| {}).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(save_verified(&data[..], &dest, data.len() as u64, &"0".repeat(128), &mut |_| {}).is_err());
        assert!(!dest.exists(), "a bad download is deleted");
        assert!(save_verified(&data[..], &dest, 3, &sha, &mut |_| {}).is_err());
        assert!(save_verified(&data[..], &dest, 99, &sha, &mut |_| {}).is_err());
    }

    fn proton_tarball(t: &Path, name: &str) -> PathBuf {
        let src = testutil::fake_proton(&t.join("src"), name);
        let path = t.join(format!("{name}.tar.gz"));
        let gz = flate2::write::GzEncoder::new(fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        b.append_dir_all(name, &src).unwrap();
        b.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn unpack_installs_an_executable_proton() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let archive = proton_tarball(t.path(), "GE-Proton10-25");
        let root = t.path().join("protons");
        let dir = unpack(&archive, &root, "GE-Proton10-25").unwrap();
        assert_eq!(dir, root.join("GE-Proton10-25"));
        let mode = fs::metadata(dir.join("proton")).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "proton stays executable");
        assert!(runners::vkd3d_x64_dir(&dir).is_some());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1, "no staging folder left");

        let note = t.path().join("note.txt");
        fs::write(&note, "hi").unwrap();
        let bad = t.path().join("bad.tar.gz");
        let gz = flate2::write::GzEncoder::new(fs::File::create(&bad).unwrap(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        b.append_path_with_name(&note, "x/note.txt").unwrap();
        b.into_inner().unwrap().finish().unwrap();
        assert!(unpack(&bad, &root, "GE-Proton1-1").is_err());
        assert!(!root.join("GE-Proton1-1").exists());
        assert!(unpack(&archive, &root, "../x").is_err());
    }

    #[test]
    fn managed_versions_are_found_and_removed() {
        let h = TempHome::new();
        testutil::fake_proton(&managed_dir(), "GE-Proton10-25");
        testutil::fake_proton(&managed_dir(), "GE-Proton10-20");
        let steam = testutil::fake_proton(&h.home().join(".local/share/Steam/steamapps/common"), "Proton - Experimental");
        let found = runners::find_protons(&h.home());
        let names: Vec<(&str, runners::Origin)> = found.iter().map(|p| (p.name.as_str(), p.origin)).collect();
        assert_eq!(
            names,
            [
                ("GE-Proton10-25", runners::Origin::Launcher),
                ("GE-Proton10-20", runners::Origin::Launcher),
                ("Proton - Experimental", runners::Origin::Steam)
            ]
        );
        assert_eq!(installed_names(), ["GE-Proton10-20", "GE-Proton10-25"]);

        let mut cfg = Config::default();
        cfg.runner.path = Some(managed_dir().join("GE-Proton10-25"));
        assert!(remove("GE-Proton10-25", &cfg).unwrap_err().to_string().contains("in use"));
        remove("GE-Proton10-20", &cfg).unwrap();
        assert!(!managed_dir().join("GE-Proton10-20").exists());
        assert!(remove("Proton - Experimental", &cfg).is_err(), "Steam's Proton is not ours to remove");
        assert!(remove("../Steam", &cfg).is_err());
        assert!(steam.exists());
    }
}

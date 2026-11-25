//! Checks the game's files against known SHA-256 values: the executable, every time the game
//! folder changes and before playing (when `game.verify` is on), and a .zip on request.
//!
//! A file that does not match is not refused: it can be another version. The check only says
//! whether the file is the one these values describe, so a changed or damaged file shows.
//!
//! Hashing a large executable takes seconds, so the result is kept per file in
//! `~/.cache/vilauncher/hashes.json`, with the file's size and modification time: the hash is
//! computed again only when either changes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::{Result, paths, update};

/// What a known file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The game's executable, as the launcher runs it.
    Exe,
    /// The .zip the game's files came in.
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Known {
    pub kind: Kind,
    pub name: &'static str,
    pub sha256: &'static str,
}

/// The files the launcher knows, by SHA-256.
pub const KNOWN: &[Known] = &[
    Known {
        kind: Kind::Exe,
        name: "Grand Theft Auto VI",
        sha256: "e95020b2fa4a721db3df59fdecf75621730ac559d8835252f6c3b658d5a08993",
    },
    Known {
        kind: Kind::Zip,
        name: "Grand Theft Auto VI",
        sha256: "7545a2f7fb4b399cf8850987f087ed9a4d8499ddb6d6c2d642fd60c27f569730",
    },
];

/// The known file with this SHA-256, if any.
pub fn identify(sha256: &str) -> Option<Known> {
    KNOWN.iter().find(|k| k.sha256.eq_ignore_ascii_case(sha256)).copied()
}

/// The SHA-256 the launcher expects for a file of this kind.
pub fn expected(kind: Kind) -> Vec<&'static str> {
    KNOWN.iter().filter(|k| k.kind == kind).map(|k| k.sha256).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub path: PathBuf,
    pub sha256: String,
    /// The known file it is, or `None` when the SHA-256 is not one the launcher knows.
    pub known: Option<Known>,
    /// The hash came from the cache (the file did not change since it was computed).
    pub cached: bool,
}

impl Verdict {
    /// The file is the known one of this kind.
    pub fn is(&self, kind: Kind) -> bool {
        self.known.is_some_and(|k| k.kind == kind)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    size: u64,
    modified: u128,
    sha256: String,
}

/// At most this many files are remembered.
const CACHE_ENTRIES: usize = 32;

fn cache_file() -> PathBuf {
    paths::cache_dir().join("hashes.json")
}

fn stamp(path: &Path) -> Result<(u64, u128)> {
    let meta = fs::metadata(path)?;
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    Ok((meta.len(), modified))
}

/// The SHA-256 of `path`, from `cache` when the file has the same size and modification time,
/// computed (and stored in `cache`) otherwise.
fn hash_with(path: &Path, cache: &mut BTreeMap<String, Entry>) -> Result<(String, bool)> {
    let key = path.to_string_lossy().into_owned();
    let (size, modified) = stamp(path)?;
    if let Some(e) = cache.get(&key).filter(|e| e.size == size && e.modified == modified) {
        return Ok((e.sha256.clone(), true));
    }
    let sha256 = update::sha256_file(path)?;
    cache.insert(key, Entry { size, modified, sha256: sha256.clone() });
    while cache.len() > CACHE_ENTRIES {
        let first = cache.keys().next().cloned();
        if let Some(k) = first {
            cache.remove(&k);
        }
    }
    Ok((sha256, false))
}

fn verify_with(path: &Path, cache_path: &Path) -> Result<Verdict> {
    let mut cache: BTreeMap<String, Entry> =
        fs::read_to_string(cache_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let (sha256, cached) = hash_with(path, &mut cache)?;
    if !cached {
        // Losing the cache only costs a second hash later; the verdict does not depend on it.
        if let Some(dir) = cache_path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(cache_path, serde_json::to_string(&cache).unwrap_or_default());
    }
    Ok(Verdict { path: path.to_path_buf(), known: identify(&sha256), sha256, cached })
}

/// Hashes `path` (or takes the remembered hash) and says which known file it is.
pub fn verify(path: &Path) -> Result<Verdict> {
    verify_with(path, &cache_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_files() {
        let exe = identify("E95020B2FA4A721DB3DF59FDECF75621730AC559D8835252F6C3B658D5A08993").unwrap();
        assert_eq!((exe.kind, exe.name), (Kind::Exe, "Grand Theft Auto VI"));
        assert_eq!(identify("7545a2f7fb4b399cf8850987f087ed9a4d8499ddb6d6c2d642fd60c27f569730").unwrap().kind, Kind::Zip);
        assert_eq!(identify(&"0".repeat(64)), None);
        assert_eq!(expected(Kind::Exe), ["e95020b2fa4a721db3df59fdecf75621730ac559d8835252f6c3b658d5a08993"]);
        for k in KNOWN {
            assert!(k.sha256.len() == 64 && k.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
    }

    #[test]
    fn hashes_once_until_the_file_changes() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("game.exe");
        let cache = t.path().join("cache/hashes.json");
        fs::write(&file, b"abc").unwrap();
        let v = verify_with(&file, &cache).unwrap();
        // SHA-256("abc")
        assert_eq!(v.sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!((v.known, v.cached), (None, false));
        assert!(!v.is(Kind::Exe));
        assert!(verify_with(&file, &cache).unwrap().cached, "same size and time: from the cache");
        fs::write(&file, b"abcd").unwrap();
        let v = verify_with(&file, &cache).unwrap();
        assert_eq!((v.sha256.as_str(), v.cached), ("88d4266fd4e6338d13b845fcf289579d209c897823b9217da3e161936f031589", false));
        // A cache that is not JSON is ignored, not an error.
        fs::write(&cache, "{").unwrap();
        assert!(!verify_with(&file, &cache).unwrap().cached);
    }
}

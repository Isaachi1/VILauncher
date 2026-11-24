//! Keeps VIctory and XCL up to date from this repository's own builds.
//!
//! The `build` workflow compiles `victory.exe` and `xcl.exe` from the newest
//! commits of LPortVI/VIctory and LPortVI/xcl every night and publishes them on
//! the `runtimes` release of LPortVI/VILauncher, with a `runtimes.json` that
//! names each archive, the commit it was built from, its size and its SHA-256.
//!
//! Only that release is used: an archive is downloaded only from the folder the
//! manifest itself came from, its SHA-256 must match, and it is unpacked next to
//! the running build and swapped in only once it is complete. Nothing from other
//! projects (including the ones PortWatch VI lists) is ever downloaded.
//!
//! An installed build lives in `~/.local/share/vilauncher/runtimes/<backend>`,
//! with the `VICTORY_COMMIT.txt` / `XCL_COMMIT.txt` the workflow writes.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backends::{MACHINE_X86_64, pe_machine};
use crate::config::BackendKind;
use crate::{Error, Result, net, paths};

pub const MANIFEST_URL: &str = "https://github.com/LPortVI/VILauncher/releases/download/runtimes/runtimes.json";

/// No runtime archive is this large; a bigger download is refused.
const MAX_ARCHIVE: u64 = 2 << 30;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Manifest {
    pub schema: u32,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub run_url: Option<String>,
    #[serde(default)]
    pub victory: Option<Asset>,
    #[serde(default)]
    pub xcl: Option<Asset>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Asset {
    /// The archive's file name, next to `runtimes.json`.
    pub file: String,
    pub sha256: String,
    pub size: u64,
    pub commit: String,
    /// `owner/name` of the repository it was built from.
    #[serde(default)]
    pub repo: Option<String>,
}

impl Manifest {
    pub fn asset(&self, kind: BackendKind) -> Option<&Asset> {
        match kind {
            BackendKind::Victory => self.victory.as_ref(),
            BackendKind::Xcl => self.xcl.as_ref(),
        }
    }
}

pub fn exe_name(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Victory => "victory.exe",
        BackendKind::Xcl => "xcl.exe",
    }
}

fn commit_file(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Victory => "VICTORY_COMMIT.txt",
        BackendKind::Xcl => "XCL_COMMIT.txt",
    }
}

pub fn manifest_url() -> String {
    std::env::var("VILAUNCHER_RUNTIMES_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| MANIFEST_URL.into())
}

pub fn runtimes_dir() -> PathBuf {
    paths::data_dir().join("runtimes")
}

pub fn install_dir(kind: BackendKind) -> PathBuf {
    runtimes_dir().join(kind.name())
}

pub fn parse_manifest(text: &str) -> Result<Manifest> {
    let m: Manifest = serde_json::from_str(text).map_err(|e| Error::Config(format!("runtimes.json: {e}")))?;
    if m.schema != 1 {
        return Err(Error::Config(format!("runtimes.json: schema {} is newer than this launcher; update VILauncher", m.schema)));
    }
    for a in [&m.victory, &m.xcl].into_iter().flatten() {
        let plain = !a.file.is_empty() && a.file.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) && !a.file.starts_with('.');
        if !plain {
            return Err(Error::Config(format!("runtimes.json: unexpected file name {:?}", a.file)));
        }
        if a.sha256.len() != 64 || !a.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Config(format!("runtimes.json: bad SHA-256 for {}", a.file)));
        }
    }
    Ok(m)
}

/// The archive's URL: always in the same folder as the manifest.
pub fn asset_url(manifest_url: &str, asset: &Asset) -> String {
    let base = manifest_url.rsplit_once('/').map_or(manifest_url, |(b, _)| b);
    format!("{base}/{}", asset.file)
}

/// The commit of the build in `dir`, from the file the workflow writes
/// (`xcl <commit>` on the first line).
pub fn installed_commit(kind: BackendKind, dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(commit_file(kind))).ok()?;
    let first = text.lines().next()?;
    first.split_whitespace().nth(1).map(str::to_owned)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeStatus {
    pub backend: BackendKind,
    /// The launcher-managed build, if one is installed.
    pub installed: Option<String>,
    pub exe: Option<PathBuf>,
    pub latest: Option<Asset>,
    pub update: bool,
}

pub fn status_from(manifest: Option<&Manifest>, root: &Path) -> Vec<RuntimeStatus> {
    [BackendKind::Victory, BackendKind::Xcl]
        .into_iter()
        .map(|kind| {
            let dir = root.join(kind.name());
            let installed = installed_commit(kind, &dir);
            let exe = Some(dir.join(exe_name(kind))).filter(|e| e.is_file());
            let latest = manifest.and_then(|m| m.asset(kind)).cloned();
            let update = latest.as_ref().is_some_and(|l| installed.as_deref() != Some(l.commit.as_str()) || exe.is_none());
            RuntimeStatus { backend: kind, installed, exe, latest, update }
        })
        .collect()
}

fn fetch_manifest_from(url: &str) -> Result<Manifest> {
    match net::get_text_opt(url, Duration::from_secs(20))? {
        Some(text) => parse_manifest(&text),
        None => Err(Error::Config(format!(
            "no runtimes published yet at {url}: the build workflow publishes them from main"
        ))),
    }
}

pub fn fetch_manifest() -> Result<Manifest> {
    fetch_manifest_from(&manifest_url())
}

/// What is installed and what the newest builds are.
pub fn status() -> Result<Vec<RuntimeStatus>> {
    let m = fetch_manifest()?;
    Ok(status_from(Some(&m), &runtimes_dir()))
}

pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    Ok(hex(&h.finalize()))
}

/// Copies `src` into `dest`, at most `asset.size` bytes, and checks the size and
/// the SHA-256 on the way. `progress` gets the bytes written so far.
pub fn save_verified(mut src: impl Read, dest: &Path, asset: &Asset, progress: &mut dyn FnMut(u64)) -> Result<()> {
    if asset.size > MAX_ARCHIVE {
        return Err(Error::Config(format!("{}: {} bytes is too large for a runtime", asset.file, asset.size)));
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut out = fs::File::create(dest)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > asset.size {
            let _ = fs::remove_file(dest);
            return Err(Error::Config(format!("{}: larger than the {} bytes runtimes.json names", asset.file, asset.size)));
        }
        h.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        progress(done);
    }
    let got = hex(&h.finalize());
    if done != asset.size || !got.eq_ignore_ascii_case(&asset.sha256) {
        let _ = fs::remove_file(dest);
        return Err(Error::Config(format!(
            "{}: the download does not match runtimes.json ({done} of {} bytes, SHA-256 {got}); nothing was installed",
            asset.file, asset.size
        )));
    }
    out.sync_all()?;
    Ok(())
}

/// Unpacks a verified `.tar.gz` into a fresh `staging` folder and checks that
/// the backend's executable is there and is an x86-64 PE file.
pub fn unpack(kind: BackendKind, archive: &Path, staging: &Path) -> Result<PathBuf> {
    if staging.exists() {
        fs::remove_dir_all(staging)?;
    }
    fs::create_dir_all(staging)?;
    let gz = flate2::read::GzDecoder::new(fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gz);
    tar.set_preserve_permissions(false);
    // `unpack` refuses entries that would land outside `staging` (`..`, absolute paths).
    tar.unpack(staging).map_err(|e| Error::Config(format!("{}: {e}", archive.display())))?;
    let exe = staging.join(exe_name(kind));
    if pe_machine(&exe) != Some(MACHINE_X86_64) {
        return Err(Error::Config(format!("{}: no x86-64 {} inside", archive.display(), exe_name(kind))));
    }
    Ok(exe)
}

/// Replaces the build in `dir` with the one unpacked in `staging`.
pub fn swap_in(staging: &Path, dir: &Path) -> Result<()> {
    let old = dir.with_extension("old");
    if old.exists() {
        fs::remove_dir_all(&old)?;
    }
    if dir.exists() {
        fs::rename(dir, &old)?;
    }
    if let Err(e) = fs::rename(staging, dir) {
        if old.exists() {
            let _ = fs::rename(&old, dir);
        }
        return Err(e.into());
    }
    if old.exists() {
        fs::remove_dir_all(&old)?;
    }
    Ok(())
}

/// Downloads the newest build of `kind`, checks it, and installs it.
/// `check` runs on the unpacked files before they replace the installed build
/// (the VirusTotal check); an error from it stops the install.
pub fn install(
    kind: BackendKind,
    progress: &mut dyn FnMut(u64, u64),
    check: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let url = manifest_url();
    let m = fetch_manifest_from(&url)?;
    let asset = m.asset(kind).ok_or_else(|| Error::Config(format!("runtimes.json has no {} build", kind.name())))?.clone();
    let archive_url = asset_url(&url, &asset);
    let resp = net::agent(Duration::from_secs(600)).get(&archive_url).call().map_err(|e| net::error(&archive_url, e))?;
    let archive = paths::cache_dir().join("downloads").join(&asset.file);
    let total = asset.size;
    save_verified(resp.into_reader(), &archive, &asset, &mut |n| progress(n, total))?;

    let dir = install_dir(kind);
    let staging = dir.with_extension("new");
    let result = unpack(kind, &archive, &staging).and_then(|_| check(&staging)).and_then(|_| swap_in(&staging, &dir));
    let _ = fs::remove_file(&archive);
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(dir.join(exe_name(kind)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn tarball(root: &Path, files: &[(&str, &Path)]) -> PathBuf {
        let path = root.join("rt.tar.gz");
        let gz = flate2::write::GzEncoder::new(fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (name, src) in files {
            b.append_path_with_name(src, name).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap();
        path
    }

    const MANIFEST: &str = r#"{"schema": 1, "published_at": "2026-11-24T06:40:00Z",
      "xcl": {"file": "xcl-abc.tar.gz", "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
              "size": 10, "commit": "abc", "repo": "LPortVI/xcl"}}"#;

    #[test]
    fn manifest_is_checked() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(m.xcl.as_ref().unwrap().commit, "abc");
        assert_eq!(m.victory, None);
        assert!(parse_manifest(&MANIFEST.replace("xcl-abc.tar.gz", "../x.tar.gz")).is_err());
        assert!(parse_manifest(&MANIFEST.replace("xcl-abc.tar.gz", "a/b.tar.gz")).is_err());
        assert!(parse_manifest(&MANIFEST.replace("\"0000", "\"zz00")).is_err());
        assert!(parse_manifest(&MANIFEST.replace("\"schema\": 1", "\"schema\": 2")).is_err());
    }

    #[test]
    fn archives_come_from_the_manifest_folder() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(
            asset_url(MANIFEST_URL, m.xcl.as_ref().unwrap()),
            "https://github.com/LPortVI/VILauncher/releases/download/runtimes/xcl-abc.tar.gz"
        );
    }

    #[test]
    fn status_compares_commits() {
        let t = tempfile::tempdir().unwrap();
        let m = parse_manifest(MANIFEST).unwrap();
        let s = status_from(Some(&m), t.path());
        assert_eq!((s[1].backend, s[1].installed.as_deref(), s[1].update), (BackendKind::Xcl, None, true));
        assert!(!s[0].update, "no victory build published");

        let dir = t.path().join("xcl");
        testutil::fake_pe(&dir.join("xcl.exe"), 0x8664);
        fs::write(dir.join("XCL_COMMIT.txt"), "xcl abc\nDXC v1.8\n").unwrap();
        let s = status_from(Some(&m), t.path());
        assert_eq!((s[1].installed.as_deref(), s[1].update), (Some("abc"), false));
        assert_eq!(s[1].exe, Some(dir.join("xcl.exe")));

        fs::write(dir.join("XCL_COMMIT.txt"), "xcl old\n").unwrap();
        assert!(status_from(Some(&m), t.path())[1].update);
    }

    #[test]
    fn download_must_match_size_and_hash() {
        let t = tempfile::tempdir().unwrap();
        let data = b"0123456789".to_vec();
        let good = Asset { file: "f".into(), sha256: sha256_hex(&data), size: 10, commit: "c".into(), repo: None };
        let dest = t.path().join("d/f");
        let mut seen = 0;
        save_verified(&data[..], &dest, &good, &mut |n| seen = n).unwrap();
        assert_eq!((fs::read(&dest).unwrap(), seen), (data.clone(), 10));

        let wrong_hash = Asset { sha256: sha256_hex(b"other"), ..good.clone() };
        assert!(save_verified(&data[..], &dest, &wrong_hash, &mut |_| {}).is_err());
        assert!(!dest.exists(), "a bad download is deleted");
        let too_small = Asset { size: 4, ..good.clone() };
        assert!(save_verified(&data[..], &dest, &too_small, &mut |_| {}).is_err());
        let too_big = Asset { size: 11, ..good };
        assert!(save_verified(&data[..], &dest, &too_big, &mut |_| {}).is_err());
    }

    #[test]
    fn unpack_and_swap_replace_the_build() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let exe = testutil::fake_pe(&src.join("xcl.exe"), 0x8664);
        fs::write(src.join("XCL_COMMIT.txt"), "xcl new\n").unwrap();
        // The workflow packs with `tar -C dir .`, so names start with `./`.
        let archive = tarball(t.path(), &[("./xcl.exe", &exe), ("./XCL_COMMIT.txt", &src.join("XCL_COMMIT.txt"))]);

        let dir = t.path().join("rt/xcl");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("XCL_COMMIT.txt"), "xcl old\n").unwrap();
        fs::write(dir.join("stale.dll"), "x").unwrap();

        let staging = dir.with_extension("new");
        unpack(BackendKind::Xcl, &archive, &staging).unwrap();
        swap_in(&staging, &dir).unwrap();
        assert_eq!(installed_commit(BackendKind::Xcl, &dir).as_deref(), Some("new"));
        assert!(!dir.join("stale.dll").exists());
        assert!(!staging.exists() && !dir.with_extension("old").exists());
    }

    #[test]
    fn unpack_refuses_an_archive_without_the_exe() {
        let t = tempfile::tempdir().unwrap();
        let note = t.path().join("note.txt");
        fs::write(&note, "hi").unwrap();
        let archive = tarball(t.path(), &[("note.txt", &note)]);
        assert!(unpack(BackendKind::Victory, &archive, &t.path().join("stage")).is_err());

        let script = t.path().join("victory.exe");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        let archive = tarball(t.path(), &[("victory.exe", &script)]);
        assert!(unpack(BackendKind::Victory, &archive, &t.path().join("stage")).is_err());
    }

    #[test]
    fn unpack_keeps_entries_inside_the_folder() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("evil.tar.gz");
        let gz = flate2::write::GzEncoder::new(fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let exe = testutil::fake_pe(&t.path().join("x/xcl.exe"), 0x8664);
        b.append_path_with_name(&exe, "xcl.exe").unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(2);
        h.set_mode(0o644);
        // `set_path` refuses `..`, so the name is written into the header by hand.
        h.as_old_mut().name[..9].copy_from_slice(b"../escape");
        h.set_cksum();
        b.append(&h, &b"hi"[..]).unwrap();
        b.into_inner().unwrap().finish().unwrap();

        let staging = t.path().join("deep/stage");
        let _ = unpack(BackendKind::Xcl, &path, &staging);
        assert!(!t.path().join("deep/escape").exists());
    }
}

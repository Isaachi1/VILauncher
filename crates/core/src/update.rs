//! Keeps VIctory and XCL up to date from the builds SixLauncher Builder publishes.
//!
//! Every 3 hours that service (SixLauncher/builder, on Railway) scores
//! PortWatch VI's efforts like the site, builds every public VIctory or XCL tree
//! scoring 59% or more for Windows, scans the binaries on VirusTotal and publishes
//! them on its `runtimes` release. `runtimes.json` names each archive, the
//! repository and commit it was built from, its size, its SHA-256, the VirusTotal
//! result for that build and the effort's score: `victory` and `xcl` are the best
//! build of each family, and `builds` lists them all, so one can be picked.
//!
//! The launcher downloads only from that release: an archive comes only from the
//! folder the manifest came from, its SHA-256 must match, the binaries inside
//! must be the ones VirusTotal scanned, a flagged build is refused, and it is
//! unpacked next to the running build and swapped in only once it is complete.
//! Compiling happens in that service because both backends are Windows (MSVC)
//! programs; nothing is built or fetched from the projects' repositories here.
//!
//! An installed build lives in `~/.local/share/sixlauncher/runtimes/<backend>`,
//! with the `VICTORY_COMMIT.txt` / `XCL_COMMIT.txt` the builder writes.
//!
//! The build an update replaces is not deleted: it moves to
//! `runtimes/previous/<backend>/<commit>`, and the last [`KEEP_PREVIOUS`] stay there.
//! [`rollback`] puts one of them back in place (the build it replaces is kept the
//! same way), so a new build that breaks the game is one click from undone. The
//! folder of the build in use never changes, so the configuration needs no edit.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backends::{MACHINE_X86_64, pe_machine};
use crate::config::BackendKind;
use crate::{Error, Result, net, paths, runners};

pub const MANIFEST_URL: &str = "https://github.com/SixLauncher/builder/releases/download/runtimes/runtimes-v2.json";
/// The manifest launchers before ScarlettPort read (VIctory and XCL only): read when the builder
/// has not published runtimes-v2.json yet.
pub const LEGACY_MANIFEST_URL: &str = "https://github.com/SixLauncher/builder/releases/download/runtimes/runtimes.json";

/// No runtime archive is this large; a bigger download is refused.
const MAX_ARCHIVE: u64 = 2 << 30;

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
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
    #[serde(default)]
    pub scarlett: Option<Asset>,
    /// The backend whose effort ranks higher on the tracker.
    #[serde(default)]
    pub recommended: Option<BackendKind>,
    /// Every published build, best first (`victory` and `xcl` are the first of each family).
    #[serde(default)]
    pub builds: Vec<Build>,
    /// Nothing was at the manifest's address (HTTP 404); set by the launcher, not read.
    #[serde(skip)]
    pub missing: bool,
    /// The lowest PortWatch VI score the builder builds (59).
    #[serde(default)]
    pub min_score: Option<f64>,
    /// How the builder chose: every effort it looked at, and why it took or skipped it.
    #[serde(default)]
    pub candidates: Vec<Candidate>,
}

/// One published build of either backend.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Build {
    pub family: BackendKind,
    /// The effort's score on PortWatch VI when it was built (0-100).
    #[serde(default)]
    pub score: Option<f64>,
    /// The one host a build is made for (`macos`: VIctory for macOS, under Wine with D3DMetal);
    /// none for the Windows builds that run under Proton or Wine on Linux.
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(flatten)]
    pub asset: Asset,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Candidate {
    pub repo: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub result: String,
}

/// What `scripts/virustotal-scan.py` found for one build.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Scan {
    /// `clean`, `flagged`, `pending` (not analysed yet), `skipped` (no key) or `error`.
    pub verdict: String,
    #[serde(default)]
    pub scanned_at: Option<String>,
    #[serde(default)]
    pub files: Vec<ScannedFile>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ScannedFile {
    pub name: String,
    pub sha256: String,
    #[serde(default)]
    pub malicious: u32,
    #[serde(default)]
    pub suspicious: u32,
    #[serde(default)]
    pub engines: u32,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub state: String,
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
    /// The tracker's name for the effort behind that repository.
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub virustotal: Option<Scan>,
}

impl Manifest {
    pub fn asset(&self, kind: BackendKind) -> Option<&Asset> {
        match kind {
            BackendKind::Victory => self.victory.as_ref(),
            BackendKind::Xcl => self.xcl.as_ref(),
            BackendKind::Scarlett => self.scarlett.as_ref(),
        }
    }

    /// The builds of `kind` for this host, best first. A manifest without `builds` (the old
    /// workflow's) offers just its `victory` / `xcl` entry.
    pub fn builds_of(&self, kind: BackendKind) -> Vec<Build> {
        self.builds_for(kind, runners::MACOS)
    }

    /// The builds of `kind` for macOS (`macos`) or for Linux (the others), best first.
    pub fn builds_for(&self, kind: BackendKind, macos: bool) -> Vec<Build> {
        let fits = |b: &&Build| match b.platform.as_deref() {
            Some("macos") => macos,
            Some(_) => false,
            None => !macos,
        };
        let list: Vec<Build> = self.builds.iter().filter(|b| b.family == kind).filter(fits).cloned().collect();
        if !list.is_empty() || macos {
            return list;
        }
        self.asset(kind).map(|a| Build { family: kind, score: None, platform: None, asset: a.clone() }).into_iter().collect()
    }

    /// The build of `kind` in the archive `file`, or the best one for this host when `file` is
    /// `None`.
    pub fn pick(&self, kind: BackendKind, file: Option<&str>) -> Option<Asset> {
        let list = self.builds_of(kind);
        match file {
            None if runners::MACOS => list.into_iter().next().map(|b| b.asset),
            None => self.asset(kind).cloned().or_else(|| list.into_iter().next().map(|b| b.asset)),
            Some(f) => list.into_iter().map(|b| b.asset).find(|a| a.file == f),
        }
    }
}

pub fn exe_name(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Victory => "victory.exe",
        BackendKind::Xcl => "xcl.exe",
        BackendKind::Scarlett => "scarlett.exe",
    }
}

/// Written next to an installed build: the manifest entry it came from.
const SOURCE_FILE: &str = "SIXLAUNCHER_SOURCE.json";
/// Written into a build when it is replaced: when that happened (Unix seconds).
const RETIRED_FILE: &str = "SIXLAUNCHER_REPLACED_AT";
/// How many replaced builds of each backend are kept for [`rollback`].
pub const KEEP_PREVIOUS: usize = 3;

fn commit_file(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Victory => "VICTORY_COMMIT.txt",
        BackendKind::Xcl => "XCL_COMMIT.txt",
        BackendKind::Scarlett => "SCARLETT_COMMIT.txt",
    }
}

pub fn manifest_url() -> String {
    std::env::var("SIXLAUNCHER_RUNTIMES_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| MANIFEST_URL.into())
}

/// Where to look, in order: SIXLAUNCHER_RUNTIMES_URL alone when it is set, else
/// runtimes-v2.json and then the legacy runtimes.json.
fn manifest_urls() -> Vec<String> {
    match std::env::var("SIXLAUNCHER_RUNTIMES_URL").ok().filter(|u| !u.is_empty()) {
        Some(u) => vec![u],
        None => vec![MANIFEST_URL.into(), LEGACY_MANIFEST_URL.into()],
    }
}

pub fn runtimes_dir() -> PathBuf {
    paths::data_dir().join("runtimes")
}

pub fn install_dir(kind: BackendKind) -> PathBuf {
    runtimes_dir().join(kind.name())
}

/// Where the builds that `kind`'s updates replaced are kept, under `root` (the runtimes folder).
pub fn previous_dir(root: &Path, kind: BackendKind) -> PathBuf {
    root.join("previous").join(kind.name())
}

pub fn parse_manifest(text: &str) -> Result<Manifest> {
    let m: Manifest = serde_json::from_str(text).map_err(|e| Error::Config(format!("runtimes.json: {e}")))?;
    if m.schema != 1 {
        return Err(Error::Config(format!("runtimes.json: schema {} is newer than this launcher; update SixLauncher", m.schema)));
    }
    for a in [m.victory.as_ref(), m.xcl.as_ref()].into_iter().flatten().chain(m.builds.iter().map(|b| &b.asset)) {
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
    /// The manifest entry the installed build came from.
    pub source: Option<Asset>,
    pub exe: Option<PathBuf>,
    /// The newest build from the repository of the installed one, or the best build.
    pub latest: Option<Asset>,
    pub update: bool,
    /// Every published build of this backend, best first.
    pub choices: Vec<Build>,
    /// Builds an update replaced, newest first: [`rollback`] brings one back.
    pub previous: Vec<PreviousBuild>,
}

/// A build an update replaced, kept so it can be brought back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreviousBuild {
    /// What [`rollback`] takes: the folder's name (the build's commit).
    pub id: String,
    pub commit: Option<String>,
    /// The manifest entry it came from (repository, effort, VirusTotal result).
    pub source: Option<Asset>,
    /// When it was replaced (Unix seconds).
    pub replaced_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Runtimes {
    pub published_at: Option<String>,
    pub recommended: Option<BackendKind>,
    pub candidates: Vec<Candidate>,
    pub backends: Vec<RuntimeStatus>,
    /// Set when the manifest could not be fetched: only what is installed is shown.
    pub offline: Option<String>,
    /// The manifest's address, when nothing is there (HTTP 404): no build is published yet, or
    /// the builder's repository is private (GitHub answers 404 to a download from a private one).
    pub unpublished: Option<String>,
}

pub fn installed_source(dir: &Path) -> Option<Asset> {
    serde_json::from_str(&fs::read_to_string(dir.join(SOURCE_FILE)).ok()?).ok()
}

pub fn status_from(manifest: Option<&Manifest>, root: &Path) -> Runtimes {
    let backends = BackendKind::ALL
        .into_iter()
        .map(|kind| {
            let dir = root.join(kind.name());
            let installed = installed_commit(kind, &dir);
            let exe = Some(dir.join(exe_name(kind))).filter(|e| e.is_file());
            let source = installed_source(&dir);
            let choices = manifest.map(|m| m.builds_of(kind)).unwrap_or_default();
            // Following the project already installed: a newer commit of it is the update,
            // not a switch to whichever project now ranks first.
            let same_repo = source.as_ref().and_then(|s| s.repo.as_ref()).and_then(|r| choices.iter().find(|b| b.asset.repo.as_ref() == Some(r)));
            let latest = same_repo.map(|b| b.asset.clone()).or_else(|| manifest.and_then(|m| m.pick(kind, None)));
            let update = latest.as_ref().is_some_and(|l| installed.as_deref() != Some(l.commit.as_str()) || exe.is_none());
            let previous = previous_from(root, kind);
            RuntimeStatus { backend: kind, installed, source, exe, latest, update, choices, previous }
        })
        .collect();
    Runtimes {
        published_at: manifest.and_then(|m| m.published_at.clone()),
        recommended: manifest.and_then(|m| m.recommended),
        candidates: manifest.map(|m| m.candidates.clone()).unwrap_or_default(),
        backends,
        offline: None,
        unpublished: manifest.filter(|m| m.missing).map(|_| manifest_url()),
    }
}

/// The builds kept in `root`'s previous folder for `kind`, newest first. Only folders
/// that still hold the backend's executable count.
pub fn previous_from(root: &Path, kind: BackendKind) -> Vec<PreviousBuild> {
    let Ok(rd) = fs::read_dir(previous_dir(root, kind)) else { return Vec::new() };
    let mut list: Vec<PreviousBuild> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(exe_name(kind)).is_file())
        .filter_map(|p| {
            let id = p.file_name()?.to_string_lossy().into_owned();
            let replaced_at = fs::read_to_string(p.join(RETIRED_FILE)).ok().and_then(|t| t.trim().parse().ok());
            Some(PreviousBuild { id, commit: installed_commit(kind, &p), source: installed_source(&p), replaced_at })
        })
        .collect();
    list.sort_by(|a, b| b.replaced_at.cmp(&a.replaced_at).then_with(|| a.id.cmp(&b.id)));
    list
}

pub fn previous(kind: BackendKind) -> Vec<PreviousBuild> {
    previous_from(&runtimes_dir(), kind)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn folder_name(commit: Option<&str>) -> String {
    match commit {
        Some(c) if crate::protons::plain_name(c) => c.to_owned(),
        _ => format!("unknown-{}", now()),
    }
}

/// Moves the replaced build `old` into the previous folder (named by its commit),
/// then keeps only the newest [`KEEP_PREVIOUS`]. A build with the commit of the one
/// now in use is just deleted: there is nothing to go back to.
fn retire(kind: BackendKind, old: &Path, root: &Path, current: Option<&str>) -> Result<()> {
    let commit = installed_commit(kind, old);
    if commit.is_some() && commit.as_deref() == current {
        fs::remove_dir_all(old)?;
        return Ok(());
    }
    let keep = previous_dir(root, kind);
    fs::create_dir_all(&keep)?;
    fs::write(old.join(RETIRED_FILE), now().to_string())?;
    let dest = keep.join(folder_name(commit.as_deref()));
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::rename(old, &dest)?;
    for stale in previous_from(root, kind).into_iter().skip(KEEP_PREVIOUS) {
        fs::remove_dir_all(keep.join(stale.id))?;
    }
    Ok(())
}

/// Puts `incoming` (a complete build) in place of `root/<kind>` and keeps the build
/// it replaces among the previous ones. If keeping fails, the old build is deleted
/// rather than leaving the new one half done.
fn replace_keeping(kind: BackendKind, incoming: &Path, root: &Path) -> Result<()> {
    let dir = root.join(kind.name());
    fs::create_dir_all(root)?;
    let old = dir.with_extension("old");
    if old.exists() {
        fs::remove_dir_all(&old)?;
    }
    if dir.exists() {
        fs::rename(&dir, &old)?;
    }
    if let Err(e) = fs::rename(incoming, &dir) {
        if old.exists() {
            let _ = fs::rename(&old, &dir);
        }
        return Err(e.into());
    }
    if old.exists() && retire(kind, &old, root, installed_commit(kind, &dir).as_deref()).is_err() && old.exists() {
        fs::remove_dir_all(&old)?;
    }
    Ok(())
}

/// Brings back the previous build `id` of `kind` (see [`previous`]); the build in use
/// becomes a previous one. Returns the executable, at the same path as before.
pub fn rollback(kind: BackendKind, id: &str) -> Result<PathBuf> {
    rollback_in(&runtimes_dir(), kind, id)
}

pub fn rollback_in(root: &Path, kind: BackendKind, id: &str) -> Result<PathBuf> {
    let src = previous_dir(root, kind).join(id);
    if !crate::protons::plain_name(id) || !src.is_dir() {
        return Err(Error::Config(format!("there is no previous {} build {id}", kind.name())));
    }
    let exe = src.join(exe_name(kind));
    if pe_machine(&exe) != Some(MACHINE_X86_64) {
        return Err(Error::Config(format!("{}: no x86-64 {} in it", src.display(), exe_name(kind))));
    }
    let _ = fs::remove_file(src.join(RETIRED_FILE));
    replace_keeping(kind, &src, root)?;
    Ok(root.join(kind.name()).join(exe_name(kind)))
}

/// Refuses a build VirusTotal flagged, and one whose binaries are not the ones
/// that were scanned. A build that was not scanned (no key, still pending) passes.
pub fn check_scan(asset: &Asset, staging: &Path) -> Result<()> {
    let Some(scan) = &asset.virustotal else { return Ok(()) };
    if scan.verdict == "flagged" {
        let names: Vec<&str> = scan.files.iter().filter(|f| f.malicious > 0).map(|f| f.name.as_str()).collect();
        return Err(Error::Config(format!("VirusTotal flagged this build ({}); not installed", names.join(", "))));
    }
    for f in &scan.files {
        // A name is a path relative to the build's folder; nothing may point outside it.
        let rel = Path::new(&f.name);
        let inside = !f.name.is_empty() && !f.name.contains('\\') && rel.components().all(|c| matches!(c, std::path::Component::Normal(_)));
        let ok = inside && sha256_file(&staging.join(rel)).is_ok_and(|h| h.eq_ignore_ascii_case(&f.sha256));
        if !ok {
            return Err(Error::Config(format!("{} is not the file VirusTotal scanned; not installed", f.name)));
        }
    }
    Ok(())
}

/// The published builds. Nothing at the address (404) is no build published yet, or a private
/// builder repository: an empty list marked `missing`, which the window explains.
fn fetch_manifest_from(url: &str) -> Result<Manifest> {
    match net::get_text_opt(url, Duration::from_secs(20))? {
        Some(text) => parse_manifest(&text),
        None => Ok(Manifest { schema: 1, missing: true, ..Manifest::default() }),
    }
}

/// The first manifest found at `urls`, with its address (archives sit beside it). When none is
/// there, the last answer (marked `missing`).
fn fetch_first(urls: &[String], fetch: impl Fn(&str) -> Result<Manifest>) -> Result<(String, Manifest)> {
    let mut last = None;
    for url in urls {
        let m = fetch(url)?;
        if !m.missing {
            return Ok((url.clone(), m));
        }
        last = Some((url.clone(), m));
    }
    last.ok_or_else(|| Error::Config("no runtimes manifest address".into()))
}

pub fn fetch_manifest() -> Result<Manifest> {
    fetch_first(&manifest_urls(), fetch_manifest_from).map(|(_, m)| m)
}

/// What is installed and what the newest builds are.
pub fn status() -> Result<Runtimes> {
    let m = fetch_manifest()?;
    Ok(status_from(Some(&m), &runtimes_dir()))
}

/// Like [`status`], but offline it still lists what is installed and the previous
/// builds (so a rollback works without the network), with the error in `offline`.
pub fn status_or_local() -> Runtimes {
    match fetch_manifest() {
        Ok(m) => status_from(Some(&m), &runtimes_dir()),
        Err(e) => Runtimes { offline: Some(e.to_string()), ..status_from(None, &runtimes_dir()) },
    }
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

/// Downloads a build of `kind` (the archive `file` from runtimes.json's list, or the best
/// one), checks it, and installs it.
/// `check` runs on the unpacked files before they replace the installed build
/// (the VirusTotal check); an error from it stops the install.
pub fn install(
    kind: BackendKind,
    file: Option<&str>,
    progress: &mut dyn FnMut(u64, u64),
    check: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let (url, m) = fetch_first(&manifest_urls(), fetch_manifest_from)?;
    let asset = m.pick(kind, file).ok_or_else(|| match file {
        Some(f) => Error::Config(format!("runtimes.json has no {} build {f}", kind.name())),
        None => Error::Config(format!("runtimes.json has no {} build", kind.name())),
    })?;
    let archive_url = asset_url(&url, &asset);
    let resp = net::agent(Duration::from_secs(600)).get(&archive_url).call().map_err(|e| net::error(&archive_url, e))?;
    let archive = paths::cache_dir().join("downloads").join(&asset.file);
    let total = asset.size;
    save_verified(resp.into_reader(), &archive, &asset, &mut |n| progress(n, total))?;

    let dir = install_dir(kind);
    let staging = dir.with_extension("new");
    let result = unpack(kind, &archive, &staging)
        .and_then(|_| check_scan(&asset, &staging))
        .and_then(|_| check(&staging))
        .and_then(|_| {
            let json = serde_json::to_string_pretty(&asset).map_err(|e| Error::Config(e.to_string()))?;
            fs::write(staging.join(SOURCE_FILE), json)?;
            replace_keeping(kind, &staging, &runtimes_dir())
        });
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
    fn the_legacy_manifest_answers_until_v2_is_published() {
        let urls = [MANIFEST_URL.to_string(), LEGACY_MANIFEST_URL.to_string()];
        let missing = || Manifest { schema: 1, missing: true, ..Manifest::default() };
        let (url, m) = fetch_first(&urls, |u| Ok(if u == MANIFEST_URL { missing() } else { parse_manifest(MANIFEST).unwrap() })).unwrap();
        assert_eq!((url.as_str(), m.missing), (LEGACY_MANIFEST_URL, false));
        let (url, _) = fetch_first(&urls, |_| Ok(parse_manifest(MANIFEST).unwrap())).unwrap();
        assert_eq!(url, MANIFEST_URL, "v2 first");
        let (url, m) = fetch_first(&urls, |_| Ok(missing())).unwrap();
        assert_eq!((url.as_str(), m.missing), (LEGACY_MANIFEST_URL, true));
        assert!(fetch_first(&urls, |_| Err(Error::Config("offline".into()))).is_err());
    }

    #[test]
    fn archives_come_from_the_manifest_folder() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(
            asset_url(MANIFEST_URL, m.xcl.as_ref().unwrap()),
            "https://github.com/SixLauncher/builder/releases/download/runtimes/xcl-abc.tar.gz"
        );
    }

    #[test]
    fn status_compares_commits() {
        let t = tempfile::tempdir().unwrap();
        let m = parse_manifest(MANIFEST).unwrap();
        let s = status_from(Some(&m), t.path()).backends;
        assert_eq!((s[1].backend, s[1].installed.as_deref(), s[1].update), (BackendKind::Xcl, None, true));
        assert!(!s[0].update, "no victory build published");

        let dir = t.path().join("xcl");
        testutil::fake_pe(&dir.join("xcl.exe"), 0x8664);
        fs::write(dir.join("XCL_COMMIT.txt"), "xcl abc\nDXC v1.8\n").unwrap();
        let s = status_from(Some(&m), t.path()).backends;
        assert_eq!((s[1].installed.as_deref(), s[1].update), (Some("abc"), false));
        assert_eq!(s[1].exe, Some(dir.join("xcl.exe")));

        fs::write(dir.join("XCL_COMMIT.txt"), "xcl old\n").unwrap();
        assert!(status_from(Some(&m), t.path()).backends[1].update);
    }

    // The shape the workflow writes (scripts/select-runtimes.sh + virustotal-scan.py).
    const FULL: &str = r#"{"schema": 1, "recommended": "victory",
      "victory": {"file": "victory-fff.tar.gz", "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
                  "size": 5, "commit": "fff", "repo": "someone/victory-fork", "effort": "Fork",
                  "virustotal": {"repo": "someone/victory-fork", "commit": "fff", "verdict": "clean",
                                 "files": [{"name": "victory.exe", "sha256": "SHA", "malicious": 0, "engines": 70, "state": "clean"}]}},
      "candidates": [{"repo": "someone/victory-fork", "name": "Fork", "rank": 5073, "family": "victory", "result": "selected"},
                     {"repo": "x/y", "name": "Other", "rank": 2500, "family": null, "result": "no build recipe"}]}"#;

    #[test]
    fn manifest_carries_the_tracker_pick_and_the_scan() {
        let m = parse_manifest(FULL).unwrap();
        assert_eq!(m.recommended, Some(BackendKind::Victory));
        assert_eq!(m.candidates[1].family, None);
        let v = m.victory.as_ref().unwrap();
        assert_eq!((v.repo.as_deref(), v.effort.as_deref()), (Some("someone/victory-fork"), Some("Fork")));
        assert_eq!(v.virustotal.as_ref().unwrap().files[0].engines, 70);
        let t = tempfile::tempdir().unwrap();
        let s = status_from(Some(&m), t.path());
        assert_eq!((s.recommended, s.candidates.len()), (Some(BackendKind::Victory), 2));
    }

    // The shape SixLauncher Builder writes: the best of each family, and every build.
    const BUILDS: &str = r#"{"schema": 1, "recommended": "victory", "min_score": 59,
      "victory": {"family": "victory", "file": "victory-zettrox-ztrx-aaa.tar.gz", "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
                  "size": 5, "commit": "aaa", "repo": "Zettrox/ztrx", "effort": "ZETTROX", "score": 64.2},
      "xcl": null,
      "builds": [
        {"family": "victory", "file": "victory-zettrox-ztrx-aaa.tar.gz", "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
         "size": 5, "commit": "aaa", "repo": "Zettrox/ztrx", "effort": "ZETTROX", "score": 64.2},
        {"family": "victory", "file": "victory-uiliannn-vitoriabr-bbb.tar.gz", "sha256": "2222222222222222222222222222222222222222222222222222222222222222",
         "size": 6, "commit": "bbb", "repo": "uiliannn/VItoriaBr", "effort": "Uilian", "score": 59.6}],
      "candidates": []}"#;

    #[test]
    fn nothing_published_is_an_empty_list() {
        // A server that answers 404, as GitHub does before the first release.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/runtimes.json", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut c, _) = listener.accept().unwrap();
            let _ = c.read(&mut [0u8; 2048]);
            c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let m = fetch_manifest_from(&url).unwrap();
        assert!(m.builds.is_empty() && m.victory.is_none() && m.xcl.is_none());
        assert!(m.builds_of(BackendKind::Victory).is_empty());
        assert!(m.missing, "the window says nothing is published there");
        assert!(!parse_manifest(BUILDS).unwrap().missing);
    }

    #[test]
    fn every_build_can_be_picked() {
        let m = parse_manifest(BUILDS).unwrap();
        let v = m.builds_of(BackendKind::Victory);
        assert_eq!(v.iter().map(|b| (b.asset.effort.as_deref(), b.score)).collect::<Vec<_>>(), [(Some("ZETTROX"), Some(64.2)), (Some("Uilian"), Some(59.6))]);
        assert!(m.builds_of(BackendKind::Xcl).is_empty());
        // A build for macOS only shows on a Mac, and a Mac sees only those.
        let mut mac = m.clone();
        mac.builds[1].platform = Some("macos".into());
        assert_eq!(mac.builds_for(BackendKind::Victory, false).len(), 1);
        assert_eq!(mac.builds_for(BackendKind::Victory, true)[0].asset.commit, "bbb");
        assert!(m.builds_for(BackendKind::Victory, true).is_empty(), "no macOS build: nothing on a Mac");
        assert_eq!(m.pick(BackendKind::Victory, None).unwrap().commit, "aaa");
        assert_eq!(m.pick(BackendKind::Victory, Some("victory-uiliannn-vitoriabr-bbb.tar.gz")).unwrap().commit, "bbb");
        assert_eq!(m.pick(BackendKind::Xcl, Some("victory-uiliannn-vitoriabr-bbb.tar.gz")), None, "a build of the other family");
        assert!(parse_manifest(&BUILDS.replace("victory-uiliannn-vitoriabr-bbb.tar.gz", "../evil.tar.gz")).is_err());
        // The old workflow's manifest: its one entry is the only choice.
        assert_eq!(parse_manifest(MANIFEST).unwrap().builds_of(BackendKind::Xcl)[0].asset.commit, "abc");
    }

    #[test]
    fn an_update_follows_the_installed_project() {
        let t = tempfile::tempdir().unwrap();
        let m = parse_manifest(BUILDS).unwrap();
        let dir = t.path().join("victory");
        testutil::fake_pe(&dir.join("victory.exe"), 0x8664);
        fs::write(dir.join("VICTORY_COMMIT.txt"), "VIctory bbb\n").unwrap();
        let mut src = m.builds[1].asset.clone();
        fs::write(dir.join(SOURCE_FILE), serde_json::to_string(&src).unwrap()).unwrap();
        let s = &status_from(Some(&m), t.path()).backends[0];
        assert_eq!((s.latest.as_ref().unwrap().commit.as_str(), s.update, s.choices.len()), ("bbb", false, 2));
        // A newer commit of the same project is an update; a better-ranked project is not.
        src.commit = "old".into();
        fs::write(dir.join("VICTORY_COMMIT.txt"), "VIctory old\n").unwrap();
        fs::write(dir.join(SOURCE_FILE), serde_json::to_string(&src).unwrap()).unwrap();
        let s = &status_from(Some(&m), t.path()).backends[0];
        assert_eq!((s.latest.as_ref().unwrap().commit.as_str(), s.update), ("bbb", true));
    }

    #[test]
    fn scan_must_be_clean_and_match_the_files() {
        let t = tempfile::tempdir().unwrap();
        let exe = testutil::fake_pe(&t.path().join("victory.exe"), 0x8664);
        let sha = sha256_file(&exe).unwrap();
        let mut asset = parse_manifest(&FULL.replace("\"SHA\"", &format!("\"{sha}\""))).unwrap().victory.unwrap();
        check_scan(&asset, t.path()).unwrap();

        let mut nested = asset.clone();
        nested.virustotal.as_mut().unwrap().files[0].name = "bin/../victory.exe".into();
        assert!(check_scan(&nested, t.path()).is_err(), "no `..` in scanned names");
        fs::create_dir_all(t.path().join("bin")).unwrap();
        fs::copy(&exe, t.path().join("bin/victory.exe")).unwrap();
        nested.virustotal.as_mut().unwrap().files[0].name = "bin/victory.exe".into();
        check_scan(&nested, t.path()).unwrap();

        fs::write(&exe, "MZ changed after the scan").unwrap();
        assert!(check_scan(&asset, t.path()).is_err());

        let scan = asset.virustotal.as_mut().unwrap();
        scan.verdict = "flagged".into();
        scan.files[0].malicious = 3;
        let why = check_scan(&asset, t.path()).unwrap_err().to_string();
        assert!(why.contains("flagged") && why.contains("victory.exe"), "{why}");

        asset.virustotal = None;
        check_scan(&asset, t.path()).unwrap();
        let mut skipped = parse_manifest(FULL).unwrap().victory.unwrap();
        let scan = skipped.virustotal.as_mut().unwrap();
        scan.verdict = "skipped".into();
        scan.files.clear();
        check_scan(&skipped, t.path()).unwrap();
    }

    #[test]
    fn download_must_match_size_and_hash() {
        let t = tempfile::tempdir().unwrap();
        let data = b"0123456789".to_vec();
        let good = Asset { file: "f".into(), sha256: sha256_hex(&data), size: 10, commit: "c".into(), repo: None, effort: None, virustotal: None };
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

    fn build(root: &Path, commit: &str) -> PathBuf {
        let dir = root.join(format!("incoming-{commit}"));
        testutil::fake_pe(&dir.join("victory.exe"), 0x8664);
        fs::write(dir.join("VICTORY_COMMIT.txt"), format!("victory {commit}\n")).unwrap();
        dir
    }

    #[test]
    fn updates_keep_previous_builds_and_roll_back() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("runtimes");
        let kind = BackendKind::Victory;
        let commit = |root: &Path| installed_commit(kind, &root.join("victory"));
        for c in ["c1", "c2", "c3", "c4", "c5"] {
            replace_keeping(kind, &build(t.path(), c), &root).unwrap();
            // Distinct replacement times, newest last.
            for (i, p) in previous_from(&root, kind).iter().rev().enumerate() {
                fs::write(previous_dir(&root, kind).join(&p.id).join(RETIRED_FILE), i.to_string()).unwrap();
            }
        }
        assert_eq!(commit(&root).as_deref(), Some("c5"));
        let ids: Vec<String> = previous_from(&root, kind).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["c4", "c3", "c2"], "only the newest {KEEP_PREVIOUS} are kept");
        assert!(!root.join("victory.old").exists());

        let exe = rollback_in(&root, kind, "c3").unwrap();
        assert_eq!(exe, root.join("victory/victory.exe"));
        assert_eq!(commit(&root).as_deref(), Some("c3"));
        let ids: Vec<String> = previous_from(&root, kind).into_iter().map(|p| p.id).collect();
        assert!(ids.contains(&"c5".to_string()) && !ids.contains(&"c3".to_string()), "{ids:?}");
        assert!(!previous_dir(&root, kind).join("c3").exists());

        assert!(rollback_in(&root, kind, "nope").is_err());
        assert!(rollback_in(&root, kind, "../victory").is_err());
        assert_eq!(commit(&root).as_deref(), Some("c3"), "a failed rollback changes nothing");

        // Reinstalling the same commit does not fill the list with copies of it.
        let before = previous_from(&root, kind).len();
        replace_keeping(kind, &build(t.path(), "c3"), &root).unwrap();
        assert_eq!(previous_from(&root, kind).len(), before);
    }

    #[test]
    fn status_lists_previous_builds_offline() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("runtimes");
        replace_keeping(BackendKind::Victory, &build(t.path(), "a1"), &root).unwrap();
        replace_keeping(BackendKind::Victory, &build(t.path(), "b2"), &root).unwrap();
        let s = status_from(None, &root);
        let v = &s.backends[0];
        assert_eq!((v.installed.as_deref(), v.update), (Some("b2"), false));
        assert_eq!(v.previous.len(), 1);
        assert_eq!((v.previous[0].id.as_str(), v.previous[0].commit.as_deref()), ("a1", Some("a1")));
        assert!(v.previous[0].replaced_at.is_some());
        assert!(s.backends[1].previous.is_empty());
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

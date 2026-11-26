//! Whether a newer SixLauncher exists, and how this copy can get it.
//!
//! Every merge into main publishes a GitHub release (`v<major>.<minor>.<run>`) with a
//! `latest.json` in the format of Tauri's updater: the version, the notes and, when the
//! release was signed, one entry per package that can replace itself
//! (`linux-x86_64-appimage`, `-deb`, `-rpm`, `windows-x86_64-nsis`). The release marked as latest serves it at
//! [`MANIFEST_URL`].
//!
//! The window installs an update through Tauri's updater, which checks the signature;
//! this module only answers "is there a newer one, and can this copy install it by
//! itself?" so that the window and the command line can say so even where the updater
//! cannot act (the tarball, a build from source).
//!
//! The Flatpak is the exception: Tauri's updater cannot replace a Flatpak, so
//! [`install_flatpak`] does it. It downloads the release's `sixlauncher.flatpak`, checks its
//! signature (`linux-x86_64-flatpak` in `latest.json`, same key and same check as the
//! updater's) and installs it on the host with `flatpak install --bundle`.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::{host, net, paths, Error, Result};

pub const REPO: &str = "SixLauncher/launcher";
pub const MANIFEST_URL: &str = "https://github.com/SixLauncher/launcher/releases/latest/download/latest.json";
pub const RELEASES_URL: &str = "https://github.com/SixLauncher/launcher/releases/latest";
/// Where every package of a release is downloaded from.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/SixLauncher/launcher/releases/download/";
/// The Flatpak's app id (packaging/flatpak).
pub const FLATPAK_ID: &str = "io.github.lportvi.sixlauncher";
/// No Flatpak bundle of the launcher is this large; a bigger download is refused.
const MAX_BUNDLE: u64 = 512 << 20;

/// How this copy of SixLauncher was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Install {
    /// An AppImage (`$APPIMAGE`): the updater replaces the file, no password asked.
    AppImage,
    /// The .deb, installed under /usr: the updater installs the new one with pkexec.
    Deb,
    /// The .rpm, installed under /usr: the updater installs the new one with pkexec.
    Rpm,
    /// The Flatpak bundle: a new bundle is installed with `flatpak install`.
    Flatpak,
    /// The Windows installer (NSIS): the updater runs the new installer, which replaces this copy.
    Nsis,
    /// The tarball or a build from source: a new copy is downloaded by hand.
    Other,
}

impl Install {
    /// This package's entry in `latest.json`, as Tauri's updater looks it up.
    pub fn manifest_key(self) -> Option<&'static str> {
        match self {
            Install::AppImage => Some("linux-x86_64-appimage"),
            Install::Deb => Some("linux-x86_64-deb"),
            Install::Rpm => Some("linux-x86_64-rpm"),
            Install::Nsis => Some("windows-x86_64-nsis"),
            Install::Flatpak => Some("linux-x86_64-flatpak"),
            Install::Other => None,
        }
    }
}

/// Where this process comes from. `packaged` is what Tauri's bundler stamped into the
/// binary (`deb`, `rpm`, `appimage`), which the window passes; the stamp alone is not
/// trusted, since the same binary also goes into the tarball: a .deb or .rpm must run
/// from /usr, an AppImage must have `$APPIMAGE` set, and the Windows installer's copy has the
/// installer's `uninstall.exe` beside it (the .zip carries the same binary without one).
pub fn detect(packaged: Option<&str>, exe: &Path, appimage_env: Option<&str>, flatpak: bool) -> Install {
    if flatpak {
        return Install::Flatpak;
    }
    let from_usr = exe.starts_with("/usr/");
    let installed = exe.parent().is_some_and(|d| d.join("uninstall.exe").is_file());
    match packaged {
        Some("nsis") if installed => Install::Nsis,
        Some("appimage") if appimage_env.is_some_and(|p| !p.is_empty()) => Install::AppImage,
        Some("deb") if from_usr => Install::Deb,
        Some("rpm") if from_usr => Install::Rpm,
        _ => Install::Other,
    }
}

/// [`detect`] for this process.
pub fn detect_current(packaged: Option<&str>) -> Install {
    let exe = std::env::current_exe().unwrap_or_default();
    let appimage = std::env::var("APPIMAGE").ok();
    let flatpak = std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists();
    detect(packaged, &exe, appimage.as_deref(), flatpak)
}

/// `latest.json`, as far as this module reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub platforms: BTreeMap<String, serde_json::Value>,
}

/// The answer for the window and the command line.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub current: String,
    /// The newest published version.
    pub latest: String,
    pub newer: bool,
    pub notes: Option<String>,
    pub install: Install,
    /// The updater can install it by itself (a signed entry for this package exists).
    pub automatic: bool,
    /// The release page, for an update by hand.
    pub url: String,
}

/// Compares two `x.y.z` versions (a leading `v` and a `-suffix` are ignored; missing parts
/// count as 0). None when either is not a version.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    fn parts(v: &str) -> Option<Vec<u64>> {
        let v = v.trim().trim_start_matches('v');
        let v = v.split(['-', '+']).next()?;
        v.split('.').map(|p| p.parse().ok()).collect()
    }
    let (mut a, mut b) = (parts(a)?, parts(b)?);
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    Some(a.cmp(&b))
}

/// What `manifest` means for a copy at version `current` installed as `install`.
pub fn evaluate(manifest: &Manifest, current: &str, install: Install, can_verify: bool) -> Check {
    let newer = compare(&manifest.version, current) == Some(Ordering::Greater);
    let has_entry = install
        .manifest_key()
        .is_some_and(|k| manifest.platforms.contains_key(k));
    let version = manifest.version.trim_start_matches('v');
    Check {
        current: current.to_string(),
        latest: version.to_string(),
        newer,
        notes: manifest.notes.clone().filter(|n| !n.trim().is_empty()),
        install,
        automatic: newer && has_entry && can_verify,
        url: format!("https://github.com/{REPO}/releases/tag/v{version}"),
    }
}

/// Reads the newest release's `latest.json`. None when no release has one yet.
pub fn fetch_manifest() -> Result<Option<Manifest>> {
    let Some(text) = net::get_text_opt(MANIFEST_URL, Duration::from_secs(15))? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|e| Error::Config(format!("{MANIFEST_URL}: {e}")))
}

// -- the Flatpak's own update ------------------------------------------------------------------

/// A package's entry in `latest.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub url: String,
    pub signature: String,
}

impl Manifest {
    pub fn entry(&self, key: &str) -> Option<Entry> {
        self.platforms.get(key).and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

fn base64_text(what: &str, b64: &str) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| Error::Config(format!("{what}: not base64 ({e})")))?;
    String::from_utf8(bytes).map_err(|_| Error::Config(format!("{what}: not text")))
}

/// Checks `data` against a signature from `latest.json` (base64 of a minisign signature file)
/// with the updater's public key (base64 of a minisign public key file), as Tauri's updater
/// does: the signature first, then the version the signature's trusted comment records, if it
/// records one, must be `version`.
pub fn verify_signature(data: &[u8], signature: &str, pubkey: &str, version: &str) -> Result<()> {
    let key = minisign_verify::PublicKey::decode(&base64_text("updater public key", pubkey)?)
        .map_err(|e| Error::Config(format!("updater public key: {e}")))?;
    let sig = minisign_verify::Signature::decode(&base64_text("signature", signature)?)
        .map_err(|e| Error::Config(format!("signature: {e}")))?;
    key.verify(data, &sig, true)
        .map_err(|e| Error::Config(format!("the download does not match its signature ({e}); nothing was installed")))?;
    let signed = sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:"));
    match signed {
        Some(v) if compare(v, version) != Some(Ordering::Equal) => Err(Error::Config(format!(
            "the signature is for version {v}, the release says {version}; nothing was installed"
        ))),
        _ => Ok(()),
    }
}

/// `--user` or `--system`: the installation this Flatpak runs from, from `/.flatpak-info`'s
/// `app-path` (a system install lives under /var/lib/flatpak, or another system path).
pub fn flatpak_installation(info: &str) -> &'static str {
    let app_path = info.lines().find_map(|l| l.trim().strip_prefix("app-path=")).unwrap_or("");
    let system = app_path.starts_with("/var/lib/flatpak/") || app_path.starts_with("/usr/") || app_path.starts_with("/run/");
    if system { "--system" } else { "--user" }
}

/// The `flatpak install` that puts `bundle` in place of the running copy, run on the host.
///
/// Not `--noninteractive`: besides the questions (`-y` answers those) it forbids polkit to ask,
/// and installing a bundle system-wide always needs the administrator's password, so a
/// `--system` install would be refused outright ("InstallBundle not allowed for user") instead
/// of the desktop's password dialog opening.
pub fn flatpak_install_argv(installation: &str, bundle: &Path) -> Vec<std::ffi::OsString> {
    let mut argv: Vec<std::ffi::OsString> =
        ["flatpak", "install", installation, "-y", "--reinstall", "--bundle"].iter().map(Into::into).collect();
    argv.push(bundle.into());
    argv
}

fn download(url: &str, dest: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Vec<u8>> {
    let resp = net::agent(Duration::from_secs(1800)).get(url).call().map_err(|e| net::error(url, e))?;
    let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok());
    if total.is_some_and(|t| t > MAX_BUNDLE) {
        return Err(Error::Config(format!("{url}: larger than any SixLauncher bundle; refused")));
    }
    let mut reader = resp.into_reader().take(MAX_BUNDLE + 1);
    let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        progress(data.len() as u64, total);
    }
    if data.len() as u64 > MAX_BUNDLE || total.is_some_and(|t| t != data.len() as u64) {
        return Err(Error::Config(format!("{url}: the download is incomplete or too large")));
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(dest, &data)?;
    Ok(data)
}

/// Updates this Flatpak to the newest release: downloads its `sixlauncher.flatpak`, checks the
/// signature against `pubkey` and installs it on the host (`flatpak install --reinstall
/// --bundle`, in the installation this copy runs from). The new version runs from the next
/// start. Returns the version installed, or None when nothing newer is published.
pub fn install_flatpak(current: &str, pubkey: &str, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Option<String>> {
    if !host::in_flatpak() {
        return Err(Error::Config("this copy of SixLauncher is not the Flatpak".into()));
    }
    let Some(manifest) = fetch_manifest()? else { return Ok(None) };
    if compare(&manifest.version, current) != Some(Ordering::Greater) {
        return Ok(None);
    }
    let version = manifest.version.trim_start_matches('v').to_string();
    let entry = manifest
        .entry("linux-x86_64-flatpak")
        .ok_or_else(|| Error::Config(format!("release {version} has no signed Flatpak; download it by hand")))?;
    if !entry.url.starts_with(DOWNLOAD_PREFIX) || entry.url.contains("..") {
        return Err(Error::Config(format!("{} is not a SixLauncher release download", entry.url)));
    }
    let bundle: PathBuf = paths::cache_dir().join("downloads").join(format!("sixlauncher-{version}.flatpak"));
    let result = download(&entry.url, &bundle, progress)
        .and_then(|data| verify_signature(&data, &entry.signature, pubkey, &version))
        .and_then(|()| {
            let info = fs::read_to_string("/.flatpak-info").unwrap_or_default();
            let installation = flatpak_installation(&info);
            let out = host::simple(&flatpak_install_argv(installation, &bundle)).output()?;
            if out.status.success() {
                Ok(())
            } else {
                Err(Error::Config(format!(
                    "flatpak install failed ({}): {}. By hand: download sixlauncher.flatpak from the release and run \
                     `flatpak install {installation} --reinstall sixlauncher.flatpak`",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                )))
            }
        });
    let _ = fs::remove_file(&bundle);
    result.map(|()| Some(version))
}

/// Starts the installed (new) Flatpak on the host, so the next window is the new version.
pub fn flatpak_relaunch() -> std::io::Result<std::process::Child> {
    host::simple(&["flatpak".into(), "run".into(), FLATPAK_ID.into()]).spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, keys: &[&str]) -> Manifest {
        Manifest {
            version: version.into(),
            notes: Some("notas".into()),
            pub_date: None,
            platforms: keys.iter().map(|k| (k.to_string(), serde_json::json!({}))).collect(),
        }
    }

    #[test]
    fn compares_versions() {
        assert_eq!(compare("0.1.57", "0.1.9"), Some(Ordering::Greater));
        assert_eq!(compare("v0.2.0", "0.1.99"), Some(Ordering::Greater));
        assert_eq!(compare("0.1.0", "0.1"), Some(Ordering::Equal));
        assert_eq!(compare("1.0.0-beta", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare("0.1.3", "0.1.12"), Some(Ordering::Less));
        assert_eq!(compare("abc", "0.1.0"), None);
    }

    #[test]
    fn trusts_the_stamp_only_where_the_package_lives() {
        let usr = Path::new("/usr/bin/sixlauncher-app");
        let home = Path::new("/home/a/sixlauncher-linux-x64/sixlauncher-app");
        assert_eq!(detect(Some("deb"), usr, None, false), Install::Deb);
        assert_eq!(detect(Some("rpm"), usr, None, false), Install::Rpm);
        // The tarball carries the same stamped binary.
        assert_eq!(detect(Some("rpm"), home, None, false), Install::Other);
        assert_eq!(detect(Some("appimage"), home, Some("/home/a/SixLauncher.AppImage"), false), Install::AppImage);
        assert_eq!(detect(Some("appimage"), home, None, false), Install::Other);
        assert_eq!(detect(None, usr, None, false), Install::Other);
        assert_eq!(detect(Some("deb"), usr, None, true), Install::Flatpak);
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("sixlauncher-app.exe");
        assert_eq!(detect(Some("nsis"), &app, None, false), Install::Other, "the .zip's copy");
        std::fs::write(t.path().join("uninstall.exe"), "").unwrap();
        assert_eq!(detect(Some("nsis"), &app, None, false), Install::Nsis);
    }

    #[test]
    fn automatic_only_with_a_signed_entry_for_this_package() {
        let m = manifest("v0.1.60", &["linux-x86_64", "linux-x86_64-appimage", "linux-x86_64-deb"]);
        let c = evaluate(&m, "0.1.57", Install::AppImage, true);
        assert!(c.newer && c.automatic);
        assert_eq!(c.latest, "0.1.60");
        assert_eq!(c.url, "https://github.com/SixLauncher/launcher/releases/tag/v0.1.60");
        assert!(!evaluate(&m, "0.1.57", Install::Rpm, true).automatic);
        assert!(!evaluate(&m, "0.1.57", Install::Flatpak, true).automatic, "no signed Flatpak in this release");
        assert!(!evaluate(&m, "0.1.57", Install::Deb, false).automatic);
        let same = evaluate(&m, "0.1.60", Install::Deb, true);
        assert!(!same.newer && !same.automatic);
        let flat = manifest("0.1.60", &["linux-x86_64-flatpak"]);
        assert!(evaluate(&flat, "0.1.57", Install::Flatpak, true).automatic);
        let win = manifest("0.1.60", &["windows-x86_64-nsis"]);
        assert!(evaluate(&win, "0.1.57", Install::Nsis, true).automatic);
        assert!(!evaluate(&win, "0.1.57", Install::AppImage, true).automatic);
        let unsigned = evaluate(&manifest("0.1.60", &[]), "0.1.57", Install::AppImage, true);
        assert!(unsigned.newer && !unsigned.automatic);
    }

    // A key and a signature of `DATA` made with `minisign -G -W` and `minisign -S`, written the
    // way Tauri writes them (base64 of the files).
    const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgMUZBOTUxNTE0NzdGQjAxClJXUUIrM2NVRlpYNkFhNXY4eWMwTWpxd3YxdkZ0N2NESDZnZmJ3SDd5VEh4cHA0TWl1MTRUQjFSCg==";
    const SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIG1pbmlzaWduIHNlY3JldCBrZXkKUlVRQiszY1VGWlg2QVNjaGpIMFlFKy83TlE1MzV0RWxJWkVQRVFaczZlYVdIQnFYb0F6TitGZmh5NU90YkdJazc1QnFzbzFvWXhpRnJjaEtQaUZwSnV0cE1jOWRTaEVPWmdFPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NjQ4OTYzCWZpbGU6ZGF0YS5iaW4JaGFzaGVkCjhXZWlnRTZtbC9HYk9sUlI1RnFzNjRIMjZwc1dpYzExMG1tdFQ0Rmt1M0dnY1BQaHFXSUhFcWl6WmkwRS9GOGFkSmRtUnNUdmZIL1g5R2NiV2pOeUFnPT0K";
    const DATA: &[u8] = b"sixlauncher test bundle\n";

    #[test]
    fn flatpak_bundle_must_match_its_signature() {
        verify_signature(DATA, SIG, PUBKEY, "0.1.60").unwrap();
        let err = verify_signature(b"sixlauncher test bundle, changed\n", SIG, PUBKEY, "0.1.60").unwrap_err();
        assert!(err.to_string().contains("does not match its signature"), "{err}");
        assert!(verify_signature(DATA, "not a signature", PUBKEY, "0.1.60").is_err());
        assert!(verify_signature(DATA, SIG, "", "0.1.60").is_err());
    }

    #[test]
    fn flatpak_installs_where_it_runs() {
        let user = "[Application]\nname=io.github.lportvi.sixlauncher\n[Instance]\napp-path=/home/eu/.local/share/flatpak/app/io.github.lportvi.sixlauncher/x86_64/master/abc/files\n";
        let system = "[Instance]\napp-path=/var/lib/flatpak/app/io.github.lportvi.sixlauncher/x86_64/master/abc/files\n";
        assert_eq!(flatpak_installation(user), "--user");
        assert_eq!(flatpak_installation(system), "--system");
        let argv = flatpak_install_argv("--user", Path::new("/home/eu/.var/app/x/cache/sixlauncher/downloads/s.flatpak"));
        let argv: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            argv,
            ["flatpak", "install", "--user", "-y", "--reinstall", "--bundle", "/home/eu/.var/app/x/cache/sixlauncher/downloads/s.flatpak"]
        );
        // polkit must be allowed to ask for the password of a system install.
        let system = flatpak_install_argv("--system", Path::new("/tmp/s.flatpak"));
        assert!(!system.iter().any(|a| a == "--noninteractive"));
    }

    #[test]
    fn manifest_entries() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.1.60","platforms":{"linux-x86_64-flatpak":{"url":"https://github.com/SixLauncher/launcher/releases/download/v0.1.60/sixlauncher.flatpak","signature":"c2ln"}}}"#,
        )
        .unwrap();
        assert_eq!(m.entry("linux-x86_64-flatpak").unwrap().signature, "c2ln");
        assert!(m.entry("linux-x86_64-deb").is_none());
    }
}

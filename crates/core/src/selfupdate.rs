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
//! cannot act (a Flatpak, the tarball, a build from source).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{net, Error, Result};

pub const REPO: &str = "SixLauncher/launcher";
pub const MANIFEST_URL: &str = "https://github.com/SixLauncher/launcher/releases/latest/download/latest.json";
pub const RELEASES_URL: &str = "https://github.com/SixLauncher/launcher/releases/latest";

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
            Install::Flatpak | Install::Other => None,
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
        assert!(!evaluate(&m, "0.1.57", Install::Flatpak, true).automatic);
        assert!(!evaluate(&m, "0.1.57", Install::Deb, false).automatic);
        let same = evaluate(&m, "0.1.60", Install::Deb, true);
        assert!(!same.newer && !same.automatic);
        let win = manifest("0.1.60", &["windows-x86_64-nsis"]);
        assert!(evaluate(&win, "0.1.57", Install::Nsis, true).automatic);
        assert!(!evaluate(&win, "0.1.57", Install::AppImage, true).automatic);
        let unsigned = evaluate(&manifest("0.1.60", &[]), "0.1.57", Install::AppImage, true);
        assert!(unsigned.newer && !unsigned.automatic);
    }
}

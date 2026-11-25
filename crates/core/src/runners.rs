//! Wine and Proton: what runs the Windows build of VIctory or XCL on Linux.
//!
//! Wine's own `d3d12.dll` (upstream vkd3d) offers neither shader model 6.6 nor
//! mesh shaders, which both backends need, so vkd3d-proton is required:
//!
//! * Proton and GE-Proton ship vkd3d-proton and use it by default.
//! * With a system Wine, `setup` copies vkd3d-proton's x64 `d3d12.dll` and
//!   `d3d12core.dll` into the prefix, and runs set `WINEDLLOVERRIDES`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::config::{Config, RunnerKind};
use crate::{host, paths};
use crate::{Error, Result};

pub const VKD3D_DLLS: [&str; 2] = ["d3d12.dll", "d3d12core.dll"];
pub const D3D12_OVERRIDE: &str = "d3d12,d3d12core=n,b";
/// Wine's placeholder DLLs in system32 carry this marker after the DOS header.
pub const WINE_BUILTIN_MARKER: &[u8] = b"Wine builtin DLL";

/// `add` plus whatever `existing` sets for other DLLs (ours win).
pub fn merge_dll_overrides(existing: Option<&str>, add: &str) -> String {
    let ours: Vec<String> = add.split('=').next().unwrap_or("").split(',').map(|d| d.trim().to_lowercase()).collect();
    let mut parts = vec![add.to_string()];
    for part in existing.unwrap_or("").split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let (dlls, mode) = match part.split_once('=') {
            Some((d, m)) => (d, Some(m)),
            None => (part, None),
        };
        let rest: Vec<&str> = dlls.split(',').map(str::trim).filter(|d| !ours.contains(&d.to_lowercase())).collect();
        if !rest.is_empty() {
            parts.push(match mode {
                Some(m) => format!("{}={m}", rest.join(",")),
                None => rest.join(","),
            });
        }
    }
    parts.join(";")
}

pub fn is_wine_placeholder(dll: &Path) -> bool {
    let mut head = [0u8; 0x200];
    let Ok(mut f) = fs::File::open(dll) else { return false };
    let n = f.read(&mut head).unwrap_or(0);
    head[..n].windows(WINE_BUILTIN_MARKER.len()).any(|w| w == WINE_BUILTIN_MARKER)
}

fn has_vkd3d(dir: &Path) -> bool {
    VKD3D_DLLS.iter().all(|d| dir.join(d).is_file())
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() && !p.is_symlink() {
            out.push(p.clone());
            walk(&p, depth - 1, out);
        }
    }
}

/// The folder holding vkd3d-proton's 64-bit DLLs in `root`: an unpacked
/// vkd3d-proton release (`x64/`) or a Proton folder
/// (`files/lib/wine/vkd3d-proton/x86_64-windows`, `files/lib64/wine/vkd3d-proton`).
pub fn vkd3d_x64_dir(root: &Path) -> Option<PathBuf> {
    for cand in [root.join("x64"), root.to_path_buf()] {
        if has_vkd3d(&cand) {
            return Some(cand);
        }
    }
    let mut dirs = Vec::new();
    walk(root, 7, &mut dirs);
    dirs.into_iter().find(|d| {
        let s = d.to_string_lossy().to_lowercase();
        let is_64 = s.contains("x86_64-windows") || s.contains("/lib64/") || s.ends_with("/x64");
        is_64 && s.contains("vkd3d") && has_vkd3d(d)
    })
}

// -- discovery ---------------------------------------------------------------------------------

pub fn steam_roots(home: &Path) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for c in [".steam/root", ".steam/steam", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/data/Steam"] {
        let p = home.join(c);
        if p.is_dir() {
            let r = fs::canonicalize(&p).unwrap_or(p.clone());
            if !seen.contains(&r) {
                seen.push(r);
                out.push(p);
            }
        }
    }
    out
}

/// Who installed a Proton folder.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Steam (Proton Experimental, or a tool in Steam's compatibilitytools.d).
    Steam,
    /// VILauncher itself ([`crate::protons`]); only these can be removed from the launcher.
    Launcher,
    /// The distribution's package (/usr/share/steam/compatibilitytools.d).
    System,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProtonInstall {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub origin: Origin,
}

pub fn proton_version(dir: &Path) -> String {
    fs::read_to_string(dir.join("version"))
        .ok()
        .and_then(|t| t.split_whitespace().last().map(str::to_owned))
        .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

fn rank(name: &str) -> (u8, Vec<std::cmp::Reverse<u64>>, String) {
    if let Some(rest) = name.strip_prefix("GE-Proton") {
        let nums = rest.split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse().ok()).map(std::cmp::Reverse).collect();
        return (0, nums, name.to_owned());
    }
    if name.contains("Experimental") {
        return (1, vec![], name.to_owned());
    }
    (2, vec![], name.to_owned())
}

/// Proton folders (each has a `proton` script), best guess first:
/// GE-Proton newest first, then Proton Experimental, then other Protons. The versions
/// VILauncher downloaded ([`crate::protons::managed_dir`]) are found along with Steam's.
pub fn find_protons(home: &Path) -> Vec<ProtonInstall> {
    let mut dirs = vec![(crate::protons::managed_dir(), Origin::Launcher)];
    for root in steam_roots(home) {
        dirs.push((root.join("compatibilitytools.d"), Origin::Steam));
        dirs.push((root.join("steamapps/common"), Origin::Steam));
    }
    dirs.push((PathBuf::from("/usr/share/steam/compatibilitytools.d"), Origin::System));
    let mut seen = Vec::new();
    let mut found = Vec::new();
    for (d, origin) in dirs {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for entry in rd.flatten().map(|e| e.path()) {
            let hidden = entry.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if !hidden && entry.join("proton").is_file() {
                let r = fs::canonicalize(&entry).unwrap_or(entry.clone());
                if !seen.contains(&r) {
                    seen.push(r);
                    let name = entry.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    found.push(ProtonInstall { version: proton_version(&entry), name, path: entry, origin });
                }
            }
        }
    }
    found.sort_by_key(|p| rank(&p.name));
    found
}

pub fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

// -- runners -----------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WineRunner {
    pub wine: PathBuf,
    pub prefix: PathBuf,
    pub vkd3d_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ProtonRunner {
    pub proton_dir: PathBuf,
    pub compat_dir: PathBuf,
    pub steam_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum Runner {
    Wine(WineRunner),
    Proton(ProtonRunner),
}

impl WineRunner {
    pub fn system32(&self) -> PathBuf {
        self.prefix.join("drive_c/windows/system32")
    }

    fn setup(&self, log: &mut dyn FnMut(String)) -> Result<()> {
        if !self.prefix.join("system.reg").is_file() {
            log(format!("creating the Wine prefix {} (wineboot)", self.prefix.display()));
            fs::create_dir_all(&self.prefix)?;
            let env = Runner::Wine(self.clone()).env(&std::env::vars().collect());
            let status = host::command(&[self.wine.clone().into(), "wineboot".into(), "-u".into()], &env, None).status()?;
            if !status.success() {
                return Err(Error::Runner(format!("wineboot failed ({status})")));
            }
            let server = self.wine.with_file_name("wineserver");
            let server = if server.is_file() { server } else { PathBuf::from("wineserver") };
            let _ = host::command(&[server.into(), "-w".into()], &env, None).status();
        }
        let Some(vkd3d) = &self.vkd3d_dir else {
            return Err(Error::Runner(
                "the wine runner needs vkd3d-proton: set the vkd3d-proton folder (an unpacked vkd3d-proton release)".into(),
            ));
        };
        let src = vkd3d_x64_dir(vkd3d).ok_or_else(|| {
            Error::Runner(format!("no 64-bit vkd3d-proton DLLs (x64/d3d12.dll, x64/d3d12core.dll) in {}", vkd3d.display()))
        })?;
        let dst = self.system32();
        fs::create_dir_all(&dst)?;
        for dll in VKD3D_DLLS {
            log(format!("installing vkd3d-proton {dll} into {}", dst.display()));
            let target = dst.join(dll);
            if target.symlink_metadata().is_ok() {
                fs::remove_file(&target)?;
            }
            fs::copy(src.join(dll), &target)?;
        }
        Ok(())
    }
}

impl Runner {
    pub fn from_config(cfg: &Config) -> Result<Runner> {
        match cfg.runner.kind {
            RunnerKind::Wine => {
                let wine = cfg.runner_path().or_else(|| which("wine")).ok_or_else(|| {
                    Error::Runner("no wine found: install Wine (9.0 or newer) or set the wine binary".into())
                })?;
                Ok(Runner::Wine(WineRunner { wine, prefix: cfg.prefix(), vkd3d_dir: cfg.vkd3d_dir() }))
            }
            RunnerKind::Proton => {
                let proton = cfg
                    .runner_path()
                    .or_else(|| find_protons(&paths::home()).into_iter().next().map(|p| p.path))
                    .ok_or_else(|| {
                        Error::Runner(
                            "no Proton found: download GE-Proton in the launcher (`vilauncher proton install latest`), \
                             install Proton Experimental through Steam, or choose a Proton folder"
                                .into(),
                        )
                    })?;
                if !proton.join("proton").is_file() {
                    return Err(Error::Runner(format!("{} is not a Proton folder (no `proton` script in it)", proton.display())));
                }
                Ok(Runner::Proton(ProtonRunner { proton_dir: proton, compat_dir: cfg.prefix(), steam_dir: None }))
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Runner::Wine(_) => "wine",
            Runner::Proton(_) => "proton",
        }
    }

    /// The Wine prefix the program runs in.
    pub fn wineprefix(&self) -> PathBuf {
        match self {
            Runner::Wine(w) => w.prefix.clone(),
            Runner::Proton(p) => p.compat_dir.join("pfx"),
        }
    }

    /// The variables this runner adds to `base` (the environment of the run).
    pub fn env(&self, base: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        match self {
            Runner::Wine(w) => {
                env.insert("WINEPREFIX".into(), w.prefix.to_string_lossy().into_owned());
                env.insert("WINEARCH".into(), "win64".into());
                env.insert(
                    "WINEDLLOVERRIDES".into(),
                    merge_dll_overrides(base.get("WINEDLLOVERRIDES").map(String::as_str), D3D12_OVERRIDE),
                );
                env.insert("WINEDEBUG".into(), base.get("WINEDEBUG").cloned().unwrap_or_else(|| "-all".into()));
            }
            Runner::Proton(p) => {
                let steam = p
                    .steam_dir
                    .clone()
                    .or_else(|| steam_roots(&paths::home()).into_iter().next())
                    .unwrap_or_else(|| p.compat_dir.clone());
                env.insert("STEAM_COMPAT_DATA_PATH".into(), p.compat_dir.to_string_lossy().into_owned());
                env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".into(), steam.to_string_lossy().into_owned());
                // With PROTON_LOG=1, Proton writes steam-*.log here, where the report finds it.
                if !base.get("PROTON_LOG_DIR").is_some_and(|d| !d.is_empty()) {
                    env.insert("PROTON_LOG_DIR".into(), crate::report::proton_log_dir().to_string_lossy().into_owned());
                }
            }
        }
        env
    }

    pub fn argv(&self, exe: &Path, args: &[String]) -> Vec<String> {
        let mut argv = match self {
            Runner::Wine(w) => vec![w.wine.to_string_lossy().into_owned()],
            Runner::Proton(p) => vec![p.proton_dir.join("proton").to_string_lossy().into_owned(), "waitforexitandrun".into()],
        };
        argv.push(exe.to_string_lossy().into_owned());
        argv.extend(args.iter().cloned());
        argv
    }

    /// Prepares the prefix: wineboot and vkd3d-proton for Wine, the folder for Proton.
    pub fn setup(&self, log: &mut dyn FnMut(String)) -> Result<()> {
        match self {
            Runner::Wine(w) => w.setup(log),
            Runner::Proton(p) => {
                fs::create_dir_all(&p.compat_dir)?;
                log(format!("Proton prefix folder: {} (Proton fills it on the first run)", p.compat_dir.display()));
                Ok(())
            }
        }
    }

    /// Stops every Windows process in the prefix (`wineserver -k`): stopping the
    /// launcher's child alone can leave the game running under Wine.
    pub fn kill_command(&self) -> Command {
        let server = match self {
            Runner::Wine(w) => w.wine.with_file_name("wineserver"),
            Runner::Proton(p) => p.proton_dir.join("files/bin/wineserver"),
        };
        let server = if server.is_file() { server } else { PathBuf::from("wineserver") };
        let mut env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
        env.insert("WINEPREFIX".into(), self.wineprefix().to_string_lossy().into_owned());
        host::command(&[server.into(), "-k".into()], &env, None)
    }

    pub fn vkd3d_installed(&self) -> bool {
        match self {
            Runner::Wine(w) => VKD3D_DLLS.iter().all(|d| {
                let p = w.system32().join(d);
                p.is_file() && !is_wine_placeholder(&p)
            }),
            Runner::Proton(p) => vkd3d_x64_dir(&p.proton_dir).is_some(),
        }
    }

    pub fn version(&self) -> String {
        match self {
            Runner::Wine(w) => match host::simple(&[w.wine.clone().into(), "--version".into()]).output() {
                Ok(o) => {
                    let s = String::from_utf8_lossy(if o.stdout.is_empty() { &o.stderr } else { &o.stdout }).trim().to_owned();
                    if s.is_empty() { "unknown".into() } else { s }
                }
                Err(e) => format!("unknown ({e})"),
            },
            Runner::Proton(p) => proton_version(&p.proton_dir),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, TempHome};

    #[test]
    fn dll_overrides_keep_other_dlls() {
        assert_eq!(merge_dll_overrides(None, D3D12_OVERRIDE), "d3d12,d3d12core=n,b");
        assert_eq!(
            merge_dll_overrides(Some("dxgi=n;d3d12=b;xinput1_3,d3d12core=n"), D3D12_OVERRIDE),
            "d3d12,d3d12core=n,b;dxgi=n;xinput1_3=n"
        );
    }

    #[test]
    fn vkd3d_found_in_release_and_in_proton() {
        let t = tempfile::tempdir().unwrap();
        let release = t.path().join("vkd3d-proton-2.14");
        for dll in VKD3D_DLLS {
            testutil::fake_pe(&release.join("x64").join(dll), 0x8664);
            testutil::fake_pe(&release.join("x86").join(dll), 0x14C);
        }
        assert_eq!(vkd3d_x64_dir(&release), Some(release.join("x64")));
        let proton = testutil::fake_proton(t.path(), "GE-Proton9-20");
        assert_eq!(vkd3d_x64_dir(&proton), Some(proton.join("files/lib/wine/vkd3d-proton/x86_64-windows")));
        assert_eq!(vkd3d_x64_dir(&t.path().join("nothing")), None);
    }

    #[test]
    fn proton_discovery_prefers_newest_ge() {
        let h = TempHome::new();
        let tools = h.home().join(".local/share/Steam/compatibilitytools.d");
        let common = h.home().join(".local/share/Steam/steamapps/common");
        for name in ["GE-Proton9-2", "GE-Proton10-1", "GE-Proton9-20"] {
            testutil::fake_proton(&tools, name);
        }
        testutil::fake_proton(&common, "Proton - Experimental");
        fs::create_dir_all(common.join("Some Game")).unwrap();
        let found: Vec<String> = find_protons(&h.home()).into_iter().map(|p| p.name).collect();
        assert_eq!(found, ["GE-Proton10-1", "GE-Proton9-20", "GE-Proton9-2", "Proton - Experimental"]);
    }

    #[test]
    fn wine_setup_installs_vkd3d_over_placeholders() {
        let t = tempfile::tempdir().unwrap();
        let prefix = testutil::fake_wine_prefix(&t.path().join("pfx"));
        let sys32 = prefix.join("drive_c/windows/system32");
        let mut placeholder = b"MZ".to_vec();
        placeholder.extend([0u8; 0x3E]);
        placeholder.extend(WINE_BUILTIN_MARKER);
        for dll in VKD3D_DLLS {
            fs::write(sys32.join(dll), &placeholder).unwrap();
        }
        let release = t.path().join("vkd3d");
        for dll in VKD3D_DLLS {
            testutil::fake_pe(&release.join("x64").join(dll), 0x8664);
        }
        let r = Runner::Wine(WineRunner { wine: "/nonexistent/wine".into(), prefix: prefix.clone(), vkd3d_dir: Some(release.clone()) });
        assert!(!r.vkd3d_installed());
        r.setup(&mut |_| {}).unwrap(); // the prefix exists, so wine itself is not started
        assert!(r.vkd3d_installed());
        assert_eq!(fs::read(sys32.join("d3d12core.dll")).unwrap(), fs::read(release.join("x64/d3d12core.dll")).unwrap());
    }

    #[test]
    fn wine_setup_without_vkd3d_fails() {
        let t = tempfile::tempdir().unwrap();
        let prefix = testutil::fake_wine_prefix(&t.path().join("pfx"));
        let r = Runner::Wine(WineRunner { wine: "/nonexistent/wine".into(), prefix, vkd3d_dir: None });
        let err = r.setup(&mut |_| {}).unwrap_err().to_string();
        assert!(err.contains("vkd3d"), "{err}");
    }
}

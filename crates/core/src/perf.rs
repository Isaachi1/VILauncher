//! MangoHud (the in-game overlay: FPS, frame time, GPU and CPU load) and Feral's
//! GameMode (CPU governor, GPU clocks and I/O priority while the game runs).
//!
//! Both act on the program that runs, and the game runs on the host even when the
//! launcher is a Flatpak (see [`crate::host`]). So what counts is a copy on the host:
//!
//! * MangoHud is a Vulkan layer, turned on with `MANGOHUD=1` (vkd3d-proton renders
//!   through Vulkan) and set up with `MANGOHUD_CONFIG`; no wrapper is needed.
//! * GameMode wraps the command: `gamemoderun proton …` preloads `libgamemodeauto`,
//!   which asks the `gamemoded` daemon to switch on.
//!
//! The Flatpak extensions (`org.freedesktop.Platform.VulkanLayer.MangoHud`,
//! `com.valvesoftware.Steam.Utility.gamemode`) only reach programs inside a Flatpak,
//! such as the Steam Flatpak. They are found and reported, so the window can say why
//! they do not count and what to install instead.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use crate::config::{Config, PerformanceSection};
use crate::{host, paths};

pub const MANGOHUD_FLATPAK: &str = "org.freedesktop.Platform.VulkanLayer.MangoHud";
pub const GAMEMODE_FLATPAK: &str = "com.valvesoftware.Steam.Utility.gamemode";
/// What the overlay shows when `[performance] mangohud_config` is not set.
pub const MANGOHUD_DEFAULT: &str = "fps,frametime,gpu_stats,cpu_stats,ram,vram";

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Tool {
    /// Installed on the host, where the game runs.
    pub installed: bool,
    /// Where it was found (a binary, a layer manifest), when known.
    pub path: Option<String>,
    /// Flatpak extensions with it that are installed (system or user).
    pub flatpak: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Tools {
    pub mangohud: Tool,
    pub gamemode: Tool,
    /// The launcher itself is a Flatpak (it looked on the host through flatpak-spawn).
    pub sandboxed: bool,
}

/// The Vulkan layer manifests MangoHud installs, under a data dir (`/usr/share`, `~/.local/share`).
const LAYER_DIRS: [&str; 2] = ["vulkan/implicit_layer.d", "vulkan/explicit_layer.d"];

fn mangohud_layer(data_dirs: &[PathBuf]) -> Option<PathBuf> {
    for d in data_dirs {
        for sub in LAYER_DIRS {
            let Ok(rd) = fs::read_dir(d.join(sub)) else { continue };
            let mut found: Vec<PathBuf> =
                rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("mangohud"))).collect();
            found.sort();
            if let Some(p) = found.into_iter().next() {
                return Some(p);
            }
        }
    }
    None
}

/// Installed Flatpak extensions named `id` under these Flatpak installations
/// (`/var/lib/flatpak`, `~/.local/share/flatpak`): each as `id//branch`.
pub fn flatpak_extensions(installs: &[PathBuf], id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for inst in installs {
        let dir = inst.join("runtime").join(id);
        let Ok(arches) = fs::read_dir(&dir) else { continue };
        for arch in arches.flatten() {
            let Ok(branches) = fs::read_dir(arch.path()) else { continue };
            for b in branches.flatten() {
                if b.path().join("active").exists() {
                    let name = format!("{id}//{}", b.file_name().to_string_lossy());
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// What is installed, looking at the given roots: tested with fake ones.
pub fn detect_in(path_dirs: &[PathBuf], data_dirs: &[PathBuf], flatpak_installs: &[PathBuf]) -> Tools {
    let find = |name: &str| path_dirs.iter().map(|d| d.join(name)).find(|f| f.is_file());
    let mangohud = {
        let bin = find("mangohud");
        let layer = mangohud_layer(data_dirs);
        Tool {
            installed: bin.is_some() || layer.is_some(),
            path: layer.or(bin).map(|p| p.display().to_string()),
            flatpak: flatpak_extensions(flatpak_installs, MANGOHUD_FLATPAK),
        }
    };
    let gamemode = {
        let bin = find("gamemoderun");
        Tool {
            installed: bin.is_some(),
            path: bin.map(|p| p.display().to_string()),
            flatpak: flatpak_extensions(flatpak_installs, GAMEMODE_FLATPAK),
        }
    };
    Tools { mangohud, gamemode, sandboxed: false }
}

fn split_env(var: &str, default: &str) -> Vec<PathBuf> {
    let v = std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.into());
    std::env::split_paths(&v).collect()
}

/// Asks the host's shell whether a program or a file is there (Flatpak only).
fn host_has(script: &str) -> Option<String> {
    let out = host::simple(&["sh".into(), "-c".into(), OsString::from(script)]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// What is installed on this machine.
pub fn detect() -> Tools {
    let home = paths::home();
    let installs = vec![PathBuf::from("/var/lib/flatpak"), home.join(".local/share/flatpak")];
    if host::in_flatpak() {
        // The sandbox has its own /usr: ask the host.
        let mut t = detect_in(&[], &[], &installs);
        t.sandboxed = true;
        if let Some(p) = host_has(
            "for f in /usr/share/vulkan/implicit_layer.d/MangoHud* \"$HOME\"/.local/share/vulkan/implicit_layer.d/MangoHud*; do \
             [ -e \"$f\" ] && { echo \"$f\"; exit 0; }; done; command -v mangohud",
        ) {
            t.mangohud.installed = true;
            t.mangohud.path = p.lines().next().map(str::to_owned);
        }
        if let Some(p) = host_has("command -v gamemoderun") {
            t.gamemode.installed = true;
            t.gamemode.path = Some(p);
        }
        return t;
    }
    let mut data: Vec<PathBuf> = paths::data_dir().parent().map(PathBuf::from).into_iter().collect();
    data.extend(split_env("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    data.push(PathBuf::from("/etc"));
    let path_dirs = split_env("PATH", "/usr/local/bin:/usr/bin:/bin");
    detect_in(&path_dirs, &data, &installs)
}

/// The variables `[performance]` adds to a run.
pub fn env_for(perf: &PerformanceSection) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if perf.mangohud {
        env.insert("MANGOHUD".into(), "1".into());
        let mut conf = perf.mangohud_config.clone().filter(|c| !c.trim().is_empty()).unwrap_or_else(|| MANGOHUD_DEFAULT.into());
        if let Some(limit) = perf.fps_limit.filter(|l| *l > 0) {
            conf = format!("{conf},fps_limit={limit}");
        }
        env.insert("MANGOHUD_CONFIG".into(), conf);
    }
    env
}

/// The command that wraps the run, if any: `gamemoderun` when GameMode is on and found.
pub fn wrapper(perf: &PerformanceSection, tools: &Tools) -> Vec<String> {
    if !perf.gamemode || !tools.gamemode.installed {
        return Vec::new();
    }
    // In a Flatpak the path is the host's, which flatpak-spawn --host runs as is.
    vec![tools.gamemode.path.clone().unwrap_or_else(|| "gamemoderun".into())]
}

/// Whether any of `[performance]` needs a look at what is installed.
pub fn wanted(cfg: &Config) -> bool {
    cfg.performance.mangohud || cfg.performance.gamemode
}

/// A line for the doctor about one tool that was turned on.
pub fn describe(package: &str, t: &Tool, sandboxed: bool) -> (bool, String) {
    if t.installed {
        let at = t.path.as_deref().map(|p| format!(": {p}")).unwrap_or_default();
        return (true, format!("installed{at}"));
    }
    let mut msg = format!("not installed on the system; install the `{package}` package");
    if !t.flatpak.is_empty() {
        msg += &format!(
            ". The Flatpak extension {} only works for Flatpak apps (such as the Steam Flatpak); the game runs on the host",
            t.flatpak.join(", ")
        );
    }
    if sandboxed {
        msg += " (looked on the host through flatpak-spawn)";
    }
    (false, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn finds_native_tools_and_flatpak_extensions() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let none = detect_in(&[root.join("bin")], &[root.join("share")], &[root.join("flatpak")]);
        assert!(!none.mangohud.installed && !none.gamemode.installed);

        testutil::executable(&root.join("bin/gamemoderun"), "#!/bin/sh\nexec \"$@\"\n");
        fs::create_dir_all(root.join("share/vulkan/implicit_layer.d")).unwrap();
        fs::write(root.join("share/vulkan/implicit_layer.d/MangoHud.x86_64.json"), "{}").unwrap();
        let ext = root.join("flatpak/runtime").join(MANGOHUD_FLATPAK).join("x86_64/24.08");
        fs::create_dir_all(ext.join("active")).unwrap();
        let found = detect_in(&[root.join("bin")], &[root.join("share")], &[root.join("flatpak")]);
        assert!(found.mangohud.installed && found.gamemode.installed);
        assert!(found.mangohud.path.unwrap().ends_with("MangoHud.x86_64.json"));
        assert_eq!(found.mangohud.flatpak, [format!("{MANGOHUD_FLATPAK}//24.08")]);
        assert!(found.gamemode.flatpak.is_empty());
    }

    #[test]
    fn flatpak_only_mangohud_is_explained() {
        let tool = Tool { installed: false, path: None, flatpak: vec![format!("{MANGOHUD_FLATPAK}//24.08")] };
        let (ok, msg) = describe("mangohud", &tool, false);
        assert!(!ok);
        assert!(msg.contains("only works for Flatpak apps"), "{msg}");
    }

    #[test]
    fn overlay_variables() {
        let mut p = PerformanceSection::default();
        assert!(env_for(&p).is_empty());
        p.mangohud = true;
        assert_eq!(env_for(&p)["MANGOHUD_CONFIG"], MANGOHUD_DEFAULT);
        p.fps_limit = Some(60);
        p.mangohud_config = Some("fps".into());
        let env = env_for(&p);
        assert_eq!((env["MANGOHUD"].as_str(), env["MANGOHUD_CONFIG"].as_str()), ("1", "fps,fps_limit=60"));
    }

    #[test]
    fn gamemode_wraps_only_when_found() {
        let mut p = PerformanceSection { gamemode: true, ..Default::default() };
        let mut tools = Tools::default();
        assert!(wrapper(&p, &tools).is_empty());
        tools.gamemode = Tool { installed: true, path: Some("/usr/bin/gamemoderun".into()), flatpak: vec![] };
        assert_eq!(wrapper(&p, &tools), ["/usr/bin/gamemoderun"]);
        p.gamemode = false;
        assert!(wrapper(&p, &tools).is_empty());
    }
}

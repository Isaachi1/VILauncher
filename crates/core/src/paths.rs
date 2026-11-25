//! Where SixLauncher keeps its files, and how Linux paths look from inside Wine.

use std::env;
use std::path::{Component, Path, PathBuf};

pub const APP: &str = "sixlauncher";

/// VIctory marks its source tree with this file and refuses to write
/// game-derived data below it (docs/asset-policy.md in VILinux).
pub const VICTORY_ROOT_MARKER: &str = ".victory-root";

pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join(APP)
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join(APP)
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join(APP)
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join(APP)
}

pub fn applications_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("applications")
}

/// The name the launcher used before it became SixLauncher, and its old app id.
const LEGACY_APP: &str = "vilauncher";
const LEGACY_FLATPAK_ID: &str = "io.github.lportvi.vilauncher";

/// Moves the folders of the old VILauncher name to the SixLauncher ones, once, so an
/// update keeps the configuration, installed builds, Protons and prefix. A folder is
/// only moved when the new one does not exist yet; paths inside `config.toml` that
/// pointed into the old folders are rewritten. Returns the folders that were moved.
pub fn migrate_legacy_dirs() -> Vec<(PathBuf, PathBuf)> {
    let flatpak = env::var_os("FLATPAK_ID").is_some();
    let kinds = [
        ("XDG_CONFIG_HOME", ".config", "config"),
        ("XDG_DATA_HOME", ".local/share", "data"),
        ("XDG_CACHE_HOME", ".cache", "cache"),
        ("XDG_STATE_HOME", ".local/state", ".local/state"),
    ];
    let pairs: Vec<(Vec<PathBuf>, PathBuf)> = kinds
        .iter()
        .map(|(var, fallback, sandbox)| {
            let base = xdg(var, fallback);
            let mut old = vec![base.join(LEGACY_APP)];
            if flatpak {
                old.push(home().join(".var/app").join(LEGACY_FLATPAK_ID).join(sandbox).join(LEGACY_APP));
            }
            (old, base.join(APP))
        })
        .collect();
    migrate_dirs(&pairs, &config_dir().join("config.toml"))
}

fn migrate_dirs(pairs: &[(Vec<PathBuf>, PathBuf)], config: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut moved = Vec::new();
    for (candidates, new) in pairs {
        if new.exists() {
            continue;
        }
        let Some(old) = candidates.iter().find(|o| o.is_dir()) else { continue };
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::rename(old, new).is_ok() {
            moved.push((old.clone(), new.clone()));
        }
    }
    if let (false, Ok(text)) = (moved.is_empty(), std::fs::read_to_string(config)) {
        let mut fixed = text.clone();
        for (old, new) in &moved {
            fixed = fixed.replace(&*old.to_string_lossy(), &new.to_string_lossy());
        }
        if fixed != text {
            let _ = std::fs::write(config, fixed);
        }
    }
    moved
}

pub fn config_file() -> PathBuf {
    match env::var_os("SIXLAUNCHER_CONFIG") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => config_dir().join("config.toml"),
    }
}

/// `~/x` -> `$HOME/x`; relative paths are taken from the current folder.
pub fn expand(path: &Path) -> PathBuf {
    let p = match path.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) => path.to_path_buf(),
    };
    if p.is_absolute() {
        p
    } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(p)
    }
}

/// `.` and `..` resolved by name, without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
            _ => {}
        }
    }
    out
}

/// The path as a program inside Wine sees it.
///
/// A new Wine or Proton prefix maps drive Z: to the Linux root, so
/// `/home/me/game` is `Z:\home\me\game`. Wine does not translate command-line
/// arguments or environment variables, so every path handed to the Windows
/// program goes through here. `doctor` checks the Z: mapping.
pub fn to_windows(path: &Path) -> String {
    let p = normalize(&expand(path));
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

/// The VIctory source tree containing `path`, if any.
pub fn inside_victory_tree(path: &Path) -> Option<PathBuf> {
    let p = normalize(&expand(path));
    p.ancestors().find(|a| a.join(VICTORY_ROOT_MARKER).is_file()).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_windows_maps_through_drive_z() {
        assert_eq!(to_windows(Path::new("/home/me/My Game/x.exe")), r"Z:\home\me\My Game\x.exe");
        assert_eq!(to_windows(Path::new("/a/b/../c/./d")), r"Z:\a\c\d");
    }

    #[test]
    fn legacy_dirs_move_once_and_config_paths_follow() {
        let t = tempfile::tempdir().unwrap();
        let (old_cfg, new_cfg) = (t.path().join("config/vilauncher"), t.path().join("config/sixlauncher"));
        let (old_data, new_data) = (t.path().join("share/vilauncher"), t.path().join("share/sixlauncher"));
        let (old_cache, new_cache) = (t.path().join("cache/vilauncher"), t.path().join("cache/sixlauncher"));
        std::fs::create_dir_all(old_data.join("protons")).unwrap();
        std::fs::create_dir_all(&old_cfg).unwrap();
        std::fs::create_dir_all(&old_cache).unwrap();
        std::fs::create_dir_all(&new_cache).unwrap();
        let prefix = format!("prefix = \"{}/compatdata\"\n", old_data.display());
        std::fs::write(old_cfg.join("config.toml"), &prefix).unwrap();
        let pairs = vec![
            (vec![old_cfg.clone()], new_cfg.clone()),
            (vec![t.path().join("nowhere"), old_data.clone()], new_data.clone()),
            (vec![old_cache.clone()], new_cache.clone()),
        ];
        let moved = migrate_dirs(&pairs, &new_cfg.join("config.toml"));
        assert_eq!(moved.len(), 2);
        assert!(new_data.join("protons").is_dir() && !old_data.exists());
        assert!(old_cache.is_dir(), "an existing new folder is never overwritten");
        let text = std::fs::read_to_string(new_cfg.join("config.toml")).unwrap();
        assert_eq!(text, format!("prefix = \"{}/compatdata\"\n", new_data.display()));
        assert!(migrate_dirs(&pairs, &new_cfg.join("config.toml")).is_empty());
    }

    #[test]
    fn victory_tree_is_found_from_below() {
        let t = tempfile::tempdir().unwrap();
        let tree = t.path().join("VILinux");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join(VICTORY_ROOT_MARKER), "marker").unwrap();
        assert_eq!(inside_victory_tree(&tree.join("sub/cache")), Some(tree));
        assert_eq!(inside_victory_tree(&t.path().join("elsewhere")), None);
    }
}

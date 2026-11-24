//! Where VILauncher keeps its files, and how Linux paths look from inside Wine.

use std::env;
use std::path::{Component, Path, PathBuf};

pub const APP: &str = "vilauncher";

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

pub fn config_file() -> PathBuf {
    match env::var_os("VILAUNCHER_CONFIG") {
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
    fn victory_tree_is_found_from_below() {
        let t = tempfile::tempdir().unwrap();
        let tree = t.path().join("VILinux");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join(VICTORY_ROOT_MARKER), "marker").unwrap();
        assert_eq!(inside_victory_tree(&tree.join("sub/cache")), Some(tree));
        assert_eq!(inside_victory_tree(&t.path().join("elsewhere")), None);
    }
}

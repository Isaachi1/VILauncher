//! The game folder: an unpacked Xbox Series (GDK) title the user owns.
//!
//! SixLauncher reads `MicrosoftGame.config` to find the title's executable. It
//! never unpacks, decrypts or modifies anything in the folder: a packaged title
//! (XVC / MSIXVC) is refused with an explanation, as VIctory and XCL do.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Error, Result};

pub const CONFIG_NAME: &str = "MicrosoftGame.config";
const PACKAGE_EXTENSIONS: &[&str] = &["xvc", "msixvc", "xvd"];

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct GameInfo {
    pub folder: PathBuf,
    pub config: Option<PathBuf>,
    pub identity: Option<String>,
    pub version: Option<String>,
    pub display_name: Option<String>,
    pub executables: Vec<String>,
}

impl GameInfo {
    pub fn title(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.identity.clone())
            .unwrap_or_else(|| self.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    }
}

/// `MicrosoftGame.config` in `folder`, whatever the case of its name.
pub fn find_config(folder: &Path) -> Option<PathBuf> {
    let exact = folder.join(CONFIG_NAME);
    if exact.is_file() {
        return Some(exact);
    }
    fs::read_dir(folder).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(CONFIG_NAME))
    })
}

pub fn parse_config(path: &Path) -> Result<GameInfo> {
    let text = fs::read_to_string(path)?;
    let doc = roxmltree::Document::parse(&text)
        .map_err(|e| Error::Game(format!("{} is not valid XML: {e}", path.display())))?;
    let mut info = GameInfo {
        folder: path.parent().unwrap_or(Path::new("/")).to_path_buf(),
        config: Some(path.to_path_buf()),
        ..Default::default()
    };
    for node in doc.descendants().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "Identity" => {
                info.identity = node.attribute("Name").map(str::to_owned).or(info.identity);
                info.version = node.attribute("Version").map(str::to_owned).or(info.version);
            }
            "Executable" => {
                if let Some(name) = node.attribute("Name") {
                    info.executables.push(name.to_owned());
                }
            }
            "ShellVisuals" => {
                if let Some(name) = node.attribute("DefaultDisplayName") {
                    info.display_name = Some(name.to_owned());
                }
            }
            _ => {}
        }
    }
    Ok(info)
}

fn has_extension(p: &Path, exts: &[&str]) -> bool {
    p.extension().is_some_and(|e| exts.iter().any(|x| e.to_string_lossy().eq_ignore_ascii_case(x)))
}

pub fn inspect(folder: &Path) -> Result<GameInfo> {
    if !folder.is_dir() {
        return Err(Error::Game(format!("game folder {} does not exist", folder.display())));
    }
    let entries: Vec<PathBuf> = fs::read_dir(folder)?.flatten().map(|e| e.path()).collect();
    let Some(config) = find_config(folder) else {
        let mut packages: Vec<String> = entries
            .iter()
            .filter(|p| has_extension(p, PACKAGE_EXTENSIONS))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        if !packages.is_empty() {
            packages.sort();
            return Err(Error::Game(format!(
                "{} holds a packaged title ({}). SixLauncher, VIctory and XCL only run unpacked game files you own; \
                 they do not unpack or decrypt packages.",
                folder.display(),
                packages.join(", ")
            )));
        }
        let mut exes: Vec<String> = entries
            .iter()
            .filter(|p| p.is_file() && has_extension(p, &["exe"]))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        exes.sort();
        return Ok(GameInfo { folder: folder.to_path_buf(), executables: exes, ..Default::default() });
    };
    let mut info = parse_config(&config)?;
    info.folder = folder.to_path_buf();
    Ok(info)
}

/// How deep and how many: enough for a title's own tree, quick on a huge folder.
const SCAN_DEPTH: usize = 4;
const SCAN_LIMIT: usize = 200;

/// Every .exe in the game folder, for picking the one to run: relative paths with `/`,
/// those `MicrosoftGame.config` names first (in its order), then the rest, shallowest first
/// and by name. Symbolic links are not followed; the walk stops at [`SCAN_DEPTH`] levels and
/// [`SCAN_LIMIT`] files.
pub fn find_executables(folder: &Path) -> Result<Vec<String>> {
    let info = inspect(folder)?;
    let mut found = Vec::new();
    let mut dirs = vec![(folder.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() && depth + 1 < SCAN_DEPTH {
                dirs.push((path, depth + 1));
            } else if kind.is_file()
                && has_extension(&path, &["exe"])
                && found.len() < SCAN_LIMIT
                && let Ok(rel) = path.strip_prefix(folder)
            {
                found.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let named: Vec<String> = info.executables.iter().map(|e| e.replace('\\', "/")).collect();
    let rank = |e: &String| named.iter().position(|n| n.eq_ignore_ascii_case(e)).unwrap_or(usize::MAX);
    found.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(a.matches('/').count().cmp(&b.matches('/').count()))
            .then(a.to_lowercase().cmp(&b.to_lowercase()))
    });
    Ok(found)
}

/// The title's executable: `override_exe` (relative to the folder or absolute),
/// else the first `<Executable>` of the config that exists, else the only .exe.
pub fn resolve_exe(info: &GameInfo, override_exe: Option<&str>) -> Result<PathBuf> {
    if let Some(o) = override_exe.filter(|o| !o.is_empty()) {
        let p = crate::paths::expand(&info.folder.join(o));
        return if p.is_file() {
            Ok(p)
        } else {
            Err(Error::Game(format!("the configured executable {} does not exist", p.display())))
        };
    }
    for name in &info.executables {
        let p = info.folder.join(name.replace('\\', "/"));
        if p.is_file() {
            return Ok(p);
        }
    }
    if info.config.is_some() && !info.executables.is_empty() {
        return Err(Error::Game(format!(
            "none of the executables {CONFIG_NAME} names exists in {}: {}",
            info.folder.display(),
            info.executables.join(", ")
        )));
    }
    if info.config.is_none() && info.executables.len() == 1 {
        return Ok(info.folder.join(&info.executables[0]));
    }
    Err(Error::Game(format!(
        "cannot tell which executable to run in {}; set the game's executable in the configuration",
        info.folder.display()
    )))
}

// -- finding a game folder (ported from ScarlettPort's first run) ----------------------------

/// How long [`search`] may take before it stops by itself.
pub const SEARCH_LIMIT: std::time::Duration = std::time::Duration::from_secs(25);
/// At most this many folders are reported.
pub const SEARCH_MAX: usize = 20;

/// Folders never searched: system folders, caches, packages and source trees. Hidden folders
/// (a leading dot) are skipped too.
const SKIP: [&str; 16] = [
    "proc", "sys", "dev", "run", "tmp", "windows", "$recycle.bin", "system volume information", "programdata",
    "appdata", "windowsapps", "node_modules", "target", "snap", "lost+found", "compatdata",
];

fn skipped(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with('.') || SKIP.contains(&n.as_str())
}

/// Where to look and how many levels below: the home folder (4) and every mounted disk
/// (`/mnt`, `/media`, `/run/media/<user>`, `/Volumes`: 4 below each), or the drive roots on
/// Windows (3).
pub fn search_starts(home: &Path) -> Vec<(PathBuf, u32)> {
    let mut out = vec![(home.to_path_buf(), 4)];
    if cfg!(windows) {
        out.extend(('C'..='Z').map(|d| PathBuf::from(format!("{d}:\\"))).filter(|p| p.is_dir()).map(|p| (p, 3)));
        return out;
    }
    let user = home.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    for mounts in [PathBuf::from("/mnt"), PathBuf::from("/media"), Path::new("/media").join(&user), Path::new("/run/media").join(&user), PathBuf::from("/Volumes")] {
        if let Ok(rd) = fs::read_dir(&mounts) {
            out.extend(rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| (e.path(), 4)));
        }
    }
    out
}

/// Breadth-first search of `starts` for game folders (one holding MicrosoftGame.config), until
/// `deadline` or [`SEARCH_MAX`] found. A found folder is not searched further, and symbolic
/// links are not followed (no loops). Reads folder listings only.
pub fn search(starts: &[(PathBuf, u32)], deadline: std::time::Instant) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<(PathBuf, u32)> = starts.iter().cloned().collect();
    while let Some((dir, depth)) = queue.pop_front() {
        if found.len() >= SEARCH_MAX || std::time::Instant::now() >= deadline {
            break;
        }
        if !seen.insert(dir.clone()) {
            continue;
        }
        if find_config(&dir).is_some() {
            found.push(dir);
            continue;
        }
        if depth == 0 {
            continue;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) && !skipped(&e.file_name().to_string_lossy()) {
                queue.push_back((e.path(), depth - 1));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    #[test]
    fn search_finds_game_folders_and_skips_the_rest() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        for d in ["Games/GTA/Content", ".cache/hidden", "node_modules/pkg", "deep/a/b/c/d/e"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for d in ["Games/GTA/Content", ".cache/hidden", "node_modules/pkg", "deep/a/b/c/d/e"] {
            fs::write(root.join(d).join("MicrosoftGame.config"), "<Game/>").unwrap();
        }
        let found = search(&[(root.to_path_buf(), 4)], std::time::Instant::now() + SEARCH_LIMIT);
        assert_eq!(found, [root.join("Games/GTA/Content")], "hidden, skipped and too deep folders are not searched");
        assert!(search(&[(root.to_path_buf(), 4)], std::time::Instant::now()).is_empty(), "the deadline stops it");
        assert_eq!(search_starts(root)[0], (root.to_path_buf(), 4));
    }

    use super::*;
    use crate::testutil;

    #[test]
    fn lists_the_executables_config_first() {
        let t = tempfile::tempdir().unwrap();
        let game = testutil::fake_game(t.path());
        testutil::fake_pe(&game.join("Launcher.exe"), 0x8664);
        testutil::fake_pe(&game.join("tools/a/crash.exe"), 0x8664);
        testutil::fake_pe(&game.join("a/b/c/d/too-deep.exe"), 0x8664);
        fs::write(game.join("readme.txt"), "x").unwrap();
        let exes = find_executables(&game).unwrap();
        assert_eq!(exes, ["bin/title.exe", "Launcher.exe", "tools/a/crash.exe"]);
    }

    #[test]
    fn config_names_the_executable() {
        let t = tempfile::tempdir().unwrap();
        let game = testutil::fake_game(t.path());
        let info = inspect(&game).unwrap();
        assert_eq!(info.identity.as_deref(), Some("Example.TestTitle"));
        assert_eq!(info.version.as_deref(), Some("1.2.3.0"));
        assert_eq!(info.title(), "Test Title");
        assert_eq!(resolve_exe(&info, None).unwrap(), game.join("bin/title.exe"));
    }

    #[test]
    fn config_name_is_case_insensitive() {
        let t = tempfile::tempdir().unwrap();
        let game = testutil::fake_game(t.path());
        fs::rename(game.join(CONFIG_NAME), game.join("microsoftgame.CONFIG")).unwrap();
        assert_eq!(resolve_exe(&inspect(&game).unwrap(), None).unwrap(), game.join("bin/title.exe"));
    }

    #[test]
    fn namespaced_config() {
        let t = tempfile::tempdir().unwrap();
        let game = testutil::fake_game(t.path());
        let text = testutil::GAME_CONFIG.replace("<Game configVersion=\"1\">", "<Game xmlns=\"urn:example\" configVersion=\"1\">");
        fs::write(game.join(CONFIG_NAME), text).unwrap();
        assert_eq!(inspect(&game).unwrap().executables, vec!["bin\\title.exe".to_string()]);
    }

    #[test]
    fn packaged_title_is_refused() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("title.msixvc"), [0u8; 16]).unwrap();
        let err = inspect(t.path()).unwrap_err().to_string();
        assert!(err.contains("do not unpack or decrypt"), "{err}");
    }

    #[test]
    fn missing_executable_is_reported() {
        let t = tempfile::tempdir().unwrap();
        let game = testutil::fake_game(t.path());
        fs::remove_file(game.join("bin/title.exe")).unwrap();
        let err = resolve_exe(&inspect(&game).unwrap(), None).unwrap_err().to_string();
        assert!(err.contains("none of the executables"), "{err}");
    }

    #[test]
    fn override_executable() {
        let t = tempfile::tempdir().unwrap();
        let game = testutil::fake_game(t.path());
        testutil::fake_pe(&game.join("other.exe"), 0x8664);
        assert_eq!(resolve_exe(&inspect(&game).unwrap(), Some("other.exe")).unwrap(), game.join("other.exe"));
    }

    #[test]
    fn folder_without_config_uses_its_only_exe() {
        let t = tempfile::tempdir().unwrap();
        testutil::fake_pe(&t.path().join("game.exe"), 0x8664);
        assert_eq!(resolve_exe(&inspect(t.path()).unwrap(), None).unwrap(), t.path().join("game.exe"));
    }
}

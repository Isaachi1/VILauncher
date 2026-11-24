//! The game folder: an unpacked Xbox Series (GDK) title the user owns.
//!
//! VILauncher reads `MicrosoftGame.config` to find the title's executable. It
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
                "{} holds a packaged title ({}). VILauncher, VIctory and XCL only run unpacked game files you own; \
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

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

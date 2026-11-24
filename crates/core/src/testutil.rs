//! Synthetic stand-ins written by the tests themselves: no game data, no real binaries.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub const GAME_CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Game configVersion="1">
  <Identity Name="Example.TestTitle" Publisher="CN=Example" Version="1.2.3.0"/>
  <ExecutableList>
    <Executable Name="bin\title.exe" TargetDeviceFamily="Scarlett" Id="Game"/>
  </ExecutableList>
  <ShellVisuals DefaultDisplayName="Test Title" PublisherDisplayName="Example"/>
</Game>
"#;

/// A PE header with nothing behind it: enough for the machine check.
pub fn fake_pe(path: &Path, machine: u16) -> PathBuf {
    let mut data = vec![0u8; 0x200];
    data[0..2].copy_from_slice(b"MZ");
    data[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    data[0x80..0x84].copy_from_slice(b"PE\0\0");
    data[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
    path.to_path_buf()
}

pub fn fake_game(root: &Path) -> PathBuf {
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("MicrosoftGame.config"), GAME_CONFIG).unwrap();
    fake_pe(&game.join("bin/title.exe"), 0x8664);
    game
}

pub fn executable(path: &Path, text: &str) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_path_buf()
}

/// A Proton folder whose `proton` script records how it was called and exits with 7.
pub fn fake_proton(root: &Path, name: &str) -> PathBuf {
    let proton = root.join(name);
    executable(
        &proton.join("proton"),
        "#!/bin/sh\nout=\"$VILAUNCHER_TEST_OUT\"\nprintf '%s\\n' \"$@\" > \"$out.argv\"\nenv > \"$out.env\"\npwd > \"$out.cwd\"\necho started\nexit 7\n",
    );
    fs::write(proton.join("version"), format!("1700000000 {name}\n")).unwrap();
    for dll in ["d3d12.dll", "d3d12core.dll"] {
        fake_pe(&proton.join("files/lib/wine/vkd3d-proton/x86_64-windows").join(dll), 0x8664);
    }
    proton
}

pub fn fake_wine_prefix(prefix: &Path) -> PathBuf {
    fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
    fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n#arch=win64\n").unwrap();
    fs::create_dir_all(prefix.join("dosdevices")).unwrap();
    std::os::unix::fs::symlink("/", prefix.join("dosdevices/z:")).unwrap();
    prefix.to_path_buf()
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// HOME and the XDG folders pointed at a temporary folder, for one test at a time.
pub struct TempHome {
    pub root: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl TempHome {
    pub fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(&home).unwrap();
        // SAFETY: tests that touch the environment hold ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
            std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
            std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
            std::env::set_var("XDG_STATE_HOME", home.join(".local/state"));
            std::env::remove_var("VILAUNCHER_CONFIG");
            std::env::remove_var("PROTON_USE_WINED3D");
        }
        TempHome { root, _guard: guard }
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }
}

//! The compatibility layers SixLauncher starts: VIctory (VILinux) and XCL (LinuxCL).
//!
//! Each one is a Windows executable built from its repository. This module
//! turns the configuration into the command line and environment each one
//! documents in its README:
//!
//! * VIctory: `victory.exe run <title exe> --execute [--console series-s]
//!   [-- title args]` with `VICTORY_GAME_DIR`, `VICTORY_DATA_DIR`, `VICTORY_CACHE_DIR`.
//! * XCL: `xcl.exe run <game folder> [-- title args]` with `XCL_DATA_ROOT`,
//!   `XCL_DXC_DIR` (DXC's `dxcompiler.dll` + `dxil.dll`) and `XCL_OPUS_DIR`
//!   (`opus.dll`: under Wine there is no Media Foundation Opus decoder).
//!
//! Every path goes through [`paths::to_windows`], because Wine hands arguments
//! and environment variables to the program unchanged.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::config::{BackendKind, Config, Console};
use crate::paths::to_windows;
use crate::{Error, Result};

pub const MACHINE_X86_64: u16 = 0x8664;

/// What to run: the backend's own arguments and environment.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

fn require(cfg: &Config) -> Result<(PathBuf, PathBuf)> {
    let exe = cfg.backend_exe().ok_or_else(|| Error::Config("no backend executable (victory.exe or xcl.exe) is set".into()))?;
    let game = cfg.game_dir().ok_or_else(|| Error::Game("no game folder is set".into()))?;
    Ok((exe, game))
}

pub fn victory_plan(cfg: &Config, game_exe: &Path) -> Result<Plan> {
    let (exe, game) = require(cfg)?;
    let mut args = vec!["run".into(), to_windows(game_exe), "--execute".into(), "--unimplemented".into()];
    args.push(cfg.backend.unimplemented.arg().into());
    if cfg.backend.console != Console::SeriesX {
        args.extend(["--console".into(), cfg.backend.console.arg().into()]);
    }
    if !cfg.game.args.is_empty() {
        args.push("--".into());
        args.extend(cfg.game.args.iter().cloned());
    }
    let env = BTreeMap::from([
        ("VICTORY_GAME_DIR".into(), to_windows(&game)),
        ("VICTORY_DATA_DIR".into(), to_windows(&cfg.data_dir())),
        ("VICTORY_CACHE_DIR".into(), to_windows(&cfg.cache_dir())),
    ]);
    Ok(Plan { exe, args, env })
}

pub fn xcl_plan(cfg: &Config, game_exe: &Path) -> Result<Plan> {
    let (exe, game) = require(cfg)?;
    // XCL takes the folder and finds the executable through MicrosoftGame.config
    // itself; an explicitly chosen executable is passed as a file instead.
    let explicit = cfg.game.exe.as_deref().is_some_and(|e| !e.is_empty());
    let target = if explicit { game_exe.to_path_buf() } else { game };
    let mut args = vec!["run".into(), to_windows(&target)];
    if !cfg.game.args.is_empty() {
        args.push("--".into());
        args.extend(cfg.game.args.iter().cloned());
    }
    let mut env = BTreeMap::from([("XCL_DATA_ROOT".into(), to_windows(&cfg.data_dir()))]);
    if let Some(d) = cfg.dxc_dir() {
        env.insert("XCL_DXC_DIR".into(), to_windows(&d));
    }
    if let Some(d) = cfg.opus_dir() {
        env.insert("XCL_OPUS_DIR".into(), to_windows(&d));
    }
    Ok(Plan { exe, args, env })
}

pub fn plan(cfg: &Config, game_exe: &Path) -> Result<Plan> {
    match cfg.backend.kind {
        BackendKind::Victory => victory_plan(cfg, game_exe),
        BackendKind::Xcl => xcl_plan(cfg, game_exe),
    }
}

/// The PE machine field of `exe` (0x8664 for x86-64), or None if it is not a PE file.
pub fn pe_machine(exe: &Path) -> Option<u16> {
    let mut head = vec![0u8; 0x1000];
    let n = fs::File::open(exe).ok()?.read(&mut head).ok()?;
    let head = &head[..n];
    if head.len() < 0x40 || &head[..2] != b"MZ" {
        return None;
    }
    let off = u32::from_le_bytes(head[0x3C..0x40].try_into().ok()?) as usize;
    if off + 6 > head.len() || &head[off..off + 4] != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes([head[off + 4], head[off + 5]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths;
    use crate::testutil::{self, TempHome};

    fn cfg(h: &TempHome) -> (Config, PathBuf) {
        let game = testutil::fake_game(h.path());
        let mut cfg = Config::default();
        cfg.game.dir = Some(game.clone());
        cfg.backend.exe = Some(h.path().join("b/victory.exe"));
        (cfg, game)
    }

    #[test]
    fn victory_command_line() {
        let h = TempHome::new();
        let (mut cfg, game) = cfg(&h);
        cfg.backend.console = Console::SeriesS;
        cfg.game.args = vec!["-x".into()];
        let plan = plan(&cfg, &game.join("bin/title.exe")).unwrap();
        assert_eq!(
            plan.args,
            ["run", &paths::to_windows(&game.join("bin/title.exe")), "--execute", "--unimplemented", "trap", "--console", "series-s", "--", "-x"]
        );
        assert_eq!(plan.env["VICTORY_GAME_DIR"], paths::to_windows(&game));
        assert!(plan.env["VICTORY_DATA_DIR"].starts_with("Z:\\"));
        assert!(plan.env["VICTORY_CACHE_DIR"].ends_with("\\sixlauncher\\victory"));
    }

    #[test]
    fn xcl_command_line() {
        let h = TempHome::new();
        let (mut cfg, game) = cfg(&h);
        cfg.backend.kind = BackendKind::Xcl;
        cfg.backend.dxc_dir = Some("/opt/dxc/bin/x64".into());
        let plan = plan(&cfg, &game.join("bin/title.exe")).unwrap();
        assert_eq!(plan.args, ["run".to_string(), paths::to_windows(&game)]);
        assert_eq!(plan.env["XCL_DXC_DIR"], r"Z:\opt\dxc\bin\x64");
        assert!(!plan.env.contains_key("XCL_OPUS_DIR"));
        assert!(plan.env["XCL_DATA_ROOT"].ends_with("\\sixlauncher\\xcl\\data"));
    }

    #[test]
    fn pe_machine_reads_the_header() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(pe_machine(&testutil::fake_pe(&t.path().join("a.exe"), 0x8664)), Some(0x8664));
        fs::write(t.path().join("b.exe"), "#!/bin/sh\n").unwrap();
        assert_eq!(pe_machine(&t.path().join("b.exe")), None);
        assert_eq!(pe_machine(&t.path().join("missing.exe")), None);
    }
}

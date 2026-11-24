//! Turning a configuration into a running process.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::config::{BackendKind, Config};
use crate::doctor::{self, Check, Status};
use crate::runners::Runner;
use crate::{backends, gamedir, paths};
use crate::{Error, Result};

/// The command line, the environment additions and the working folder of a run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Launch {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub cache: PathBuf,
}

pub fn build(cfg: &Config) -> Result<Launch> {
    let game_dir = cfg.game_dir().ok_or_else(|| Error::Game("no game folder is set".into()))?;
    let info = gamedir::inspect(&game_dir)?;
    let game_exe = gamedir::resolve_exe(&info, cfg.game.exe.as_deref())?;
    let runner = Runner::from_config(cfg)?;
    let plan = backends::plan(cfg, &game_exe)?;
    let base: BTreeMap<String, String> = std::env::vars().collect();
    let mut env = runner.env(&base);
    env.extend(plan.env);
    env.extend(cfg.env.clone());
    Ok(Launch { argv: runner.argv(&plan.exe, &plan.args), env, cwd: cfg.data_dir(), cache: cfg.cache_dir() })
}

/// The configuration checks that stop a run.
pub fn blocking_failures(cfg: &Config) -> Vec<Check> {
    doctor::config_checks(cfg).into_iter().filter(|c| c.status == Status::Fail).collect()
}

impl Launch {
    /// The command, with its data and cache folders created.
    pub fn command(&self) -> Result<Command> {
        fs::create_dir_all(&self.cwd)?;
        fs::create_dir_all(&self.cache)?;
        let argv: Vec<std::ffi::OsString> = self.argv.iter().map(Into::into).collect();
        Ok(crate::host::command(&argv, &self.env, Some(&self.cwd)))
    }

    /// The launch as shell lines: the environment, the folder, the command.
    pub fn shell_text(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.env {
            out += &format!("{k}={}\n", shell_quote(v));
        }
        out += &format!("cd {}\n", shell_quote(&self.cwd.to_string_lossy()));
        out += &self.argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
        out.push('\n');
        out
    }
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Where a run started from the menu writes its output.
pub fn log_file() -> PathBuf {
    paths::state_dir().join("last-run.log")
}

/// An `Exec=` argument (desktop entry spec: double quotes, backslash escapes;
/// Exec is also a string value, whose own escaping doubles each backslash).
pub fn desktop_quote(arg: &str) -> String {
    if !arg.chars().any(|c| " \t\n\"'\\><~|&;$*?#()`".contains(c)) {
        return arg.to_owned();
    }
    let mut q = String::from("\"");
    for c in arg.chars() {
        if "\"`$\\".contains(c) {
            q.push('\\');
        }
        q.push(c);
    }
    q.push('"');
    q.replace('\\', "\\\\")
}

pub fn desktop_entry(cfg: &Config, exec: &[String]) -> String {
    let title = cfg
        .game_dir()
        .and_then(|d| gamedir::inspect(&d).ok())
        .map(|i| i.title())
        .unwrap_or_else(|| "VILauncher".into());
    let backend = match cfg.backend.kind {
        BackendKind::Victory => "VIctory",
        BackendKind::Xcl => "XCL",
    };
    let exec: Vec<String> = exec.iter().map(|a| desktop_quote(a)).collect();
    format!(
        "[Desktop Entry]\nType=Application\nName={title} (VILauncher)\nComment=Runs your copy through {backend} on Wine/Proton\n\
         Exec={}\nTerminal=false\nCategories=Game;\n",
        exec.join(" ")
    )
}

pub fn write_desktop_entry(cfg: &Config, exec: &[String]) -> Result<PathBuf> {
    let path = paths::applications_dir().join("vilauncher.desktop");
    fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))?;
    fs::write(&path, desktop_entry(cfg, exec))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RunnerKind;
    use crate::runners;
    use crate::testutil::{self, TempHome};

    fn victory_config(h: &TempHome) -> (Config, PathBuf, PathBuf, PathBuf) {
        let game = testutil::fake_game(h.path());
        let exe = testutil::fake_pe(&h.path().join("VIctory build/victory.exe"), 0x8664);
        let proton = testutil::fake_proton(&h.home().join(".local/share/Steam/compatibilitytools.d"), "GE-Proton10-1");
        let mut cfg = Config::default();
        cfg.game.dir = Some(game.clone());
        cfg.game.args = vec!["-windowed".into(), "a b".into()];
        cfg.backend.exe = Some(exe.clone());
        (cfg, game, exe, proton)
    }

    #[test]
    fn proton_launch_runs_the_backend() {
        let h = TempHome::new();
        let (cfg, game, exe, proton) = victory_config(&h);
        assert!(blocking_failures(&cfg).is_empty(), "{:?}", blocking_failures(&cfg));
        let launch = build(&cfg).unwrap();
        let rec = h.path().join("rec");
        let status = launch.command().unwrap().env("VILAUNCHER_TEST_OUT", &rec).output().unwrap().status;
        assert_eq!(status.code(), Some(7)); // the fake proton's exit code comes back
        let argv: Vec<String> = fs::read_to_string(rec.with_extension("argv")).unwrap().lines().map(str::to_owned).collect();
        assert_eq!(
            argv,
            [
                "waitforexitandrun",
                &exe.to_string_lossy(),
                "run",
                &paths::to_windows(&game.join("bin/title.exe")),
                "--execute",
                "--unimplemented",
                "trap",
                "--",
                "-windowed",
                "a b"
            ]
        );
        assert_eq!(launch.argv[0], proton.join("proton").to_string_lossy());
        let env = fs::read_to_string(rec.with_extension("env")).unwrap();
        assert!(env.contains(&format!("STEAM_COMPAT_DATA_PATH={}", cfg.prefix().display())), "{env}");
        assert!(env.contains(&format!("VICTORY_GAME_DIR={}", paths::to_windows(&game))), "{env}");
        assert_eq!(fs::read_to_string(rec.with_extension("cwd")).unwrap().trim(), cfg.data_dir().to_string_lossy());
        assert!(cfg.cache_dir().is_dir());
    }

    #[test]
    fn missing_game_blocks_the_run() {
        let h = TempHome::new();
        let (mut cfg, ..) = victory_config(&h);
        cfg.game.dir = None;
        let names: Vec<String> = blocking_failures(&cfg).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["game"]);
        assert!(build(&cfg).is_err());
    }

    #[test]
    fn wine_launch_sets_overrides() {
        let h = TempHome::new();
        let game = testutil::fake_game(h.path());
        let exe = testutil::fake_pe(&h.path().join("xcl/xcl.exe"), 0x8664);
        for dll in ["dxcompiler.dll", "dxil.dll"] {
            testutil::fake_pe(&exe.with_file_name(dll), 0x8664);
        }
        let prefix = testutil::fake_wine_prefix(&h.path().join("pfx"));
        let release = h.path().join("vkd3d");
        for dll in runners::VKD3D_DLLS {
            testutil::fake_pe(&release.join("x64").join(dll), 0x8664);
        }
        let wine = testutil::executable(&h.path().join("bin/wine"), "#!/bin/sh\necho wine-9.22\n");
        let mut cfg = Config::default();
        cfg.game.dir = Some(game.clone());
        cfg.backend.kind = BackendKind::Xcl;
        cfg.backend.exe = Some(exe.clone());
        cfg.runner.kind = RunnerKind::Wine;
        cfg.runner.path = Some(wine.clone());
        cfg.runner.prefix = Some(prefix.clone());
        cfg.runner.vkd3d_dir = Some(release);
        assert_eq!(blocking_failures(&cfg).len(), 1); // vkd3d-proton not installed yet
        Runner::from_config(&cfg).unwrap().setup(&mut |_| {}).unwrap();
        assert!(blocking_failures(&cfg).is_empty(), "{:?}", blocking_failures(&cfg));
        let text = build(&cfg).unwrap().shell_text();
        assert!(text.contains("WINEDLLOVERRIDES=d3d12,d3d12core=n,b\n"), "{text}");
        assert!(text.contains(&format!("WINEPREFIX={}\n", prefix.display())), "{text}");
        assert!(text.contains(&format!("{} {} run 'Z:", wine.display(), exe.display())), "{text}");
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("plain/path-1.exe"), "plain/path-1.exe");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(desktop_quote("/usr/bin/vilauncher"), "/usr/bin/vilauncher");
        assert_eq!(desktop_quote("/a b/$x\"y"), r#""/a b/\\$x\\"y""#);
    }

    #[test]
    fn desktop_entry_names_the_game() {
        let h = TempHome::new();
        let (cfg, ..) = victory_config(&h);
        let path = write_desktop_entry(&cfg, &["/usr/bin/vilauncher".into(), "run".into()]).unwrap();
        let entry = fs::read_to_string(path).unwrap();
        assert!(entry.contains("Name=Test Title (VILauncher)\n"), "{entry}");
        assert!(entry.contains("Exec=/usr/bin/vilauncher run\n"), "{entry}");
    }
}

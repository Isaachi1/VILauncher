//! Turning a configuration into a running process.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::config::Config;
use crate::doctor::{self, Check, Status};
use crate::runners::Runner;
use crate::{backends, gamedir, gpu, paths, perf};
use crate::{Error, Result};

/// The command line, the environment additions and the working folder of a run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Launch {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub cache: PathBuf,
    /// The runner's prefix folder (Proton's STEAM_COMPAT_DATA_PATH, Wine's WINEPREFIX), created
    /// by the run: Proton opens `pfx.lock` in it first and does not create it (Steam always
    /// does). None when the program runs natively (Windows), which has no prefix.
    pub prefix: Option<PathBuf>,
}

pub fn build(cfg: &Config) -> Result<Launch> {
    let tools = if perf::wanted(cfg) { perf::detect() } else { perf::Tools::default() };
    build_with(cfg, &gpu::list(), &tools)
}

/// [`build`] on a machine with these GPUs and these tools installed.
pub fn build_with(cfg: &Config, gpus: &[gpu::Gpu], tools: &perf::Tools) -> Result<Launch> {
    let game_dir = cfg.game_dir().ok_or_else(|| Error::Game("no game folder is set".into()))?;
    let info = gamedir::inspect(&game_dir)?;
    let game_exe = gamedir::resolve_exe(&info, cfg.game.exe.as_deref())?;
    let runner = Runner::from_config(cfg)?;
    let prefix = match &runner {
        Runner::Proton(p) => Some(p.compat_dir.clone()),
        Runner::Wine(w) => Some(w.prefix.clone()),
        Runner::Native => None,
    };
    let plan = backends::plan(cfg, &game_exe)?;
    let base: BTreeMap<String, String> = std::env::vars().collect();
    let mut env = runner.env(&base);
    env.extend(plan.env);
    env.extend(gpu::env_for(cfg, gpus));
    env.extend(perf::env_for(&cfg.performance));
    env.extend(cfg.env.clone());
    let mut argv = perf::wrapper(&cfg.performance, tools);
    argv.extend(runner.argv(&plan.exe, &plan.args));
    Ok(Launch { argv, env, cwd: cfg.data_dir(), cache: cfg.cache_dir(), prefix })
}

/// The configuration checks that stop a run.
pub fn blocking_failures(cfg: &Config) -> Vec<Check> {
    doctor::config_checks(cfg).into_iter().filter(|c| c.status == Status::Fail).collect()
}

impl Launch {
    /// The command, with its data, cache and prefix folders created, and Proton's log folder
    /// (PROTON_LOG_DIR), which Proton writes steam-*.log into without creating it.
    pub fn command(&self) -> Result<Command> {
        fs::create_dir_all(&self.cwd)?;
        fs::create_dir_all(&self.cache)?;
        if let Some(prefix) = &self.prefix {
            fs::create_dir_all(prefix)?;
            if let Some(logs) = self.env.get("PROTON_LOG_DIR").filter(|d| !d.is_empty()) {
                fs::create_dir_all(logs)?;
            }
        }
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
        .unwrap_or_else(|| "SixLauncher".into());
    let backend = cfg.backend.kind.display();
    let exec: Vec<String> = exec.iter().map(|a| desktop_quote(a)).collect();
    format!(
        "[Desktop Entry]\nType=Application\nName={title} (SixLauncher)\nComment=Runs your copy through {backend} on Wine/Proton\n\
         Exec={}\nTerminal=false\nCategories=Game;\n",
        exec.join(" ")
    )
}

/// A command-line argument as Windows programs split them (CommandLineToArgvW's rules).
pub fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_owned();
    }
    let mut q = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                q.push_str(&"\\".repeat(slashes * 2 + 1));
                slashes = 0;
            }
            _ => {
                q.push_str(&"\\".repeat(slashes));
                slashes = 0;
            }
        }
        if c != '\\' {
            q.push(c);
        }
    }
    q.push_str(&"\\".repeat(slashes * 2));
    q.push('"');
    q
}

/// The menu entry: a `.desktop` file in the applications folder, or on Windows a shortcut
/// (`.lnk`) in the Start menu, written by PowerShell through WScript.Shell.
pub fn write_desktop_entry(cfg: &Config, exec: &[String]) -> Result<PathBuf> {
    if cfg!(windows) {
        return write_start_menu_shortcut(cfg, exec);
    }
    let path = paths::applications_dir().join("sixlauncher.desktop");
    fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))?;
    fs::write(&path, desktop_entry(cfg, exec))?;
    Ok(path)
}

fn write_start_menu_shortcut(cfg: &Config, exec: &[String]) -> Result<PathBuf> {
    let title = cfg
        .game_dir()
        .and_then(|d| gamedir::inspect(&d).ok())
        .map(|i| i.title())
        .unwrap_or_else(|| "SixLauncher".into());
    // Characters a file name cannot hold become spaces.
    let name: String = title.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let dir = paths::applications_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{} (SixLauncher).lnk", name.trim()));
    let (target, args) = exec.split_first().ok_or_else(|| Error::Config("nothing to start from the shortcut".into()))?;
    let args: Vec<String> = args.iter().map(|a| windows_quote(a)).collect();
    // The values travel as variables, so no quoting of PowerShell's own is needed.
    let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:SL_LNK); \
                  $s.TargetPath = $env:SL_TARGET; $s.Arguments = $env:SL_ARGS; \
                  $s.WorkingDirectory = Split-Path -Parent $env:SL_TARGET; $s.Save()";
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert("SL_LNK".into(), path.to_string_lossy().into_owned());
    env.insert("SL_TARGET".into(), target.clone());
    env.insert("SL_ARGS".into(), args.join(" "));
    let argv: Vec<std::ffi::OsString> =
        ["powershell", "-NoProfile", "-NonInteractive", "-Command", script].iter().map(Into::into).collect();
    let out = crate::host::command(&argv, &env, None).output()?;
    if !out.status.success() || !path.is_file() {
        return Err(Error::Config(format!(
            "could not create the Start menu shortcut ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendKind, RunnerKind};
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
    fn proton_launch_creates_the_compat_folder() {
        // Proton opens compatdata/pfx.lock before anything else and does not create the
        // folder itself (Steam always does): a run without "Preparar prefixo" failed with
        // FileNotFoundError on pfx.lock.
        let h = TempHome::new();
        let (cfg, ..) = victory_config(&h);
        let compat = cfg.prefix();
        assert!(!compat.exists());
        build(&cfg).unwrap().command().unwrap();
        assert!(compat.is_dir(), "{} was not created", compat.display());
    }

    #[test]
    fn proton_launch_creates_the_log_folder() {
        // With "Logs detalhados", Proton writes steam-*.log into PROTON_LOG_DIR, which nothing
        // created: the log never appeared.
        let h = TempHome::new();
        let (mut cfg, ..) = victory_config(&h);
        cfg.env.insert("PROTON_LOG".into(), "1".into());
        let logs = crate::report::proton_log_dir();
        assert!(!logs.exists());
        build(&cfg).unwrap().command().unwrap();
        assert!(logs.is_dir(), "{} was not created", logs.display());
    }

    #[test]
    fn wine_launch_creates_the_prefix_folder() {
        let h = TempHome::new();
        let (mut cfg, ..) = victory_config(&h);
        let wine = testutil::executable(&h.path().join("bin/wine"), "#!/bin/sh\necho wine-9.22\n");
        let prefix = h.path().join("new prefix");
        cfg.runner.kind = RunnerKind::Wine;
        cfg.runner.path = Some(wine);
        cfg.runner.prefix = Some(prefix.clone());
        build(&cfg).unwrap().command().unwrap();
        assert!(prefix.is_dir(), "{} was not created", prefix.display());
    }

    #[test]
    fn proton_launch_gives_protonfixes_a_game_id() {
        // protonfixes reads SteamAppId / SteamGameId, else the last number of
        // STEAM_COMPAT_DATA_PATH; ".../compatdata" has none, so GE-Proton stopped with IndexError.
        let h = TempHome::new();
        let (cfg, ..) = victory_config(&h);
        let env = build(&cfg).unwrap().env;
        assert_eq!(env.get("SteamAppId").map(String::as_str), Some("0"));
        assert_eq!(env.get("SteamGameId").map(String::as_str), Some("0"));
        let mut base = BTreeMap::new();
        base.insert("SteamAppId".to_string(), "123".to_string());
        let kept = Runner::from_config(&cfg).unwrap().env(&base);
        assert!(!kept.contains_key("SteamAppId"), "an id already set is kept");
    }

    #[test]
    fn proton_launch_runs_the_backend() {
        let h = TempHome::new();
        let (cfg, game, exe, proton) = victory_config(&h);
        assert!(blocking_failures(&cfg).is_empty(), "{:?}", blocking_failures(&cfg));
        let launch = build(&cfg).unwrap();
        let rec = h.path().join("rec");
        let status = launch.command().unwrap().env("SIXLAUNCHER_TEST_OUT", &rec).output().unwrap().status;
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
    fn gpu_and_performance_options_reach_the_run() {
        let h = TempHome::new();
        let (mut cfg, _, exe, proton) = victory_config(&h);
        let gpus = gpu::tests::hybrid_nvidia(&h.path().join("sys"));
        cfg.performance.mangohud = true;
        cfg.performance.gamemode = true;
        cfg.env.insert("PROTON_ENABLE_NVAPI".into(), "0".into());
        let mut tools = perf::Tools::default();
        // GameMode on but not installed: the run goes ahead without it.
        let l = build_with(&cfg, &gpus, &tools).unwrap();
        assert_eq!(l.argv[0], proton.join("proton").to_string_lossy());
        assert_eq!(l.env["__NV_PRIME_RENDER_OFFLOAD"], "1");
        assert_eq!(l.env["MANGOHUD"], "1");
        assert_eq!(l.env["PROTON_ENABLE_NVAPI"], "0", "[env] wins over the preset");
        tools.gamemode = perf::Tool { installed: true, path: Some("/usr/bin/gamemoderun".into()), flatpak: vec![] };
        let l = build_with(&cfg, &gpus, &tools).unwrap();
        assert_eq!(&l.argv[..3], ["/usr/bin/gamemoderun", &*proton.join("proton").to_string_lossy(), "waitforexitandrun"]);
        assert_eq!(l.argv[3], exe.to_string_lossy());
    }

    #[test]
    fn windows_quoting() {
        assert_eq!(windows_quote("run"), "run");
        assert_eq!(windows_quote(""), "\"\"");
        assert_eq!(windows_quote(r"C:\My Games\x.exe"), r#""C:\My Games\x.exe""#);
        assert_eq!(windows_quote(r#"a"b"#), r#""a\"b""#);
        assert_eq!(windows_quote(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("plain/path-1.exe"), "plain/path-1.exe");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(desktop_quote("/usr/bin/sixlauncher"), "/usr/bin/sixlauncher");
        assert_eq!(desktop_quote("/a b/$x\"y"), r#""/a b/\\$x\\"y""#);
    }

    #[test]
    fn desktop_entry_names_the_game() {
        let h = TempHome::new();
        let (cfg, ..) = victory_config(&h);
        let path = write_desktop_entry(&cfg, &["/usr/bin/sixlauncher".into(), "run".into()]).unwrap();
        let entry = fs::read_to_string(path).unwrap();
        assert!(entry.contains("Name=Test Title (SixLauncher)\n"), "{entry}");
        assert!(entry.contains("Exec=/usr/bin/sixlauncher run\n"), "{entry}");
    }
}

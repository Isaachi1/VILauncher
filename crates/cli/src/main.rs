//! `vilauncher`: the launcher without a window (scripts, the menu entry, SSH).

use std::path::PathBuf;
use std::process::ExitCode;

use vilauncher_core::config::{self, BackendKind, Config, Console, RunnerKind};
use vilauncher_core::doctor::{self, Status};
use vilauncher_core::runners::{self, Runner};
use vilauncher_core::{gamedir, launch, paths};

const HELP: &str = "\
vilauncher - runs an Xbox Series title you own on Linux through VIctory (VILinux)
or XCL (LinuxCL), on Proton or Wine with vkd3d-proton.

Usage:
  vilauncher setup [options]      write the configuration and prepare the prefix
      --game DIR                  the unpacked game folder (with MicrosoftGame.config)
      --exe PATH                  the title's executable, relative to the game folder
      --backend-exe FILE          victory.exe or xcl.exe (the Windows build)
      --backend victory|xcl       default: from the executable's name
      --console series-x|series-s victory only
      --runner proton|wine
      --runner-path PATH          a Proton folder, or the wine binary
      --prefix DIR                where the prefix lives
      --vkd3d DIR                 wine: an unpacked vkd3d-proton release
      --dxc-dir DIR               xcl: DXC's bin/x64 (dxcompiler.dll, dxil.dll)
      --opus-dir DIR              xcl: a folder with opus.dll
      --no-prefix                 only write the configuration
  vilauncher doctor [--config-only]   check the machine and the configuration
  vilauncher run [--dry-run] [--force] [-- title args]
  vilauncher info [DIR]           what MicrosoftGame.config says about a game folder
  vilauncher protons              the Proton installs found
  vilauncher config               print the configuration
  vilauncher desktop              add a menu entry that runs the game
";

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}

fn abs(s: &str) -> PathBuf {
    paths::normalize(&paths::expand(PathBuf::from(s).as_path()))
}

fn setup(args: &[String]) -> ExitCode {
    let path = paths::config_file();
    let mut cfg = match config::load_from(&path) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let mut no_prefix = false;
    let mut backend_set = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if flag == "--no-prefix" {
            no_prefix = true;
            continue;
        }
        let Some(v) = it.next() else { return fail(format!("{flag} needs a value")) };
        match flag.as_str() {
            "--game" => cfg.game.dir = Some(abs(v)),
            "--exe" => cfg.game.exe = Some(v.clone()).filter(|v| !v.is_empty()),
            "--backend-exe" => {
                cfg.backend.exe = Some(abs(v));
                if !backend_set
                    && let Some(k) = config::guess_backend(&abs(v)) {
                        cfg.backend.kind = k;
                    }
            }
            "--backend" => {
                backend_set = true;
                cfg.backend.kind = match v.as_str() {
                    "victory" => BackendKind::Victory,
                    "xcl" => BackendKind::Xcl,
                    _ => return fail("--backend is victory or xcl"),
                }
            }
            "--console" => {
                cfg.backend.console = match v.as_str() {
                    "series-x" => Console::SeriesX,
                    "series-s" => Console::SeriesS,
                    _ => return fail("--console is series-x or series-s"),
                }
            }
            "--runner" => {
                cfg.runner.kind = match v.as_str() {
                    "proton" => RunnerKind::Proton,
                    "wine" => RunnerKind::Wine,
                    _ => return fail("--runner is proton or wine"),
                }
            }
            "--runner-path" => cfg.runner.path = Some(abs(v)),
            "--prefix" => cfg.runner.prefix = Some(abs(v)),
            "--vkd3d" => cfg.runner.vkd3d_dir = Some(abs(v)),
            "--dxc-dir" => cfg.backend.dxc_dir = Some(abs(v)),
            "--opus-dir" => cfg.backend.opus_dir = Some(abs(v)),
            _ => return fail(format!("unknown option {flag}")),
        }
    }
    if cfg.runner.kind == RunnerKind::Proton && cfg.runner.path.is_none()
        && let Some(p) = runners::find_protons(&paths::home()).into_iter().next() {
            println!("using {} ({})", p.name, p.path.display());
            cfg.runner.path = Some(p.path);
        }
    if let Err(e) = config::save_to(&cfg, &path) {
        return fail(e);
    }
    println!("configuration written to {}", path.display());
    if !no_prefix {
        let res = Runner::from_config(&cfg).and_then(|r| r.setup(&mut |l| println!("{l}")));
        if let Err(e) = res {
            return fail(e);
        }
    }
    println!("next: `vilauncher doctor`, then `vilauncher run`");
    ExitCode::SUCCESS
}

fn print_checks(checks: &[doctor::Check]) -> usize {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for c in checks {
        let s = match c.status {
            Status::Ok => "  OK",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        };
        println!("[{s}] {:<width$}  {}", c.name, c.detail);
    }
    let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
    println!("\n{fails} failed, {warns} warnings, {} ok", checks.len() - fails - warns);
    fails
}

fn load() -> Result<Config, ExitCode> {
    config::load().map_err(fail)
}

fn run(args: &[String]) -> ExitCode {
    let mut cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let (mut dry, mut force) = (false, false);
    for (i, a) in args.iter().enumerate() {
        match a.as_str() {
            "--dry-run" => dry = true,
            "--force" => force = true,
            "--" => {
                cfg.game.args = args[i + 1..].to_vec();
                break;
            }
            _ => return fail(format!("unknown option {a}")),
        }
    }
    if !force {
        let fails = launch::blocking_failures(&cfg);
        if !fails.is_empty() {
            for c in &fails {
                eprintln!("error: {}: {}", c.name, c.detail);
            }
            eprintln!("(`vilauncher doctor` shows every check; --force runs anyway)");
            return ExitCode::FAILURE;
        }
    }
    let l = match launch::build(&cfg) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    if dry {
        print!("{}", l.shell_text());
        return ExitCode::SUCCESS;
    }
    let mut cmd = match l.command() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        // Started from the menu: keep the output somewhere it can be read.
        let log = launch::log_file();
        let _ = std::fs::create_dir_all(log.parent().unwrap());
        if let Ok(f) = std::fs::File::create(&log)
            && let Ok(f2) = f.try_clone() {
                cmd.stdout(f).stderr(f2);
            }
    }
    match cmd.status() {
        Ok(s) => ExitCode::from(s.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => fail(format!("{}: {e}", l.argv[0])),
    }
}

fn info(args: &[String]) -> ExitCode {
    let cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let folder = match args.first() {
        Some(d) => abs(d),
        None => match cfg.game_dir() {
            Some(d) => d,
            None => return fail("no game folder: pass one or run `vilauncher setup --game <folder>`"),
        },
    };
    let override_exe = if args.is_empty() { cfg.game.exe.as_deref() } else { None };
    let res = gamedir::inspect(&folder).and_then(|i| gamedir::resolve_exe(&i, override_exe).map(|e| (i, e)));
    let (i, exe) = match res {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let dash = |o: &Option<String>| o.clone().unwrap_or_else(|| "-".into());
    println!("folder:      {}", i.folder.display());
    println!("config:      {}", i.config.as_ref().map(|c| c.display().to_string()).unwrap_or_else(|| "-".into()));
    println!("title:       {}", dash(&i.display_name));
    println!("identity:    {} {}", dash(&i.identity), i.version.clone().unwrap_or_default());
    println!("executables: {}", if i.executables.is_empty() { "-".into() } else { i.executables.join(", ") });
    println!("runs:        {}", exe.display());
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("setup") => setup(rest),
        Some("doctor") => {
            let cfg = match load() {
                Ok(c) => c,
                Err(code) => return code,
            };
            let checks = if rest.iter().any(|a| a == "--config-only") { doctor::config_checks(&cfg) } else { doctor::run_all(&cfg) };
            if print_checks(&checks) > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
        }
        Some("run") => run(rest),
        Some("info") => info(rest),
        Some("protons") => {
            let found = runners::find_protons(&paths::home());
            for p in &found {
                println!("{:<24} {}", p.version, p.path.display());
            }
            if found.is_empty() {
                println!("no Proton found (install GE-Proton or Proton Experimental through Steam)");
            }
            ExitCode::SUCCESS
        }
        Some("config") => match load().map(|c| c.to_toml()) {
            Ok(Ok(t)) => {
                println!("# {}\n{t}", paths::config_file().display());
                ExitCode::SUCCESS
            }
            Ok(Err(e)) => fail(e),
            Err(code) => code,
        },
        Some("desktop") => {
            let cfg = match load() {
                Ok(c) => c,
                Err(code) => return code,
            };
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "vilauncher".into());
            match launch::write_desktop_entry(&cfg, &[exe, "run".into()]) {
                Ok(p) => {
                    println!("wrote {} (a run's output goes to {})", p.display(), launch::log_file().display());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        Some("--version") => {
            println!("vilauncher {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("-h" | "--help" | "help") | None => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => fail(format!("unknown command {other}; see `vilauncher --help`")),
    }
}

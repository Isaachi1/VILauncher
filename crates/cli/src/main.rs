//! `vilauncher`: the launcher without a window (scripts, the menu entry, SSH).

use std::path::PathBuf;
use std::process::ExitCode;

use vilauncher_core::config::{self, BackendKind, Config, Console, RunnerKind};
use vilauncher_core::doctor::{self, Status};
use vilauncher_core::fixes::{self, Fix};
use vilauncher_core::runners::{self, Runner};
use vilauncher_core::{gamedir, launch, paths, report, status, update, virustotal};

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
  vilauncher fix [max_map_count|userfaultfd ...] [--print]
                                  fix the kernel settings the checks warn about (pkexec asks
                                  for the password; kept in /etc/sysctl.d); without a name,
                                  every one that warns; --print only shows the commands
  vilauncher report [FILE.zip]    one .zip for the backends' developers: the checks, the
                                  configuration, the system, and the logs of the last run,
                                  Proton (PROTON_LOG=1), vkd3d-proton and the backend
  vilauncher run [--dry-run] [--force] [-- title args]
  vilauncher info [DIR]           what MicrosoftGame.config says about a game folder
  vilauncher protons              the Proton installs found
  vilauncher config               print the configuration
  vilauncher desktop              add a menu entry that runs the game
  vilauncher status               where VIctory, XCL and the others stand (PortWatch VI)
  vilauncher update [victory|xcl|best] [--force]
                                  the installed and the newest builds (built by CI from the
                                  most advanced public VIctory / XCL trees on PortWatch VI and
                                  scanned on VirusTotal); with a backend, install its newest
                                  build and use it; `best` is the one ranked higher. A build
                                  VirusTotal flags is never installed; with a local key the
                                  files are looked up again (--force skips only that)
  vilauncher virustotal [--upload FILE]
                                  look up the backend folder's .exe/.dll on VirusTotal by hash;
                                  --upload sends one of them (by name) for analysis
  vilauncher virustotal --key KEY save the API key (\"\" removes it); or VILAUNCHER_VT_KEY
";

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}

fn abs(s: &str) -> PathBuf {
    paths::normalize(&paths::expand(PathBuf::from(s).as_path()))
}

fn print_status(r: &status::Report) {
    println!("PortWatch VI ({}), updated {}", r.source, r.updated.as_deref().unwrap_or("?"));
    if let Some(e) = &r.stale {
        println!("offline, showing the cached copy: {e}");
    }
    let o = &r.overall;
    let leader = o.leader.as_deref().and_then(|id| r.efforts.iter().find(|e| e.id == id)).map_or("-", |e| e.name.as_str());
    println!(
        "overall {}% (leader: {leader}), top-3 average {}%, {}/{} boot the game\n",
        o.score, o.top3, o.booted, o.tracked
    );
    for e in &r.efforts {
        let ours = e.backend.map(|b| format!(" [backend {}]", b.name())).unwrap_or_default();
        let stage = if e.partial { format!("{} (em parte)", e.stage) } else { e.stage.clone() };
        let fps = if e.gta.fps > 0.0 { format!(", ~{} fps", e.gta.fps) } else { String::new() };
        println!("{:>5}%  {:<26} {:<34} {}{fps}{ours}", e.score, e.name, e.repo.as_deref().unwrap_or("(código fechado)"), stage);
    }
}

fn print_verdicts(list: &[virustotal::Verdict]) {
    for v in list {
        let what = match v.state {
            virustotal::State::Clean => format!("clean ({} engines, {} suspicious)", v.engines, v.suspicious),
            virustotal::State::Flagged => format!("FLAGGED: {} of {} engines call it malicious", v.malicious, v.engines),
            virustotal::State::Unknown => "not known to VirusTotal yet".into(),
        };
        println!("{:<20} {what}  {}", v.file, v.link);
    }
}

fn virustotal_cmd(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("--key") {
        return match virustotal::save_key(args.get(1).map_or("", String::as_str)) {
            Ok(()) => {
                println!("saved in {}", virustotal::key_file().display());
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        };
    }
    let Some(key) = virustotal::key() else {
        return fail("no VirusTotal API key: `vilauncher virustotal --key KEY` or VILAUNCHER_VT_KEY");
    };
    let cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let dir = match virustotal::backend_dir(&cfg) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    if args.first().map(String::as_str) == Some("--upload") {
        let Some(name) = args.get(1) else { return fail("--upload needs a file name") };
        let found = virustotal::binaries(&dir).ok().and_then(|l| l.into_iter().find(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name.as_str())));
        let Some(path) = found else { return fail(format!("{name} is not a binary in {}", dir.display())) };
        return match virustotal::upload(&key, &path) {
            Ok(id) => {
                println!("sent {}; analysis {id}. Run `vilauncher virustotal` again in a few minutes.", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        };
    }
    match virustotal::check_dir(&dir, &key) {
        Ok(list) => {
            print_verdicts(&list);
            if virustotal::blocking(&list).is_some() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
        }
        Err(e) => fail(e),
    }
}

fn update_cmd(args: &[String]) -> ExitCode {
    let force = args.iter().any(|a| a == "--force");
    let args: Vec<String> = args.iter().filter(|a| *a != "--force").cloned().collect();
    let kind = match args.first().map(String::as_str) {
        None => None,
        Some("victory") => Some(BackendKind::Victory),
        Some("xcl") => Some(BackendKind::Xcl),
        Some("best") => match update::fetch_manifest().map(|m| m.recommended) {
            Ok(Some(k)) => Some(k),
            Ok(None) => return fail("runtimes.json names no recommended backend"),
            Err(e) => return fail(e),
        },
        Some(other) => return fail(format!("unknown backend {other}; victory, xcl or best")),
    };
    let Some(kind) = kind else {
        return match update::status() {
            Ok(rt) => {
                for r in &rt.backends {
                    let latest = r.latest.as_ref().map_or("none published".to_string(), |a| {
                        let vt = a.virustotal.as_ref().map_or("not scanned", |s| s.verdict.as_str());
                        format!("{} {} (VirusTotal: {vt})", a.repo.as_deref().unwrap_or("?"), &a.commit[..a.commit.len().min(12)])
                    });
                    let flag = if r.update { "  <- update available" } else { "" };
                    let installed = r.installed.as_deref().map_or("-", |c| &c[..c.len().min(12)]);
                    println!("{:<8} installed {installed:<13} newest {latest}{flag}", r.backend.name());
                }
                if let Some(best) = rt.recommended {
                    println!("most advanced on PortWatch VI: {} (`vilauncher update best`)", best.name());
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        };
    };
    let mut cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let mut shown = 0u64;
    let mut progress = |done: u64, total: u64| {
        if done == total || done - shown >= total / 20 {
            shown = done;
            eprint!("\rdownloading {}: {}%", kind.name(), done * 100 / total.max(1));
        }
    };
    let key = virustotal::key();
    let mut check = |dir: &std::path::Path| -> vilauncher_core::Result<()> {
        // The workflow's VirusTotal result is checked by `update::install` itself; a
        // local key only adds a second, fresh lookup.
        let Some(key) = &key else { return Ok(()) };
        let list = virustotal::check_dir(dir, key)?;
        eprintln!();
        print_verdicts(&list);
        match virustotal::blocking(&list) {
            Some(why) if !force => Err(vilauncher_core::Error::Config(format!("{why}\nnot installed; `--force` installs it anyway"))),
            _ => Ok(()),
        }
    };
    match update::install(kind, &mut progress, &mut check) {
        Ok(exe) => {
            eprintln!();
            cfg.backend.kind = kind;
            cfg.backend.exe = Some(exe.clone());
            if let Err(e) = config::save(&cfg) {
                return fail(e);
            }
            println!("installed {} and set it as the backend", exe.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!();
            fail(e)
        }
    }
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

fn fix_cmd(args: &[String]) -> ExitCode {
    let print = args.iter().any(|a| a == "--print");
    let mut list = Vec::new();
    for a in args.iter().filter(|a| *a != "--print") {
        match Fix::parse(a) {
            Some(f) => list.push(f),
            None => return fail(format!("unknown fix {a}; max_map_count or userfaultfd")),
        }
    }
    if list.is_empty() {
        list = doctor::machine_checks().into_iter().filter_map(|c| c.fix.map(|p| p.id)).collect();
        if list.is_empty() {
            println!("nothing to fix: no kernel setting warns");
            return ExitCode::SUCCESS;
        }
    }
    for f in &list {
        println!("{} = {}  (kept in {})", f.setting(), f.value(), f.file());
    }
    if list.contains(&Fix::Userfaultfd) {
        println!(
            "\nnote: unprivileged userfaultfd lets every program use it, and it has helped kernel exploits;\n\
             that is why many distributions turn it off. Undo: delete the file and reboot."
        );
    }
    if print {
        println!("\n{}", list.iter().map(|f| f.manual()).collect::<Vec<_>>().join("\n"));
        return ExitCode::SUCCESS;
    }
    match fixes::apply(&list) {
        Ok(()) => {
            println!("done; `vilauncher doctor` checks again");
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn report_cmd(args: &[String]) -> ExitCode {
    let cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let dest = args.first().map(|a| abs(a)).unwrap_or_else(report::default_path);
    match report::write(&cfg, env!("CARGO_PKG_VERSION"), &dest) {
        Ok(w) => {
            for f in &w.files {
                println!("  {f}");
            }
            println!("wrote {} ({:.1} MB); the home folder's path is shown as ~", w.path.display(), w.bytes as f64 / 1e6);
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
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
        Some("fix") => fix_cmd(rest),
        Some("report") => report_cmd(rest),
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
            let mut exec = vilauncher_core::host::self_exec("vilauncher");
            exec.push("run".into());
            match launch::write_desktop_entry(&cfg, &exec) {
                Ok(p) => {
                    println!("wrote {} (a run's output goes to {})", p.display(), launch::log_file().display());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        Some("update") => update_cmd(rest),
        Some("virustotal") => virustotal_cmd(rest),
        Some("status") => match status::report() {
            Ok(r) => {
                print_status(&r);
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
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

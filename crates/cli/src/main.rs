//! `sixlauncher`: the launcher without a window (scripts, the menu entry, SSH).

use std::path::PathBuf;
use std::process::ExitCode;

use sixlauncher_core::config::{self, BackendKind, Config, Console, GpuProfile, RunnerKind, ScarlettPreset};
use sixlauncher_core::doctor::{self, Status};
use sixlauncher_core::fixes::{self, Fix};
use sixlauncher_core::runners::{self, Runner};
use sixlauncher_core::{gamedir, gpu, integrity, launch, paths, perf, protons, report, scarlett, status, update, virustotal};

const HELP: &str = "\
sixlauncher - runs an Xbox Series title you own on Linux through VIctory (VILinux)
or XCL (LinuxCL), on Proton or Wine with vkd3d-proton; on macOS on a Wine with D3DMetal;
on Windows natively (the `native` runner, the default there).

Usage:
  sixlauncher setup [options]      write the configuration and prepare the prefix
      --game DIR                  the unpacked game folder (with MicrosoftGame.config)
      --exe PATH                  the title's executable, relative to the game folder
      --backend-exe FILE          victory.exe, xcl.exe or scarlett.exe (the Windows build)
      --backend victory|xcl|scarlett  default: from the executable's name
      --console series-x|series-s victory and scarlett
      --scarlett-preset stable|look|experimental|custom
      --runner proton|wine|native  native: Windows only
      --runner-path PATH          a Proton folder, or the wine binary
      --prefix DIR                where the prefix lives
      --vkd3d DIR                 wine: an unpacked vkd3d-proton release
      --dxc-dir DIR               xcl: DXC's bin/x64 (dxcompiler.dll, dxil.dll)
      --opus-dir DIR              xcl: a folder with opus.dll
      --gpu auto|system|PCI       which GPU runs the game (auto: a dedicated one; see `gpus`)
      --gpu-profile auto|off|nvidia|amd|intel
                                  the vendor preset of variables (auto: the chosen GPU's)
      --mangohud on|off           MangoHud's overlay (FPS, frame time, GPU / CPU load)
      --fps-limit N               cap the frame rate through MangoHud (0: no cap)
      --gamemode on|off           Feral's GameMode while the game runs
      --no-prefix                 only write the configuration
  sixlauncher doctor [--config-only]   check the machine and the configuration
  sixlauncher verify [FILE]        the SHA-256 of the game's executable (or of FILE, such as the
                                  game's .zip) against the known ones; `[game] verify = false`
                                  turns off the automatic check
  sixlauncher fix [max_map_count|userfaultfd ...] [--print]
                                  fix the kernel settings the checks warn about (pkexec asks
                                  for the password; kept in /etc/sysctl.d); without a name,
                                  every one that warns; --print only shows the commands
  sixlauncher report [FILE.zip]    one .zip for the backends' developers: the checks, the
                                  configuration, the system, and the logs of the last run,
                                  Proton (PROTON_LOG=1), vkd3d-proton and the backend
  sixlauncher run [--dry-run] [--force] [-- title args]
  sixlauncher info [DIR]           what MicrosoftGame.config says about a game folder
  sixlauncher protons              the Proton installs found (Steam's and the launcher's own)
  sixlauncher proton available     GE-Proton releases on GitHub
  sixlauncher proton install [NAME|latest] [--use]
                                  download a GE-Proton release (size and SHA-512 checked) into
                                  ~/.local/share/sixlauncher/protons; --use also selects it
  sixlauncher proton use NAME|PATH select an installed Proton (empty name: the best found)
  sixlauncher proton remove NAME   delete a Proton the launcher downloaded
  sixlauncher prepare-shaders [--dry-run]
                                  ScarlettPort: translate every shader and make the pipelines
                                  ahead of a run (`scarlett prepare`), for the chosen preset;
                                  a stopped run resumes where it was
  sixlauncher find-games           look for game folders (MicrosoftGame.config) in the home
                                  folder and on mounted disks, for up to 25 s
  sixlauncher gpus                 the GPUs, the one `auto` picks, MangoHud and GameMode
  sixlauncher config               print the configuration
  sixlauncher desktop              add a menu entry that runs the game
  sixlauncher status               where VIctory, XCL and the others stand (PortWatch VI)
  sixlauncher update [victory|xcl|scarlett|best] [--build NAME] [--force]
                                  the installed and the published builds (ScarlettPort, the
                                  default, and each public VIctory / XCL tree scoring 59% or
                                  more on PortWatch VI, scanned on VirusTotal); with a backend, install its best build (--build:
                                  a project, repository or archive from the list) and use it;
                                  `best` is the backend ranked higher. A build VirusTotal flags is never installed;
                                  with a local key the files are looked up again (--force skips
                                  only that)
  sixlauncher rollback victory|xcl|scarlett [ID]
                                  list the builds earlier updates replaced (the last 3 are
                                  kept); with an ID (or `last`), put that build back in place
  sixlauncher virustotal [--upload FILE]
                                  look up the backend folder's .exe/.dll on VirusTotal by hash;
                                  --upload sends one of them (by name) for analysis
  sixlauncher virustotal --key KEY save the API key (\"\" removes it); or SIXLAUNCHER_VT_KEY
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
        return fail("no VirusTotal API key: `sixlauncher virustotal --key KEY` or SIXLAUNCHER_VT_KEY");
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
                println!("sent {}; analysis {id}. Run `sixlauncher virustotal` again in a few minutes.", path.display());
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
    let mut build: Option<String> = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" => {}
            "--build" => match it.next() {
                Some(b) => build = Some(b.clone()),
                None => return fail("--build needs a project, repository or archive name"),
            },
            _ => rest.push(a.clone()),
        }
    }
    let args = rest;
    let kind = match args.first().map(String::as_str) {
        None => None,
        Some("best") => match update::fetch_manifest().map(|m| m.recommended) {
            Ok(Some(k)) => Some(k),
            Ok(None) => return fail("runtimes.json names no recommended backend"),
            Err(e) => return fail(e),
        },
        Some(other) => match BackendKind::from_name(other) {
            Some(k) => Some(k),
            None => return fail(format!("unknown backend {other}; victory, xcl, scarlett or best")),
        },
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
                    for b in &r.choices {
                        let a = &b.asset;
                        let score = b.score.map_or(String::new(), |s| format!("{s:>5.1}%  "));
                        let vt = a.virustotal.as_ref().map_or("not scanned", |s| s.verdict.as_str());
                        println!(
                            "           {score}{:<12} {:<28} {}  VirusTotal: {vt}",
                            a.effort.as_deref().unwrap_or("-"),
                            a.repo.as_deref().unwrap_or("?"),
                            &a.commit[..a.commit.len().min(12)]
                        );
                    }
                }
                if let Some(best) = rt.recommended {
                    println!("most advanced on PortWatch VI: {} (`sixlauncher update best`)", best.name());
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
    let mut check = |dir: &std::path::Path| -> sixlauncher_core::Result<()> {
        // The workflow's VirusTotal result is checked by `update::install` itself; a
        // local key only adds a second, fresh lookup.
        let Some(key) = &key else { return Ok(()) };
        let list = virustotal::check_dir(dir, key)?;
        eprintln!();
        print_verdicts(&list);
        match virustotal::blocking(&list) {
            Some(why) if !force => Err(sixlauncher_core::Error::Config(format!("{why}\nnot installed; `--force` installs it anyway"))),
            _ => Ok(()),
        }
    };
    // --build: a project (as PortWatch VI names it), a repository or an archive name.
    let file = match &build {
        None => None,
        Some(want) => {
            let m = match update::fetch_manifest() {
                Ok(m) => m,
                Err(e) => return fail(e),
            };
            let want_l = want.to_ascii_lowercase();
            let found = m.builds_of(kind).into_iter().map(|b| b.asset).find(|a| {
                a.file == *want
                    || a.repo.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(want))
                    || a.effort.as_deref().is_some_and(|e| e.to_ascii_lowercase() == want_l)
            });
            match found {
                Some(a) => Some(a.file),
                None => return fail(format!("no {} build {want}; `sixlauncher update` lists them", kind.name())),
            }
        }
    };
    match update::install(kind, file.as_deref(), &mut progress, &mut check) {
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

fn origin(o: runners::Origin) -> &'static str {
    match o {
        runners::Origin::Steam => "steam",
        runners::Origin::Launcher => "sixlauncher",
        runners::Origin::System => "system",
    }
}

fn proton_cmd(args: &[String]) -> ExitCode {
    let use_it = args.iter().any(|a| a == "--use");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--use").collect();
    match args.first().map(|a| a.as_str()) {
        Some("available") => match protons::available() {
            Ok(list) => {
                for r in &list {
                    let mark = if r.installed { "  (installed)" } else { "" };
                    let date = r.published_at.as_deref().map_or("", |d| &d[..d.len().min(10)]);
                    println!("{:<18} {date:<10} {:>6.0} MB{mark}", r.name, r.size as f64 / 1048576.0);
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
        Some("install") => {
            let name = args.get(1).map_or("latest", |a| a.as_str());
            let mut shown = 0u64;
            let mut progress = |done: u64, total: u64| {
                if done == total || done - shown >= total / 20 {
                    shown = done;
                    eprint!("\rdownloading {name}: {}%", done * 100 / total.max(1));
                }
            };
            let installed = match protons::install(name, &mut progress) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!();
                    return fail(e);
                }
            };
            eprintln!();
            println!("installed {} in {}", installed.name, installed.path.display());
            if use_it {
                return use_proton(Some(installed.path));
            }
            println!("`sixlauncher proton use {}` selects it", installed.name);
            ExitCode::SUCCESS
        }
        Some("use") => {
            let Some(which) = args.get(1) else { return use_proton(None) };
            let found = runners::find_protons(&paths::home());
            let path = match found.into_iter().find(|p| &p.name == *which) {
                Some(p) => p.path,
                None => abs(which),
            };
            if !path.join("proton").is_file() {
                return fail(format!("{which} is not an installed Proton (see `sixlauncher protons`)"));
            }
            use_proton(Some(path))
        }
        Some("remove") => {
            let Some(name) = args.get(1) else { return fail("proton remove needs a name") };
            let cfg = match load() {
                Ok(c) => c,
                Err(code) => return code,
            };
            match protons::remove(name, &cfg) {
                Ok(()) => {
                    println!("removed {name}");
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        _ => fail("proton available | install [NAME|latest] [--use] | use NAME | remove NAME"),
    }
}

fn use_proton(path: Option<PathBuf>) -> ExitCode {
    let mut cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    cfg.runner.kind = RunnerKind::Proton;
    cfg.runner.path = path.clone();
    if let Err(e) = config::save(&cfg) {
        return fail(e);
    }
    match path {
        Some(p) => println!("the game runs with {}", p.display()),
        None => println!("the game runs with the best Proton found"),
    }
    ExitCode::SUCCESS
}

fn rollback_cmd(args: &[String]) -> ExitCode {
    let Some(kind) = args.first().and_then(|a| BackendKind::from_name(a)) else {
        return fail("rollback victory|xcl|scarlett [ID|last]");
    };
    let list = update::previous(kind);
    let Some(id) = args.get(1) else {
        if list.is_empty() {
            println!("no previous {} build is kept (one is kept after each update)", kind.name());
        }
        for p in &list {
            let from = p.source.as_ref().and_then(|s| s.repo.clone()).unwrap_or_else(|| "?".into());
            println!("{:<14} {from}", p.id);
        }
        return ExitCode::SUCCESS;
    };
    let id = if id == "last" {
        match list.first() {
            Some(p) => p.id.clone(),
            None => return fail(format!("no previous {} build is kept", kind.name())),
        }
    } else {
        // A commit prefix is enough when it names one build.
        let hits: Vec<&update::PreviousBuild> = list.iter().filter(|p| p.id.starts_with(id.as_str())).collect();
        match hits.as_slice() {
            [one] => one.id.clone(),
            [] => return fail(format!("no previous {} build {id}; `sixlauncher rollback {}` lists them", kind.name(), kind.name())),
            _ => return fail(format!("{id} names more than one build")),
        }
    };
    match update::rollback(kind, &id) {
        Ok(exe) => {
            println!("{} is back to {id} ({})", kind.name(), exe.display());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
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
                cfg.backend.kind = match BackendKind::from_name(v) {
                    Some(k) => k,
                    None => return fail("--backend is victory, xcl or scarlett"),
                }
            }
            "--console" => {
                cfg.backend.console = match v.as_str() {
                    "series-x" => Console::SeriesX,
                    "series-s" => Console::SeriesS,
                    _ => return fail("--console is series-x or series-s"),
                }
            }
            "--scarlett-preset" => {
                cfg.scarlett.preset = match v.as_str() {
                    "stable" => ScarlettPreset::Stable,
                    "look" => ScarlettPreset::Look,
                    "experimental" => ScarlettPreset::Experimental,
                    "custom" => ScarlettPreset::Custom,
                    _ => return fail("--scarlett-preset is stable, look, experimental or custom"),
                }
            }
            "--runner" => {
                cfg.runner.kind = match v.as_str() {
                    "proton" => RunnerKind::Proton,
                    "wine" => RunnerKind::Wine,
                    "native" => RunnerKind::Native,
                    _ => return fail("--runner is proton, wine or native"),
                }
            }
            "--runner-path" => cfg.runner.path = Some(abs(v)),
            "--prefix" => cfg.runner.prefix = Some(abs(v)),
            "--vkd3d" => cfg.runner.vkd3d_dir = Some(abs(v)),
            "--dxc-dir" => cfg.backend.dxc_dir = Some(abs(v)),
            "--opus-dir" => cfg.backend.opus_dir = Some(abs(v)),
            "--gpu" => cfg.gpu.device = v.clone(),
            "--gpu-profile" => {
                cfg.gpu.profile = match v.as_str() {
                    "auto" => GpuProfile::Auto,
                    "off" => GpuProfile::Off,
                    "nvidia" => GpuProfile::Nvidia,
                    "amd" => GpuProfile::Amd,
                    "intel" => GpuProfile::Intel,
                    _ => return fail("--gpu-profile is auto, off, nvidia, amd or intel"),
                }
            }
            "--mangohud" | "--gamemode" => {
                let on = match v.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return fail(format!("{flag} is on or off")),
                };
                if flag == "--mangohud" { cfg.performance.mangohud = on } else { cfg.performance.gamemode = on }
            }
            "--fps-limit" => match v.parse::<u32>() {
                Ok(n) => cfg.performance.fps_limit = Some(n).filter(|n| *n > 0),
                Err(_) => return fail("--fps-limit is a number"),
            },
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
    println!("next: `sixlauncher doctor`, then `sixlauncher run`");
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

fn verify_cmd(args: &[String]) -> ExitCode {
    let path = match args.first() {
        Some(f) => PathBuf::from(f),
        None => {
            let cfg = match load() {
                Ok(c) => c,
                Err(code) => return code,
            };
            let Some(dir) = cfg.game_dir() else {
                return fail("no game folder configured; pass a file: `sixlauncher verify FILE`");
            };
            match gamedir::inspect(&dir).and_then(|info| gamedir::resolve_exe(&info, cfg.game.exe.as_deref())) {
                Ok(exe) => exe,
                Err(e) => return fail(e),
            }
        }
    };
    match integrity::verify(&path) {
        Ok(v) => {
            println!("{}
SHA-256 {}", v.path.display(), v.sha256);
            match v.known {
                Some(k) => {
                    let what = match k.kind {
                        integrity::Kind::Exe => "executable",
                        integrity::Kind::Zip => ".zip",
                    };
                    println!("ok: the known {} {what}", k.name);
                    ExitCode::SUCCESS
                }
                None => {
                    println!("not a known file: another version, or a changed or damaged file");
                    ExitCode::FAILURE
                }
            }
        }
        Err(e) => fail(e),
    }
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
            eprintln!("(`sixlauncher doctor` shows every check; --force runs anyway)");
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
    if let Some(note) = scarlett::before_run(&cfg) {
        eprintln!("{note}");
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

fn prepare_shaders(args: &[String]) -> ExitCode {
    let cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let dry = match args {
        [] => false,
        [a] if a == "--dry-run" => true,
        [a, ..] => return fail(format!("unknown option {a}")),
    };
    let l = match launch::prepare_shaders(&cfg) {
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
    let mut child = match cmd.stdout(std::process::Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("{}: {e}", l.argv[0])),
    };
    let mut finished = false;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufRead::lines(std::io::BufReader::new(out)).map_while(Result::ok) {
            println!("{line}");
            finished |= matches!(scarlett::parse_prep(&line), Some(scarlett::PrepLine::Done { .. }));
        }
    }
    match child.wait() {
        Ok(s) if s.success() && (finished || scarlett::cache_complete(&cfg)) => match scarlett::mark_prepared(&cfg) {
            Ok(()) => {
                println!("shaders prepared in {}", scarlett::prep_cache(&cfg).display());
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
        Ok(s) if s.success() => fail("scarlett prepare ended without `prepare done`: is this runtime older than 0.5?"),
        Ok(s) => fail(format!("scarlett prepare failed (exit code {:?}); run it again to resume", s.code())),
        Err(e) => fail(e),
    }
}

fn gpus() -> ExitCode {
    let cfg = match load() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let list = gpu::list();
    let auto = gpu::pick_auto(&list).map(|g| g.slot.clone());
    for g in &list {
        let mut tags = vec![g.driver.clone()];
        if g.boot_vga {
            tags.push("boot".into());
        }
        tags.push(if g.integrated { "integrated" } else { "dedicated" }.into());
        if auto.as_deref() == Some(g.slot.as_str()) {
            tags.push("auto".into());
        }
        println!("{}  {:<40} {}  [{}]", g.slot, g.name, g.ids(), tags.join(", "));
    }
    if list.is_empty() {
        println!("no GPU found in /sys/class/drm");
    }
    println!("\nconfigured: device = {}, profile = {}", cfg.gpu.device, cfg.gpu.profile.name());
    for (k, v) in gpu::env_for(&cfg, &list) {
        println!("  {k}={v}");
    }
    let tools = perf::detect();
    for (name, package, t) in [("MangoHud", "mangohud", &tools.mangohud), ("GameMode", "gamemode", &tools.gamemode)] {
        println!("{name:<9} {}", perf::describe(package, t, tools.sandboxed).1);
    }
    ExitCode::SUCCESS
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
            None => return fail("no game folder: pass one or run `sixlauncher setup --game <folder>`"),
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
            println!("done; `sixlauncher doctor` checks again");
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
    sixlauncher_core::paths::migrate_legacy_dirs();
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
        Some("verify") => verify_cmd(rest),
        Some("fix") => fix_cmd(rest),
        Some("report") => report_cmd(rest),
        Some("run") => run(rest),
        Some("prepare-shaders") => prepare_shaders(rest),
        Some("find-games") => {
            let found = gamedir::search(&gamedir::search_starts(&paths::home()), std::time::Instant::now() + gamedir::SEARCH_LIMIT);
            for dir in &found {
                let title = gamedir::inspect(dir).map(|i| i.title()).unwrap_or_default();
                println!("{}  {title}", dir.display());
            }
            if found.is_empty() {
                println!("no game folder found");
            }
            ExitCode::SUCCESS
        }
        Some("info") => info(rest),
        Some("gpus") => gpus(),
        Some("protons") => {
            let found = runners::find_protons(&paths::home());
            for p in &found {
                println!("{:<24} {:<10} {}", p.version, origin(p.origin), p.path.display());
            }
            if found.is_empty() {
                println!("no Proton found: `sixlauncher proton install latest` downloads GE-Proton");
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
            let mut exec = sixlauncher_core::host::self_exec("sixlauncher");
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
        Some("proton") => proton_cmd(rest),
        Some("rollback") => rollback_cmd(rest),
        Some("virustotal") => virustotal_cmd(rest),
        Some("status") => match status::report() {
            Ok(r) => {
                print_status(&r);
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
        Some("--version") => {
            println!("sixlauncher {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("-h" | "--help" | "help") | None => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => fail(format!("unknown command {other}; see `sixlauncher --help`")),
    }
}

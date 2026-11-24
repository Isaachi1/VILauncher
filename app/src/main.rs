//! VILauncher's window (Tauri): configure, check, prepare the prefix, play, read the log,
//! update the backends and see where each effort stands.
//!
//! Everything it does goes through `vilauncher-core`, the same code as the
//! `vilauncher` command line.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use vilauncher_core::config::{self, Config};
use vilauncher_core::doctor::{self, Check};
use vilauncher_core::gamedir::{self, GameInfo};
use vilauncher_core::launch;
use vilauncher_core::paths;
use vilauncher_core::runners::{self, ProtonInstall, Runner};
use vilauncher_core::status::{self, Report};
use vilauncher_core::update::{self, Runtimes};
use vilauncher_core::virustotal::{self, Verdict};

/// The game the window started, if it is still running.
#[derive(Default)]
struct Running {
    child: Arc<Mutex<Option<Child>>>,
    runner: Mutex<Option<Runner>>,
}

#[derive(Serialize)]
struct AppState {
    config: Config,
    config_path: PathBuf,
    protons: Vec<ProtonInstall>,
    log_file: PathBuf,
    running: bool,
    /// A VirusTotal API key is saved (the key itself never goes to the window).
    virustotal: bool,
}

#[derive(Serialize)]
struct Installed {
    /// The new build's executable; None when VirusTotal stopped the install.
    exe: Option<String>,
    /// Whether VirusTotal was asked (there is a key).
    checked: bool,
    verdicts: Vec<Verdict>,
    blocked: Option<String>,
}

#[derive(Clone, Serialize)]
struct Progress {
    backend: config::BackendKind,
    done: u64,
    total: u64,
}

#[derive(Clone, Serialize)]
struct LogLine {
    stream: &'static str,
    line: String,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn get_state(running: State<Running>) -> Result<AppState, String> {
    Ok(AppState {
        config: config::load().map_err(err)?,
        config_path: paths::config_file(),
        protons: runners::find_protons(&paths::home()),
        log_file: launch::log_file(),
        running: running.child.lock().unwrap().is_some(),
        virustotal: virustotal::key().is_some(),
    })
}

#[tauri::command]
fn save_config(config: Config) -> Result<String, String> {
    config::save(&config).map(|p| p.display().to_string()).map_err(err)
}

#[tauri::command]
fn guess_backend(exe: String) -> Option<config::BackendKind> {
    config::guess_backend(&PathBuf::from(exe))
}

#[tauri::command]
async fn run_doctor(config: Config) -> Result<Vec<Check>, String> {
    tauri::async_runtime::spawn_blocking(move || doctor::run_all(&config)).await.map_err(err)
}

#[tauri::command]
fn game_info(dir: String) -> Result<GameInfo, String> {
    gamedir::inspect(&paths::expand(&PathBuf::from(dir))).map_err(err)
}

#[tauri::command]
async fn setup_prefix(config: Config) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut lines = Vec::new();
        Runner::from_config(&config).and_then(|r| r.setup(&mut |l| lines.push(l))).map_err(err)?;
        Ok(lines)
    })
    .await
    .map_err(err)?
}

/// Where each effort stands, from PortWatch VI (read only; the cached copy when offline).
#[tauri::command]
async fn portwatch_status() -> Result<Report, String> {
    tauri::async_runtime::spawn_blocking(status::report).await.map_err(err)?.map_err(err)
}

/// The installed and the newest builds of VIctory and XCL (the `runtimes` release), with
/// the repository each comes from, its VirusTotal result and the tracker's pick.
#[tauri::command]
async fn runtime_status() -> Result<Runtimes, String> {
    tauri::async_runtime::spawn_blocking(update::status).await.map_err(err)?.map_err(err)
}

/// Downloads, checks and installs the newest build of `backend`. With a VirusTotal
/// key, its binaries are looked up first and a flagged build is not installed
/// unless `force`. Progress arrives as `runtime-progress` events.
#[tauri::command]
async fn install_runtime(app: AppHandle, running: State<'_, Running>, backend: config::BackendKind, force: bool) -> Result<Installed, String> {
    if running.child.lock().unwrap().is_some() {
        return Err("stop the game before updating its runtime".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut last = 0u64;
        let mut progress = |done: u64, total: u64| {
            // About one event per percent.
            if done == total || done - last >= total / 100 {
                last = done;
                let _ = app.emit("runtime-progress", Progress { backend, done, total });
            }
        };
        let key = virustotal::key();
        let mut verdicts = Vec::new();
        let mut blocked = None;
        let result = update::install(backend, &mut progress, &mut |dir| {
            let Some(key) = &key else { return Ok(()) };
            verdicts = virustotal::check_dir(dir, key)?;
            match virustotal::blocking(&verdicts).filter(|_| !force) {
                Some(why) => {
                    blocked = Some(why.clone());
                    Err(vilauncher_core::Error::Config(why))
                }
                None => Ok(()),
            }
        });
        let checked = key.is_some();
        match result {
            Ok(exe) => Ok(Installed { exe: Some(exe.display().to_string()), checked, verdicts, blocked: None }),
            Err(_) if blocked.is_some() => Ok(Installed { exe: None, checked, verdicts, blocked }),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// Saves the VirusTotal API key (an empty one removes it).
#[tauri::command]
fn virustotal_key(key: String) -> Result<bool, String> {
    virustotal::save_key(&key).map_err(err)?;
    Ok(virustotal::key().is_some())
}

/// Looks up the binaries next to the backend executable in use.
#[tauri::command]
async fn virustotal_check(config: Config) -> Result<Vec<Verdict>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let key = virustotal::key().ok_or("no VirusTotal API key is saved")?;
        let dir = virustotal::backend_dir(&config).map_err(err)?;
        virustotal::check_dir(&dir, &key).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Sends one binary of the backend's folder (by name) to VirusTotal for analysis.
#[tauri::command]
async fn virustotal_upload(config: Config, file: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let key = virustotal::key().ok_or("no VirusTotal API key is saved")?;
        let dir = virustotal::backend_dir(&config).map_err(err)?;
        let path = virustotal::binaries(&dir)
            .map_err(err)?
            .into_iter()
            .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == file))
            .ok_or_else(|| format!("{file} is not a binary of the backend's folder"))?;
        virustotal::upload(&key, &path).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn dry_run(config: Config) -> Result<String, String> {
    launch::build(&config).map(|l| l.shell_text()).map_err(err)
}

#[tauri::command]
fn create_desktop_entry(config: Config) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(err)?;
    launch::write_desktop_entry(&config, &[exe.to_string_lossy().into_owned()]).map(|p| p.display().to_string()).map_err(err)
}

/// Starts the game. Its output arrives as `game-log` events and is also written
/// to the log file; `game-exit` carries the exit code.
#[tauri::command]
fn launch_game(app: AppHandle, running: State<Running>, config: Config, force: bool) -> Result<(), String> {
    if running.child.lock().unwrap().is_some() {
        return Err("the game is already running".into());
    }
    if !force {
        let fails = launch::blocking_failures(&config);
        if !fails.is_empty() {
            let list: Vec<String> = fails.iter().map(|c| format!("{}: {}", c.name, c.detail)).collect();
            return Err(list.join("\n"));
        }
    }
    let l = launch::build(&config).map_err(err)?;
    let runner = Runner::from_config(&config).map_err(err)?;
    let mut cmd = l.command().map_err(err)?;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{}: {e}", l.argv[0]))?;

    let log_path = launch::log_file();
    let _ = std::fs::create_dir_all(log_path.parent().unwrap());
    let log = Arc::new(Mutex::new(std::fs::File::create(&log_path).map_err(err)?));
    writeln!(log.lock().unwrap(), "# {}", l.argv.iter().map(|a| launch::shell_quote(a)).collect::<Vec<_>>().join(" ")).ok();

    let mut readers = Vec::new();
    let pipes: [(&'static str, Option<Box<dyn std::io::Read + Send>>); 2] = [
        ("stdout", child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)),
        ("stderr", child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)),
    ];
    for (stream, pipe) in pipes {
        let Some(pipe) = pipe else { continue };
        let (app, log) = (app.clone(), log.clone());
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                writeln!(log.lock().unwrap(), "{line}").ok();
                let _ = app.emit("game-log", LogLine { stream, line });
            }
        }));
    }

    *running.child.lock().unwrap() = Some(child);
    *running.runner.lock().unwrap() = Some(runner);
    let slot = running.child.clone();
    std::thread::spawn(move || {
        let code = loop {
            {
                let mut guard = slot.lock().unwrap();
                match guard.as_mut().map(|c| c.try_wait()) {
                    Some(Ok(Some(status))) => {
                        *guard = None;
                        break status.code();
                    }
                    Some(Ok(None)) => {}
                    Some(Err(_)) | None => {
                        *guard = None;
                        break None;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        };
        for r in readers {
            let _ = r.join();
        }
        let _ = app.emit("game-exit", code);
    });
    Ok(())
}

#[tauri::command]
fn stop_game(running: State<Running>) -> Result<(), String> {
    if let Some(child) = running.child.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
    if let Some(runner) = running.runner.lock().unwrap().as_ref() {
        let _ = runner.kill_command().status();
    }
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Running::default())
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_config,
            guess_backend,
            run_doctor,
            game_info,
            setup_prefix,
            dry_run,
            portwatch_status,
            runtime_status,
            install_runtime,
            virustotal_key,
            virustotal_check,
            virustotal_upload,
            create_desktop_entry,
            launch_game,
            stop_game
        ])
        .run(tauri::generate_context!())
        .expect("error while running VILauncher");
}

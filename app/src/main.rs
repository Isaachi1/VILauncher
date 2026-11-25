//! VILauncher's window (Tauri): configure, check, prepare the prefix, play, read the log,
//! update the backends and see where each effort stands. It also keeps itself up to date
//! from this repository's releases (see `app_update_check`).
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
use tauri_plugin_updater::UpdaterExt;
use vilauncher_core::avatars;
use vilauncher_core::config::{self, Config};
use vilauncher_core::doctor::{self, Check};
use vilauncher_core::gamedir::{self, GameInfo};
use vilauncher_core::launch;
use vilauncher_core::{gpu, notify, perf};
use vilauncher_core::paths;
use vilauncher_core::protons::{self, Release};
use vilauncher_core::runners::{self, ProtonInstall, Runner};
use vilauncher_core::selfupdate;
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

/// The efforts' pictures as `data:` URLs by id (downloaded here, cached for a week),
/// so the window itself never loads anything from other sites.
#[tauri::command]
async fn portwatch_avatars(wanted: Vec<avatars::Wanted>) -> Result<std::collections::HashMap<String, String>, String> {
    tauri::async_runtime::spawn_blocking(move || avatars::fetch_all(&wanted)).await.map_err(err)
}

/// The installed and the newest builds of VIctory and XCL (the `runtimes` release), with
/// the repository each comes from, its VirusTotal result, the tracker's pick and the
/// builds kept from earlier updates. Offline, only what is installed (`offline` says why).
#[tauri::command]
async fn runtime_status() -> Result<Runtimes, String> {
    tauri::async_runtime::spawn_blocking(update::status_or_local).await.map_err(err)
}

/// Puts back a build an earlier update replaced; the one in use is kept in its place.
#[tauri::command]
async fn runtime_rollback(running: State<'_, Running>, backend: config::BackendKind, id: String) -> Result<String, String> {
    if running.child.lock().unwrap().is_some() {
        return Err("stop the game before changing its runtime".into());
    }
    tauri::async_runtime::spawn_blocking(move || update::rollback(backend, &id).map(|p| p.display().to_string()))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Every Proton found: Steam's and the ones the launcher downloaded.
#[tauri::command]
fn list_protons() -> Vec<ProtonInstall> {
    runners::find_protons(&paths::home())
}

/// GE-Proton releases on GitHub, marking the ones already downloaded.
#[tauri::command]
async fn proton_available() -> Result<Vec<Release>, String> {
    tauri::async_runtime::spawn_blocking(protons::available).await.map_err(err)?.map_err(err)
}

#[derive(Clone, Serialize)]
struct ProtonProgress {
    name: String,
    done: u64,
    total: u64,
}

/// Downloads a GE-Proton release, checks its size and SHA-512 and unpacks it into the
/// launcher's folder. Progress arrives as `proton-progress` events.
#[tauri::command]
async fn proton_install(app: AppHandle, name: String) -> Result<ProtonInstall, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut last = 0u64;
        let label = name.clone();
        protons::install(&name, &mut |done, total| {
            if done == total || done - last >= total / 100 {
                last = done;
                let _ = app.emit("proton-progress", ProtonProgress { name: label.clone(), done, total });
            }
        })
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// Deletes a Proton the launcher downloaded (never Steam's, never the one in use).
#[tauri::command]
async fn proton_remove(running: State<'_, Running>, name: String, config: Config) -> Result<(), String> {
    if running.child.lock().unwrap().is_some() {
        return Err("stop the game before removing a Proton".into());
    }
    tauri::async_runtime::spawn_blocking(move || protons::remove(&name, &config)).await.map_err(err)?.map_err(err)
}

/// Downloads, checks and installs a build of `backend`: the archive `file` from the list of
/// published builds, or the best one. With a VirusTotal
/// key, its binaries are looked up first and a flagged build is not installed
/// unless `force`. Progress arrives as `runtime-progress` events.
#[tauri::command]
async fn install_runtime(
    app: AppHandle,
    running: State<'_, Running>,
    backend: config::BackendKind,
    force: bool,
    file: Option<String>,
) -> Result<Installed, String> {
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
        let result = update::install(backend, file.as_deref(), &mut progress, &mut |dir| {
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

/// Only one look at a time, so two looks never announce the same news twice.
#[derive(Default)]
struct Notifier(tauri::async_runtime::Mutex<()>);

/// What changed since the last look (an effort passed another in the race, a new runtime
/// build, a new VILauncher), filtered by the switches in `[notifications]`. The window
/// words each event in the player's language and shows it as a desktop notification.
#[tauri::command]
async fn notifications_poll(app: AppHandle, notifier: State<'_, Notifier>) -> Result<Vec<notify::Event>, String> {
    let _one = notifier.0.lock().await;
    let current = app.package_info().version.to_string();
    let settings = config::load().map_err(err)?.notifications;
    tauri::async_runtime::spawn_blocking(move || notify::poll(&settings, &current)).await.map_err(err)
}

/// Turns each kind of notification on or off, in the saved configuration (the rest of it
/// is left as saved).
#[tauri::command]
fn notifications_set(settings: config::NotificationsSection) -> Result<config::NotificationsSection, String> {
    let mut cfg = config::load().map_err(err)?;
    cfg.notifications = settings;
    config::save(&cfg).map_err(err)?;
    Ok(cfg.notifications)
}

/// The package Tauri's bundler stamped into this binary, if any.
fn packaged() -> Option<&'static str> {
    use tauri::utils::config::BundleType;
    match tauri::utils::platform::bundle_type()? {
        BundleType::AppImage => Some("appimage"),
        BundleType::Deb => Some("deb"),
        BundleType::Rpm => Some("rpm"),
        _ => None,
    }
}

/// The updater's public key (tauri.conf.json > plugins.updater.pubkey, filled in by CI).
/// Without one nothing can be verified, so nothing is installed automatically.
fn updater_pubkey(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.trim().is_empty())
}

/// Whether a newer VILauncher is published (the newest release's latest.json), and whether
/// this copy can install it by itself: an AppImage, or the .deb / .rpm installed under /usr,
/// from a signed release. A Flatpak or the tarball is told where to download it.
#[tauri::command]
async fn app_update_check(app: AppHandle) -> Result<Option<selfupdate::Check>, String> {
    let current = app.package_info().version.to_string();
    let can_verify = updater_pubkey(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let install = selfupdate::detect_current(packaged());
        selfupdate::fetch_manifest()
            .map(|m| m.map(|m| selfupdate::evaluate(&m, &current, install, can_verify)))
            .map_err(err)
    })
    .await
    .map_err(err)?
}

#[derive(Clone, Serialize)]
struct AppUpdateProgress {
    done: u64,
    total: Option<u64>,
}

/// Downloads the newest VILauncher, checks its signature and installs it over this one
/// (the AppImage file is replaced; a .deb or .rpm is installed with pkexec, which asks for
/// the password). Progress arrives as `app-update-progress` events. Returns the version
/// installed, or None when there was nothing newer. The new version runs on the next start
/// (`app_restart`).
#[tauri::command]
async fn app_update_install(app: AppHandle) -> Result<Option<String>, String> {
    if !updater_pubkey(&app) {
        return Err("this copy of VILauncher has no updater public key".into());
    }
    let Some(update) = app.updater().map_err(err)?.check().await.map_err(err)? else {
        return Ok(None);
    };
    let mut done = 0u64;
    let mut last = 0u64;
    let emitter = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                // About one event per percent.
                let step = total.map_or(1 << 20, |t| (t / 100).max(1));
                if done - last >= step || Some(done) == total {
                    last = done;
                    let _ = emitter.emit("app-update-progress", AppUpdateProgress { done, total });
                }
            },
            || {},
        )
        .await
        .map_err(err)?;
    Ok(Some(update.version))
}

/// Starts the installed (new) VILauncher in place of this one. Refused while the game runs,
/// since the window owns its process and its log.
#[tauri::command]
fn app_restart(app: AppHandle, running: State<Running>) -> Result<(), String> {
    if running.child.lock().unwrap().is_some() {
        return Err("stop the game before restarting VILauncher".into());
    }
    app.request_restart();
    Ok(())
}

#[derive(Serialize)]
struct Hardware {
    gpus: Vec<gpu::Gpu>,
    /// The PCI address `auto` picks.
    auto: Option<String>,
    tools: perf::Tools,
}

/// The GPUs, and whether MangoHud and GameMode are installed where the game runs.
#[tauri::command]
async fn hardware() -> Result<Hardware, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let gpus = gpu::list();
        let auto = gpu::pick_auto(&gpus).map(|g| g.slot.clone());
        Hardware { gpus, auto, tools: perf::detect() }
    })
    .await
    .map_err(err)
}

#[tauri::command]
fn dry_run(config: Config) -> Result<String, String> {
    launch::build(&config).map(|l| l.shell_text()).map_err(err)
}

#[tauri::command]
fn create_desktop_entry(config: Config) -> Result<String, String> {
    let exec = vilauncher_core::host::self_exec("vilauncher-app");
    launch::write_desktop_entry(&config, &exec).map(|p| p.display().to_string()).map_err(err)
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .manage(Running::default())
        .manage(Notifier::default())
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_config,
            guess_backend,
            run_doctor,
            game_info,
            setup_prefix,
            dry_run,
            hardware,
            portwatch_status,
            portwatch_avatars,
            runtime_status,
            runtime_rollback,
            install_runtime,
            list_protons,
            proton_available,
            proton_install,
            proton_remove,
            virustotal_key,
            virustotal_check,
            virustotal_upload,
            app_update_check,
            app_update_install,
            app_restart,
            notifications_poll,
            notifications_set,
            create_desktop_entry,
            launch_game,
            stop_game
        ])
        .run(tauri::generate_context!())
        .expect("error while running VILauncher");
}

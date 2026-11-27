//! SixLauncher's window (Tauri): configure, check, prepare the prefix, play, read the log,
//! update the backends and see where each effort stands. It also keeps itself up to date
//! from this repository's releases (see `app_update_check`).
//!
//! Everything it does goes through `sixlauncher-core`, the same code as the
//! `sixlauncher` command line.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;
use sixlauncher_core::avatars;
use sixlauncher_core::config::{self, Config};
use sixlauncher_core::doctor::{self, Check};
use sixlauncher_core::fixes::{self, Fix};
use sixlauncher_core::gamedir::{self, GameInfo};
use sixlauncher_core::integrity;
use sixlauncher_core::launch;
use sixlauncher_core::{gpu, notify, perf};
use sixlauncher_core::paths;
use sixlauncher_core::report;
use sixlauncher_core::scarlett;
use sixlauncher_core::protons::{self, Release};
use sixlauncher_core::runners::{self, ProtonInstall, Runner};
use sixlauncher_core::selfupdate;
use sixlauncher_core::status::{self, Report};
use sixlauncher_core::update::{self, Runtimes};
use sixlauncher_core::virustotal::{self, Verdict};

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
    /// `linux`, `macos` or `windows`: which runners the window offers.
    platform: &'static str,
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
        platform: std::env::consts::OS,
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

/// Applies the kernel-setting fixes the checks offered (by id), as root: pkexec asks for
/// the password. The window shows what each one writes before calling this, then checks again.
#[tauri::command]
async fn apply_fixes(fixes: Vec<String>) -> Result<(), String> {
    let list: Vec<Fix> = fixes.iter().map(|id| Fix::parse(id).ok_or_else(|| format!("unknown fix {id}"))).collect::<Result<_, _>>()?;
    tauri::async_runtime::spawn_blocking(move || fixes::apply(&list)).await.map_err(err)?.map_err(err)
}

/// Where the report goes unless the player picks another place.
#[tauri::command]
fn report_default_path() -> String {
    report::default_path().display().to_string()
}

/// Writes the .zip for the backends' developers: checks, configuration, system, and the
/// logs of the launcher, Proton, vkd3d-proton and the backend.
#[tauri::command]
async fn export_report(app: AppHandle, config: Config, path: String) -> Result<report::Written, String> {
    let version = app.package_info().version.to_string();
    let dest = paths::expand(&PathBuf::from(path));
    tauri::async_runtime::spawn_blocking(move || report::write(&config, &version, &dest)).await.map_err(err)?.map_err(err)
}

#[tauri::command]
fn game_info(dir: String) -> Result<GameInfo, String> {
    gamedir::inspect(&paths::expand(&PathBuf::from(dir))).map_err(err)
}

/// The .exe files in the game folder, for the Executável step (config's own first).
#[tauri::command]
async fn game_executables(dir: String) -> Result<Vec<String>, String> {
    let dir = paths::expand(&PathBuf::from(dir));
    tauri::async_runtime::spawn_blocking(move || gamedir::find_executables(&dir)).await.map_err(err)?.map_err(err)
}

/// The game's executable against the known SHA-256 (integrity.rs); `None` when the
/// automatic check is off or there is no game folder yet.
#[tauri::command]
async fn verify_game(config: Config) -> Result<Option<integrity::Verdict>, String> {
    if !config.game.verify {
        return Ok(None);
    }
    let Some(dir) = config.game_dir() else { return Ok(None) };
    tauri::async_runtime::spawn_blocking(move || {
        let exe = gamedir::inspect(&dir).and_then(|info| gamedir::resolve_exe(&info, config.game.exe.as_deref()))?;
        integrity::verify(&exe).map(Some)
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// Any file (the game's .zip) against the known SHA-256 values.
#[tauri::command]
async fn verify_file(path: String) -> Result<integrity::Verdict, String> {
    let path = paths::expand(&PathBuf::from(path));
    tauri::async_runtime::spawn_blocking(move || integrity::verify(&path)).await.map_err(err)?.map_err(err)
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
                    Err(sixlauncher_core::Error::Config(why))
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
/// build, a new SixLauncher), filtered by the switches in `[notifications]`. The window
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
        BundleType::Nsis => Some("nsis"),
        _ => None,
    }
}

/// The updater's public key (tauri.conf.json > plugins.updater.pubkey, filled in by CI).
/// Without one nothing can be verified, so nothing is installed automatically.
fn updater_pubkey(app: &AppHandle) -> bool {
    pubkey(app).is_some()
}

fn pubkey(app: &AppHandle) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
}

/// Whether a newer SixLauncher is published (the newest release's latest.json), and whether
/// this copy can install it by itself: an AppImage, the .deb / .rpm installed under /usr, the
/// Windows installer's copy or the Flatpak, from a signed release. The tarball and the .zip are
/// told where to download it.
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

/// Downloads the newest SixLauncher, checks its signature and installs it over this one
/// (the AppImage file is replaced; a .deb or .rpm is installed with pkexec, which asks for
/// the password). Progress arrives as `app-update-progress` events. Returns the version
/// installed, or None when there was nothing newer. The new version runs on the next start
/// (`app_restart`).
#[tauri::command]
async fn app_update_install(app: AppHandle) -> Result<Option<String>, String> {
    let Some(key) = pubkey(&app) else {
        return Err("this copy of SixLauncher has no updater public key".into());
    };
    // Tauri's updater cannot replace a Flatpak: the core downloads the signed bundle, checks it
    // with the same key and installs it on the host with `flatpak install`.
    if selfupdate::detect_current(packaged()) == selfupdate::Install::Flatpak {
        let current = app.package_info().version.to_string();
        let emitter = app.clone();
        return tauri::async_runtime::spawn_blocking(move || {
            let mut last = 0u64;
            selfupdate::install_flatpak(&current, &key, &mut |done, total| {
                let step = total.map_or(1 << 20, |t| (t / 100).max(1));
                if done - last >= step || Some(done) == total {
                    last = done;
                    let _ = emitter.emit("app-update-progress", AppUpdateProgress { done, total });
                }
            })
        })
        .await
        .map_err(err)?
        .map_err(err);
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

/// Starts the installed (new) SixLauncher in place of this one. Refused while the game runs,
/// since the window owns its process and its log.
#[tauri::command]
fn app_restart(app: AppHandle, running: State<Running>) -> Result<(), String> {
    if running.child.lock().unwrap().is_some() {
        return Err("stop the game before restarting SixLauncher".into());
    }
    // A Flatpak restarted from inside keeps running the old copy's files: the new one is
    // started on the host and this one closes.
    if selfupdate::detect_current(packaged()) == selfupdate::Install::Flatpak {
        selfupdate::flatpak_relaunch().map_err(err)?;
        app.exit(0);
        return Ok(());
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
    let exec = sixlauncher_core::host::self_exec("sixlauncher-app");
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
    start(app, &running, l, runner, Arc::new(|_: &AppHandle, _: &str| {}), Box::new(|app: &AppHandle, code: Option<i32>| {
        let _ = app.emit("game-exit", code);
    }))
}

/// What `prep-exit` carries: the exit code, and whether `scarlett prepare` reached its end
/// and the cache is now marked prepared.
#[derive(Clone, Serialize)]
struct PrepExit {
    code: Option<i32>,
    prepared: bool,
}

/// ScarlettPort: `scarlett prepare` for the chosen preset, in the game's place (Parar stops
/// it). Its lines arrive as `game-log`, its progress as `prep-line` (scarlett::PrepLine), and
/// `prep-exit` says how it ended.
#[tauri::command]
fn prepare_shaders(app: AppHandle, running: State<Running>, config: Config) -> Result<(), String> {
    if running.child.lock().unwrap().is_some() {
        return Err("the game is already running".into());
    }
    let l = launch::prepare_shaders(&config).map_err(err)?;
    let runner = Runner::from_config(&config).map_err(err)?;
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = finished.clone();
    start(
        app,
        &running,
        l,
        runner,
        Arc::new(move |app: &AppHandle, line: &str| {
            if let Some(event) = scarlett::parse_prep(line) {
                if matches!(event, scarlett::PrepLine::Done { .. }) {
                    seen.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                let _ = app.emit("prep-line", event);
            }
        }),
        Box::new(move |app: &AppHandle, code: Option<i32>| {
            let prepared = code == Some(0) && finished.load(std::sync::atomic::Ordering::SeqCst) && scarlett::mark_prepared(&config).is_ok();
            let _ = app.emit("prep-exit", PrepExit { code, prepared });
        }),
    )
}

type OnLine = Arc<dyn Fn(&AppHandle, &str) + Send + Sync>;
type OnExit = Box<dyn FnOnce(&AppHandle, Option<i32>) + Send>;

/// Runs `l` as the window's one process: its output goes to the log file and to `game-log`
/// (and to `on_line`); `on_exit` gets the exit code once every line was read.
fn start(app: AppHandle, running: &Running, l: launch::Launch, runner: Runner, on_line: OnLine, on_exit: OnExit) -> Result<(), String> {
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
        let (app, log, on_line) = (app.clone(), log.clone(), on_line.clone());
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                writeln!(log.lock().unwrap(), "{line}").ok();
                on_line(&app, &line);
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
        on_exit(&app, code);
    });
    Ok(())
}

#[tauri::command]
fn stop_game(running: State<Running>) -> Result<(), String> {
    let pid = running.child.lock().unwrap().as_ref().map(Child::id);
    // On Windows the process tree hangs from the backend, so it goes first (taskkill /T).
    if let Some(runner) = running.runner.lock().unwrap().as_ref()
        && let Some(mut kill) = runner.kill_command(pid)
    {
        let _ = kill.status();
    }
    if let Some(child) = running.child.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
    Ok(())
}

fn main() {
    sixlauncher_core::paths::migrate_legacy_dirs();
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
            apply_fixes,
            report_default_path,
            export_report,
            game_info,
            game_executables,
            verify_game,
            verify_file,
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
            prepare_shaders,
            stop_game
        ])
        .run(tauri::generate_context!())
        .expect("error while running SixLauncher");
}

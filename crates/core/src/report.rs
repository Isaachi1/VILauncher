//! The report for the backends' developers: one .zip with what they ask for when a run
//! goes wrong.
//!
//! * `README.txt`, `system.txt`: the launcher, the distribution, the kernel, the CPU,
//!   the memory, the kernel settings the checks look at and `vulkaninfo --summary`;
//! * `checks.txt` / `checks.json`: every check ([`doctor`]);
//! * `config.toml` and `launch.txt`: the configuration and the exact command of a run;
//! * `logs/last-run.log`: the last run's output, which holds the backend's own output and
//!   vkd3d-proton's (it writes to stderr unless `VKD3D_LOG_FILE` says otherwise);
//! * `logs/proton/`: Proton's `steam-*.log`, written when `PROTON_LOG=1` (the window's
//!   "detailed logs"); SixLauncher points `PROTON_LOG_DIR` at [`proton_log_dir`];
//! * `logs/vkd3d/`: the file `VKD3D_LOG_FILE` names, when it is set;
//! * `logs/backend/`: `*.log` files the backend left in its data and cache folders;
//! * `crash/`: the newest Windows crash dumps (`*.dmp`) in the prefix.
//!
//! The home folder's path is replaced by `~` in every text file. Large logs keep their
//! beginning and their end.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::Config;
use crate::doctor::{self, Check, Status};
use crate::runners::Runner;
use crate::{host, launch, paths};
use crate::{Error, Result};

/// A text log larger than this keeps its first [`HEAD`] bytes and its last bytes.
pub const LOG_CAP: u64 = 24 << 20;
const HEAD: u64 = 2 << 20;
const DUMP_CAP: u64 = 64 << 20;
const MAX_PROTON_LOGS: usize = 3;
const MAX_BACKEND_LOGS: usize = 12;
const MAX_DUMPS: usize = 3;

/// What was written.
#[derive(Debug, Clone, Serialize)]
pub struct Written {
    pub path: PathBuf,
    pub files: Vec<String>,
    pub bytes: u64,
}

/// Where Proton writes `steam-*.log` for runs SixLauncher starts (`PROTON_LOG_DIR`).
pub fn proton_log_dir() -> PathBuf {
    paths::state_dir().join("proton-logs")
}

/// `sixlauncher-report-YYYYMMDD-HHMMSS.zip` (UTC).
pub fn file_name(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("sixlauncher-report-{year:04}{month:02}{day:02}-{:02}{:02}{:02}.zip", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// The report's default place: the home folder.
pub fn default_path() -> PathBuf {
    paths::home().join(file_name(SystemTime::now()))
}

/// `text` with the home folder's path (also as Wine shows it) replaced by `~`.
pub fn redact(text: &str, home: &Path) -> String {
    let h = home.to_string_lossy();
    if h.len() < 2 || h == "/" {
        return text.to_owned();
    }
    let win = paths::to_windows(home);
    text.replace(win.as_str(), r"Z:\~").replace(h.as_ref(), "~")
}

/// The file, or its first [`HEAD`] bytes and its end when it is larger than `cap`.
fn read_capped(path: &Path, cap: u64) -> std::io::Result<Vec<u8>> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len <= cap {
        let mut all = Vec::with_capacity(len as usize);
        f.read_to_end(&mut all)?;
        return Ok(all);
    }
    let head = HEAD.min(cap / 2);
    let mut out = vec![0; head as usize];
    f.read_exact(&mut out)?;
    out.extend_from_slice(format!("\n\n[... SixLauncher left out {} bytes of the middle ...]\n\n", len - cap).as_bytes());
    f.seek(SeekFrom::Start(len - (cap - head)))?;
    f.read_to_end(&mut out)?;
    Ok(out)
}

/// Files under `dir` (down to `depth` folders) that `want` accepts, newest first.
fn newest(dir: &Path, depth: usize, want: &dyn Fn(&Path) -> bool, max: usize) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, want: &dyn Fn(&Path) -> bool, out: &mut Vec<(SystemTime, PathBuf)>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for p in rd.flatten().map(|e| e.path()) {
            if p.is_symlink() {
                continue;
            }
            if p.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, want, out);
                }
            } else if want(&p) {
                let t = p.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
                out.push((t, p));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, depth, want, &mut found);
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().take(max).map(|(_, p)| p).collect()
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// A Windows path (`Z:\x\y`) as the Linux path it maps to; other paths unchanged.
fn from_windows(p: &str) -> PathBuf {
    match p.strip_prefix("Z:").or_else(|| p.strip_prefix("z:")) {
        Some(rest) => PathBuf::from(rest.replace('\\', "/")),
        None => PathBuf::from(p),
    }
}

fn run_text(argv: &[&str]) -> String {
    let argv: Vec<std::ffi::OsString> = argv.iter().map(Into::into).collect();
    match host::simple(&argv).stdin(std::process::Stdio::null()).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            if !o.status.success() {
                s += &format!("\n({} exited with {}: {})\n", argv[0].to_string_lossy(), o.status, String::from_utf8_lossy(&o.stderr).trim());
            }
            s
        }
        Err(e) => format!("({} could not run: {e})\n", argv[0].to_string_lossy()),
    }
}

fn read_or(path: &str) -> String {
    fs::read_to_string(path).map(|s| s.trim().to_owned()).unwrap_or_else(|e| format!("(cannot read {path}: {e})"))
}

fn system_text(version: &str) -> String {
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|c| c.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split_once(':')).map(|(_, m)| m.trim().to_owned()))
        .unwrap_or_else(|| "?".into());
    let mem: String = fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .filter(|l| ["MemTotal:", "MemAvailable:", "SwapTotal:"].iter().any(|k| l.starts_with(k)))
        .map(|l| format!("{l}\n"))
        .collect();
    // In a Flatpak, /etc/os-release is the runtime's; the host's is the one that matters.
    let os = if host::in_flatpak() { run_text(&["cat", "/etc/os-release"]) } else { read_or("/etc/os-release") };
    format!(
        "SixLauncher {version} ({})\n\n## kernel\n{}\n\n## distribution\n{}\n\n## CPU\n{cpu}\n\n## memory\n{mem}\n\
         ## kernel settings\nvm.max_map_count = {}\nvm.unprivileged_userfaultfd = {}\n\n## vulkaninfo --summary\n{}",
        if host::in_flatpak() { "Flatpak" } else { "native" },
        read_or("/proc/version"),
        os.trim(),
        read_or("/proc/sys/vm/max_map_count"),
        read_or("/proc/sys/vm/unprivileged_userfaultfd"),
        run_text(&["vulkaninfo", "--summary"]),
    )
}

fn checks_text(checks: &[Check]) -> String {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    checks
        .iter()
        .map(|c| {
            let s = match c.status {
                Status::Ok => "  OK",
                Status::Warn => "WARN",
                Status::Fail => "FAIL",
            };
            format!("[{s}] {:<width$}  {}\n", c.name, c.detail)
        })
        .collect()
}

/// A file of the report: its name in the .zip, its contents, and whether it is text.
struct Entry {
    name: String,
    data: Vec<u8>,
    text: bool,
}

/// Everything the report holds, before the home folder is taken out.
fn collect(cfg: &Config, version: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut text = |name: &str, s: String| out.push(Entry { name: name.into(), data: s.into_bytes(), text: true });

    let checks = doctor::run_all(cfg);
    let runner = Runner::from_config(cfg).ok();
    let summary = format!(
        "SixLauncher {version} report, {}\n\nrunner: {}\nbackend: {} ({})\nchecks: {} failed, {} warnings\n\n\
         The home folder's path is shown as ~. Files:\n  system.txt   the machine, the kernel settings, vulkaninfo\n  \
         checks.txt   SixLauncher's checks (checks.json: the same, as data)\n  config.toml  the configuration\n  \
         launch.txt   the exact command and environment of a run\n  logs/        the last run, Proton, vkd3d-proton, the backend\n  \
         crash/       Windows crash dumps from the prefix, if any\n",
        file_name(SystemTime::now()).trim_start_matches("sixlauncher-report-").trim_end_matches(".zip"),
        runner.as_ref().map_or("(not configured)".into(), |r| format!("{} {}", r.name(), r.version())),
        cfg.backend.kind.name(),
        cfg.backend_exe().map_or("no executable".into(), |p| p.display().to_string()),
        checks.iter().filter(|c| c.status == Status::Fail).count(),
        checks.iter().filter(|c| c.status == Status::Warn).count(),
    );
    text("README.txt", summary);
    text("system.txt", system_text(version));
    text("checks.txt", checks_text(&checks));
    text("checks.json", serde_json::to_string_pretty(&checks).unwrap_or_default());
    text("config.toml", cfg.to_toml().unwrap_or_else(|e| format!("# {e}\n")));
    text("launch.txt", launch::build(cfg).map(|l| l.shell_text()).unwrap_or_else(|e| format!("# cannot build the launch: {e}\n")));

    let mut file = |name: String, path: &Path, cap: u64, is_text: bool| {
        if let Ok(data) = read_capped(path, cap) {
            out.push(Entry { name, data, text: is_text });
        }
    };
    let file_name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();

    file("logs/last-run.log".into(), &launch::log_file(), LOG_CAP, true);

    // Proton: our folder, the one the configuration names, and Proton's default ($HOME).
    let steam_log = |p: &Path| {
        let n = p.file_name().unwrap_or_default().to_string_lossy();
        n.starts_with("steam-") && n.ends_with(".log")
    };
    let mut proton_dirs = vec![proton_log_dir(), paths::home()];
    if let Some(d) = cfg.env.get("PROTON_LOG_DIR").filter(|d| !d.is_empty()) {
        proton_dirs.insert(0, paths::expand(Path::new(d)));
    }
    proton_dirs.dedup();
    let mut proton_logs: Vec<PathBuf> = proton_dirs.iter().flat_map(|d| newest(d, 0, &steam_log, MAX_PROTON_LOGS)).collect();
    proton_logs.sort_by_key(|p| std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH)));
    proton_logs.dedup();
    for p in proton_logs.into_iter().take(MAX_PROTON_LOGS) {
        file(format!("logs/proton/{}", file_name(&p)), &p, LOG_CAP, true);
    }

    let vkd3d = cfg.env.get("VKD3D_LOG_FILE").cloned().or_else(|| std::env::var("VKD3D_LOG_FILE").ok()).filter(|v| !v.is_empty());
    if let Some(v) = vkd3d {
        let p = paths::expand(&from_windows(&v));
        file(format!("logs/vkd3d/{}", file_name(&p)), &p, LOG_CAP, true);
    }

    for (label, dir) in [("data", cfg.data_dir()), ("cache", cfg.cache_dir())] {
        for p in newest(&dir, 3, &|p| has_ext(p, "log"), MAX_BACKEND_LOGS) {
            let rel = p.strip_prefix(&dir).unwrap_or(&p).to_string_lossy().into_owned();
            file(format!("logs/backend/{label}/{rel}"), &p, LOG_CAP, true);
        }
    }

    if let Some(r) = &runner {
        let users = r.wineprefix().join("drive_c/users");
        let dumps: Vec<PathBuf> = fs::read_dir(&users)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|u| newest(&u.path().join("AppData/Local/CrashDumps"), 0, &|p| has_ext(p, "dmp"), MAX_DUMPS))
            .collect();
        for p in dumps.into_iter().take(MAX_DUMPS) {
            file(format!("crash/{}", file_name(&p)), &p, DUMP_CAP, false);
        }
    }
    out
}

/// Writes the report to `dest` (a .zip). `version` is the launcher's.
pub fn write(cfg: &Config, version: &str, dest: &Path) -> Result<Written> {
    let home = paths::home();
    let entries = collect(cfg, version);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("zip.part");
    let zip_err = |e: zip::result::ZipError| Error::Io(std::io::Error::other(e));
    let mut zip = zip::ZipWriter::new(fs::File::create(&tmp)?);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o644);
    let mut files = Vec::new();
    for e in entries {
        let data = if e.text { redact(&String::from_utf8_lossy(&e.data), &home).into_bytes() } else { e.data };
        zip.start_file(e.name.as_str(), opts).map_err(zip_err)?;
        zip.write_all(&data)?;
        files.push(e.name);
    }
    zip.finish().map_err(zip_err)?;
    fs::rename(&tmp, dest)?;
    let bytes = fs::metadata(dest)?.len();
    Ok(Written { path: dest.to_path_buf(), files, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, TempHome};
    use std::time::Duration;

    #[test]
    fn names_carry_the_utc_time() {
        let t = UNIX_EPOCH + Duration::from_secs(1_795_572_070); // 2026-11-25 02:01:10 UTC
        assert_eq!(file_name(t), "sixlauncher-report-20261125-020110.zip");
        assert_eq!(file_name(UNIX_EPOCH), "sixlauncher-report-19700101-000000.zip");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(file_name(leap), "sixlauncher-report-20000229-000000.zip");
    }

    #[test]
    fn the_home_folder_is_taken_out() {
        let home = Path::new("/home/isaac");
        assert_eq!(
            redact(r"/home/isaac/game/x.exe and Z:\home\isaac\game\x.exe", home),
            r"~/game/x.exe and Z:\~\game\x.exe"
        );
        assert_eq!(redact("/home/isaac", Path::new("/")), "/home/isaac");
    }

    #[test]
    fn large_logs_keep_both_ends() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("big.log");
        let body: Vec<u8> = (0..100u32).flat_map(|i| format!("{i:04}\n").into_bytes()).collect();
        fs::write(&p, &body).unwrap();
        assert_eq!(read_capped(&p, 1000).unwrap(), body);
        let cut = String::from_utf8(read_capped(&p, 100).unwrap()).unwrap();
        assert!(cut.starts_with("0000\n"), "{cut}");
        assert!(cut.ends_with("0099\n"), "{cut}");
        assert!(cut.contains("left out 400 bytes"), "{cut}");
    }

    #[test]
    fn windows_paths_map_back() {
        assert_eq!(from_windows(r"Z:\home\me\vkd3d.log"), PathBuf::from("/home/me/vkd3d.log"));
        assert_eq!(from_windows("/tmp/v.log"), PathBuf::from("/tmp/v.log"));
    }

    #[test]
    fn the_zip_holds_the_logs() {
        let h = TempHome::new();
        let mut cfg = Config::default();
        cfg.game.dir = Some(testutil::fake_game(h.path()));
        cfg.runner.path = Some(testutil::fake_proton(h.path(), "GE-Proton10-1"));
        let vkd3d = h.path().join("vkd3d.log");
        fs::write(&vkd3d, "vkd3d: fixme\n").unwrap();
        cfg.env.insert("VKD3D_LOG_FILE".into(), paths::to_windows(&vkd3d));

        let last = launch::log_file();
        fs::create_dir_all(last.parent().unwrap()).unwrap();
        fs::write(&last, format!("run from {}\n", h.home().display())).unwrap();
        fs::create_dir_all(proton_log_dir()).unwrap();
        fs::write(proton_log_dir().join("steam-0.log"), "proton log\n").unwrap();
        fs::write(h.home().join("unrelated.log"), "no\n").unwrap();
        let backend_log = cfg.data_dir().join("logs/victory.log");
        fs::create_dir_all(backend_log.parent().unwrap()).unwrap();
        fs::write(&backend_log, "backend\n").unwrap();
        let dumps = Runner::from_config(&cfg).unwrap().wineprefix().join("drive_c/users/steamuser/AppData/Local/CrashDumps");
        fs::create_dir_all(&dumps).unwrap();
        fs::write(dumps.join("title.exe.1234.dmp"), b"MDMP\0\x01").unwrap();

        let dest = h.path().join("out/report.zip");
        let w = write(&cfg, "0.1.0", &dest).unwrap();
        assert!(w.bytes > 0 && dest.is_file());
        for name in [
            "README.txt",
            "system.txt",
            "checks.txt",
            "checks.json",
            "config.toml",
            "launch.txt",
            "logs/last-run.log",
            "logs/proton/steam-0.log",
            "logs/vkd3d/vkd3d.log",
            "logs/backend/data/logs/victory.log",
            "crash/title.exe.1234.dmp",
        ] {
            assert!(w.files.iter().any(|f| f == name), "{name} missing from {:?}", w.files);
        }
        assert!(!w.files.iter().any(|f| f.contains("unrelated")));

        let mut zip = zip::ZipArchive::new(fs::File::open(&dest).unwrap()).unwrap();
        let mut log = String::new();
        zip.by_name("logs/last-run.log").unwrap().read_to_string(&mut log).unwrap();
        assert_eq!(log, "run from ~\n");
        let mut dump = Vec::new();
        zip.by_name("crash/title.exe.1234.dmp").unwrap().read_to_end(&mut dump).unwrap();
        assert_eq!(dump, b"MDMP\0\x01");
    }
}

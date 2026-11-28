//! A memory ceiling for the game, so that a game that runs out of memory takes only itself down.
//!
//! On Linux a program started from the launcher shares the launcher's control group, which is
//! often the desktop shell's own (a launcher started from the shell's menu). A game that kept
//! growing filled RAM and swap, the whole machine stalled swapping, and `systemd-oomd` then
//! killed that group: the game, the launcher and the desktop shell with it, which looked like
//! the system crashing.
//!
//! So the game runs in a systemd user scope of its own (`systemd-run --user --scope`), with
//! `MemoryMax` (70% of RAM) and `MemorySwapMax` (a quarter of RAM). Past those the kernel ends
//! the game alone, while the desktop still has memory left. 70% and not more: the GPU driver's
//! memory (3-4 GB with NVIDIA) is not counted in the scope, and with a game at three quarters
//! RAM and swap still filled up. Below the ceilings the launcher also watches how hard memory
//! presses ([`Strain`]) and ends the game when it only swaps. Where there is no systemd user
//! manager (or on Windows) the game runs as before.

use std::ffi::OsString;
use std::sync::atomic::{AtomicU32, Ordering};

const GIB: u64 = 1 << 30;

/// The scope a run is put in and its ceilings, in bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Guard {
    pub unit: String,
    pub memory_max: u64,
    pub swap_max: u64,
}

/// The ceilings for a machine with `ram` bytes of memory: (memory, swap).
pub fn limits(ram: u64) -> (u64, u64) {
    let memory = (ram / 10 * 7).max(2 * GIB).min(ram);
    (memory, ram / 4)
}

/// The machine's memory (MemTotal in `/proc/meminfo`), in bytes.
fn ram() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_mem_total(&info)
}

fn parse_mem_total(info: &str) -> Option<u64> {
    let line = info.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Whether `systemd-run --user --scope` works here (checked once: it needs a user manager).
fn available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        if std::env::var_os("SIXLAUNCHER_NO_MEMORY_GUARD").is_some_and(|v| v == "1") {
            return false;
        }
        let probe: Vec<OsString> =
            ["systemd-run", "--user", "--scope", "--quiet", "--collect", "true"].iter().map(Into::into).collect();
        crate::host::simple(&probe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// The guard for the next run, or `None` where there is none to be had (not Linux, no user
/// manager, `SIXLAUNCHER_NO_MEMORY_GUARD=1`; and in tests, which start no game).
pub fn plan() -> Option<Guard> {
    if cfg!(test) || !cfg!(target_os = "linux") || !available() {
        return None;
    }
    static RUN: AtomicU32 = AtomicU32::new(0);
    let (memory_max, swap_max) = limits(ram()?);
    let unit = format!("sixlauncher-game-{}-{}", std::process::id(), RUN.fetch_add(1, Ordering::Relaxed));
    Some(Guard { unit, memory_max, swap_max })
}

impl Guard {
    /// `argv` run inside the guard's scope. Not collected when it fails, so that
    /// [`Guard::outcome`] can still ask how it ended.
    pub fn wrap(&self, argv: &[OsString]) -> Vec<OsString> {
        let mut out: Vec<OsString> = vec![
            "systemd-run".into(),
            "--user".into(),
            "--scope".into(),
            "--quiet".into(),
            format!("--unit={}", self.unit).into(),
            "--description=SixLauncher game".into(),
            format!("--property=MemoryMax={}", self.memory_max).into(),
            format!("--property=MemorySwapMax={}", self.swap_max).into(),
            "--".into(),
        ];
        out.extend(argv.iter().cloned());
        out
    }

    /// The log's line describing the guard.
    pub fn describe(&self) -> String {
        format!(
            "# memory guard: systemd scope {}.scope, at most {} of memory and {} of swap",
            self.unit,
            gib(self.memory_max),
            gib(self.swap_max)
        )
    }

    /// After the run: whether the scope ended for want of memory (`Result=oom-kill`), and the
    /// scope cleared away. Asks systemd, so call it once the process is gone.
    pub fn outcome(&self) -> bool {
        let unit = format!("{}.scope", self.unit);
        let ask = |args: &[&str]| -> Option<String> {
            let argv: Vec<OsString> = args.iter().map(Into::into).collect();
            let out = crate::host::simple(&argv).stdin(std::process::Stdio::null()).output().ok()?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        let result = ask(&["systemctl", "--user", "show", "--property=Result", "--value", &unit]);
        let _ = ask(&["systemctl", "--user", "reset-failed", &unit]);
        result.as_deref() == Some("oom-kill")
    }
}

/// What the scope's control group holds, in bytes: memory, swap, and the peaks of each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub memory: u64,
    pub swap: u64,
    pub memory_peak: u64,
    pub swap_peak: u64,
}

impl Usage {
    /// The log's line for it.
    pub fn line(&self, max: &Guard) -> String {
        format!(
            "# game memory: {} of {} in memory, {} of {} in swap (peaks {} and {})",
            gib(self.memory),
            gib(max.memory_max),
            gib(self.swap),
            gib(max.swap_max),
            gib(self.memory_peak),
            gib(self.swap_peak)
        )
    }
}

/// The cgroup v2 folder of a control group as systemd names it (`/user.slice/...`).
fn cgroup_dir(group: &str) -> std::path::PathBuf {
    std::path::Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'))
}

/// One number from a cgroup file, 0 when it cannot be read.
fn read_number(dir: &std::path::Path, file: &str) -> u64 {
    std::fs::read_to_string(dir.join(file)).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0)
}

impl Guard {
    /// The scope's control group folder, once systemd made it (it is made as the run starts).
    pub fn cgroup(&self) -> Option<std::path::PathBuf> {
        let unit = format!("{}.scope", self.unit);
        let argv: Vec<OsString> =
            ["systemctl", "--user", "show", "--property=ControlGroup", "--value", &unit].iter().map(Into::into).collect();
        let out = crate::host::simple(&argv).stdin(std::process::Stdio::null()).output().ok()?;
        let group = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (!group.is_empty()).then(|| cgroup_dir(&group)).filter(|d| d.is_dir())
    }
}

/// What the control group in `dir` holds now.
pub fn usage(dir: &std::path::Path) -> Usage {
    Usage {
        memory: read_number(dir, "memory.current"),
        swap: read_number(dir, "memory.swap.current"),
        memory_peak: read_number(dir, "memory.peak"),
        swap_peak: read_number(dir, "memory.swap.peak"),
    }
}

/// Whether a reading is worth a log line after `last`: memory or swap moved by half a gigabyte.
pub fn worth_logging(last: Option<Usage>, now: Usage) -> bool {
    let moved = |a: u64, b: u64| a.abs_diff(b) >= GIB / 2;
    last.is_none_or(|l| moved(l.memory, now.memory) || moved(l.swap, now.swap))
}

/// How hard memory is pressing, beyond the ceilings: the game's stall (the share of the last
/// 10 s in which all its tasks waited for memory, `full avg10` in `memory.pressure`), and the
/// whole machine's memory and swap left (`/proc/meminfo`).
///
/// The ceilings alone did not do: a game below its `MemoryMax`, with the GPU driver's memory
/// (counted in no scope) and the desktop's apps beside it, left the machine without memory, and
/// the kernel then swapped the game in and out without end (zram swap). Nothing went over a
/// ceiling, so nothing was ended, and the whole machine stood still until the player ended it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Strain {
    pub stall: f64,
    pub available: u64,
    pub ram: u64,
    pub swap_free: u64,
    pub swap_total: u64,
}

/// The game's stall at which it is ended (twice in a row, [`Watch`]): a third of the time
/// waiting for memory is swapping without end, not a game that runs.
const STALL_LIMIT: f64 = 30.0;

/// The strain now, for the control group in `dir`.
pub fn strain(dir: &std::path::Path) -> Strain {
    let pressure = std::fs::read_to_string(dir.join("memory.pressure")).unwrap_or_default();
    let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    parse_strain(&pressure, &info)
}

fn parse_strain(pressure: &str, info: &str) -> Strain {
    let stall = pressure
        .lines()
        .find(|l| l.starts_with("full "))
        .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let kib = |key: &str| -> u64 {
        info.lines()
            .find(|l| l.starts_with(key) && l[key.len()..].starts_with(':'))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(0, |k| k * 1024)
    };
    Strain {
        stall,
        available: kib("MemAvailable"),
        ram: kib("MemTotal"),
        swap_free: kib("SwapFree"),
        swap_total: kib("SwapTotal"),
    }
}

impl Strain {
    /// Why the game should be ended now, if it should: it stalls on memory, or the machine has
    /// almost no memory and almost no swap left (where systemd-oomd ends the desktop instead).
    pub fn too_much(&self) -> Option<String> {
        if self.stall >= STALL_LIMIT {
            return Some(format!("it spent {:.0}% of the last 10 seconds waiting for memory", self.stall));
        }
        let low_memory = self.ram > 0 && self.available < self.ram / 25;
        let low_swap = self.swap_free < self.swap_total / 10;
        (low_memory && low_swap).then(|| {
            format!(
                "the computer had only {} of memory and {} of swap left",
                gib(self.available),
                gib(self.swap_free)
            )
        })
    }
}

/// Readings in a row past the limit before the game is ended: one bad reading (a level
/// loading) is let pass.
#[derive(Debug, Default)]
pub struct Watch {
    strikes: u32,
}

impl Watch {
    /// The reason to end the game, once `strain` was too much twice in a row.
    pub fn check(&mut self, strain: &Strain) -> Option<String> {
        match strain.too_much() {
            Some(why) => {
                self.strikes += 1;
                (self.strikes >= 2).then_some(why)
            }
            None => {
                self.strikes = 0;
                None
            }
        }
    }
}

impl Guard {
    /// Ends every process of the scope (Proton's whole tree), at once.
    pub fn kill(&self) {
        let unit = format!("{}.scope", self.unit);
        let argv: Vec<OsString> =
            ["systemctl", "--user", "kill", "--signal=SIGKILL", &unit].iter().map(Into::into).collect();
        let _ = crate::host::simple(&argv).stdin(std::process::Stdio::null()).status();
    }
}

/// Bytes as gigabytes with one decimal, for the log.
pub fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_gets_seventy_percent_of_ram_and_a_quarter_in_swap() {
        assert_eq!(limits(16 * GIB), (16 * GIB / 10 * 7, 4 * GIB));
        assert_eq!(limits(32 * GIB), (32 * GIB / 10 * 7, 8 * GIB));
        // A small machine still lets the game have 2 GB, never more than there is.
        assert_eq!(limits(2 * GIB).0, 2 * GIB);
        assert_eq!(limits(GIB).0, GIB);
    }

    #[test]
    fn a_reading_is_logged_when_it_moves_half_a_gigabyte() {
        let at = |memory, swap| Usage { memory, swap, ..Usage::default() };
        assert!(worth_logging(None, at(GIB, 0)), "the first one");
        assert!(!worth_logging(Some(at(GIB, 0)), at(GIB + GIB / 4, 0)));
        assert!(worth_logging(Some(at(GIB, 0)), at(2 * GIB, 0)));
        assert!(worth_logging(Some(at(2 * GIB, 0)), at(GIB, 0)), "falling too");
        assert!(worth_logging(Some(at(GIB, 0)), at(GIB, GIB)), "swap");
    }

    #[test]
    fn usage_is_read_from_the_cgroup_files() {
        let dir = std::env::temp_dir().join(format!("sixlauncher-memguard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (file, v) in [("memory.current", "1073741824\n"), ("memory.swap.current", "0\n"), ("memory.peak", "2147483648\n")] {
            std::fs::write(dir.join(file), v).unwrap();
        }
        let u = usage(&dir);
        assert_eq!((u.memory, u.swap, u.memory_peak, u.swap_peak), (GIB, 0, 2 * GIB, 0), "a missing file reads 0");
        let g = Guard { unit: "u".into(), memory_max: 12 * GIB, swap_max: 4 * GIB };
        assert_eq!(u.line(&g), "# game memory: 1.0 GB of 12.0 GB in memory, 0.0 GB of 4.0 GB in swap (peaks 2.0 GB and 0.0 GB)");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(cgroup_dir("/user.slice/x.scope"), std::path::Path::new("/sys/fs/cgroup/user.slice/x.scope"));
    }

    #[test]
    fn the_game_is_ended_when_it_stalls_on_memory_twice_in_a_row() {
        let info = "MemTotal: 16000000 kB\nMemAvailable: 8000000 kB\nSwapTotal: 16000000 kB\nSwapFree: 12000000 kB\n";
        let calm = parse_strain("some avg10=5.00 avg60=1.00 avg300=0.00 total=1\nfull avg10=2.50 avg60=0 avg300=0 total=1\n", info);
        assert_eq!(calm.stall, 2.5);
        assert_eq!((calm.ram, calm.available), (16000000 * 1024, 8000000 * 1024));
        assert!(calm.too_much().is_none());
        let stuck = parse_strain("some avg10=90.00 avg60=0 avg300=0 total=1\nfull avg10=55.10 avg60=0 avg300=0 total=1\n", info);
        assert!(stuck.too_much().unwrap().contains("55%"));
        let mut w = Watch::default();
        assert!(w.check(&stuck).is_none(), "one reading is let pass");
        assert!(w.check(&calm).is_none());
        assert!(w.check(&stuck).is_none(), "the count starts over");
        assert!(w.check(&stuck).is_some());
    }

    #[test]
    fn the_game_is_ended_when_the_machine_runs_out_of_memory_and_swap() {
        let at = |available, swap_free| Strain { stall: 0.0, available, ram: 16 * GIB, swap_free, swap_total: 16 * GIB };
        assert!(at(GIB / 2, GIB).too_much().unwrap().contains("left"));
        assert!(at(GIB / 2, 4 * GIB).too_much().is_none(), "swap still there");
        assert!(at(2 * GIB, GIB).too_much().is_none(), "memory still there");
        assert!(parse_strain("", "").too_much().is_none(), "nothing read, nothing done");
    }

    #[test]
    fn mem_total_is_read_in_kib() {
        let info = "MemFree:  1000 kB\nMemTotal:       16452588 kB\nSwapTotal: 0 kB\n";
        assert_eq!(parse_mem_total(info), Some(16452588 * 1024));
        assert_eq!(parse_mem_total("nothing"), None);
    }

    #[test]
    fn the_command_runs_in_its_own_scope_with_the_ceilings() {
        let g = Guard { unit: "sixlauncher-game-7-0".into(), memory_max: 12 * GIB, swap_max: 4 * GIB };
        let argv: Vec<OsString> = vec!["/p/proton".into(), "runinprefix".into(), "game.exe".into()];
        let wrapped = g.wrap(&argv);
        let text: Vec<String> = wrapped.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(&text[..3], ["systemd-run", "--user", "--scope"]);
        assert!(text.contains(&"--unit=sixlauncher-game-7-0".to_owned()));
        assert!(text.contains(&format!("--property=MemoryMax={}", 12 * GIB)));
        assert!(text.contains(&format!("--property=MemorySwapMax={}", 4 * GIB)));
        assert!(!text.iter().any(|a| a == "--collect"), "kept after a failure, to ask why it ended");
        let sep = text.iter().position(|a| a == "--").unwrap();
        assert_eq!(&text[sep + 1..], ["/p/proton", "runinprefix", "game.exe"]);
        assert!(g.describe().contains("12.0 GB") && g.describe().contains("4.0 GB"));
    }
}

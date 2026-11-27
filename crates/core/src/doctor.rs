//! Checks of the machine and the configuration before a run.
//!
//! Each check gives OK, WARN (it may run, but something the backends need is
//! missing or unverified) or FAIL (a run cannot work). A launch refuses to start
//! while a configuration check fails, unless forced.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::backends::{self, MACHINE_X86_64};
use crate::config::{BackendKind, Config, RunnerKind};
use crate::fixes::{self, Fix};
use crate::runners::{self, Runner};
use crate::{Result, gamedir, gpu, integrity, paths, perf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub status: Status,
    pub name: String,
    pub detail: String,
    /// A fix the window can apply (as root, after asking), for a warning about a kernel setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<fixes::Plan>,
    /// A stable id of the message, with its values in `args`, for the window to show it in the
    /// user's language; `detail` stays the English text (the CLI, the report, the fallback).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
}

fn check(status: Status, name: &str, detail: impl Into<String>) -> Check {
    Check { status, name: name.into(), detail: detail.into(), fix: None, code: None, args: BTreeMap::new() }
}

impl Check {
    fn coded(mut self, code: &str, args: &[(&str, String)]) -> Check {
        self.code = Some(code.into());
        self.args = args.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
        self
    }

    fn fixable(mut self, fix: Fix) -> Check {
        self.fix = Some(fix.plan());
        self
    }
}

const GIB: u64 = 1 << 30;
/// SteamOS' value; large titles map far more regions than the kernel default (65530).
pub const MAX_MAP_COUNT: u64 = 1_048_576;

// -- the machine -------------------------------------------------------------------------------

pub fn check_platform() -> Check {
    platform_check(std::env::consts::OS, std::env::consts::ARCH)
}

/// Linux x86-64 runs the title's code directly; macOS on Apple Silicon runs it through Rosetta 2
/// under a Wine with D3DMetal (VIctory for macOS); Windows x86-64 runs the backend natively.
pub fn platform_check(os: &str, arch: &str) -> Check {
    if os == "linux" && arch == "x86_64" {
        check(Status::Ok, "platform", "Linux x86_64").coded("platform.linux", &[])
    } else if os == "windows" && arch == "x86_64" {
        check(Status::Ok, "platform", "Windows x86_64: VIctory and XCL run natively, with the system's D3D12").coded("platform.windows", &[])
    } else if os == "macos" && arch == "aarch64" {
        check(Status::Ok, "platform", "macOS on Apple Silicon: Rosetta 2 runs the title's x86-64 code under Wine with D3DMetal").coded("platform.macArm", &[])
    } else if os == "macos" {
        check(Status::Fail, "platform", "macOS on Intel: VIctory for macOS needs Apple Silicon (M1 or newer)").coded("platform.intelMac", &[])
    } else {
        check(
            Status::Fail,
            "platform",
            format!(
                "{os} {arch}: the title's x86-64 code runs directly on the CPU, so an x86-64 Linux or Windows (or an Apple Silicon Mac) is required"
            ),
        ).coded("platform.unsupported", &[("os", os.to_string()), ("arch", arch.to_string())])
    }
}

pub fn check_avx2(cpuinfo: Option<&str>) -> Check {
    let Some(cpuinfo) = cpuinfo else { return check(Status::Warn, "CPU", "cannot read /proc/cpuinfo").coded("cpu.unreadable", &[]) };
    let flags = cpuinfo.lines().find(|l| l.starts_with("flags")).and_then(|l| l.split_once(':')).map(|(_, f)| f).unwrap_or("");
    if flags.split_whitespace().any(|f| f == "avx2") {
        check(Status::Ok, "CPU", "AVX2 present").coded("cpu.ok", &[])
    } else {
        check(Status::Fail, "CPU", "no AVX2: the console's code uses it, and VIctory requires it").coded("cpu.noAvx2", &[])
    }
}

/// [`check_avx2`] asked of the CPU itself, where there is no /proc/cpuinfo (Windows).
pub fn check_avx2_cpu() -> Check {
    #[cfg(target_arch = "x86_64")]
    let avx2 = std::arch::is_x86_feature_detected!("avx2");
    #[cfg(not(target_arch = "x86_64"))]
    let avx2 = false;
    check_avx2(Some(if avx2 { "flags\t: avx2" } else { "flags\t:" }))
}

pub fn check_memory(meminfo: Option<&str>) -> Check {
    let Some(meminfo) = meminfo else { return check(Status::Warn, "memory", "cannot read /proc/meminfo").coded("memory.unreadable", &[]) };
    let kib = |key: &str| -> u64 {
        meminfo
            .lines()
            .find(|l| l.starts_with(key) && l[key.len()..].starts_with(':'))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let (ram, swap) = (kib("MemTotal") * 1024, kib("SwapTotal") * 1024);
    let detail = format!("{:.1} GiB RAM + {:.1} GiB swap", ram as f64 / GIB as f64, swap as f64 / GIB as f64);
    if ram + swap < 24 * GIB {
        check(
            Status::Warn,
            "memory",
            format!("{detail}; XCL measured ~15 GiB committed in a newer title's menus, plus the GPU's own: 32 GiB or more is safer"),
        ).coded("memory.low", &[("total", detail.clone())])
    } else {
        check(Status::Ok, "memory", detail)
    }
}

pub fn check_max_map_count(value: Option<&str>) -> Check {
    let Some(n) = value.and_then(|v| v.trim().parse::<u64>().ok()) else {
        return check(Status::Warn, "vm.max_map_count", "cannot read it").coded("mapcount.unreadable", &[]);
    };
    if n < MAX_MAP_COUNT {
        check(Status::Warn, "vm.max_map_count", format!("{n}; large titles need more mappings: `sudo sysctl -w vm.max_map_count={MAX_MAP_COUNT}`")).coded("mapcount.low", &[("n", n.to_string()), ("want", MAX_MAP_COUNT.to_string())])
            .fixable(Fix::MaxMapCount)
    } else {
        check(Status::Ok, "vm.max_map_count", n.to_string())
    }
}

/// vkd3d-proton imports the title's memory as GPU heaps (OpenExistingHeapFromAddress);
/// Wine backs the write watches that needs with userfaultfd. XCL issue #175: without
/// them the GPU reads zeros where the title wrote (XCL logs it in its start-up UMA check).
pub fn check_userfaultfd(unprivileged: Option<&str>, device_usable: bool) -> Check {
    if unprivileged.map(str::trim) == Some("1") || device_usable {
        check(Status::Ok, "userfaultfd", "available to Wine").coded("userfaultfd.ok", &[])
    } else {
        let c = check(
            Status::Warn,
            "userfaultfd",
            "not available to unprivileged processes; vkd3d-proton may not see the title's memory writes (XCL #175). \
             Fix: `sudo sysctl -w vm.unprivileged_userfaultfd=1` or give your user access to /dev/userfaultfd",
        ).coded("userfaultfd.off", &[]);
        // Without the setting (a kernel built without it) there is nothing to switch on.
        if unprivileged.is_some() { c.fixable(Fix::Userfaultfd) } else { c }
    }
}

/// Reads `vulkaninfo` output: the devices, and whether mesh shaders are there.
pub fn check_vulkan(output: Option<&str>) -> Vec<Check> {
    let Some(out) = output else {
        return vec![check(Status::Warn, "Vulkan", "vulkaninfo not found or failed (package vulkan-tools); cannot check the GPU").coded("vulkan.noTool", &[])];
    };
    let mut names: Vec<String> = out
        .lines()
        .filter(|l| l.trim_start().starts_with("deviceName"))
        .filter_map(|l| l.split_once('=').map(|(_, n)| n.trim().to_owned()))
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return vec![check(Status::Fail, "Vulkan", "no Vulkan device: vkd3d-proton needs a Vulkan driver (Mesa RADV, NVIDIA)").coded("vulkan.noDevice", &[])];
    }
    let mesh = if out.contains("VK_EXT_mesh_shader") {
        check(Status::Ok, "mesh shaders", "VK_EXT_mesh_shader present").coded("mesh.ok", &[])
    } else {
        check(
            Status::Warn,
            "mesh shaders",
            "no VK_EXT_mesh_shader: the console's geometry / NGG programs run as mesh shaders; the world will not draw",
        ).coded("mesh.missing", &[])
    };
    vec![check(Status::Ok, "Vulkan", names.join(", ")), mesh]
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn device_usable(path: &str) -> bool {
    fs::OpenOptions::new().read(true).write(true).open(path).is_ok()
}

fn vulkaninfo() -> Option<String> {
    // In a Flatpak the host's vulkaninfo is the one that sees the host's drivers.
    let tool = if crate::host::in_flatpak() { "vulkaninfo".into() } else { runners::which("vulkaninfo")? };
    let out = crate::host::simple(&[tool.into()]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn machine_checks() -> Vec<Check> {
    // The other checks read Linux's /proc and Vulkan; on a Mac D3DMetal stands in for them, and
    // on Windows the system's D3D12 driver.
    if runners::WINDOWS {
        return vec![check_platform(), check_avx2_cpu()];
    }
    if runners::MACOS {
        return vec![check_platform()];
    }
    let mut checks = vec![
        check_platform(),
        check_avx2(read("/proc/cpuinfo").as_deref()),
        check_memory(read("/proc/meminfo").as_deref()),
        check_max_map_count(read("/proc/sys/vm/max_map_count").as_deref()),
        check_userfaultfd(read("/proc/sys/vm/unprivileged_userfaultfd").as_deref(), device_usable("/dev/userfaultfd")),
    ];
    checks.extend(check_vulkan(vulkaninfo().as_deref()));
    checks
}

// -- the configuration -------------------------------------------------------------------------

pub fn check_runner(cfg: &Config) -> Vec<Check> {
    let runner = match Runner::from_config(cfg) {
        Ok(r) => r,
        Err(e) => return vec![check(Status::Fail, "runner", e.to_string())],
    };
    let mut checks = vec![check(Status::Ok, "runner", format!("{} {}", runner.name(), runner.version()))];
    if matches!(runner, Runner::Native) {
        checks.push(check(Status::Ok, "d3d12", "the system's own D3D12 (the GPU driver's); no prefix needed").coded("d3d12.native", &[]));
        return checks;
    }
    let is_wine = matches!(runner, Runner::Wine(_));
    if !is_wine && std::env::var_os("PROTON_USE_WINED3D").is_some_and(|v| !v.is_empty() && v != "0") {
        checks.push(check(Status::Fail, "vkd3d-proton", "PROTON_USE_WINED3D is set: unset it, D3D12 must go through vkd3d-proton").coded("vkd3d.wined3d", &[]));
    } else if runners::MACOS && is_wine {
        checks.push(check(Status::Ok, "d3d12", "D3D12 goes to the Wine build's D3DMetal (4.0 or newer is needed)").coded("d3d12.metal", &[]));
    } else if runner.vkd3d_installed() {
        checks.push(check(Status::Ok, "vkd3d-proton", if is_wine { "installed in the prefix" } else { "bundled with Proton" }).coded(if is_wine { "vkd3d.inPrefix" } else { "vkd3d.bundled" }, &[]));
    } else if let Runner::Wine(w) = &runner {
        checks.push(check(
            Status::Fail,
            "vkd3d-proton",
            format!("not in {}: choose an unpacked vkd3d-proton release and prepare the prefix", w.system32().display()),
        ).coded("vkd3d.notInWine", &[("path", w.system32().display().to_string())]));
    } else {
        checks.push(check(Status::Warn, "vkd3d-proton", "not found in this Proton; it may not support D3D12").coded("vkd3d.notInProton", &[]));
    }
    let pfx = runner.wineprefix().unwrap_or_default();
    let z = pfx.join("dosdevices/z:");
    let prefix_check = if !pfx.join("system.reg").is_file() {
        if is_wine {
            check(Status::Fail, "prefix", format!("{} not created yet: prepare the prefix", pfx.display())).coded("prefix.notCreated", &[("path", pfx.display().to_string())])
        } else {
            check(Status::Warn, "prefix", format!("{} not created yet; Proton creates it on the first run", pfx.display())).coded("prefix.protonFirstRun", &[("path", pfx.display().to_string())])
        }
    } else if fs::read_to_string(pfx.join("system.reg")).unwrap_or_default().contains("#arch=win32") {
        check(Status::Fail, "prefix", format!("{} is a 32-bit prefix; delete it and prepare it again", pfx.display())).coded("prefix.win32", &[("path", pfx.display().to_string())])
    } else if !z.is_symlink() {
        check(Status::Fail, "prefix", format!("{} has no drive Z:; paths would not resolve", pfx.display())).coded("prefix.noZ", &[("path", pfx.display().to_string())])
    } else if fs::canonicalize(&z).ok().as_deref() != Some(Path::new("/")) {
        check(Status::Fail, "prefix", format!("drive Z: of {} is not the Linux root; paths would not resolve", pfx.display())).coded("prefix.zNotRoot", &[("path", pfx.display().to_string())])
    } else {
        check(Status::Ok, "prefix", pfx.display().to_string())
    };
    checks.push(prefix_check);
    checks
}

pub fn check_backend(cfg: &Config) -> Vec<Check> {
    let Some(exe) = cfg.backend_exe() else {
        let which = match cfg.backend.kind {
            BackendKind::Victory => "victory.exe (VILinux)",
            BackendKind::Xcl => "xcl.exe (LinuxCL)",
            BackendKind::Scarlett => "scarlett.exe (ScarlettPort)",
        };
        return vec![check(Status::Fail, "backend", format!("no backend executable: the Windows build of {which}")).coded("backend.none", &[("which", which.to_string())])];
    };
    if !exe.is_file() {
        return vec![check(Status::Fail, "backend", format!("{} does not exist", exe.display())).coded("backend.missing", &[("path", exe.display().to_string())])];
    }
    if backends::pe_machine(&exe) != Some(MACHINE_X86_64) {
        return vec![check(Status::Fail, "backend", format!("{} is not a 64-bit Windows executable", exe.display())).coded("backend.notWin64", &[("path", exe.display().to_string())])];
    }
    let mut checks = vec![check(Status::Ok, "backend", format!("{}: {}", cfg.backend.kind.name(), exe.display()))];
    let exe_dir = exe.parent().unwrap_or(Path::new("/")).to_path_buf();
    if cfg.backend.kind == BackendKind::Xcl {
        let dxc = cfg.dxc_dir().unwrap_or_else(|| exe_dir.clone());
        let missing: Vec<&str> = ["dxcompiler.dll", "dxil.dll"].into_iter().filter(|d| !dxc.join(d).is_file()).collect();
        checks.push(if missing.is_empty() {
            check(Status::Ok, "DXC", dxc.display().to_string())
        } else {
            check(
                Status::Fail,
                "DXC",
                format!(
                    "{} not in {}: XCL compiles shaders with DXC; choose a DirectXShaderCompiler release's bin/x64",
                    missing.join(", "),
                    dxc.display()
                ),
            ).coded("dxc.missing", &[("files", missing.join(", ")), ("path", dxc.display().to_string())])
        });
        let opus = cfg.opus_dir().unwrap_or(exe_dir);
        checks.push(if ["opus.dll", "libopus.dll", "libopus-0.dll"].iter().any(|n| opus.join(n).is_file()) {
            check(Status::Ok, "Opus", opus.display().to_string())
        } else {
            check(
                Status::Warn,
                "Opus",
                if runners::WINDOWS {
                    "no opus.dll: without it XCL falls back to Windows' Opus decoder, when one is installed"
                } else {
                    "no opus.dll: Wine has no Opus decoder of its own, so Opus audio stays silent"
                },
            ).coded(if runners::WINDOWS { "opus.missingWindows" } else { "opus.missingWine" }, &[])
        });
    }
    // On Windows the runtime loads D3D12 from the Agility SDK folder beside it (D3D12SDKPath);
    // under Wine and Proton, vkd3d-proton is D3D12 and the folder is not read.
    if cfg.backend.kind == BackendKind::Scarlett && cfg.runner.kind == RunnerKind::Native {
        let d3d12 = exe.parent().unwrap_or(Path::new("/")).join("D3D12");
        checks.push(if d3d12.join("D3D12Core.dll").is_file() {
            check(Status::Ok, "D3D12 Agility", d3d12.display().to_string())
        } else {
            check(
                Status::Fail,
                "D3D12 Agility",
                format!("no D3D12\\D3D12Core.dll beside {}: extract the whole ScarlettPort release, D3D12 folder included", exe.display()),
            ).coded("agility.missing", &[("path", d3d12.display().to_string())])
        });
    }
    // Without prepared shaders the first runs translate them in game: long loads and stutter.
    if cfg.backend.kind == BackendKind::Scarlett {
        checks.push(if crate::scarlett::prepared(cfg) {
            check(Status::Ok, "shaders", crate::scarlett::prep_cache(cfg).display().to_string()).coded("shaders.prepared", &[])
        } else {
            check(Status::Warn, "shaders", "not prepared for this ScarlettPort build and preset: the first run translates them in game (long loads, stutter)")
                .coded("shaders.unprepared", &[])
        });
    }
    for (name, dir) in [("data folder", cfg.data_dir()), ("cache folder", cfg.cache_dir())] {
        if let Some(tree) = paths::inside_victory_tree(&dir) {
            checks.push(check(
                Status::Fail,
                name,
                format!("{} is inside the VIctory source tree {}; game-derived data must stay out of it", dir.display(), tree.display()),
            ).coded("folder.inTree", &[("path", dir.display().to_string()), ("tree", tree.display().to_string())]));
        }
    }
    checks
}

pub fn check_game(cfg: &Config) -> Vec<Check> {
    let Some(dir) = cfg.game_dir() else {
        return vec![check(Status::Fail, "game", "no game folder: the unpacked folder of a game you own").coded("game.none", &[])];
    };
    if let Some(tree) = paths::inside_victory_tree(&dir) {
        return vec![check(
            Status::Fail,
            "game",
            format!("{} is inside the VIctory source tree {}; keep game files outside it", dir.display(), tree.display()),
        ).coded("game.inTree", &[("path", dir.display().to_string()), ("tree", tree.display().to_string())])];
    }
    let resolved = gamedir::inspect(&dir).and_then(|info| gamedir::resolve_exe(&info, cfg.game.exe.as_deref()).map(|exe| (info, exe)));
    let (info, exe) = match resolved {
        Ok(r) => r,
        Err(e) => return vec![check(Status::Fail, "game", e.to_string())],
    };
    if backends::pe_machine(&exe) != Some(MACHINE_X86_64) {
        return vec![check(
            Status::Fail,
            "game",
            format!("{} is not a readable 64-bit executable; SixLauncher needs the unpacked files of a game you own", exe.display()),
        ).coded("game.notWin64", &[("path", exe.display().to_string())])];
    }
    let version = info.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
    let shown = exe.strip_prefix(&info.folder).unwrap_or(&exe).display().to_string();
    let mut checks = vec![check(Status::Ok, "game", format!("{}{version}: {shown}", info.title()))];
    if cfg.game.verify {
        checks.push(check_game_hash(integrity::verify(&exe), &shown));
    }
    checks
}

/// The executable against the known SHA-256: a warning, never a failure, when it differs
/// (another version of the game is not an error).
pub fn check_game_hash(verdict: Result<integrity::Verdict>, shown: &str) -> Check {
    match verdict {
        Ok(v) if v.is(integrity::Kind::Exe) => check(
            Status::Ok,
            "game-sha256",
            format!("{shown} is the known {} executable (SHA-256 {})", v.known.map_or("", |k| k.name), &v.sha256[..12]),
        ),
        Ok(v) => check(
            Status::Warn,
            "game-sha256",
            format!(
                "{shown}: SHA-256 {} is not the known executable's ({}): another version, or a changed or damaged file",
                &v.sha256[..12],
                integrity::expected(integrity::Kind::Exe).iter().map(|h| &h[..12]).collect::<Vec<_>>().join(", ")
            ),
        ).coded("hash.mismatch", &[("file", shown.to_string()), ("sha", v.sha256[..12].to_string())]),
        Err(e) => check(Status::Warn, "game-sha256", format!("{shown}: could not read it to check its SHA-256: {e}")).coded("hash.unreadable", &[("file", shown.to_string()), ("error", e.to_string())]),
    }
}

/// Which GPU runs the game and what `[gpu]` adds for it.
pub fn check_gpu(cfg: &Config, gpus: &[gpu::Gpu]) -> Check {
    let vars = |env: std::collections::BTreeMap<String, String>| {
        if env.is_empty() { String::new() } else { format!("; adds {}", env.keys().cloned().collect::<Vec<_>>().join(", ")) }
    };
    match gpu::choose(&cfg.gpu.device, gpus) {
        gpu::Choice::Gpu(g) => {
            let how = if gpus.len() < 2 {
                "the only GPU"
            } else if g.nvidia_blob() && !g.boot_vga {
                "PRIME render offload"
            } else if !g.boot_vga {
                "DRI_PRIME offload"
            } else {
                "the boot GPU"
            };
            let mut detail = format!("{} ({}, {}): {how}{}", g.name, g.slot, g.driver, vars(gpu::env_for(cfg, gpus)));
            let status = if g.integrated && gpus.iter().any(|o| !o.integrated) {
                detail += ". It is the integrated GPU while a dedicated one is there";
                Status::Warn
            } else {
                Status::Ok
            };
            check(status, "GPU", detail)
        }
        gpu::Choice::System => {
            let what = if gpus.is_empty() { "no GPU found in /sys/class/drm" } else { "left to the system" };
            check(Status::Ok, "GPU", format!("{what}{}", vars(gpu::env_for(cfg, gpus))))
        }
        gpu::Choice::Missing(slot) => check(
            Status::Warn,
            "GPU",
            format!("{slot} is not on this machine (now); the system picks the GPU. Choose another one or `auto`"),
        ).coded("gpu.missing", &[("slot", slot.to_string())]),
    }
}

/// MangoHud and GameMode, when they are turned on.
pub fn check_performance(cfg: &Config, tools: &perf::Tools) -> Vec<Check> {
    let mut checks = Vec::new();
    for (on, name, package, tool) in [
        (cfg.performance.mangohud, "MangoHud", "mangohud", &tools.mangohud),
        (cfg.performance.gamemode, "GameMode", "gamemode", &tools.gamemode),
    ] {
        if on {
            let (ok, detail) = perf::describe(package, tool, tools.sandboxed);
            let detail = if ok { detail } else { format!("{detail}; the game runs without it") };
            checks.push(check(if ok { Status::Ok } else { Status::Warn }, name, detail));
        }
    }
    checks
}

pub fn config_checks(cfg: &Config) -> Vec<Check> {
    let mut checks = check_runner(cfg);
    checks.extend(check_backend(cfg));
    checks.extend(check_game(cfg));
    // GPU choice, MangoHud and GameMode are Linux's (DRM, Vulkan layers, Feral's daemon).
    if runners::MACOS || runners::WINDOWS {
        return checks;
    }
    checks.push(check_gpu(cfg, &gpu::list()));
    if perf::wanted(cfg) {
        checks.extend(check_performance(cfg, &perf::detect()));
    }
    checks
}

pub fn run_all(cfg: &Config) -> Vec<Check> {
    let mut checks = machine_checks();
    checks.extend(config_checks(cfg));
    checks
}

#[cfg(test)]
mod tests {
    #[test]
    fn coded_checks_carry_their_id_and_values() {
        let c = super::check_max_map_count(Some("65530"));
        assert_eq!(c.code.as_deref(), Some("mapcount.low"));
        assert_eq!(c.args.get("n").map(String::as_str), Some("65530"));
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["code"], "mapcount.low");
        assert_eq!(json["args"]["want"], super::MAX_MAP_COUNT.to_string());
        let plain = serde_json::to_value(super::check_max_map_count(Some("2000000"))).unwrap();
        assert!(plain.get("code").is_none() && plain.get("args").is_none(), "{plain}");
        assert_eq!(super::check_userfaultfd(Some("0\n"), false).code.as_deref(), Some("userfaultfd.off"));
        assert_eq!(super::check_avx2(Some("flags\t: fpu avx2\n")).code.as_deref(), Some("cpu.ok"));
    }

    use super::*;
    use crate::testutil::{self, TempHome};

    fn statuses(checks: &[Check]) -> std::collections::BTreeMap<String, Status> {
        checks.iter().map(|c| (c.name.clone(), c.status)).collect()
    }

    #[test]
    fn machine_checks_from_text() {
        assert_eq!(platform_check("linux", "x86_64").status, Status::Ok);
        assert_eq!(platform_check("macos", "aarch64").status, Status::Ok);
        assert_eq!(platform_check("macos", "x86_64").status, Status::Fail);
        assert_eq!(platform_check("windows", "x86_64").status, Status::Ok);
        assert_eq!(platform_check("windows", "aarch64").status, Status::Fail);
        assert_eq!(platform_check("freebsd", "x86_64").status, Status::Fail);
        assert_eq!(check_avx2(Some("flags\t: fpu sse avx avx2 bmi2\n")).status, Status::Ok);
        assert_eq!(check_avx2(Some("flags\t: fpu sse avx\n")).status, Status::Fail);
        assert_eq!(check_memory(Some("MemTotal:       16000000 kB\nSwapTotal:       2000000 kB\n")).status, Status::Warn);
        assert_eq!(check_memory(Some("MemTotal:       32000000 kB\nSwapTotal:       8000000 kB\n")).status, Status::Ok);
        assert_eq!(check_max_map_count(Some("65530\n")).status, Status::Warn);
        assert_eq!(check_max_map_count(Some("2147483642\n")).status, Status::Ok);
        assert_eq!(check_userfaultfd(Some("0\n"), false).status, Status::Warn);
        assert_eq!(check_userfaultfd(Some("0\n"), true).status, Status::Ok);
        assert_eq!(check_userfaultfd(Some("1\n"), false).status, Status::Ok);
    }

    #[test]
    fn kernel_warnings_offer_a_fix() {
        let fix = |c: Check| c.fix.map(|p| p.id);
        assert_eq!(fix(check_max_map_count(Some("65530\n"))), Some(Fix::MaxMapCount));
        assert_eq!(fix(check_max_map_count(Some("1048576\n"))), None);
        assert_eq!(fix(check_max_map_count(None)), None);
        assert_eq!(fix(check_userfaultfd(Some("0\n"), false)), Some(Fix::Userfaultfd));
        assert_eq!(fix(check_userfaultfd(None, false)), None);
        assert_eq!(fix(check_userfaultfd(Some("1\n"), false)), None);
        let json = serde_json::to_value(check_max_map_count(Some("65530"))).unwrap();
        assert_eq!(json["fix"]["file"], "/etc/sysctl.d/80-sixlauncher-max-map-count.conf");
        assert!(serde_json::to_value(check_platform()).unwrap().get("fix").is_none());
    }

    #[test]
    fn vulkan_check_reads_vulkaninfo() {
        let out = "GPU0:\n\tdeviceName        = AMD Radeon RX 6700 XT (RADV NAVI22)\n\tVK_EXT_mesh_shader : extension revision 1\n";
        let c = check_vulkan(Some(out));
        assert_eq!((c[0].status, c[1].status), (Status::Ok, Status::Ok));
        assert_eq!(c[0].detail, "AMD Radeon RX 6700 XT (RADV NAVI22)");
        assert_eq!(check_vulkan(Some(&out.replace("VK_EXT_mesh_shader", "")))[1].status, Status::Warn);
        assert_eq!(check_vulkan(Some("nothing\n"))[0].status, Status::Fail);
        assert_eq!(check_vulkan(None)[0].status, Status::Warn);
    }

    #[test]
    fn data_inside_victory_tree_fails() {
        let h = TempHome::new();
        let tree = h.path().join("VILinux");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join(".victory-root"), "marker").unwrap();
        let mut cfg = Config::default();
        cfg.backend.exe = Some(testutil::fake_pe(&h.path().join("b/victory.exe"), 0x8664));
        cfg.dirs.data = Some(tree.join("data"));
        let fails: Vec<String> = check_backend(&cfg).into_iter().filter(|c| c.status == Status::Fail).map(|c| c.name).collect();
        assert_eq!(fails, ["data folder"]);
    }

    #[test]
    fn xcl_needs_dxc() {
        let h = TempHome::new();
        let exe = testutil::fake_pe(&h.path().join("b/xcl.exe"), 0x8664);
        let mut cfg = Config::default();
        cfg.backend.kind = BackendKind::Xcl;
        cfg.backend.exe = Some(exe.clone());
        let s = statuses(&check_backend(&cfg));
        assert_eq!((s["DXC"], s["Opus"]), (Status::Fail, Status::Warn));
        for dll in ["dxcompiler.dll", "dxil.dll", "opus.dll"] {
            testutil::fake_pe(&exe.with_file_name(dll), 0x8664);
        }
        let s = statuses(&check_backend(&cfg));
        assert_eq!((s["DXC"], s["Opus"]), (Status::Ok, Status::Ok));
    }

    #[test]
    fn wine_prefix_checks() {
        let h = TempHome::new();
        let prefix = testutil::fake_wine_prefix(&h.path().join("pfx"));
        let wine = testutil::executable(&h.path().join("bin/wine"), "#!/bin/sh\necho wine-9.22\n");
        let mut cfg = Config::default();
        cfg.runner.kind = RunnerKind::Wine;
        cfg.runner.path = Some(wine);
        cfg.runner.prefix = Some(prefix.clone());
        let checks = check_runner(&cfg);
        assert_eq!(checks[0].detail, "wine wine-9.22");
        let s = statuses(&checks);
        assert_eq!((s["vkd3d-proton"], s["prefix"]), (Status::Fail, Status::Ok));
        fs::remove_file(prefix.join("dosdevices/z:")).unwrap();
        std::os::unix::fs::symlink("/tmp", prefix.join("dosdevices/z:")).unwrap();
        assert_eq!(statuses(&check_runner(&cfg))["prefix"], Status::Fail);
    }

    #[test]
    fn proton_with_wined3d_fails() {
        let h = TempHome::new();
        let mut cfg = Config::default();
        cfg.runner.path = Some(testutil::fake_proton(h.path(), "GE-Proton9-20"));
        assert_eq!(statuses(&check_runner(&cfg))["vkd3d-proton"], Status::Ok);
        // SAFETY: TempHome holds the environment lock.
        unsafe { std::env::set_var("PROTON_USE_WINED3D", "1") };
        assert_eq!(statuses(&check_runner(&cfg))["vkd3d-proton"], Status::Fail);
        unsafe { std::env::remove_var("PROTON_USE_WINED3D") };
    }

    #[test]
    fn gpu_check_names_the_offload() {
        let t = tempfile::tempdir().unwrap();
        let gpus = gpu::tests::hybrid_nvidia(t.path());
        let mut cfg = Config::default();
        let c = check_gpu(&cfg, &gpus);
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.starts_with("NVIDIA GeForce RTX 3050 Mobile (0000:01:00.0, nvidia): PRIME render offload; adds "), "{}", c.detail);
        cfg.gpu.device = "0000:00:02.0".into();
        assert_eq!(check_gpu(&cfg, &gpus).status, Status::Warn);
        cfg.gpu.device = "0000:07:00.0".into();
        assert_eq!(check_gpu(&cfg, &gpus).status, Status::Warn);
        cfg.gpu.device = "system".into();
        assert_eq!(check_gpu(&cfg, &gpus).status, Status::Ok);
    }

    #[test]
    fn performance_checks_only_when_on() {
        let mut cfg = Config::default();
        let tools = perf::Tools::default();
        assert!(check_performance(&cfg, &tools).is_empty());
        cfg.performance.mangohud = true;
        let c = check_performance(&cfg, &tools);
        assert_eq!((c.len(), c[0].status), (1, Status::Warn));
        assert!(c[0].detail.contains("`mangohud` package"), "{}", c[0].detail);
    }

    #[test]
    fn game_checks() {
        let h = TempHome::new();
        let mut cfg = Config::default();
        assert_eq!(check_game(&cfg)[0].status, Status::Fail);
        cfg.game.dir = Some(testutil::fake_game(h.path()));
        let c = check_game(&cfg);
        assert_eq!(c[0].status, Status::Ok);
        assert_eq!(c[0].detail, "Test Title 1.2.3.0: bin/title.exe");
        // The test executable is not the known one: a warning, not a failure.
        assert_eq!((c.len(), c[1].name.as_str(), c[1].status), (2, "game-sha256", Status::Warn));
        assert!(c[1].detail.contains("is not the known executable's (e95020b2fa4a)"), "{}", c[1].detail);
        cfg.game.verify = false;
        assert_eq!(check_game(&cfg).len(), 1, "verify off: no hash");
    }

    #[test]
    fn known_executable_passes() {
        let known = integrity::KNOWN.iter().find(|k| k.kind == integrity::Kind::Exe).unwrap();
        let v = integrity::Verdict { path: "g.exe".into(), sha256: known.sha256.into(), known: Some(*known), cached: true };
        let c = check_game_hash(Ok(v), "g.exe");
        assert_eq!(c.status, Status::Ok);
        assert_eq!(c.detail, "g.exe is the known Grand Theft Auto VI executable (SHA-256 e95020b2fa4a)");
    }
}

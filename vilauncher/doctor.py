"""``vilauncher doctor``: checks the machine and the configuration before a run.

Each check gives OK, WARN (it may run, but something the backends need is
missing or unverified) or FAIL (a run cannot work). ``run`` refuses to start
while any check fails, unless asked to with ``--force``.
"""

from __future__ import annotations

import os
import platform
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

from . import backends, gamedir, paths, runners

OK, WARN, FAIL = "OK", "WARN", "FAIL"
GIB = 1 << 30
# SteamOS' value; large titles map far more regions than the kernel default (65530).
MAX_MAP_COUNT = 1048576


@dataclass
class Check:
    status: str
    name: str
    detail: str


def _read(path: str) -> str | None:
    try:
        return Path(path).read_text()
    except OSError:
        return None


# -- the machine -----------------------------------------------------------------------------


def check_platform() -> Check:
    system, machine = platform.system(), platform.machine()
    if system == "Linux" and machine in ("x86_64", "AMD64"):
        return Check(OK, "platform", f"{system} {machine}")
    return Check(FAIL, "platform", f"{system} {machine}: the title's x86-64 code runs directly on the CPU, so an x86-64 Linux is required")


def check_avx2(cpuinfo: str | None = None) -> Check:
    cpuinfo = cpuinfo if cpuinfo is not None else _read("/proc/cpuinfo")
    if cpuinfo is None:
        return Check(WARN, "CPU", "cannot read /proc/cpuinfo")
    flags = next((l.split(":", 1)[1].split() for l in cpuinfo.splitlines() if l.startswith("flags")), [])
    if "avx2" in flags:
        return Check(OK, "CPU", "AVX2 present")
    return Check(FAIL, "CPU", "no AVX2: the console's code uses it, and VIctory requires it")


def check_memory(meminfo: str | None = None) -> Check:
    meminfo = meminfo if meminfo is not None else _read("/proc/meminfo")
    if meminfo is None:
        return Check(WARN, "memory", "cannot read /proc/meminfo")
    kib = {}
    for line in meminfo.splitlines():
        key, _, rest = line.partition(":")
        if rest.strip().endswith("kB"):
            kib[key] = int(rest.split()[0])
    ram = kib.get("MemTotal", 0) * 1024
    swap = kib.get("SwapTotal", 0) * 1024
    detail = f"{ram / GIB:.1f} GiB RAM + {swap / GIB:.1f} GiB swap"
    if ram + swap < 24 * GIB:
        return Check(WARN, "memory", f"{detail}; XCL measured ~15 GiB committed in a newer title's menus, plus the GPU's own: 32 GiB or more is safer")
    return Check(OK, "memory", detail)


def check_max_map_count(value: str | None = None) -> Check:
    value = value if value is not None else _read("/proc/sys/vm/max_map_count")
    if value is None:
        return Check(WARN, "vm.max_map_count", "cannot read it")
    n = int(value.strip())
    if n < MAX_MAP_COUNT:
        return Check(WARN, "vm.max_map_count", f"{n}; large titles need more mappings: `sudo sysctl -w vm.max_map_count={MAX_MAP_COUNT}`")
    return Check(OK, "vm.max_map_count", str(n))


def check_userfaultfd(unpriv: str | None = None, dev_ok: bool | None = None) -> Check:
    """vkd3d-proton imports the title's memory as GPU heaps
    (OpenExistingHeapFromAddress); Wine backs the write watches that needs with
    userfaultfd. XCL issue #175: without them the GPU reads zeros where the
    title wrote (XCL then logs it in its start-up UMA check)."""
    unpriv = unpriv if unpriv is not None else _read("/proc/sys/vm/unprivileged_userfaultfd")
    if dev_ok is None:
        dev_ok = os.access("/dev/userfaultfd", os.R_OK | os.W_OK)
    if (unpriv or "").strip() == "1" or dev_ok:
        return Check(OK, "userfaultfd", "available to Wine")
    return Check(
        WARN,
        "userfaultfd",
        "not available to unprivileged processes; vkd3d-proton may not see the title's memory writes (XCL #175). "
        "Fix: `sudo sysctl -w vm.unprivileged_userfaultfd=1` or give your user access to /dev/userfaultfd",
    )


def check_vulkan(run=subprocess.run) -> list[Check]:
    tool = shutil.which("vulkaninfo")
    if tool is None:
        return [Check(WARN, "Vulkan", "vulkaninfo not found (package vulkan-tools); cannot check the GPU")]
    try:
        out = run([tool], capture_output=True, text=True, timeout=60).stdout
    except (OSError, subprocess.TimeoutExpired) as e:
        return [Check(WARN, "Vulkan", f"vulkaninfo failed: {e}")]
    names = sorted({l.split("=", 1)[1].strip() for l in out.splitlines() if l.strip().startswith("deviceName")})
    if not names:
        return [Check(FAIL, "Vulkan", "no Vulkan device: vkd3d-proton needs a Vulkan driver (Mesa RADV, NVIDIA)")]
    checks = [Check(OK, "Vulkan", ", ".join(names))]
    if "VK_EXT_mesh_shader" in out:
        checks.append(Check(OK, "mesh shaders", "VK_EXT_mesh_shader present"))
    else:
        checks.append(Check(WARN, "mesh shaders", "no VK_EXT_mesh_shader: the console's geometry / NGG programs run as mesh shaders; the world will not draw"))
    return checks


# -- the configuration -----------------------------------------------------------------------


def check_runner(cfg) -> list[Check]:
    try:
        runner = runners.make_runner(cfg)
    except runners.RunnerError as e:
        return [Check(FAIL, "runner", str(e))]
    checks = [Check(OK, "runner", f"{runner.name} {runner.version()}")]
    if runner.name == "proton" and os.environ.get("PROTON_USE_WINED3D"):
        checks.append(Check(FAIL, "vkd3d-proton", "PROTON_USE_WINED3D is set: unset it, D3D12 must go through vkd3d-proton"))
    elif runner.vkd3d_installed():
        checks.append(Check(OK, "vkd3d-proton", "installed" if runner.name == "wine" else "bundled with Proton"))
    elif runner.name == "wine":
        checks.append(Check(FAIL, "vkd3d-proton", f"not in {runner.system32()}: run `vilauncher setup --vkd3d <unpacked vkd3d-proton release>`"))
    else:
        checks.append(Check(WARN, "vkd3d-proton", f"not found in {runner.proton_dir}; this Proton may not support D3D12"))
    pfx = runner.wineprefix()
    z = pfx / "dosdevices/z:"
    if not (pfx / "system.reg").is_file():
        state = WARN if runner.name == "proton" else FAIL
        hint = "Proton creates it on the first run" if runner.name == "proton" else "run `vilauncher setup`"
        checks.append(Check(state, "prefix", f"{pfx} not created yet; {hint}"))
    else:
        reg = _read(str(pfx / "system.reg")) or ""
        if "#arch=win32" in reg:
            checks.append(Check(FAIL, "prefix", f"{pfx} is a 32-bit prefix; delete it and run `vilauncher setup`"))
        elif z.is_symlink() and os.path.realpath(z) != "/":
            checks.append(Check(FAIL, "prefix", f"drive Z: of {pfx} is not the Linux root; paths would not resolve"))
        elif not z.is_symlink():
            checks.append(Check(FAIL, "prefix", f"{pfx} has no drive Z:; paths would not resolve"))
        else:
            checks.append(Check(OK, "prefix", str(pfx)))
    return checks


def check_backend(cfg) -> list[Check]:
    exe = cfg.backend_exe
    if exe is None:
        return [Check(FAIL, "backend", f"no [backend] exe: the Windows build of {'victory.exe (VILinux)' if cfg.backend == 'victory' else 'xcl.exe (LinuxCL)'}")]
    if not exe.is_file():
        return [Check(FAIL, "backend", f"{exe} does not exist")]
    machine = backends.pe_machine(exe)
    if machine != 0x8664:
        return [Check(FAIL, "backend", f"{exe} is not a 64-bit Windows executable")]
    checks = [Check(OK, "backend", f"{cfg.backend}: {exe}")]
    if cfg.backend == "xcl":
        dxc = cfg.dxc_dir or exe.parent
        missing = [d for d in ("dxcompiler.dll", "dxil.dll") if not (dxc / d).is_file()]
        if missing:
            checks.append(Check(FAIL, "DXC", f"{', '.join(missing)} not in {dxc}: XCL compiles shaders with DXC; set [backend] dxc_dir to a DirectXShaderCompiler release's bin/x64"))
        else:
            checks.append(Check(OK, "DXC", str(dxc)))
        opus = cfg.opus_dir or exe.parent
        if any((opus / n).is_file() for n in ("opus.dll", "libopus.dll", "libopus-0.dll")):
            checks.append(Check(OK, "Opus", str(opus)))
        else:
            checks.append(Check(WARN, "Opus", "no opus.dll: Wine has no Opus decoder of its own, so Opus audio stays silent; set [backend] opus_dir"))
    for name, d in (("data", cfg.resolved_data_dir()), ("cache", cfg.resolved_cache_dir())):
        tree = paths.inside_victory_tree(d)
        if tree:
            checks.append(Check(FAIL, f"{name} folder", f"{d} is inside the VIctory source tree {tree}; game-derived data must stay out of it"))
    return checks


def check_game(cfg) -> list[Check]:
    if cfg.game_dir is None:
        return [Check(FAIL, "game", "no [game] dir: the unpacked folder of a game you own")]
    tree = paths.inside_victory_tree(cfg.game_dir)
    if tree:
        return [Check(FAIL, "game", f"{cfg.game_dir} is inside the VIctory source tree {tree}; keep game files outside it")]
    try:
        info = gamedir.inspect(cfg.game_dir)
        exe = gamedir.resolve_exe(info, cfg.game_exe)
    except gamedir.GameDirError as e:
        return [Check(FAIL, "game", str(e))]
    if backends.pe_machine(exe) != 0x8664:
        return [Check(FAIL, "game", f"{exe} is not a readable 64-bit executable; VILauncher needs the unpacked files of a game you own")]
    title = info.display_name or info.identity or info.folder.name
    version = f" {info.version}" if info.version else ""
    return [Check(OK, "game", f"{title}{version}: {exe.relative_to(info.folder) if exe.is_relative_to(info.folder) else exe}")]


def run_all(cfg, machine: bool = True) -> list[Check]:
    checks: list[Check] = []
    if machine:
        checks += [check_platform(), check_avx2(), check_memory(), check_max_map_count(), check_userfaultfd()]
        checks += check_vulkan()
    checks += check_runner(cfg)
    checks += check_backend(cfg)
    checks += check_game(cfg)
    return checks

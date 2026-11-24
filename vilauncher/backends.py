"""The compatibility layers VILauncher starts: VIctory (VILinux) and XCL (LinuxCL).

Each one is a Windows executable built from its repository. This module turns
the launcher's configuration into the command line and environment each one
documents in its README:

* VIctory: ``victory.exe run <title exe> --execute [--console series-s]
  [-- title args]`` with ``VICTORY_GAME_DIR``, ``VICTORY_DATA_DIR`` and
  ``VICTORY_CACHE_DIR``.
* XCL: ``xcl.exe run <game folder> [-- title args]`` with ``XCL_DATA_ROOT``,
  ``XCL_DXC_DIR`` (DXC's ``dxcompiler.dll`` + ``dxil.dll``) and ``XCL_OPUS_DIR``
  (``opus.dll``: under Wine there is no Media Foundation Opus decoder).

Every path goes through :func:`paths.to_windows`, because Wine hands
arguments and environment variables to the program unchanged.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .paths import to_windows


@dataclass
class Plan:
    """What to run: the backend's own arguments and environment."""

    exe: Path
    args: list[str]
    env: dict[str, str]


def victory_plan(cfg, game_exe: Path) -> Plan:
    args = ["run", to_windows(game_exe), "--execute", "--unimplemented", cfg.unimplemented]
    if cfg.console != "series-x":
        args += ["--console", cfg.console]
    if cfg.game_args:
        args += ["--", *cfg.game_args]
    env = {
        "VICTORY_GAME_DIR": to_windows(cfg.game_dir),
        "VICTORY_DATA_DIR": to_windows(cfg.resolved_data_dir()),
        "VICTORY_CACHE_DIR": to_windows(cfg.resolved_cache_dir()),
    }
    return Plan(exe=cfg.backend_exe, args=args, env=env)


def xcl_plan(cfg, game_exe: Path) -> Plan:
    # XCL takes the folder and finds the executable through MicrosoftGame.config
    # itself; an explicitly chosen executable is passed as a file instead.
    target = game_exe if cfg.game_exe else cfg.game_dir
    args = ["run", to_windows(target)]
    if cfg.game_args:
        args += ["--", *cfg.game_args]
    env = {"XCL_DATA_ROOT": to_windows(cfg.resolved_data_dir())}
    if cfg.dxc_dir:
        env["XCL_DXC_DIR"] = to_windows(cfg.dxc_dir)
    if cfg.opus_dir:
        env["XCL_OPUS_DIR"] = to_windows(cfg.opus_dir)
    return Plan(exe=cfg.backend_exe, args=args, env=env)


def plan(cfg, game_exe: Path) -> Plan:
    return {"victory": victory_plan, "xcl": xcl_plan}[cfg.backend](cfg, game_exe)


def guess_backend(exe: Path) -> str | None:
    name = Path(exe).name.lower()
    if name.startswith("victory"):
        return "victory"
    if name.startswith("xcl"):
        return "xcl"
    return None


def pe_machine(exe: Path) -> int | None:
    """The PE machine field of ``exe`` (0x8664 for x86-64), or None if it is not a PE file."""
    try:
        with open(exe, "rb") as f:
            head = f.read(0x1000)
    except OSError:
        return None
    if len(head) < 0x40 or head[:2] != b"MZ":
        return None
    off = int.from_bytes(head[0x3C:0x40], "little")
    if off + 6 > len(head) or head[off:off + 4] != b"PE\0\0":
        return None
    return int.from_bytes(head[off + 4:off + 6], "little")

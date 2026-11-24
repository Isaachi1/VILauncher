"""Synthetic stand-ins written by the tests themselves: no game data, no real binaries."""

from __future__ import annotations

import os
import stat
from pathlib import Path

GAME_CONFIG = """<?xml version="1.0" encoding="utf-8"?>
<Game configVersion="1">
  <Identity Name="Example.TestTitle" Publisher="CN=Example" Version="1.2.3.0"/>
  <ExecutableList>
    <Executable Name="bin\\title.exe" TargetDeviceFamily="Scarlett" Id="Game"/>
  </ExecutableList>
  <ShellVisuals DefaultDisplayName="Test Title" PublisherDisplayName="Example"/>
</Game>
"""


def fake_pe(path: Path, machine: int = 0x8664) -> Path:
    """A PE header with nothing behind it: enough for the machine check."""
    data = bytearray(0x200)
    data[0:2] = b"MZ"
    data[0x3C:0x40] = (0x80).to_bytes(4, "little")
    data[0x80:0x84] = b"PE\0\0"
    data[0x84:0x86] = machine.to_bytes(2, "little")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(data))
    return path


def fake_game(root: Path) -> Path:
    game = root / "game"
    game.mkdir(parents=True)
    (game / "MicrosoftGame.config").write_text(GAME_CONFIG)
    fake_pe(game / "bin" / "title.exe")
    return game


def executable(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def fake_proton(root: Path, name: str = "GE-Proton9-20") -> Path:
    """A Proton folder whose `proton` script records how it was called."""
    proton = root / name
    executable(
        proton / "proton",
        "#!/bin/sh\n"
        'out="$VILAUNCHER_TEST_OUT"\n'
        'printf "%s\\n" "$@" > "$out.argv"\n'
        'env > "$out.env"\n'
        'pwd > "$out.cwd"\n'
        "exit 7\n",
    )
    (proton / "version").write_text(f"1700000000 {name}\n")
    vk = proton / "files/lib/wine/vkd3d-proton/x86_64-windows"
    for dll in ("d3d12.dll", "d3d12core.dll"):
        fake_pe(vk / dll)
    return proton


def fake_wine_prefix(prefix: Path) -> Path:
    (prefix / "drive_c/windows/system32").mkdir(parents=True)
    (prefix / "system.reg").write_text("WINE REGISTRY Version 2\n#arch=win64\n")
    (prefix / "dosdevices").mkdir()
    os.symlink("/", prefix / "dosdevices/z:")
    return prefix

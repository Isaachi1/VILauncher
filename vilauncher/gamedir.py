"""The game folder: an unpacked Xbox Series (GDK) title the user owns.

VILauncher reads ``MicrosoftGame.config`` to find the title's executable. It
never unpacks, decrypts or modifies anything in the folder: a packaged title
(XVC / MSIXVC) is refused with an explanation, the same as VIctory and XCL do.
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path

CONFIG_NAME = "MicrosoftGame.config"
PACKAGE_SUFFIXES = (".xvc", ".msixvc", ".xvd")


class GameDirError(Exception):
    pass


@dataclass
class GameInfo:
    folder: Path
    config: Path | None
    identity: str | None = None
    version: str | None = None
    display_name: str | None = None
    executables: list[str] = field(default_factory=list)


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def find_config(folder: Path) -> Path | None:
    """``MicrosoftGame.config`` in ``folder``, whatever the case of its name."""
    exact = folder / CONFIG_NAME
    if exact.is_file():
        return exact
    if folder.is_dir():
        for entry in folder.iterdir():
            if entry.name.lower() == CONFIG_NAME.lower() and entry.is_file():
                return entry
    return None


def parse_config(path: Path) -> GameInfo:
    try:
        root = ET.parse(path).getroot()
    except ET.ParseError as e:
        raise GameDirError(f"{path} is not valid XML: {e}") from e
    info = GameInfo(folder=path.parent, config=path)
    for el in root.iter():
        tag = _local(el.tag)
        if tag == "Identity":
            info.identity = el.get("Name") or info.identity
            info.version = el.get("Version") or info.version
        elif tag == "Executable" and el.get("Name"):
            info.executables.append(el.get("Name"))
        elif tag == "ShellVisuals" and el.get("DefaultDisplayName"):
            info.display_name = el.get("DefaultDisplayName")
    return info


def inspect(folder: Path) -> GameInfo:
    folder = Path(folder).expanduser()
    if not folder.is_dir():
        raise GameDirError(f"game folder {folder} does not exist")
    packages = [p.name for p in folder.iterdir() if p.suffix.lower() in PACKAGE_SUFFIXES]
    config = find_config(folder)
    if config is None:
        if packages:
            raise GameDirError(
                f"{folder} holds a packaged title ({', '.join(sorted(packages))}). VILauncher, VIctory "
                "and XCL only run unpacked game files you own; they do not unpack or decrypt packages."
            )
        exes = sorted(p.name for p in folder.glob("*.exe"))
        return GameInfo(folder=folder, config=None, executables=exes)
    info = parse_config(config)
    info.folder = folder
    return info


def resolve_exe(info: GameInfo, override: str | None = None) -> Path:
    """The title's executable: ``override`` (relative to the folder or absolute),
    else the first ``<Executable>`` of the config that exists, else the only .exe."""
    if override:
        p = Path(override).expanduser()
        p = p if p.is_absolute() else info.folder / p
        if not p.is_file():
            raise GameDirError(f"the configured executable {p} does not exist")
        return p
    for name in info.executables:
        p = info.folder / name.replace("\\", "/")
        if p.is_file():
            return p
    if info.config is not None and info.executables:
        raise GameDirError(
            f"none of the executables {CONFIG_NAME} names exists in {info.folder}: {', '.join(info.executables)}"
        )
    if info.config is None and len(info.executables) == 1:
        return info.folder / info.executables[0]
    raise GameDirError(
        f"cannot tell which executable to run in {info.folder}; set [game] exe in the config"
    )

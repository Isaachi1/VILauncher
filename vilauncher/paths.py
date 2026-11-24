"""Where VILauncher keeps its files, and how Linux paths look from inside Wine."""

from __future__ import annotations

import os
from pathlib import Path

APP = "vilauncher"

# VIctory marks its source tree with this file and refuses to write
# game-derived data below it (docs/asset-policy.md in VILinux).
VICTORY_ROOT_MARKER = ".victory-root"


def _xdg(var: str, fallback: str) -> Path:
    value = os.environ.get(var)
    if value:
        return Path(value)
    return Path.home() / fallback


def config_dir() -> Path:
    return _xdg("XDG_CONFIG_HOME", ".config") / APP


def data_dir() -> Path:
    return _xdg("XDG_DATA_HOME", ".local/share") / APP


def cache_dir() -> Path:
    return _xdg("XDG_CACHE_HOME", ".cache") / APP


def state_dir() -> Path:
    return _xdg("XDG_STATE_HOME", ".local/state") / APP


def config_file() -> Path:
    return Path(os.environ.get("VILAUNCHER_CONFIG") or config_dir() / "config.toml")


def to_windows(path: Path | str) -> str:
    """The path as a program inside Wine sees it.

    A new Wine or Proton prefix maps drive Z: to the Linux root, so
    ``/home/me/game`` is ``Z:\\home\\me\\game``. Wine does not translate
    command-line arguments or environment variables, so every path handed to
    the Windows program goes through here. ``doctor`` checks the Z: mapping.
    """
    p = Path(path).expanduser()
    if not p.is_absolute():
        p = Path.cwd() / p
    return "Z:" + os.path.normpath(str(p)).replace("/", "\\")


def inside_victory_tree(path: Path) -> Path | None:
    """The VIctory source tree containing ``path``, if any."""
    p = Path(path).expanduser().absolute()
    for parent in (p, *p.parents):
        if (parent / VICTORY_ROOT_MARKER).is_file():
            return parent
    return None

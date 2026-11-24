"""Wine and Proton: what runs the Windows build of VIctory or XCL on Linux.

Both VIctory and XCL are Windows programs. They already carry Wine-specific
code paths (they detect Wine through ntdll's ``wine_get_version``), and XCL's
D3D12 code is written against vkd3d-proton's behaviour. On Linux they run
under Wine with vkd3d-proton as the D3D12 implementation (D3D12 -> Vulkan).
The title's x86-64 code still runs directly on the CPU: there is no CPU
emulation anywhere in the chain.

Wine's own ``d3d12.dll`` (upstream vkd3d) does not offer shader model 6.6 or
mesh shaders, which both backends need, so vkd3d-proton is required:

* ``proton``: Proton and GE-Proton ship vkd3d-proton and use it by default.
* ``wine``: a system Wine; ``setup`` copies vkd3d-proton's x64 ``d3d12.dll``
  and ``d3d12core.dll`` into the prefix, and runs use ``WINEDLLOVERRIDES``.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

VKD3D_DLLS = ("d3d12.dll", "d3d12core.dll")
D3D12_OVERRIDE = "d3d12,d3d12core=n,b"
# Wine's placeholder DLLs in system32 carry this marker after the DOS header.
WINE_BUILTIN_MARKER = b"Wine builtin DLL"


class RunnerError(Exception):
    pass


def merge_dll_overrides(existing: str | None, add: str) -> str:
    """``add`` plus whatever ``existing`` sets for other DLLs (ours win)."""
    ours = {dll.strip().lower() for dll in add.split("=", 1)[0].split(",")}
    kept = []
    for part in (existing or "").split(";"):
        part = part.strip()
        if not part:
            continue
        dlls = [d.strip() for d in part.split("=", 1)[0].split(",")]
        rest = [d for d in dlls if d.lower() not in ours]
        if rest:
            mode = part.split("=", 1)[1] if "=" in part else ""
            kept.append(",".join(rest) + (f"={mode}" if "=" in part else ""))
    return ";".join([add, *kept])


def is_wine_placeholder(dll: Path) -> bool:
    try:
        with open(dll, "rb") as f:
            return WINE_BUILTIN_MARKER in f.read(0x200)
    except OSError:
        return False


def vkd3d_x64_dir(root: Path) -> Path | None:
    """The folder holding vkd3d-proton's 64-bit DLLs in ``root``.

    ``root`` may be an unpacked vkd3d-proton release (``x64/``) or a Proton
    folder (``files/lib/wine/vkd3d-proton/x86_64-windows`` or
    ``files/lib64/wine/vkd3d-proton``)."""
    root = Path(root).expanduser()
    for cand in (root / "x64", root):
        if all((cand / d).is_file() for d in VKD3D_DLLS):
            return cand
    if not root.is_dir():
        return None
    for core in sorted(root.rglob("d3d12core.dll")):
        parts = {p.lower() for p in core.parent.parts}
        is_64 = "x86_64-windows" in parts or "lib64" in parts or "x64" in parts
        if is_64 and any("vkd3d" in p for p in parts) and (core.parent / "d3d12.dll").is_file():
            return core.parent
    return None


# -- discovery -------------------------------------------------------------------------------


def steam_roots(home: Path | None = None) -> list[Path]:
    home = home or Path.home()
    cands = [
        home / ".steam/root",
        home / ".steam/steam",
        home / ".local/share/Steam",
        home / ".var/app/com.valvesoftware.Steam/data/Steam",
    ]
    seen, out = set(), []
    for c in cands:
        if c.is_dir():
            r = c.resolve()
            if r not in seen:
                seen.add(r)
                out.append(c)
    return out


def find_protons(home: Path | None = None) -> list[Path]:
    """Proton folders (each has a ``proton`` script), best guess first:
    GE-Proton newest first, then Proton Experimental, then other Protons."""
    dirs = []
    for root in steam_roots(home):
        dirs += [root / "compatibilitytools.d", root / "steamapps/common"]
    dirs.append(Path("/usr/share/steam/compatibilitytools.d"))
    seen, found = set(), []
    for d in dirs:
        if not d.is_dir():
            continue
        for entry in d.iterdir():
            if (entry / "proton").is_file():
                r = entry.resolve()
                if r not in seen:
                    seen.add(r)
                    found.append(entry)

    def rank(p: Path):
        name = p.name
        if name.startswith("GE-Proton"):
            nums = [int(x) for x in name[len("GE-Proton"):].replace("-", ".").split(".") if x.isdigit()]
            return (0, [-n for n in nums], name)
        if "Experimental" in name:
            return (1, [], name)
        return (2, [], name)

    return sorted(found, key=rank)


def proton_version(proton_dir: Path) -> str:
    v = proton_dir / "version"
    if v.is_file():
        text = v.read_text(errors="replace").split()
        return text[-1] if text else proton_dir.name
    return proton_dir.name


# -- runners ---------------------------------------------------------------------------------


@dataclass
class WineRunner:
    wine: Path
    prefix: Path
    vkd3d_dir: Path | None = None
    name: str = "wine"

    def wineprefix(self) -> Path:
        return self.prefix

    def env(self, base: dict[str, str]) -> dict[str, str]:
        return {
            "WINEPREFIX": str(self.prefix),
            "WINEARCH": "win64",
            "WINEDLLOVERRIDES": merge_dll_overrides(base.get("WINEDLLOVERRIDES"), D3D12_OVERRIDE),
            "WINEDEBUG": base.get("WINEDEBUG", "-all"),
        }

    def argv(self, exe: Path, args: list[str]) -> list[str]:
        return [str(self.wine), str(exe), *args]

    def system32(self) -> Path:
        return self.prefix / "drive_c/windows/system32"

    def setup(self, log=print) -> None:
        if not (self.prefix / "system.reg").is_file():
            log(f"creating the Wine prefix {self.prefix} (wineboot)")
            self.prefix.mkdir(parents=True, exist_ok=True)
            env = {**os.environ, **self.env(dict(os.environ))}
            subprocess.run([str(self.wine), "wineboot", "-u"], env=env, check=True)
            server = self.wine.with_name("wineserver")
            subprocess.run([str(server) if server.exists() else "wineserver", "-w"], env=env, check=False)
        if self.vkd3d_dir is None:
            raise RunnerError("the wine runner needs vkd3d-proton: set [runner] vkd3d_dir (an unpacked vkd3d-proton release)")
        src = vkd3d_x64_dir(self.vkd3d_dir)
        if src is None:
            raise RunnerError(f"no 64-bit vkd3d-proton DLLs (x64/d3d12.dll, x64/d3d12core.dll) in {self.vkd3d_dir}")
        dst = self.system32()
        dst.mkdir(parents=True, exist_ok=True)
        for dll in VKD3D_DLLS:
            log(f"installing vkd3d-proton {dll} into {dst}")
            target = dst / dll
            if target.is_symlink() or target.exists():
                target.unlink()
            shutil.copy2(src / dll, target)

    def vkd3d_installed(self) -> bool:
        return all((self.system32() / d).is_file() and not is_wine_placeholder(self.system32() / d) for d in VKD3D_DLLS)

    def version(self) -> str:
        try:
            out = subprocess.run([str(self.wine), "--version"], capture_output=True, text=True, timeout=30)
            return out.stdout.strip() or out.stderr.strip()
        except (OSError, subprocess.TimeoutExpired) as e:
            return f"unknown ({e})"


@dataclass
class ProtonRunner:
    proton_dir: Path
    compat_dir: Path
    steam_dir: Path | None = None
    name: str = "proton"

    def wineprefix(self) -> Path:
        return self.compat_dir / "pfx"

    def env(self, base: dict[str, str]) -> dict[str, str]:
        steam = self.steam_dir or next(iter(steam_roots()), None) or self.compat_dir
        return {
            "STEAM_COMPAT_DATA_PATH": str(self.compat_dir),
            "STEAM_COMPAT_CLIENT_INSTALL_PATH": str(steam),
        }

    def argv(self, exe: Path, args: list[str]) -> list[str]:
        return [str(self.proton_dir / "proton"), "waitforexitandrun", str(exe), *args]

    def setup(self, log=print) -> None:
        self.compat_dir.mkdir(parents=True, exist_ok=True)
        log(f"Proton prefix folder: {self.compat_dir} (Proton fills it on the first run)")

    def vkd3d_installed(self) -> bool:
        return vkd3d_x64_dir(self.proton_dir) is not None

    def version(self) -> str:
        return proton_version(self.proton_dir)


def make_runner(cfg) -> WineRunner | ProtonRunner:
    if cfg.runner == "wine":
        wine = cfg.runner_path or (Path(w) if (w := shutil.which("wine")) else None)
        if wine is None:
            raise RunnerError("no wine found: install Wine (9.0 or newer) or set [runner] path")
        return WineRunner(wine=wine, prefix=cfg.resolved_prefix(), vkd3d_dir=cfg.vkd3d_dir)
    proton = cfg.runner_path or next(iter(find_protons()), None)
    if proton is None:
        raise RunnerError(
            "no Proton found: install GE-Proton or Proton Experimental through Steam, "
            "or set [runner] path to a Proton folder"
        )
    if not (proton / "proton").is_file():
        raise RunnerError(f"{proton} is not a Proton folder (no `proton` script in it)")
    return ProtonRunner(proton_dir=proton, compat_dir=cfg.resolved_prefix())

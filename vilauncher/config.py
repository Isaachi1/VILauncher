"""The launcher's configuration file (``~/.config/vilauncher/config.toml``)."""

from __future__ import annotations

import json
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from . import paths

BACKENDS = ("victory", "xcl")
RUNNERS = ("proton", "wine")
CONSOLES = ("series-x", "series-s")


class ConfigError(Exception):
    pass


def _opt_path(value: str | None) -> Path | None:
    return Path(value).expanduser() if value else None


@dataclass
class Config:
    # [game]
    game_dir: Path | None = None
    game_exe: str | None = None
    game_args: list[str] = field(default_factory=list)
    # [backend]
    backend: str = "victory"
    backend_exe: Path | None = None
    console: str = "series-x"
    unimplemented: str = "trap"
    dxc_dir: Path | None = None
    opus_dir: Path | None = None
    # [runner]
    runner: str = "proton"
    runner_path: Path | None = None
    prefix: Path | None = None
    vkd3d_dir: Path | None = None
    # [dirs]
    data_dir: Path | None = None
    cache_dir: Path | None = None
    # [env]
    env: dict[str, str] = field(default_factory=dict)

    # -- derived -------------------------------------------------------------

    def resolved_prefix(self) -> Path:
        if self.prefix:
            return self.prefix
        return paths.data_dir() / ("compatdata" if self.runner == "proton" else "prefix")

    def resolved_data_dir(self) -> Path:
        return self.data_dir or paths.data_dir() / self.backend / "data"

    def resolved_cache_dir(self) -> Path:
        return self.cache_dir or paths.cache_dir() / self.backend

    def validate(self) -> None:
        if self.backend not in BACKENDS:
            raise ConfigError(f"[backend] kind must be one of {', '.join(BACKENDS)}, not {self.backend!r}")
        if self.runner not in RUNNERS:
            raise ConfigError(f"[runner] kind must be one of {', '.join(RUNNERS)}, not {self.runner!r}")
        if self.console not in CONSOLES:
            raise ConfigError(f"[backend] console must be one of {', '.join(CONSOLES)}, not {self.console!r}")
        if self.unimplemented not in ("trap", "zero"):
            raise ConfigError(f"[backend] unimplemented must be trap or zero, not {self.unimplemented!r}")
        if not all(isinstance(a, str) for a in self.game_args):
            raise ConfigError("[game] args must be a list of strings")

    # -- file ----------------------------------------------------------------

    @classmethod
    def from_toml(cls, text: str) -> "Config":
        try:
            doc = tomllib.loads(text)
        except tomllib.TOMLDecodeError as e:
            raise ConfigError(f"config is not valid TOML: {e}") from e
        g, b, r, d = (doc.get(k, {}) for k in ("game", "backend", "runner", "dirs"))
        cfg = cls(
            game_dir=_opt_path(g.get("dir")),
            game_exe=g.get("exe") or None,
            game_args=list(g.get("args", [])),
            backend=b.get("kind", "victory"),
            backend_exe=_opt_path(b.get("exe")),
            console=b.get("console", "series-x"),
            unimplemented=b.get("unimplemented", "trap"),
            dxc_dir=_opt_path(b.get("dxc_dir")),
            opus_dir=_opt_path(b.get("opus_dir")),
            runner=r.get("kind", "proton"),
            runner_path=_opt_path(r.get("path")),
            prefix=_opt_path(r.get("prefix")),
            vkd3d_dir=_opt_path(r.get("vkd3d_dir")),
            data_dir=_opt_path(d.get("data")),
            cache_dir=_opt_path(d.get("cache")),
            env={str(k): str(v) for k, v in doc.get("env", {}).items()},
        )
        cfg.validate()
        return cfg

    def to_toml(self) -> str:
        def s(v) -> str:
            # JSON string escapes are valid TOML basic-string escapes.
            return json.dumps(str(v), ensure_ascii=False)

        def opt(key: str, v) -> list[str]:
            return [f"{key} = {s(v)}"] if v else []

        lines = [
            "# VILauncher configuration. `vilauncher setup` writes it; edit freely.",
            "",
            "[game]",
            "# The unpacked folder of a game you own (the folder with MicrosoftGame.config).",
            *opt("dir", self.game_dir),
            *opt("exe", self.game_exe),
            f"args = [{', '.join(s(a) for a in self.game_args)}]",
            "",
            "[backend]",
            "# victory = VILinux (VIctory), xcl = LinuxCL (XCL). exe is its Windows build (victory.exe / xcl.exe).",
            f"kind = {s(self.backend)}",
            *opt("exe", self.backend_exe),
            f"console = {s(self.console)}",
            f"unimplemented = {s(self.unimplemented)}",
            *opt("dxc_dir", self.dxc_dir),
            *opt("opus_dir", self.opus_dir),
            "",
            "[runner]",
            "# proton = a Proton / GE-Proton folder (has vkd3d-proton), wine = a wine binary plus vkd3d_dir.",
            f"kind = {s(self.runner)}",
            *opt("path", self.runner_path),
            *opt("prefix", self.prefix),
            *opt("vkd3d_dir", self.vkd3d_dir),
            "",
            "[dirs]",
            *opt("data", self.data_dir),
            *opt("cache", self.cache_dir),
            "",
            "[env]",
            *[f"{k} = {s(v)}" for k, v in sorted(self.env.items())],
            "",
        ]
        return "\n".join(lines)


def load(path: Path | None = None) -> Config:
    path = path or paths.config_file()
    if not path.is_file():
        return Config()
    return Config.from_toml(path.read_text(encoding="utf-8"))


def save(cfg: Config, path: Path | None = None) -> Path:
    cfg.validate()
    path = path or paths.config_file()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(cfg.to_toml(), encoding="utf-8")
    return path

"""``vilauncher``: the command line."""

from __future__ import annotations

import argparse
import os
import re
import shlex
import subprocess
import sys
from datetime import datetime
from pathlib import Path

from . import __version__, backends, config, doctor, gamedir, paths, runners


def build_launch(cfg) -> tuple[list[str], dict[str, str], Path]:
    """The command line, the environment additions and the working folder of a run."""
    if cfg.game_dir is None:
        raise gamedir.GameDirError("no [game] dir: run `vilauncher setup --game <folder>`")
    if cfg.backend_exe is None:
        raise runners.RunnerError("no [backend] exe: run `vilauncher setup --backend-exe <victory.exe | xcl.exe>`")
    info = gamedir.inspect(cfg.game_dir)
    game_exe = gamedir.resolve_exe(info, cfg.game_exe)
    runner = runners.make_runner(cfg)
    plan = backends.plan(cfg, game_exe)
    env = {**runner.env(dict(os.environ)), **plan.env, **cfg.env}
    argv = runner.argv(plan.exe, plan.args)
    return argv, env, cfg.resolved_data_dir()


def _print_checks(checks) -> int:
    width = max(len(c.name) for c in checks)
    for c in checks:
        print(f"[{c.status:>4}] {c.name:<{width}}  {c.detail}")
    fails = sum(c.status == doctor.FAIL for c in checks)
    warns = sum(c.status == doctor.WARN for c in checks)
    print(f"\n{fails} failed, {warns} warnings, {len(checks) - fails - warns} ok")
    return fails


# -- commands --------------------------------------------------------------------------------


def cmd_setup(a) -> int:
    path = paths.config_file()
    cfg = config.load(path)
    if a.game:
        cfg.game_dir = Path(a.game).expanduser().absolute()
    if a.exe is not None:
        cfg.game_exe = a.exe or None
    if a.backend_exe:
        cfg.backend_exe = Path(a.backend_exe).expanduser().absolute()
        cfg.backend = a.backend or backends.guess_backend(cfg.backend_exe) or cfg.backend
    elif a.backend:
        cfg.backend = a.backend
    if a.runner:
        cfg.runner = a.runner
    if a.runner_path:
        cfg.runner_path = Path(a.runner_path).expanduser().absolute()
    if a.prefix:
        cfg.prefix = Path(a.prefix).expanduser().absolute()
    if a.vkd3d:
        cfg.vkd3d_dir = Path(a.vkd3d).expanduser().absolute()
    if a.dxc_dir:
        cfg.dxc_dir = Path(a.dxc_dir).expanduser().absolute()
    if a.opus_dir:
        cfg.opus_dir = Path(a.opus_dir).expanduser().absolute()
    if a.console:
        cfg.console = a.console
    if a.args is not None:
        cfg.game_args = shlex.split(a.args)
    if cfg.runner == "proton" and cfg.runner_path is None:
        found = runners.find_protons()
        if found:
            cfg.runner_path = found[0]
            print(f"using Proton {found[0]}")
    config.save(cfg, path)
    print(f"configuration written to {path}")
    if not a.no_prefix:
        try:
            runners.make_runner(cfg).setup()
        except (runners.RunnerError, subprocess.CalledProcessError) as e:
            print(f"error: {e}", file=sys.stderr)
            return 1
    print("next: `vilauncher doctor`, then `vilauncher run`")
    return 0


def cmd_doctor(a) -> int:
    cfg = config.load()
    return 1 if _print_checks(doctor.run_all(cfg, machine=not a.config_only)) else 0


def cmd_info(a) -> int:
    cfg = config.load()
    folder = Path(a.folder).expanduser() if a.folder else cfg.game_dir
    if folder is None:
        print("error: no game folder: pass one or run `vilauncher setup --game <folder>`", file=sys.stderr)
        return 1
    try:
        info = gamedir.inspect(folder)
        exe = gamedir.resolve_exe(info, cfg.game_exe if not a.folder else None)
    except gamedir.GameDirError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    print(f"folder:      {info.folder}")
    print(f"config:      {info.config or '-'}")
    print(f"title:       {info.display_name or '-'}")
    print(f"identity:    {info.identity or '-'} {info.version or ''}".rstrip())
    print(f"executables: {', '.join(info.executables) or '-'}")
    print(f"runs:        {exe}")
    return 0


def cmd_run(a) -> int:
    cfg = config.load()
    if a.args:
        cfg.game_args = a.args[1:] if a.args[0] == "--" else a.args
    if not a.force:
        fails = [c for c in doctor.run_all(cfg, machine=False) if c.status == doctor.FAIL]
        if fails:
            for c in fails:
                print(f"error: {c.name}: {c.detail}", file=sys.stderr)
            print("(`vilauncher doctor` shows every check; --force runs anyway)", file=sys.stderr)
            return 1
    try:
        argv, env, cwd = build_launch(cfg)
    except (gamedir.GameDirError, runners.RunnerError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    if a.dry_run:
        for k, v in sorted(env.items()):
            print(f"{k}={shlex.quote(v)}")
        print(f"cd {shlex.quote(str(cwd))}")
        print(shlex.join(argv))
        return 0
    for d in (cwd, cfg.resolved_cache_dir()):
        d.mkdir(parents=True, exist_ok=True)
    log = None
    if not sys.stdout.isatty():
        # Started from a desktop entry: keep the output somewhere it can be read.
        paths.state_dir().mkdir(parents=True, exist_ok=True)
        log_path = paths.state_dir() / "last-run.log"
        log = open(log_path, "w", encoding="utf-8")
        log.write(f"# {datetime.now().isoformat(timespec='seconds')} {shlex.join(argv)}\n")
        log.flush()
    try:
        proc = subprocess.run(argv, env={**os.environ, **env}, cwd=cwd, stdout=log, stderr=log)
    finally:
        if log:
            log.close()
    return proc.returncode


def cmd_protons(a) -> int:
    found = runners.find_protons()
    for p in found:
        print(f"{runners.proton_version(p):<24} {p}")
    if not found:
        print("no Proton found (install GE-Proton or Proton Experimental through Steam)")
    return 0


def cmd_config(a) -> int:
    path = paths.config_file()
    print(f"# {path}")
    print(config.load(path).to_toml())
    return 0


def desktop_entry(cfg, exe: str) -> str:
    name = "VILauncher"
    if cfg.game_dir:
        try:
            info = gamedir.inspect(cfg.game_dir)
            name = info.display_name or info.identity or name
        except gamedir.GameDirError:
            pass
    return "\n".join([
        "[Desktop Entry]",
        "Type=Application",
        f"Name={name} (VILauncher)",
        f"Comment=Runs your copy through {cfg.backend} on Wine/Proton",
        f"Exec={exe} run",
        "Terminal=false",
        "Categories=Game;",
        "",
    ])


def _desktop_quote(arg: str) -> str:
    """An Exec= argument (desktop entry spec: double quotes, backslash escapes)."""
    if not any(c in arg for c in ' \t\n"\'\\><~|&;$*?#()`'):
        return arg
    quoted = '"' + re.sub(r'(["`$\\])', r'\\\1', arg) + '"'
    # Exec is also a string value, whose own escaping doubles each backslash.
    return quoted.replace("\\", "\\\\")


def cmd_desktop(a) -> int:
    cfg = config.load()
    target = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "applications/vilauncher.desktop"
    target.parent.mkdir(parents=True, exist_ok=True)
    exe = _desktop_quote(sys.argv[0]) if Path(sys.argv[0]).name == "vilauncher" else f"{_desktop_quote(sys.executable)} -m vilauncher"
    target.write_text(desktop_entry(cfg, exe), encoding="utf-8")
    print(f"wrote {target} (the output of a run goes to {paths.state_dir() / 'last-run.log'})")
    return 0


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="vilauncher",
        description="Runs an Xbox Series title you own on Linux through VIctory (VILinux) or XCL (LinuxCL), "
        "on Wine or Proton with vkd3d-proton.",
    )
    p.add_argument("--version", action="version", version=f"vilauncher {__version__}")
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("setup", help="write the configuration and prepare the Wine/Proton prefix")
    s.add_argument("--game", help="the unpacked game folder (with MicrosoftGame.config)")
    s.add_argument("--exe", help="the title's executable, relative to the game folder (default: from MicrosoftGame.config)")
    s.add_argument("--args", help="arguments for the title, as one shell-quoted string")
    s.add_argument("--backend", choices=config.BACKENDS, help="victory (VILinux) or xcl (LinuxCL)")
    s.add_argument("--backend-exe", help="victory.exe or xcl.exe (the Windows build)")
    s.add_argument("--console", choices=config.CONSOLES, help="victory only: the console model the title sees")
    s.add_argument("--runner", choices=config.RUNNERS)
    s.add_argument("--runner-path", help="a Proton folder, or the wine binary")
    s.add_argument("--prefix", help="where the prefix lives")
    s.add_argument("--vkd3d", help="wine runner: an unpacked vkd3d-proton release")
    s.add_argument("--dxc-dir", help="xcl only: DXC's bin/x64 (dxcompiler.dll, dxil.dll)")
    s.add_argument("--opus-dir", help="xcl only: a folder with opus.dll")
    s.add_argument("--no-prefix", action="store_true", help="only write the configuration")
    s.set_defaults(func=cmd_setup)

    d = sub.add_parser("doctor", help="check the machine and the configuration")
    d.add_argument("--config-only", action="store_true", help="skip the machine checks")
    d.set_defaults(func=cmd_doctor)

    r = sub.add_parser("run", help="run the game")
    r.add_argument("--dry-run", action="store_true", help="print the environment and command instead of running")
    r.add_argument("--force", action="store_true", help="run even when a configuration check fails")
    r.add_argument("args", nargs=argparse.REMAINDER, help="-- then arguments for the title")
    r.set_defaults(func=cmd_run)

    i = sub.add_parser("info", help="what MicrosoftGame.config says about a game folder")
    i.add_argument("folder", nargs="?")
    i.set_defaults(func=cmd_info)

    sub.add_parser("protons", help="list the Proton installs found").set_defaults(func=cmd_protons)
    sub.add_parser("config", help="print the configuration").set_defaults(func=cmd_config)
    sub.add_parser("desktop", help="add a menu entry that runs the game").set_defaults(func=cmd_desktop)
    return p


def main(argv: list[str] | None = None) -> int:
    a = parser().parse_args(argv)
    try:
        return a.func(a)
    except config.ConfigError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1

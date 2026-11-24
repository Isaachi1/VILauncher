import contextlib
import io
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tests import helpers
from vilauncher import backends, cli, config, doctor, gamedir, paths, runners


class TempHome(unittest.TestCase):
    """Every test gets its own HOME and XDG folders."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        env = {
            "HOME": str(self.root / "home"),
            "XDG_CONFIG_HOME": str(self.root / "home/.config"),
            "XDG_DATA_HOME": str(self.root / "home/.local/share"),
            "XDG_CACHE_HOME": str(self.root / "home/.cache"),
            "XDG_STATE_HOME": str(self.root / "home/.local/state"),
        }
        (self.root / "home").mkdir()
        self._env = mock.patch.dict(os.environ, env)
        self._env.start()
        os.environ.pop("VILAUNCHER_CONFIG", None)
        os.environ.pop("PROTON_USE_WINED3D", None)

    def tearDown(self):
        self._env.stop()
        self._tmp.cleanup()

    def cli(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(list(argv))
        return code, out.getvalue(), err.getvalue()


class PathsTest(unittest.TestCase):
    def test_to_windows_maps_through_drive_z(self):
        self.assertEqual(paths.to_windows("/home/me/My Game/x.exe"), r"Z:\home\me\My Game\x.exe")
        self.assertEqual(paths.to_windows("/a/b/../c"), r"Z:\a\c")

    def test_victory_tree_is_found_from_below(self):
        with tempfile.TemporaryDirectory() as t:
            tree = Path(t) / "VILinux"
            (tree / "sub").mkdir(parents=True)
            (tree / ".victory-root").write_text("marker")
            self.assertEqual(paths.inside_victory_tree(tree / "sub" / "cache"), tree)
            self.assertIsNone(paths.inside_victory_tree(Path(t) / "elsewhere"))


class GameDirTest(unittest.TestCase):
    def test_config_names_the_executable(self):
        with tempfile.TemporaryDirectory() as t:
            game = helpers.fake_game(Path(t))
            info = gamedir.inspect(game)
            self.assertEqual(info.identity, "Example.TestTitle")
            self.assertEqual(info.version, "1.2.3.0")
            self.assertEqual(info.display_name, "Test Title")
            self.assertEqual(gamedir.resolve_exe(info), game / "bin/title.exe")

    def test_config_name_is_case_insensitive(self):
        with tempfile.TemporaryDirectory() as t:
            game = helpers.fake_game(Path(t))
            (game / "MicrosoftGame.config").rename(game / "microsoftgame.CONFIG")
            self.assertEqual(gamedir.resolve_exe(gamedir.inspect(game)), game / "bin/title.exe")

    def test_namespaced_config(self):
        with tempfile.TemporaryDirectory() as t:
            game = helpers.fake_game(Path(t))
            text = helpers.GAME_CONFIG.replace('<Game configVersion="1">', '<Game xmlns="urn:example" configVersion="1">')
            (game / "MicrosoftGame.config").write_text(text)
            self.assertEqual(gamedir.inspect(game).executables, ["bin\\title.exe"])

    def test_packaged_title_is_refused(self):
        with tempfile.TemporaryDirectory() as t:
            (Path(t) / "title.msixvc").write_bytes(b"\0" * 16)
            with self.assertRaisesRegex(gamedir.GameDirError, "do not unpack or decrypt"):
                gamedir.inspect(Path(t))

    def test_missing_executable_is_reported(self):
        with tempfile.TemporaryDirectory() as t:
            game = helpers.fake_game(Path(t))
            (game / "bin/title.exe").unlink()
            with self.assertRaisesRegex(gamedir.GameDirError, "none of the executables"):
                gamedir.resolve_exe(gamedir.inspect(game))

    def test_override_executable(self):
        with tempfile.TemporaryDirectory() as t:
            game = helpers.fake_game(Path(t))
            helpers.fake_pe(game / "other.exe")
            self.assertEqual(gamedir.resolve_exe(gamedir.inspect(game), "other.exe"), game / "other.exe")


class ConfigTest(TempHome):
    def test_round_trip(self):
        cfg = config.Config(
            game_dir=Path("/g/My \"Game\""), game_args=["-a", "b c"], backend="xcl",
            backend_exe=Path("/b/xcl.exe"), runner="wine", vkd3d_dir=Path("/v"),
            dxc_dir=Path("/dxc"), env={"VICTORY_LOG": "info", "DXVK_HUD": "fps"},
        )
        back = config.Config.from_toml(cfg.to_toml())
        self.assertEqual(back, cfg)

    def test_bad_values_are_refused(self):
        with self.assertRaises(config.ConfigError):
            config.Config.from_toml('[backend]\nkind = "rpcs3"\n')
        with self.assertRaises(config.ConfigError):
            config.Config.from_toml("[game\n")

    def test_missing_file_gives_defaults(self):
        cfg = config.load()
        self.assertEqual((cfg.backend, cfg.runner), ("victory", "proton"))
        self.assertEqual(cfg.resolved_data_dir(), self.root / "home/.local/share/vilauncher/victory/data")


class RunnerTest(TempHome):
    def test_dll_overrides_keep_other_dlls(self):
        self.assertEqual(runners.merge_dll_overrides(None, runners.D3D12_OVERRIDE), "d3d12,d3d12core=n,b")
        self.assertEqual(
            runners.merge_dll_overrides("dxgi=n;d3d12=b;xinput1_3,d3d12core=n", runners.D3D12_OVERRIDE),
            "d3d12,d3d12core=n,b;dxgi=n;xinput1_3=n",
        )

    def test_vkd3d_found_in_release_and_in_proton(self):
        release = self.root / "vkd3d-proton-2.14"
        for dll in runners.VKD3D_DLLS:
            helpers.fake_pe(release / "x64" / dll)
            helpers.fake_pe(release / "x86" / dll, machine=0x14C)
        self.assertEqual(runners.vkd3d_x64_dir(release), release / "x64")
        proton = helpers.fake_proton(self.root)
        self.assertEqual(runners.vkd3d_x64_dir(proton), proton / "files/lib/wine/vkd3d-proton/x86_64-windows")
        self.assertIsNone(runners.vkd3d_x64_dir(self.root / "nothing"))

    def test_proton_discovery_prefers_newest_ge(self):
        tools = self.root / "home/.local/share/Steam/compatibilitytools.d"
        common = self.root / "home/.local/share/Steam/steamapps/common"
        for name in ("GE-Proton9-2", "GE-Proton10-1", "GE-Proton9-20"):
            helpers.fake_proton(tools, name)
        helpers.fake_proton(common, "Proton - Experimental")
        (common / "Some Game").mkdir()
        found = [p.name for p in runners.find_protons(self.root / "home")]
        self.assertEqual(found, ["GE-Proton10-1", "GE-Proton9-20", "GE-Proton9-2", "Proton - Experimental"])

    def test_wine_setup_installs_vkd3d_over_placeholders(self):
        prefix = helpers.fake_wine_prefix(self.root / "pfx")
        sys32 = prefix / "drive_c/windows/system32"
        for dll in runners.VKD3D_DLLS:
            (sys32 / dll).write_bytes(b"MZ" + b"\0" * 0x3E + runners.WINE_BUILTIN_MARKER)
        release = self.root / "vkd3d"
        for dll in runners.VKD3D_DLLS:
            helpers.fake_pe(release / "x64" / dll)
        r = runners.WineRunner(wine=Path("/nonexistent/wine"), prefix=prefix, vkd3d_dir=release)
        self.assertFalse(r.vkd3d_installed())
        r.setup(log=lambda *_: None)  # the prefix exists, so wine itself is not started
        self.assertTrue(r.vkd3d_installed())
        self.assertEqual((sys32 / "d3d12core.dll").read_bytes(), (release / "x64/d3d12core.dll").read_bytes())

    def test_wine_setup_without_vkd3d_fails(self):
        prefix = helpers.fake_wine_prefix(self.root / "pfx")
        r = runners.WineRunner(wine=Path("/nonexistent/wine"), prefix=prefix)
        with self.assertRaisesRegex(runners.RunnerError, "vkd3d"):
            r.setup(log=lambda *_: None)


class BackendTest(TempHome):
    def cfg(self, **kw):
        game = helpers.fake_game(self.root)
        return config.Config(game_dir=game, backend_exe=self.root / "b/victory.exe", **kw), game

    def test_victory_command_line(self):
        cfg, game = self.cfg(console="series-s", game_args=["-x"])
        plan = backends.plan(cfg, game / "bin/title.exe")
        self.assertEqual(
            plan.args,
            ["run", paths.to_windows(game / "bin/title.exe"), "--execute", "--unimplemented", "trap",
             "--console", "series-s", "--", "-x"],
        )
        self.assertEqual(plan.env["VICTORY_GAME_DIR"], paths.to_windows(game))
        self.assertTrue(plan.env["VICTORY_DATA_DIR"].startswith("Z:\\"))
        self.assertTrue(plan.env["VICTORY_CACHE_DIR"].endswith("\\vilauncher\\victory"))

    def test_xcl_command_line(self):
        cfg, game = self.cfg(backend="xcl", dxc_dir=Path("/opt/dxc/bin/x64"))
        plan = backends.plan(cfg, game / "bin/title.exe")
        self.assertEqual(plan.args, ["run", paths.to_windows(game)])
        self.assertEqual(plan.env["XCL_DXC_DIR"], r"Z:\opt\dxc\bin\x64")
        self.assertNotIn("XCL_OPUS_DIR", plan.env)

    def test_guess_backend_and_pe(self):
        self.assertEqual(backends.guess_backend(Path("/x/victory.exe")), "victory")
        self.assertEqual(backends.guess_backend(Path("/x/XCL.EXE")), "xcl")
        self.assertIsNone(backends.guess_backend(Path("/x/game.exe")))
        self.assertEqual(backends.pe_machine(helpers.fake_pe(self.root / "a.exe")), 0x8664)
        (self.root / "b.exe").write_text("#!/bin/sh\n")
        self.assertIsNone(backends.pe_machine(self.root / "b.exe"))


class DoctorTest(TempHome):
    def test_machine_checks(self):
        self.assertEqual(doctor.check_avx2("flags\t: fpu sse avx avx2 bmi2\n").status, doctor.OK)
        self.assertEqual(doctor.check_avx2("flags\t: fpu sse avx\n").status, doctor.FAIL)
        mem = "MemTotal:       16000000 kB\nSwapTotal:       2000000 kB\n"
        self.assertEqual(doctor.check_memory(mem).status, doctor.WARN)
        mem = "MemTotal:       32000000 kB\nSwapTotal:       8000000 kB\n"
        self.assertEqual(doctor.check_memory(mem).status, doctor.OK)
        self.assertEqual(doctor.check_max_map_count("65530\n").status, doctor.WARN)
        self.assertEqual(doctor.check_max_map_count("2147483642\n").status, doctor.OK)
        self.assertEqual(doctor.check_userfaultfd("0\n", dev_ok=False).status, doctor.WARN)
        self.assertEqual(doctor.check_userfaultfd("0\n", dev_ok=True).status, doctor.OK)
        self.assertEqual(doctor.check_userfaultfd("1\n", dev_ok=False).status, doctor.OK)

    def test_vulkan_check_reads_vulkaninfo(self):
        out = "GPU0:\n\tdeviceName        = AMD Radeon RX 6700 XT (RADV NAVI22)\n\tVK_EXT_mesh_shader : extension revision 1\n"
        run = lambda *a, **k: mock.Mock(stdout=out)
        with mock.patch("shutil.which", return_value="/usr/bin/vulkaninfo"):
            checks = doctor.check_vulkan(run=run)
        self.assertEqual([c.status for c in checks], [doctor.OK, doctor.OK])
        with mock.patch("shutil.which", return_value="/usr/bin/vulkaninfo"):
            checks = doctor.check_vulkan(run=lambda *a, **k: mock.Mock(stdout=out.replace("VK_EXT_mesh_shader", "")))
        self.assertEqual(checks[1].status, doctor.WARN)

    def test_data_inside_victory_tree_fails(self):
        tree = self.root / "VILinux"
        tree.mkdir()
        (tree / ".victory-root").write_text("marker")
        exe = helpers.fake_pe(self.root / "b/victory.exe")
        cfg = config.Config(backend_exe=exe, data_dir=tree / "data")
        fails = [c for c in doctor.check_backend(cfg) if c.status == doctor.FAIL]
        self.assertEqual([c.name for c in fails], ["data folder"])

    def test_xcl_needs_dxc(self):
        exe = helpers.fake_pe(self.root / "b/xcl.exe")
        cfg = config.Config(backend="xcl", backend_exe=exe)
        by_name = {c.name: c.status for c in doctor.check_backend(cfg)}
        self.assertEqual(by_name["DXC"], doctor.FAIL)
        self.assertEqual(by_name["Opus"], doctor.WARN)
        for dll in ("dxcompiler.dll", "dxil.dll", "opus.dll"):
            helpers.fake_pe(exe.parent / dll)
        by_name = {c.name: c.status for c in doctor.check_backend(cfg)}
        self.assertEqual((by_name["DXC"], by_name["Opus"]), (doctor.OK, doctor.OK))

    def test_wine_prefix_checks(self):
        prefix = helpers.fake_wine_prefix(self.root / "pfx")
        wine = helpers.executable(self.root / "bin/wine", "#!/bin/sh\necho wine-9.22\n")
        cfg = config.Config(runner="wine", runner_path=wine, prefix=prefix)
        by_name = {c.name: (c.status, c.detail) for c in doctor.check_runner(cfg)}
        self.assertEqual(by_name["runner"], (doctor.OK, "wine wine-9.22"))
        self.assertEqual(by_name["vkd3d-proton"][0], doctor.FAIL)
        self.assertEqual(by_name["prefix"][0], doctor.OK)
        (prefix / "dosdevices/z:").unlink()
        os.symlink("/tmp", prefix / "dosdevices/z:")
        by_name = {c.name: c.status for c in doctor.check_runner(cfg)}
        self.assertEqual(by_name["prefix"], doctor.FAIL)

    def test_proton_with_wined3d_fails(self):
        proton = helpers.fake_proton(self.root)
        cfg = config.Config(runner_path=proton)
        with mock.patch.dict(os.environ, {"PROTON_USE_WINED3D": "1"}):
            by_name = {c.name: c.status for c in doctor.check_runner(cfg)}
        self.assertEqual(by_name["vkd3d-proton"], doctor.FAIL)


class CliTest(TempHome):
    def setup_victory(self):
        game = helpers.fake_game(self.root)
        exe = helpers.fake_pe(self.root / "VIctory build/victory.exe")
        proton = helpers.fake_proton(self.root / "home/.local/share/Steam/compatibilitytools.d")
        code, out, err = self.cli("setup", "--game", str(game), "--backend-exe", str(exe), "--args", "-windowed 'a b'")
        self.assertEqual(code, 0, err)
        return game, exe, proton

    def test_setup_detects_backend_and_proton(self):
        game, exe, proton = self.setup_victory()
        cfg = config.load()
        self.assertEqual((cfg.backend, cfg.runner, cfg.runner_path), ("victory", "proton", proton))
        self.assertEqual(cfg.game_args, ["-windowed", "a b"])
        self.assertTrue(cfg.resolved_prefix().is_dir())

    def test_run_refuses_without_game(self):
        exe = helpers.fake_pe(self.root / "victory.exe")
        config.save(config.Config(backend_exe=exe, runner_path=helpers.fake_proton(self.root)))
        code, out, err = self.cli("run")
        self.assertEqual(code, 1)
        self.assertIn("no [game] dir", err)

    def test_dry_run_prints_the_launch(self):
        game, exe, proton = self.setup_victory()
        code, out, err = self.cli("run", "--dry-run")
        self.assertEqual(code, 0, err)
        self.assertIn(f"{proton}/proton waitforexitandrun", out)
        self.assertIn("--execute", out)
        self.assertIn("VICTORY_GAME_DIR=", out)

    def test_run_starts_proton_with_the_backend(self):
        game, exe, proton = self.setup_victory()
        rec = self.root / "rec"
        with mock.patch.dict(os.environ, {"VILAUNCHER_TEST_OUT": str(rec)}):
            code, out, err = self.cli("run", "--", "-extra")
        self.assertEqual(code, 7)  # the fake proton's exit code comes back
        argv = (rec.with_suffix(".argv")).read_text().splitlines()
        self.assertEqual(
            argv,
            ["waitforexitandrun", str(exe), "run", paths.to_windows(game / "bin/title.exe"), "--execute",
             "--unimplemented", "trap", "--", "-extra"],
        )
        env = dict(l.split("=", 1) for l in rec.with_suffix(".env").read_text().splitlines() if "=" in l)
        self.assertEqual(env["STEAM_COMPAT_DATA_PATH"], str(config.load().resolved_prefix()))
        self.assertEqual(env["VICTORY_GAME_DIR"], paths.to_windows(game))
        self.assertEqual(rec.with_suffix(".cwd").read_text().strip(), str(config.load().resolved_data_dir()))
        log = self.root / "home/.local/state/vilauncher/last-run.log"
        self.assertTrue(log.is_file())  # stdout is not a terminal under the test runner

    def test_wine_run_sets_overrides(self):
        game = helpers.fake_game(self.root)
        exe = helpers.fake_pe(self.root / "xcl/xcl.exe")
        for dll in ("dxcompiler.dll", "dxil.dll"):
            helpers.fake_pe(exe.parent / dll)
        prefix = helpers.fake_wine_prefix(self.root / "pfx")
        release = self.root / "vkd3d"
        for dll in runners.VKD3D_DLLS:
            helpers.fake_pe(release / "x64" / dll)
        wine = helpers.executable(self.root / "bin/wine", "#!/bin/sh\necho wine-9.22\n")
        code, out, err = self.cli(
            "setup", "--game", str(game), "--backend-exe", str(exe), "--runner", "wine",
            "--runner-path", str(wine), "--prefix", str(prefix), "--vkd3d", str(release),
        )
        self.assertEqual(code, 0, err)
        code, out, err = self.cli("run", "--dry-run")
        self.assertEqual(code, 0, err)
        self.assertIn("WINEDLLOVERRIDES=d3d12,d3d12core=n,b\n", out)
        self.assertIn(f"WINEPREFIX={prefix}", out)
        self.assertIn(f"{wine} {exe} run", out)

    def test_forced_run_without_game_is_an_error(self):
        config.save(config.Config(backend_exe=helpers.fake_pe(self.root / "victory.exe"), runner_path=helpers.fake_proton(self.root)))
        code, out, err = self.cli("run", "--force", "--dry-run")
        self.assertEqual(code, 1)
        self.assertIn("no [game] dir", err)

    def test_desktop_exec_quoting(self):
        self.assertEqual(cli._desktop_quote("/usr/bin/vilauncher"), "/usr/bin/vilauncher")
        self.assertEqual(cli._desktop_quote('/a b/$x"y'), r'"/a b/\\$x\\"y"')

    def test_info_and_desktop(self):
        game, exe, proton = self.setup_victory()
        code, out, err = self.cli("info")
        self.assertEqual(code, 0, err)
        self.assertIn("Test Title", out)
        code, out, err = self.cli("desktop")
        self.assertEqual(code, 0, err)
        entry = (self.root / "home/.local/share/applications/vilauncher.desktop").read_text()
        self.assertIn("Name=Test Title (VILauncher)", entry)
        self.assertIn(" run\n", entry)


if __name__ == "__main__":
    unittest.main()

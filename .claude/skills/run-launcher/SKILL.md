---
name: run-launcher
description: Build, run, and drive the SixLauncher window (Tauri) and its `sixlauncher` CLI. Use when asked to start or open the launcher, run its tests, build it, take a screenshot of its UI, click through its tabs, or wire it to a local ScarlettPort build.
---

SixLauncher is a Rust workspace (`crates/core`, `crates/cli`, `app` = Tauri 2 window, `ui` = plain
HTML/JS). Drive the window with `.claude/skills/run-launcher/driver.sh`: it runs the app in its own
Xvfb display, sends keys/clicks with `xdotool` and screenshots with ImageMagick. All paths are relative
to `launcher/`. Verified on CachyOS (Arch), rustc 1.100.1.

## Prerequisites

```bash
sudo pacman -S --needed rustup webkit2gtk-4.1 xorg-server-xvfb xdotool imagemagick
rustup default stable
```

(`gtk3`, `libsoup3`, `librsvg`, `base-devel` were already present.)

## Build

```bash
cargo build --workspace     # target/debug/sixlauncher-app (window) and target/debug/sixlauncher (CLI)
```

## Run (agent path)

```bash
D=.claude/skills/run-launcher/driver.sh
$D start                 # Xvfb on :97 + the window, SANDBOXED config (see below)
$D ss 01-start           # -> /tmp/sixlauncher-drive/shots/01-start.png  (Read it to look)
$D key ctrl+3            # Verificação tab (runs the checks)
$D click 314 29          # window-relative coords: the "Compatibilidade" tab at 1040x780
$D ss 02-compat
$D cli doctor --config-only   # the CLI, same XDG dirs as the window
$D stop
```

| command | what it does |
|---|---|
| `start [--real]` | Xvfb + window. Default: XDG dirs under `/tmp/sixlauncher-drive/home`. `--real`: your `~/.config/sixlauncher` |
| `ss [name]` | screenshot of the window, prints the path |
| `key <keys...>` | xdotool keys: `ctrl+1`..`ctrl+5` tabs (Jogo, Compatibilidade, Verificação, Log, Status), `ctrl+d` checks, `ctrl+Return` play |
| `click <x> <y>` | left click relative to the window |
| `type <text>` | type into the focused field |
| `scroll <n> [x y]` | mouse wheel, `n` notches down (negative: up); one notch is about 85 px |
| `cli <args...>` | `target/debug/sixlauncher` with the same XDG dirs |
| `log` / `stop` | app stdout+stderr / kill window and Xvfb |

The window is 1040x780. Tabs at y=29: Jogo x≈221, Compatibilidade x≈314, Verificação x≈433,
Log x≈512, Status x≈571 (they shift a few px as the selected pill changes; prefer `key ctrl+N`).

### Point it at a local ScarlettPort build

Build `scarlett.exe` with `../ScarlettPort/.claude/skills/run-scarlettport/sp.sh build`, then:

```bash
$D cli setup --backend scarlett --backend-exe ../ScarlettPort/engine/target/x86_64-pc-windows-msvc/release/scarlett.exe --no-prefix
$D stop; $D start; $D key ctrl+2; $D ss backend   # "Executável próprio (ScarlettPort)" is selected
```

`setup` stores the path absolute. `sixlauncher run --dry-run` prints the exact Proton command line
and environment a run would use, without starting anything.

The ScarlettPort settings (preset, player name, game window, features) are in
**Compatibilidade › Avançado**: `$D key ctrl+2; $D click 285 462; $D scroll 8`. A `<select>` opens
a GTK popup on `click`; pick with `$D key Down Return` (the window saves itself 300 ms after a
change; check `/tmp/sixlauncher-drive/home/config/sixlauncher/config.toml`).

### Exercise Jogar without a game (fake title)

The same fake title the core's tests use (`crates/core/src/testutil.rs`): a `MicrosoftGame.config`
and a 512-byte PE. `scarlett.exe` prepares it (nothing to translate) and its run exits with code 1,
so Jogar → shader preparation → game start → exit takes a few seconds:

```bash
python3 - <<'PY'
import os, struct
g = '/tmp/fake-title'; os.makedirs(g + '/bin', exist_ok=True)
open(g + '/MicrosoftGame.config', 'w').write('<?xml version="1.0" encoding="utf-8"?>\n<Game configVersion="1">\n  <Identity Name="Example.TestTitle" Publisher="CN=Example" Version="1.2.3.0"/>\n  <ExecutableList>\n    <Executable Name="bin\\title.exe" TargetDeviceFamily="Scarlett" Id="Game"/>\n  </ExecutableList>\n  <ShellVisuals DefaultDisplayName="Test Title" PublisherDisplayName="Example"/>\n</Game>\n')
d = bytearray(0x200); d[0:2] = b'MZ'; d[0x3C:0x40] = struct.pack('<I', 0x80); d[0x80:0x84] = b'PE\0\0'; d[0x84:0x86] = struct.pack('<H', 0x8664)
open(g + '/bin/title.exe', 'wb').write(d)
PY
rm -rf /tmp/sixlauncher-drive/home
$D cli setup --game /tmp/fake-title --backend scarlett --backend-exe ../ScarlettPort/engine/target/x86_64-pc-windows-msvc/release/scarlett.exe --no-prefix
sed -i 's/^verify = true$/verify = false/' /tmp/sixlauncher-drive/home/config/sixlauncher/config.toml   # unknown exe hash
$D start; $D key ctrl+Return; sleep 15; $D ss after-play
cat /tmp/sixlauncher-drive/home/cache/sixlauncher/scarlett/prepared.txt   # written by the preparation
head -1 /tmp/sixlauncher-drive/home/state/sixlauncher/last-run.log        # the last command (the game's run)
```

## Run (human path)

```bash
./target/debug/sixlauncher-app   # opens on your desktop session; close the window to quit
```

## Test

```bash
cargo test --workspace   # 146 passed in crates/core (simulated Proton/Wine, local fake servers)
```

## Gotchas

- **No WebDriver on Arch.** `tauri-driver` needs `WebKitWebDriver`, which no Arch/CachyOS package
  ships. That's why the driver is xdotool + screenshots, not DOM queries.
- **The window opens on the host's Wayland session** unless `WAYLAND_DISPLAY` is unset and
  `GDK_BACKEND=x11` is set: the driver does both, or Xvfb stays empty and `xdotool search` finds nothing.
- **Don't scroll with arrow keys or Page Up/Down**: after a click on a tab, arrows switch tabs
  (it happened: Avançado became VirusTotal). Use `scroll`.
- **`cargo test` does not rebuild the window.** After changing `app/` or `ui/`, run
  `cargo build --workspace` before `$D start`, or you drive the old binary (it happened: Jogar
  skipped a feature that was already in the code).
- **Sandboxed by default.** Without the sandbox the window reads `~/.config/sixlauncher/config.toml`,
  and `ctrl+Return` would start the real game through Proton. Use `start --real` only on purpose.
- **The UI fills in after the window maps** (config, checks, PortWatch VI and update banner come from
  `invoke()`s): `start` waits 2 s; the Verificação tab needs ~3 s after `ctrl+3` to finish checking.
- **The update banner** ("SixLauncher 0.1.x disponível") shows because the local build is 0.1.0: it
  is not an error and pushes the content down about 120 px.
- **`ui/` is embedded at compile time, even in the debug build** (`frontendDist: ../ui`, no dev
  server): after editing `ui/*` run `cargo build --workspace` (~2 s, only `sixlauncher-app`
  recompiles), then `$D start`. Restarting alone still shows the old UI.
- **`type` is only typed into a field that has focus**: `click` on the field first. The driver
  waits 0.5 s after `type`; without it the screenshot misses the last characters.

## Troubleshooting

- **Screenshot is all black**: `xdotool` picked a window left over from an earlier session (its pid
  files were deleted with `/tmp/sixlauncher-drive`). The driver now finds the window by pid and
  `start` runs `stop` first, which also kills any stray `SixLauncher` window and Xvfb on `:97`.
- **`MESA-EGL: warning: DRI3 error: Could not get DRI3 device`** in `$D log`: harmless under Xvfb;
  WebKit falls back to software rendering and the page draws normally.

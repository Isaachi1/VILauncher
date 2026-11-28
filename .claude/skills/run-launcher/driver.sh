#!/usr/bin/env bash
# Drive the SixLauncher window (Tauri/WebKitGTK) headlessly: Xvfb + xdotool + ImageMagick.
# No WebDriver: Arch ships no WebKitWebDriver, so tauri-driver can't run here.
#
#   driver.sh start [--real]   start Xvfb and the window (sandboxed XDG dirs unless --real)
#   driver.sh ss [name]        screenshot the window -> $SL_RUN/shots/<name>.png (prints the path)
#   driver.sh key <keys...>    xdotool key names, e.g. ctrl+3  ctrl+d  Tab  Return
#   driver.sh click <x> <y>    left click, coordinates relative to the window's top-left
#   driver.sh type <text>      type text into the focused field
#   driver.sh scroll <n> [x y] mouse wheel: n notches down (negative: up), over x,y (default 520,500)
#   driver.sh cli <args...>    run the `sixlauncher` CLI with the same XDG dirs as the window
#   driver.sh log              the window's stdout/stderr
#   driver.sh stop             kill the window and Xvfb
#
# Env: SL_RUN (state dir, default /tmp/sixlauncher-drive), SL_DISPLAY (default :97),
#      SL_BIN (target dir, default <unit>/target/debug).
set -euo pipefail

UNIT=$(cd "$(dirname "$0")/../../.." && pwd)
RUN=${SL_RUN:-/tmp/sixlauncher-drive}
DISP=${SL_DISPLAY:-:97}
BIN=${SL_BIN:-$UNIT/target/debug}
mkdir -p "$RUN/shots"

xdg_env() {
  # Sandboxed unless `start --real` wrote a marker: never touch the user's config by accident.
  if [[ -f $RUN/real ]]; then return; fi
  export XDG_CONFIG_HOME=$RUN/home/config XDG_DATA_HOME=$RUN/home/data \
         XDG_CACHE_HOME=$RUN/home/cache XDG_STATE_HOME=$RUN/home/state
  mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"
}

# By pid, not just title: a window left over from an earlier session (state dir deleted) would match too.
wid() { DISPLAY=$DISP xdotool search --onlyvisible --pid "$(cat "$RUN/app.pid" 2>/dev/null || echo 0)" --name '^SixLauncher$' 2>/dev/null | head -1; }

case ${1:-} in
  start)
    "$0" stop >/dev/null   # one window per display, always
    rm -f "$RUN/real"; [[ ${2:-} == --real ]] && touch "$RUN/real"
    [[ -x $BIN/sixlauncher-app ]] || { echo "build first: cargo build --workspace" >&2; exit 1; }
    if ! DISPLAY=$DISP xdotool getmouselocation >/dev/null 2>&1; then
      Xvfb "$DISP" -screen 0 1280x900x24 -nolisten tcp >"$RUN/xvfb.log" 2>&1 &
      echo $! >"$RUN/xvfb.pid"
      for _ in $(seq 50); do DISPLAY=$DISP xdotool getmouselocation >/dev/null 2>&1 && break; sleep 0.1; done
    fi
    xdg_env
    # GDK_BACKEND=x11 + no WAYLAND_DISPLAY: otherwise GTK opens on the host's Wayland session.
    # WEBKIT_DISABLE_DMABUF_RENDERER: no GPU buffers under Xvfb (blank webview otherwise).
    env -u WAYLAND_DISPLAY DISPLAY=$DISP GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1 \
      "$BIN/sixlauncher-app" >"$RUN/app.log" 2>&1 &
    echo $! >"$RUN/app.pid"
    for _ in $(seq 100); do [[ -n $(wid) ]] && break; sleep 0.2; done
    [[ -n $(wid) ]] || { echo "window did not appear; see $RUN/app.log" >&2; cat "$RUN/app.log" >&2; exit 1; }
    sleep 2 # the UI's first invoke()s (config, checks) fill the page after the window maps
    echo "window $(wid) on $DISP, pid $(cat "$RUN/app.pid")"
    ;;
  ss)
    out=$RUN/shots/${2:-shot-$(date +%H%M%S)}.png
    DISPLAY=$DISP import -window "$(wid)" "$out"
    echo "$out"
    ;;
  key)
    shift; w=$(wid)
    DISPLAY=$DISP xdotool windowactivate --sync "$w" 2>/dev/null || DISPLAY=$DISP xdotool windowfocus --sync "$w"
    DISPLAY=$DISP xdotool key --clearmodifiers "$@"; sleep 0.5
    ;;
  click)
    w=$(wid)
    DISPLAY=$DISP xdotool mousemove --window "$w" "$2" "$3" click 1; sleep 0.5
    ;;
  scroll)
    # The wheel, not keys: arrow keys move between tabs when a tab has focus.
    w=$(wid); n=$2; btn=5; ((n < 0)) && { btn=4; n=$((-n)); }
    DISPLAY=$DISP xdotool mousemove --window "$w" "${3:-520}" "${4:-500}" click --repeat "$n" --delay 40 $btn; sleep 0.5
    ;;
  type)
    shift; DISPLAY=$DISP xdotool type --delay 20 -- "$*"; sleep 0.5
    ;;
  cli)
    shift; xdg_env; exec "$BIN/sixlauncher" "$@"
    ;;
  log) cat "$RUN/app.log" ;;
  stop)
    for p in app xvfb; do [[ -f $RUN/$p.pid ]] && kill "$(cat "$RUN/$p.pid")" 2>/dev/null || true; rm -f "$RUN/$p.pid"; done
    # Strays whose pid files are gone: anything still on our display.
    for w in $(DISPLAY=$DISP xdotool search --name '^SixLauncher$' 2>/dev/null); do
      kill "$(DISPLAY=$DISP xdotool getwindowpid "$w")" 2>/dev/null || true; done
    pkill -f "^Xvfb $DISP " 2>/dev/null || true
    for _ in $(seq 30); do DISPLAY=$DISP xdotool getmouselocation >/dev/null 2>&1 || break; sleep 0.1; done
    echo stopped
    ;;
  *) sed -n '2,15p' "$0"; exit 2 ;;
esac

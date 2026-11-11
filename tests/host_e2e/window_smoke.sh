#!/bin/bash
# Smoke test of the window (tao + wry + muda) on Linux, in a virtual display.
#
#   tests/host_e2e/window_smoke.sh [path/to/supertracker]
#
# Needs xvfb-run, xdotool, node and the GTK/WebKitGTK runtime. It starts the real window, and checks
# through the bridge trace (SUPERTRACKER_TRACE=1) that:
#   - the interface loads from the supertracker:// scheme and completes its handshake over the web
#     view's message channel (requests one way, script deliveries the other);
#   - a menu shortcut (Ctrl+K) reaches the page as a `command` event;
#   - a right click opens a native context menu and its dismissal answers `ui.contextMenu`;
#   - Ctrl+O opens the native file chooser and Escape answers `project.open` with "cancelled";
#   - screenshots for AI clients (window_screenshot.js decodes the PNG files): `--ctl app.screenshot`
#     and the MCP tool `screenshot` over `--mcp` return a picture of the web view at the window's
#     size, scaled down to a requested width, that is not one flat colour and that changes when the
#     window does (the command palette); a request made while the file chooser keeps the window's
#     loop busy fails with a message after ten seconds instead of hanging; a host without a window
#     (`--serve`) refuses with a message. SUPERTRACKER_SMOKE_PICTURES=<folder> keeps the pictures;
#   - Ctrl+Q runs the quit handshake (app.quitRequested, two app.quitAnswer) and the process exits 0,
#     leaving the recovery copy and the window's settings in the support folder.
# See tests/host_e2e/README.md.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-${SUPERTRACKER_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/supertracker}}"
[ -x "$BIN" ] || { echo "FAIL build the host first (cargo build -p st-app); no executable at $BIN"; exit 1; }
for tool in xvfb-run xdotool node; do command -v "$tool" > /dev/null || { echo "FAIL $tool is not installed"; exit 1; }; done

if [ -z "${ST_WINDOW_SMOKE_INNER:-}" ]; then
  WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
  status=0
  ST_WINDOW_SMOKE_INNER="$WORK" xvfb-run -a -s "-screen 0 1600x1000x24" "$0" "$BIN" || status=$?
  exit $status
fi

WORK="$ST_WINDOW_SMOKE_INNER"; LOG="$WORK/trace.log"
SHOTS="$ROOT/tests/host_e2e/window_screenshot.js"
PICTURES="${SUPERTRACKER_SMOKE_PICTURES:-$WORK/pictures}"; mkdir -p "$PICTURES"
# The clients of the control socket (--ctl, --mcp) find it through the same support folder as the host.
export SUPERTRACKER_SUPPORT_DIR="$WORK/support"
SUPERTRACKER_TRACE=1 "$BIN" --ui "$ROOT/ui" > "$LOG" 2>&1 &
APP=$!
failures=0
check() { if grep -qE "$2" "$LOG"; then echo "PASS $1"; else echo "FAIL $1 (no line matching: $2)"; failures=$((failures + 1)); fi; }
wait_for() { for _ in $(seq 1 "${2:-100}"); do grep -qE "$1" "$LOG" && return 0; sleep 0.1; done; return 1; }

wait_for "st-host: [0-9]+ engine.project -> ok" 200
check "the interface loads in the web view and completes its handshake" "st-host: [0-9]+ ready -> ok"
check "the page publishes its project to the host" "st-host: [0-9]+ engine.project -> ok"

xdotool mousemove 700 600 click 1; sleep 0.3

# Screenshots of the window as it is now. Without a window manager the window's X geometry is its
# content: the menu bar and, below it, the web view.
eval "$(xdotool search --onlyvisible --name '^SuperTracker$' | head -n 1 | xargs xdotool getwindowgeometry --shell | grep -E '^(WIDTH|HEIGHT)=')"
node "$SHOTS" "$BIN" window "${WIDTH:-0}" "${HEIGHT:-0}" "$PICTURES" || failures=$((failures + $?))

xdotool key ctrl+k; wait_for "st-host: event command" 50
check "a menu shortcut reaches the page as a command event" "st-host: event command"
sleep 0.5
node "$SHOTS" "$BIN" changed "$PICTURES/window.png" "$PICTURES/window-palette.png" || failures=$((failures + $?))
xdotool key Escape; sleep 0.5

xdotool mousemove 700 600 click 3; sleep 1; xdotool key Escape; wait_for "ui.contextMenu -> ok" 50
check "a native context menu opens and its dismissal is answered" "st-host: [0-9]+ ui.contextMenu -> ok"

xdotool key ctrl+o; sleep 2
# The chooser runs its own loop inside the window's: the screenshot request waits behind it, and the
# client is told so after ten seconds.
busy="$("$BIN" --ctl app.screenshot 2>&1 > /dev/null)"; status=$?
if [ "$status" -eq 1 ] && echo "$busy" | grep -q "did not produce a screenshot within 10 s"; then
  echo "PASS a screenshot requested while a file chooser is open fails with a message ($busy)"
else
  echo "FAIL a screenshot requested while a file chooser is open: exit status $status, message: $busy"; failures=$((failures + 1))
fi
xdotool key Escape; wait_for "project.open -> ok" 50
check "the native file chooser opens and Cancel is answered" "st-host: [0-9]+ project.open -> ok"

xdotool key ctrl+q
for _ in $(seq 1 100); do kill -0 "$APP" 2> /dev/null || break; sleep 0.1; done
if kill -0 "$APP" 2> /dev/null; then
  echo "FAIL the window did not quit"; failures=$((failures + 1)); kill "$APP"
else
  wait "$APP"; status=$?
  if [ "$status" -eq 0 ]; then echo "PASS Quit ends the process with status 0"; else echo "FAIL exit status $status"; failures=$((failures + 1)); fi
fi
check "quitting asks the page first" "st-host: event app.quitRequested"
[ "$(grep -cE 'app.quitAnswer -> ok' "$LOG")" -eq 2 ] && echo "PASS the page answered: deciding, then proceed" || { echo "FAIL expected two app.quitAnswer replies"; failures=$((failures + 1)); }
[ -s "$WORK/support/Recovery.supertracker" ] && echo "PASS the recovery copy was written before quitting" || { echo "FAIL no recovery copy"; failures=$((failures + 1)); }
grep -q '"window"' "$WORK/support/settings.json" 2> /dev/null && echo "PASS the window's size was remembered in settings.json" || { echo "FAIL no window settings"; failures=$((failures + 1)); }


# A host without a window: nothing can take the picture, and the client is told why.
export SUPERTRACKER_SUPPORT_DIR="$WORK/serve-support"
"$BIN" --serve --ui "$ROOT/ui" > "$WORK/serve.log" 2>&1 < /dev/null &
SERVE=$!
for _ in $(seq 1 100); do "$BIN" --ctl app.ping > /dev/null 2>&1 && break; sleep 0.1; done
node "$SHOTS" "$BIN" no-window || failures=$((failures + $?))
kill "$SERVE" 2> /dev/null; wait "$SERVE" 2> /dev/null

if [ "$failures" -ne 0 ]; then echo; echo "--- bridge trace ---"; grep "st-host:" "$LOG"; exit 1; fi

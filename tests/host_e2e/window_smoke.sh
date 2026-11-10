#!/bin/bash
# Smoke test of the window (tao + wry + muda) on Linux, in a virtual display.
#
#   tests/host_e2e/window_smoke.sh [path/to/supertracker]
#
# Needs xvfb-run, xdotool and the GTK/WebKitGTK runtime. It starts the real window, and checks through
# the bridge trace (SUPERTRACKER_TRACE=1) that:
#   - the interface loads from the supertracker:// scheme and completes its handshake over the web
#     view's message channel (requests one way, script deliveries the other);
#   - a menu shortcut (Ctrl+K) reaches the page as a `command` event;
#   - a right click opens a native context menu and its dismissal answers `ui.contextMenu`;
#   - Ctrl+O opens the native file chooser and Escape answers `project.open` with "cancelled";
#   - Ctrl+Q runs the quit handshake (app.quitRequested, two app.quitAnswer) and the process exits 0,
#     leaving the recovery copy and the window's settings in the support folder.
# See tests/host_e2e/README.md.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-${SUPERTRACKER_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/supertracker}}"
[ -x "$BIN" ] || { echo "FAIL build the host first (cargo build -p st-app); no executable at $BIN"; exit 1; }
for tool in xvfb-run xdotool; do command -v "$tool" > /dev/null || { echo "FAIL $tool is not installed"; exit 1; }; done

if [ -z "${ST_WINDOW_SMOKE_INNER:-}" ]; then
  WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
  status=0
  ST_WINDOW_SMOKE_INNER="$WORK" xvfb-run -a -s "-screen 0 1600x1000x24" "$0" "$BIN" || status=$?
  exit $status
fi

WORK="$ST_WINDOW_SMOKE_INNER"; LOG="$WORK/trace.log"
SUPERTRACKER_TRACE=1 SUPERTRACKER_SUPPORT_DIR="$WORK/support" "$BIN" --ui "$ROOT/ui" > "$LOG" 2>&1 &
APP=$!
failures=0
check() { if grep -qE "$2" "$LOG"; then echo "PASS $1"; else echo "FAIL $1 (no line matching: $2)"; failures=$((failures + 1)); fi; }
wait_for() { for _ in $(seq 1 "${2:-100}"); do grep -qE "$1" "$LOG" && return 0; sleep 0.1; done; return 1; }

wait_for "st-host: [0-9]+ engine.project -> ok" 200
check "the interface loads in the web view and completes its handshake" "st-host: [0-9]+ ready -> ok"
check "the page publishes its project to the host" "st-host: [0-9]+ engine.project -> ok"

xdotool mousemove 700 600 click 1; sleep 0.3
xdotool key ctrl+k; wait_for "st-host: event command" 50
check "a menu shortcut reaches the page as a command event" "st-host: event command"
xdotool key Escape; sleep 0.5

xdotool mousemove 700 600 click 3; sleep 1; xdotool key Escape; wait_for "ui.contextMenu -> ok" 50
check "a native context menu opens and its dismissal is answered" "st-host: [0-9]+ ui.contextMenu -> ok"

xdotool key ctrl+o; sleep 2; xdotool key Escape; wait_for "project.open -> ok" 50
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

if [ "$failures" -ne 0 ]; then echo; echo "--- bridge trace ---"; grep "st-host:" "$LOG"; exit 1; fi

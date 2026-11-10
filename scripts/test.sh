#!/bin/bash
# Run the test suites.
#   scripts/test.sh            everything
#   scripts/test.sh rust       the Rust crates and the plug-in container
#   scripts/test.sh ui         the interface tests (node)
#   scripts/test.sh e2e        the interface in a browser against the real host (needs Playwright + Chromium)
# Tests never open an audio device: SUPERTRACKER_AUDIO_DEVICE=null selects the silent one.
set -euo pipefail
cd "$(dirname "$0")/.."
export SUPERTRACKER_AUDIO_DEVICE="${SUPERTRACKER_AUDIO_DEVICE:-null}"
TARGET="${CARGO_TARGET_DIR:-$PWD/target}"
WHAT="${1:-all}"
rust() {
  cargo test --workspace
  CARGO_TARGET_DIR="$TARGET" cargo test --manifest-path crates/plugin-host/Cargo.toml
}
ui() {
  for script in ui/*.js; do node --check "$script"; done
  # One node process per test file, as many at a time as there are processors.
  local jobs; jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
  local failed
  failed="$(printf '%s\n' tests/*_tests.js | xargs -P "$jobs" -I{} sh -c 'node "$1" >/dev/null 2>&1 || echo "$1"' _ {})"
  if [[ -n "$failed" ]]; then
    echo "Interface tests failed:" >&2; echo "$failed" >&2
    echo "Run one with: node tests/<name>_tests.js" >&2
    return 1
  fi
  echo "Interface tests passed ($(ls tests/*_tests.js | wc -l | tr -d ' ') files)."
}
e2e() {
  cargo build -p st-app
  SUPERTRACKER_BIN="$TARGET/debug/supertracker" node tests/host_e2e/run.js
}
case "$WHAT" in
  rust) rust ;;
  ui) ui ;;
  e2e) e2e ;;
  all) rust; ui; e2e ;;
  *) echo "Usage: scripts/test.sh [rust|ui|e2e]" >&2; exit 64 ;;
esac

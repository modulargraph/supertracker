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
  local status=0
  cargo test --workspace --no-fail-fast || status=1
  CARGO_TARGET_DIR="$TARGET" cargo test --manifest-path crates/plugin-host/Cargo.toml --no-fail-fast || status=1
  return $status
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
  cargo build -p st-app || return 1
  # The host looks for the plug-in container next to its own executable; the test loads the CLAP test plug-in.
  # Both are built here so the test never runs against older ones.
  CARGO_TARGET_DIR="$TARGET" cargo build --manifest-path crates/plugin-host/Cargo.toml -p stpc-container -p stpc-clap-test-plugin || return 1
  SUPERTRACKER_BIN="$TARGET/debug/supertracker" node tests/host_e2e/run.js
}
case "$WHAT" in
  rust) rust ;;
  ui) ui ;;
  e2e) e2e ;;
  all)
    # Every part runs; the script fails if any of them did.
    status=0
    rust || { status=1; echo "Rust tests failed." >&2; }
    ui || status=1
    e2e || { status=1; echo "Browser test failed." >&2; }
    exit $status ;;
  *) echo "Usage: scripts/test.sh [rust|ui|e2e]" >&2; exit 64 ;;
esac

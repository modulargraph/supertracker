#!/bin/zsh
# User Track Freeze end to end (built-in synth + AUDelay, flat and routed mixer,
# AU instrument). Runs the CLI self-test without a window, in a private support
# folder, and never touches the user's Recovery.supertracker.
# Covers Song mode, Pattern mode (pattern export, loop wrap, engine pattern clips)
# and audition refusal. SUPERTRACKER_FREEZE_ONLY=flat|routed|flat-au-instrument
# runs a single host scenario.
set -euo pipefail
cd "$(dirname "$0")/.."
# Shares the warm debug build in .build with scripts/build.sh debug (debug builds include the self-tests).
export CLANG_MODULE_CACHE_PATH="$PWD/.build/clang-module-cache"
swift build --disable-sandbox -c debug --scratch-path .build
out="${1:-$PWD/test-output/track-freeze-$(date +%Y%m%d-%H%M%S)}"
support="$(cd "$(mktemp -d "${TMPDIR:-/tmp}/supertracker-freeze-support.XXXXXX")" && pwd -P)"
trap 'rm -rf "$support"' EXIT
mkdir -p "$out"
set +e
SUPERTRACKER_SUPPORT_DIR="$support" perl -e 'alarm 600; exec @ARGV' \
  .build/debug/SuperTracker --freeze-self-test "$out" > "$out/run.log" 2>&1
result=$?
set -e
cat "$out/run.log"
echo "exit $result" | tee -a "$out/run.log"
exit $result

#!/bin/bash
# Native app self-tests (slow, serial). Run via: scripts/test.sh --native
set -euo pipefail
cd "$(dirname "$0")/.."
  mkdir -p "$PWD/test-output"
  native_run_dir="$(mktemp -d "$PWD/test-output/native-regression.XXXXXX")"
  # The run folder holds a full debug app: remove it when every self-test passes, keep it as evidence otherwise.
  trap 'status=$?; if [ "$status" -eq 0 ]; then rm -rf "$native_run_dir"; else echo "Native self-test output kept: $native_run_dir" >&2; fi' EXIT
  native_test_app="$native_run_dir/SuperTrackerRegression.app"
  ./scripts/build.sh debug "$native_test_app"
  run_native() {
    SUPERTRACKER_SUPPORT_DIR="$native_run_dir/private-$1-support" SUPERTRACKER_NO_MK3=1 \
      python3 scripts/run-native-test.py --timeout 600 -- "$native_test_app/Contents/MacOS/SuperTracker" "$2" "$native_run_dir/$1"
  }
  run_native audio --self-test
  run_native inputs --line-input-self-test
  run_native midi --midi-output-self-test
  run_native kits --kits-self-test
  run_native inserts --inserts-self-test
  # NI Raum (when installed) on an input-less return bus: it renders, and parameter writes read back and are captured.
  run_native deferred-parameters --deferred-parameters-self-test
  # Built-in insert chains, direct sources, folded solo gates and sparse send routes vs the per-node graph.
  run_native insert-chains --insert-chain-self-test
  # Bus and master insert lanes on slots > 0 reach their own insert (built-in chains, mixed AU chains, live bus edits).
  run_native bus-insert-automation --bus-insert-automation-self-test
  # Instrument insert chains: per-track instances ahead of track inserts, live edits, reassign, lanes, locks, kits.
  run_native instrument-effects --instrument-effects-self-test
  run_native workflow --ui-workflow-self-test
  run_native webkit --ui-bridge-self-test
  run_native recovery --ui-plugin-recovery-self-test
  run_native restart --render-restart-self-test
  run_native many-tracks --many-tracks-self-test
  # Render selection to sample: private offline host, tails, master/track-only, render while playing.
  run_native render-selection --render-selection-self-test
  # Full-graph render cost of a 16-pad kit session (debug build, so a very generous coarse budget).
  ST_RENDER_PERF_SCENARIO=template-kit-playing,template-kit-stopped ST_RENDER_PERF_SECONDS=1 ST_RENDER_PERF_BLOCKS=512 ST_RENDER_PERF_BUDGET=60 \
    run_native render-performance --render-performance
  # The original VST3 fixture bundle only (tests/vst3_fixture_tests.sh builds the same one).
  mkdir -p "$native_run_dir/Fixture.vst3/Contents/MacOS"
  plutil -create xml1 "$native_run_dir/Fixture.vst3/Contents/Info.plist"
  for key in CFBundleIdentifier:studio.supertracker.tests.fixture CFBundleExecutable:Fixture CFBundlePackageType:BNDL CFBundleVersion:1 CFBundleShortVersionString:1.7.1; do
    plutil -insert "${key%%:*}" -string "${key#*:}" "$native_run_dir/Fixture.vst3/Contents/Info.plist"
  done
  clang++ -std=c++17 -O2 -fvisibility=hidden -bundle -I Vendor/vst3sdk tests/vst3_fixture.mm Sources/VST3Host/VST3SDK.cpp \
    -framework CoreFoundation -o "$native_run_dir/Fixture.vst3/Contents/MacOS/Fixture"
  SUPERTRACKER_VST3_FIXTURE="$native_run_dir/Fixture.vst3" run_native audio-host-fixes --audio-host-fixes-self-test
  # Note edits, autosave captures and adopted snapshots while playing: no graph pause, no routing re-plan.
  SUPERTRACKER_VST3_FIXTURE="$native_run_dir/Fixture.vst3" ST_HARDWARE_EDITOR_DISABLE_MIDI=1 run_native live-publish --live-publish-self-test

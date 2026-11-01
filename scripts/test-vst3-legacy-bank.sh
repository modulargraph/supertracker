#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Snapshot the host, tests and required SDK sources into an isolated evidence
# directory. Only original in-memory components execute; no bundle is loaded.
mkdir -p test-output
build=$(mktemp -d "$PWD/test-output/vst3-legacy-bank.XXXXXX")
# Remove the private build on success; keep it for debugging when a step fails.
trap 'status=$?; if [ "$status" -eq 0 ]; then rm -rf "$build"; else echo "Kept for debugging: $build" >&2; fi' EXIT
mkdir -p "$build/source/Sources/VST3Host/include" "$build/source/tests" \
  "$build/source/Vendor/vst3sdk/public.sdk/source/vst"
cp Sources/VST3Host/VST3Host.mm Sources/VST3Host/VST3SDK.cpp Sources/VST3Host/STHardwarePatchBaseline.h "$build/source/Sources/VST3Host/"
cp Sources/VST3Host/include/VST3Host.h "$build/source/Sources/VST3Host/include/"
cp tests/vst3_legacy_bank_tests.mm tests/vst3_state_fixture.h "$build/source/tests/"
cp -R Vendor/vst3sdk/pluginterfaces "$build/source/Vendor/vst3sdk/"
cp -R Vendor/vst3sdk/public.sdk/source/vst/utility "$build/source/Vendor/vst3sdk/public.sdk/source/vst/"
clang++ -std=c++17 -O1 -g -fobjc-arc -I "$build/source/Vendor/vst3sdk" \
  -I "$build/source/Sources/VST3Host/include" \
  "$build/source/tests/vst3_legacy_bank_tests.mm" "$build/source/Sources/VST3Host/VST3SDK.cpp" \
  -framework AppKit -framework Foundation -framework CoreFoundation \
  -o "$build/vst3-legacy-bank-tests"
python3 - "$build/vst3-legacy-bank-tests" <<'PY' | tee "$build/results.log"
import subprocess, sys
subprocess.run(sys.argv[1:], check=True, timeout=30)
PY

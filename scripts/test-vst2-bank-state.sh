#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

# A unique private bundle is built from our original fixture. Nothing scans the
# user's plug-in folders, opens an app/editor, or creates an audio device.
mkdir -p test-output
build=$(mktemp -d "$PWD/test-output/vst2-bank-state.XXXXXX")
# Remove the private build on success; keep it for debugging when a step fails.
trap 'status=$?; if [ "$status" -eq 0 ]; then rm -rf "$build"; else echo "Kept for debugging: $build" >&2; fi' EXIT
mkdir -p "$build/source" "$build/OriginalBank.vst/Contents/MacOS"
cp Sources/VST2Host/VST2Host.mm Sources/VST2Host/include/VST2Host.h \
  tests/vst2_debug_fixture.cpp tests/vst2_bank_state_tests.mm "$build/source/"
python3 - "$build/OriginalBank.vst/Contents/Info.plist" <<'PY'
import pathlib, plistlib, sys
pathlib.Path(sys.argv[1]).write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'studio.supertracker.tests.original-bank',
    'CFBundleExecutable': 'OriginalBank', 'CFBundlePackageType': 'BNDL',
    'CFBundleName': 'Original Bank VST2 Fixture', 'CFBundleVersion': '1',
    'CFBundleShortVersionString': '1'
}))
PY
clang++ -std=c++17 -O1 -g -bundle -DORIGINAL_BANK_PROGRAMS \
  "$build/source/vst2_debug_fixture.cpp" \
  -o "$build/OriginalBank.vst/Contents/MacOS/OriginalBank"
clang++ -std=c++17 -O1 -g -fobjc-arc -I "$build/source" \
  "$build/source/VST2Host.mm" "$build/source/vst2_bank_state_tests.mm" \
  -framework AppKit -framework Foundation -framework CoreFoundation \
  -o "$build/vst2-bank-state-tests"
python3 - "$build/vst2-bank-state-tests" "$build/OriginalBank.vst" <<'PY' | tee "$build/results.log"
import subprocess, sys
subprocess.run(sys.argv[1:], check=True, timeout=30)
PY

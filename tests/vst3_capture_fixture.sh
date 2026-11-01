#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
# This exact-version test fixture has its own path and identity. Never overwrite
# Fixture.vst3 or an installed vendor bundle with a compatibility-test component.
bundle_name="SettledCaptureFixture"
bundle_identifier="studio.supertracker.tests.settled-capture-fixture"
fixture_flags=(-DST_VST3_CAPTURE_INITIALIZATION_EDITS=0)
if [[ "${1-}" == "--initialization-edits" && $# -eq 1 ]]; then
  bundle_name="SettledCaptureEditsFixture"
  bundle_identifier="studio.supertracker.tests.settled-capture-edits-fixture"
  fixture_flags=(-DST_VST3_CAPTURE_INITIALIZATION_EDITS=1)
elif [[ $# -gt 0 ]]; then
  echo "Usage: $0 [--initialization-edits]" >&2
  exit 2
fi
bundle_path="test-output/$bundle_name.vst3"
mkdir -p "$bundle_path/Contents/MacOS"
python3 - "$bundle_path" "$bundle_name" "$bundle_identifier" <<'PY'
import pathlib, plistlib, sys
(pathlib.Path(sys.argv[1]) / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleIdentifier': sys.argv[3],
    'CFBundleExecutable': sys.argv[2], 'CFBundlePackageType': 'BNDL',
    'CFBundleName': 'SuperTracker Async Capture Instrument',
    'CFBundleVersion': '1.7.1', 'CFBundleShortVersionString': '1.7.1',
    'LSMinimumSystemVersion': '14.0'
}))
PY
clang++ -std=c++17 -O2 -g -mmacosx-version-min=14.0 -fvisibility=hidden -bundle \
  "${fixture_flags[@]}" -I Vendor/vst3sdk tests/vst3_capture_fixture.mm Sources/VST3Host/VST3SDK.cpp \
  -framework CoreFoundation -o "$bundle_path/Contents/MacOS/$bundle_name"
echo "Built original fixture: $PWD/$bundle_path"

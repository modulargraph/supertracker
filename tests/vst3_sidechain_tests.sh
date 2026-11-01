#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
compile_flags=(-std=c++17 -O2 -g)
bundle_path="test-output/SidechainFixture.vst3"
test_path="test-output/vst3-sidechain-tests"
if [[ "${1-}" == "--sanitize" ]]; then
  compile_flags=(-std=c++17 -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer)
  bundle_path="test-output/SidechainFixtureSanitized.vst3"
  test_path="test-output/vst3-sidechain-sanitized"
elif [[ $# -gt 0 ]]; then
  echo "Usage: $0 [--sanitize]" >&2
  exit 2
fi
mkdir -p "$bundle_path/Contents/MacOS"
python3 - "$bundle_path" <<'PY'
import pathlib, plistlib, sys
(pathlib.Path(sys.argv[1]) / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'studio.supertracker.tests.sidechain-fixture',
    'CFBundleExecutable': 'SidechainFixture', 'CFBundlePackageType': 'BNDL',
    'CFBundleName': 'SuperTracker VST3 Sidechain Fixture', 'CFBundleVersion': '1'
}))
PY
clang++ "${compile_flags[@]}" -fvisibility=hidden -bundle -I Vendor/vst3sdk \
  tests/vst3_sidechain_fixture.mm Sources/VST3Host/VST3SDK.cpp -framework CoreFoundation \
  -o "$bundle_path/Contents/MacOS/SidechainFixture"
clang++ "${compile_flags[@]}" -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  Sources/VST3Host/VST3Host.mm Sources/VST3Host/VST3SDK.cpp \
  tests/vst3_sidechain_tests.mm -framework AppKit -framework CoreFoundation \
  -o "$test_path"
"$test_path" "$PWD/$bundle_path"

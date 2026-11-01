#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p test-output/Fixture.vst3/Contents/MacOS
python3 - <<'PY'
import pathlib, plistlib
pathlib.Path('test-output/Fixture.vst3/Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'studio.supertracker.tests.fixture',
    'CFBundleExecutable': 'Fixture', 'CFBundlePackageType': 'BNDL',
    'CFBundleName': 'SuperTracker VST3 Fixture', 'CFBundleVersion': '1',
    'CFBundleShortVersionString': '1.7.1'
}))
PY
clang++ -std=c++17 -O2 -g -fvisibility=hidden -bundle -I Vendor/vst3sdk \
  tests/vst3_fixture.mm Sources/VST3Host/VST3SDK.cpp -framework CoreFoundation \
  -o test-output/Fixture.vst3/Contents/MacOS/Fixture
clang++ -std=c++17 -O2 -g -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  tests/vst3_core_tests.mm Sources/VST3Host/VST3SDK.cpp -framework AppKit \
  -framework CoreFoundation -o test-output/vst3-core-tests
./test-output/vst3-core-tests
clang++ -std=c++17 -O2 -g -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  tests/vst3_transport_tests.mm Sources/VST3Host/VST3SDK.cpp -framework AppKit \
  -framework CoreFoundation -o test-output/vst3-transport-tests
./test-output/vst3-transport-tests
mkdir -p test-output/transport-modules/VST3Host test-output/transport-module-cache
cat > test-output/transport-modules/VST3Host/module.modulemap <<'MODULE'
module VST3Host {
  header "../../../Sources/VST3Host/include/VST3Host.h"
  export *
}
MODULE
swiftc -O -swift-version 5 -I test-output/transport-modules -module-cache-path test-output/transport-module-cache \
  Sources/SuperTracker/VST3TransportContext.swift tests/VST3TransportContextTests.swift \
  -o test-output/vst3-transport-context-tests
./test-output/vst3-transport-context-tests
clang++ -std=c++17 -O2 -g -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  tests/vst3_state_tests.mm Sources/VST3Host/VST3SDK.cpp -framework AppKit \
  -framework CoreFoundation -o test-output/vst3-state-tests
./test-output/vst3-state-tests
clang++ -std=c++17 -O2 -g -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  tests/vst3_baseline_tests.mm Sources/VST3Host/VST3SDK.cpp -framework AppKit \
  -framework CoreFoundation -o test-output/vst3-baseline-tests
./test-output/vst3-baseline-tests
clang++ -std=c++17 -O2 -g -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  tests/vst3_save_flush_tests.mm Sources/VST3Host/VST3SDK.cpp -framework AppKit \
  -framework CoreFoundation -o test-output/vst3-save-flush-tests
./test-output/vst3-save-flush-tests
clang++ -std=c++17 -O2 -g -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  Sources/VST3Host/VST3Host.mm Sources/VST3Host/VST3SDK.cpp \
  tests/vst3_lifecycle_tests.mm -framework AppKit -framework CoreFoundation \
  -o test-output/vst3-lifecycle-tests
python3 tests/vst3_lifecycle_runner.py

clang++ -std=c++17 -O2 -g -fobjc-arc -I Vendor/vst3sdk -I Sources/VST3Host/include \
  Sources/VST3Host/VST3Host.mm Sources/VST3Host/VST3SDK.cpp \
  tests/vst3_restart_tests.mm -framework AppKit -framework CoreFoundation \
  -o test-output/vst3-restart-tests
: > test-output/vst3-restart.log
ST_VST3_FIXTURE_LATENCY_RESTART=1 ST_VST3_FIXTURE_TRACE="$PWD/test-output/vst3-restart.log" \
  ./test-output/vst3-restart-tests "$PWD/test-output/Fixture.vst3" "$PWD/test-output/vst3-restart.log"

# Original indexed auxiliary-input effects; no installed vendor plugins.
bash tests/vst3_sidechain_tests.sh

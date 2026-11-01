#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mk3_test_output="${1:-.build/test-bin/mk3-lifecycle}"
mkdir -p "$mk3_test_output"
# Keep the production Driver unchanged: IOKit sessions above it are replaced by inert test doubles.
# This extraction fails visibly if the declaration changes; no hardware or NI process is accessed.
python3 - "$mk3_test_output/Driver.swift" "${ST_MK3_DRIVER_SOURCE:-Sources/SuperTracker/MaschineMK3.swift}" <<'PY'
from pathlib import Path
import sys
source=Path(sys.argv[2]).read_text()
marker='@MainActor\nfinal class MaschineMK3Driver'
assert source.count(marker)==1, 'expected the actual main-actor Maschine driver declaration'
helper=source[source.index('enum MK3ScreenTransferValidation {'):source.index('final class MK3ScreenSession:')]
Path(sys.argv[1]).write_text('import Foundation\n'+helper+source[source.index(marker):])
PY
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MaschineMK3Protocol.swift Sources/SuperTracker/MaschineMK3Screen.swift tests/MaschineMK3LifecycleFakes.swift "$mk3_test_output/Driver.swift" tests/MaschineMK3LifecycleTests.swift -o "$mk3_test_output/tests"
for mk3_scenario in release duplicate reconnect shutdown shutdown-queued shutdown-release shared-shutdown restore-retry missing-device open-retry initialization-retry transfer-validation; do
  "$mk3_test_output/tests" "$mk3_scenario"
done

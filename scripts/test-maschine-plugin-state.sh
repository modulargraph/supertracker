#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
PLUGIN_STATE_TEST_OUTPUT="${1:-$PWD/test-output/maschine-plugin-state}"
mkdir -p "$PLUGIN_STATE_TEST_OUTPUT" .build/clang-module-cache
shasum -a 256 Sources/SuperTracker/MaschineGroupArchive.swift Sources/SuperTracker/MaschineGroupPluginState.swift \
  tests/MaschineGroupPluginStateTests.swift > "$PLUGIN_STATE_TEST_OUTPUT/source-sha256.txt"
xcrun swiftc -swift-version 5 -module-cache-path .build/clang-module-cache \
  Sources/SuperTracker/MaschineGroupArchive.swift Sources/SuperTracker/MaschineGroupPluginState.swift \
  tests/MaschineGroupPluginStateTests.swift -o "$PLUGIN_STATE_TEST_OUTPUT/tests"
"$PLUGIN_STATE_TEST_OUTPUT/tests" | tee "$PLUGIN_STATE_TEST_OUTPUT/results.txt"
shasum -a 256 -c "$PLUGIN_STATE_TEST_OUTPUT/source-sha256.txt"

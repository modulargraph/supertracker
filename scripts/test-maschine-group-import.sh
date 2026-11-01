#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
MASCHINE_TEST_OUTPUT="${1:-$PWD/test-output/maschine-group-tests}"
mkdir -p "$MASCHINE_TEST_OUTPUT/TrackerEngine"
cp Sources/TrackerEngine/include/STBuiltinEffects.h "$MASCHINE_TEST_OUTPUT/TrackerEngine/"
cat > "$MASCHINE_TEST_OUTPUT/TrackerEngine/module.modulemap" <<'MODULE'
module TrackerEngine { header "STBuiltinEffects.h" export * }
MODULE
clang++ -std=c++17 -O2 -Wall -Wextra -Werror -c Sources/TrackerEngine/BuiltinEffects.cpp -o "$MASCHINE_TEST_OUTPUT/BuiltinEffects.o"
xcrun swiftc -O -whole-module-optimization -swift-version 5 -module-cache-path "$MASCHINE_TEST_OUTPUT/module-cache" -I "$MASCHINE_TEST_OUTPUT" \
  Sources/SuperTracker/MaschineGroup*.swift Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/ProjectLayout.swift \
  Sources/SuperTracker/ProjectMigration.swift Sources/SuperTracker/KitGroups.swift Sources/SuperTracker/BuiltinWavetables.swift Sources/SuperTracker/WavetableFormat.swift \
  tests/MaschineGroupImportTests.swift tests/MaschineGroupVersionTests.swift "$MASCHINE_TEST_OUTPUT/BuiltinEffects.o" -Xlinker -lc++ \
  -o "$MASCHINE_TEST_OUTPUT/import-tests"
"$MASCHINE_TEST_OUTPUT/import-tests" | tee "$MASCHINE_TEST_OUTPUT/results.txt"

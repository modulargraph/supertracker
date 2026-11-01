#!/bin/bash
# Kit lanes -> track groups migration and the group <-> .stkit round trip (tests/KitGroupsMigrationTests.swift).
# Uses the "1988 Kit.mxgrp" named by SUPERTRACKER_1988_KIT when set; SUPERTRACKER_KIT_MIGRATION_FIXTURE adds a real session file.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${1:-$PWD/.build/test-output/kit-groups-migration}"
mkdir -p "$OUT/TrackerEngine"
cp Sources/TrackerEngine/include/STBuiltinEffects.h "$OUT/TrackerEngine/"
cat > "$OUT/TrackerEngine/module.modulemap" <<'MODULE'
module TrackerEngine { header "STBuiltinEffects.h" export * }
MODULE
clang++ -std=c++17 -O2 -c Sources/TrackerEngine/BuiltinEffects.cpp -o "$OUT/BuiltinEffects.o"
xcrun swiftc -swift-version 5 -module-cache-path "$OUT/module-cache" -I "$OUT" \
  Sources/SuperTracker/MaschineGroup*.swift Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift \
  Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/PayloadValue.swift \
  Sources/SuperTracker/ProjectMigration.swift Sources/SuperTracker/KitGroups.swift Sources/SuperTracker/BuiltinWavetables.swift Sources/SuperTracker/WavetableFormat.swift \
  tests/KitGroupsMigrationTests.swift "$OUT/BuiltinEffects.o" -Xlinker -lc++ -o "$OUT/kit-groups-migration"
"$OUT/kit-groups-migration"

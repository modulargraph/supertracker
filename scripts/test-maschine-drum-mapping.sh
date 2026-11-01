#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
output="${1:-test-output/maschine-drum-mapping}"
mkdir -p "$output/module" "$output/cache"
root="$(pwd)"
printf 'module TrackerEngine { header "%s/Sources/TrackerEngine/include/STBuiltinEffects.h" export * }\n' "$root" > "$output/module/module.modulemap"
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/BuiltinEffects.cpp -o "$output/BuiltinEffects.o"
swiftc -swift-version 5 -O -module-cache-path "$output/cache" -I "$output/module" Sources/SuperTracker/MaschineGroupDrumMapping.swift tests/MaschineGroupDrumMappingTests.swift "$output/BuiltinEffects.o" -lc++ -o "$output/mapper-tests"
"$output/mapper-tests"

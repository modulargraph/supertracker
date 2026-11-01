#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_build=$(mktemp -d /private/tmp/supertracker-spatial-effects.XXXXXX)
trap 'rm -rf "$test_build"' EXIT
mkdir -p "$test_build/TrackerEngine"
cp Sources/TrackerEngine/include/STBuiltinEffects.h "$test_build/TrackerEngine/"
cat > "$test_build/TrackerEngine/module.modulemap" <<'MODULE'
module TrackerEngine { header "STBuiltinEffects.h" export * }
MODULE
clang++ -std=c++17 -O2 -Wall -Wextra -Werror -I Sources/TrackerEngine \
    tests/builtin_spatial_delay_tests.cpp -o "$test_build/spatial-effects"
"$test_build/spatial-effects" "$@"
clang++ -std=c++17 -O2 -Wall -Wextra -Werror -c Sources/TrackerEngine/BuiltinEffects.cpp -o "$test_build/BuiltinEffects.o"
xcrun swiftc -O -swift-version 5 -module-cache-path "$test_build/module-cache" -I "$test_build" \
    Sources/SuperTracker/MaschineGroupSpatialMapping.swift tests/MaschineGroupSpatialMappingTests.swift \
    "$test_build/BuiltinEffects.o" -Xlinker -lc++ -o "$test_build/spatial-mapping"
"$test_build/spatial-mapping"

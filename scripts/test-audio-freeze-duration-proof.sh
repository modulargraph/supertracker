#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_mode="${1:-optimized}"
test_build="$PWD/test-output/freeze-duration-proof/$test_mode"
mkdir -p "$test_build/module/TrackerEngine" "$test_build/module-cache"
cat > "$test_build/module/TrackerEngine/module.modulemap" <<MODULE
module TrackerEngine {
  header "$PWD/Sources/TrackerEngine/include/TrackerEngine.h"
  export *
}
MODULE
c_flags=(-std=c++17 -O2)
swift_flags=(-O -swift-version 5)
case "$test_mode" in
  optimized) ;;
  asan) c_flags+=(-fsanitize=address -fno-omit-frame-pointer); swift_flags+=(-sanitize=address) ;;
  *) echo "Usage: $0 [optimized|asan]" >&2; exit 2 ;;
esac
# Immutable map validation only: no engine rendering, devices, plug-ins or UI.
clang++ "${c_flags[@]}" -I Sources/TrackerEngine/include -c Sources/TrackerEngine/TrackerClock.cpp -o "$test_build/clock.o"
clang++ "${c_flags[@]}" -I Sources/TrackerEngine/include -c Sources/TrackerEngine/ClockTimingMap.cpp -o "$test_build/map.o"
swiftc "${swift_flags[@]}" -module-cache-path "$test_build/module-cache" -I "$test_build/module" \
  Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioFreezeDurationProof.swift \
  tests/AudioFreezeDurationProofTests.swift "$test_build/clock.o" "$test_build/map.o" \
  -Xlinker -lc++ -o "$test_build/freeze-duration-proof-tests"
"$test_build/freeze-duration-proof-tests"

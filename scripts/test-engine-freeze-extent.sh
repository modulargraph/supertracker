#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
extent_build="$PWD/test-output/engine-freeze-extent/build"
mkdir -p "$extent_build"
flags=(-std=c++17 -O2 -pthread)
suffix=optimized
if [[ "${1:-}" == "--sanitize" ]]; then flags+=(-g -fsanitize=address,undefined -fno-omit-frame-pointer); suffix=asan-ubsan; fi
clang++ "${flags[@]}" -I Sources/TrackerEngine/include Sources/TrackerEngine/TrackerEngine.cpp \
  Sources/TrackerEngine/TrackerClock.cpp Sources/TrackerEngine/ClockTimingMap.cpp \
  tests/engine_freeze_extent_tests.cpp -o "$extent_build/freeze-extent-$suffix"
"$extent_build/freeze-extent-$suffix"

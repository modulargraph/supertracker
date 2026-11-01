#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ack_build="$PWD/test-output/engine-input-ack/build"
mkdir -p "$ack_build"
flags=(-std=c++17 -O2 -pthread)
suffix=optimized
if [[ "${1:-}" == "--sanitize" ]]; then flags+=(-g -fsanitize=address,undefined -fno-omit-frame-pointer); suffix=asan-ubsan; fi
if [[ "${1:-}" == "--thread-sanitize" ]]; then flags+=(-g -fsanitize=thread); suffix=tsan; fi
clang++ "${flags[@]}" -I Sources/TrackerEngine/include Sources/TrackerEngine/TrackerEngine.cpp \
  Sources/TrackerEngine/TrackerClock.cpp Sources/TrackerEngine/ClockTimingMap.cpp \
  tests/engine_input_ack_tests.cpp -o "$ack_build/input-ack-$suffix"
"$ack_build/input-ack-$suffix"

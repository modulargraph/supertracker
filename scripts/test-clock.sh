#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p test-output/clock
compiler="${CLOCK_CXX:-clang++}"
flags=(-std=c++17 -Wall -Wextra -Wpedantic -Werror)
output=test-output/clock/clock-tests
if [[ "${1:-}" == "--sanitize" ]]; then
  flags+=(-O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer)
  output=test-output/clock/clock-tests-sanitized
else
  flags+=(-O2)
fi
"$compiler" "${flags[@]}" -I Sources/TrackerEngine/include Sources/TrackerEngine/ClockTimingMap.cpp Sources/TrackerEngine/TrackerClock.cpp tests/clock_tests.cpp -o "$output"
"$output"

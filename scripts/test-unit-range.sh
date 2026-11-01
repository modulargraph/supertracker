#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p test-output/unit-range
compiler="${UNIT_RANGE_CXX:-clang++}"
flags=(-std=c++17 -pthread -Wall -Wextra -Wpedantic -Werror)
output=test-output/unit-range/unit-range-tests
if [[ "${1:-}" == "--sanitize" ]]; then
  flags+=(-O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer)
  output=test-output/unit-range/unit-range-tests-sanitized
else
  flags+=(-O2)
fi
"$compiler" "${flags[@]}" -I Sources/TrackerEngine/include \
  Sources/TrackerEngine/ClockTimingMap.cpp Sources/TrackerEngine/TrackerClock.cpp \
  Sources/TrackerEngine/TrackerUnitRange.cpp tests/unit_range_tests.cpp -o "$output"
"$output"

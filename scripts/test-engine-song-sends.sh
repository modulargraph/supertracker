#!/bin/bash
# Song order, song section loop and delay/reverb send return parameters (engine only).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build/test-bin
for flags in "-O2" "-O1 -g -fsanitize=address,undefined"; do
  # shellcheck disable=SC2086
  clang++ -std=c++17 $flags -pthread -I Sources/TrackerEngine/include Sources/TrackerEngine/TrackerEngine.cpp Sources/TrackerEngine/TrackerClock.cpp Sources/TrackerEngine/ClockTimingMap.cpp tests/engine_song_sends_tests.cpp -o .build/test-bin/engine-song-sends
  .build/test-bin/engine-song-sends
done

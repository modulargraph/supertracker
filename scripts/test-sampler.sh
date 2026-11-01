#!/bin/bash
# Sample instrument and sample editor: engine play-mode PCM checks, editor backend and editor UI.
set -euo pipefail
cd "$(dirname "$0")/.."
out="$PWD/.build/test-bin"; mkdir -p "$out" "$PWD/.build/clang-module-cache"
flags=(-std=c++17 -O2 -pthread)
if [[ "${1:-}" == "--sanitize" ]]; then flags=(-std=c++17 -O1 -g -pthread -fsanitize=address,undefined -fno-omit-frame-pointer); fi
clang++ "${flags[@]}" -I Sources/TrackerEngine/include Sources/TrackerEngine/TrackerEngine.cpp \
  Sources/TrackerEngine/TrackerClock.cpp Sources/TrackerEngine/ClockTimingMap.cpp tests/sampler_modes_tests.cpp -o "$out/sampler-modes"
"$out/sampler-modes"
swiftc -O -module-cache-path "$PWD/.build/clang-module-cache" Sources/SuperTracker/AudioSamples.swift Sources/SuperTracker/AudioStretch.swift \
  tests/SampleEditorTests.swift -o "$out/sample-editor"
"$out/sample-editor"
node --check Resources/Web/sampler.js
node tests/sampler_editor_ui_tests.js

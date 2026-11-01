#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build/test-bin
clang++ -std=c++17 -O2 -pthread -I Sources/TrackerEngine/include \
  Sources/TrackerEngine/TrackerEngine.cpp Sources/TrackerEngine/TrackerClock.cpp \
  Sources/TrackerEngine/ClockTimingMap.cpp tests/engine_dsp_regressions.cpp \
  -o .build/test-bin/engine-dsp
.build/test-bin/engine-dsp
clang++ -std=c++17 -O2 -pthread -I Sources/TrackerEngine/include \
  Sources/TrackerEngine/TrackerEngine.cpp Sources/TrackerEngine/TrackerClock.cpp \
  Sources/TrackerEngine/ClockTimingMap.cpp tests/engine_parameter_bounds_tests.cpp \
  -o .build/test-bin/engine-parameter-bounds
.build/test-bin/engine-parameter-bounds

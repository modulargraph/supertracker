#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build/test-bin
clang++ -std=c++17 -O2 -Wall -Wextra -I Sources/TrackerEngine -I Sources/TrackerEngine/include \
  Sources/TrackerEngine/BuiltinEffects.cpp tests/drum_synth_tests.cpp \
  -o .build/test-bin/drum-synths
.build/test-bin/drum-synths

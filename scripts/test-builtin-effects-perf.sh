#!/bin/bash
# CPU budget for the built-in effects: every catalogue effect (defaults and a
# typical Maschine-kit setting) must stay under a generous ceiling of 3% of
# realtime per instance (48 kHz, 128-frame blocks, median of 5 trials), and a
# silence-gated instance must be near-free. Ceilings are ~10x the measured
# cost on an Apple M4, so the check only trips on real regressions.
# Built at -Os: that is what SwiftPM's release configuration uses for the
# TrackerEngine C++ target, so this measures what the app actually runs.
# `.build/test-bin/builtin-effects-perf --bench` prints the full table.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build/test-bin
clang++ -std=c++17 -Os -Wall -Wextra -I Sources/TrackerEngine/include \
  Sources/TrackerEngine/BuiltinEffects.cpp tests/builtin_effects_perf_tests.cpp \
  -o .build/test-bin/builtin-effects-perf
.build/test-bin/builtin-effects-perf

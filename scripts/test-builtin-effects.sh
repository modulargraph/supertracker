#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build/test-bin
clang++ -std=c++17 -O2 -pthread -Wall -Wextra -I Sources/TrackerEngine/include \
  Sources/TrackerEngine/BuiltinEffects.cpp tests/builtin_effects_tests.cpp \
  -o .build/test-bin/builtin-effects
.build/test-bin/builtin-effects

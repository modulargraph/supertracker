#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build/test-bin .build/test-modules/TrackerEngine
cat > .build/test-modules/TrackerEngine/module.modulemap <<'MODULE'
module TrackerEngine {
  header "../../../Sources/TrackerEngine/include/TrackerEngine.h"
  export *
}
MODULE
# The unit renders through the Objective-C render shim (Sources/AURenderShim).
mkdir -p .build/test-modules/AURenderShim
cat > .build/test-modules/AURenderShim/module.modulemap <<'MODULE'
module AURenderShim {
  header "../../../Sources/AURenderShim/include/AURenderShim.h"
  export *
}
MODULE
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-$PWD/.build/clang-module-cache}"
clang -fobjc-arc -fmodules -fmodules-cache-path="$CLANG_MODULE_CACHE_PATH" -O2 -I Sources/AURenderShim/include -c Sources/AURenderShim/AURenderShim.m -o .build/test-bin/builtin-adapter-shim.o
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/BuiltinEffects.cpp -o .build/test-bin/builtin-effects-adapter.o
swiftc -O -swift-version 5 -I .build/test-modules -module-cache-path "$CLANG_MODULE_CACHE_PATH" Sources/SuperTracker/BuiltinEffectsAudioUnit.swift Sources/SuperTracker/AudioRenderUnit.swift tests/BuiltinEffectsAudioUnitTests.swift .build/test-bin/builtin-effects-adapter.o .build/test-bin/builtin-adapter-shim.o -Xlinker -lc++ -o .build/test-bin/builtin-effect-adapter
.build/test-bin/builtin-effect-adapter

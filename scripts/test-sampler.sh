#!/bin/bash
# Sample editor: editor backend and editor UI. (The engine's play-mode PCM checks are the Rust test
# rust/tracker-engine/tests/sampler_modes.rs.)
set -euo pipefail
cd "$(dirname "$0")/.."
out="$PWD/.build/test-bin"; mkdir -p "$out" "$PWD/.build/clang-module-cache"
swiftc -O -module-cache-path "$PWD/.build/clang-module-cache" Sources/SuperTracker/AudioSamples.swift Sources/SuperTracker/AudioStretch.swift \
  tests/SampleEditorTests.swift -o "$out/sample-editor"
"$out/sample-editor"
node --check Resources/Web/sampler.js
node tests/sampler_editor_ui_tests.js

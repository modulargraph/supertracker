#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_build="${1:-$PWD/test-output/freeze-track-fingerprint}"
mkdir -p "$test_build/module-cache"
xcrun swiftc -O -swift-version 5 -module-cache-path "$test_build/module-cache" \
  Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioFreezeTrackFingerprint.swift tests/AudioFreezeTrackFingerprintTests.swift \
  -o "$test_build/freeze-track-fingerprint-tests"
"$test_build/freeze-track-fingerprint-tests" "$test_build"

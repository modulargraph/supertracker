#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
sampler_test_output="${1:-$PWD/test-output/maschine-sampler-mapping}"
mkdir -p "$sampler_test_output/module-cache"
xcrun swiftc -O -swift-version 5 -module-cache-path "$sampler_test_output/module-cache" \
  Sources/SuperTracker/MaschineGroupSamplerMapping.swift tests/MaschineGroupSamplerMappingTests.swift \
  -o "$sampler_test_output/sampler-mapping-tests"
"$sampler_test_output/sampler-mapping-tests" | tee "$sampler_test_output/results.txt"

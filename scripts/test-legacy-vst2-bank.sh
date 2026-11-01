#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
output="${1:-test-output/legacy-vst2-bank}"
mkdir -p "$output"
swiftc -O -module-cache-path "$output/cache" Sources/SuperTracker/AudioLegacyVST2Bank.swift tests/AudioLegacyVST2BankTests.swift -o "$output/bank-tests"
"$output/bank-tests" | tee "$output/report.json"

#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
export_transaction_output="$PWD/test-output/audio-export-transaction"
export_transaction_build="$export_transaction_output/build"
mkdir -p "$export_transaction_build/module-cache"
swiftc -O -swift-version 5 -module-cache-path "$export_transaction_build/module-cache" \
  Sources/SuperTracker/AudioExportTransaction.swift tests/AudioExportTransactionTests.swift \
  -o "$export_transaction_build/export-transaction-tests"
"$export_transaction_build/export-transaction-tests" "$export_transaction_output"
shasum -a 256 Sources/SuperTracker/AudioExportTransaction.swift tests/AudioExportTransactionTests.swift \
  scripts/test-audio-export-transaction.sh "$export_transaction_build/export-transaction-tests" \
  "$export_transaction_output/report.json" > "$export_transaction_output/SHA256SUMS"

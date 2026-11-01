#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."
METADATA_TEST_OUTPUT="$PWD/test-output/library-metadata"
mkdir -p "$METADATA_TEST_OUTPUT/module-cache"

shasum -a 256 Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift Sources/SuperTracker/NKSMetadata.swift tests/LibraryMetadataTests.swift > "$METADATA_TEST_OUTPUT/source-sha256.txt"
xcrun swiftc -D SELFTESTS -swift-version 5 -module-cache-path "$METADATA_TEST_OUTPUT/module-cache" \
  Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift Sources/SuperTracker/NKSMetadata.swift tests/LibraryMetadataTests.swift \
  -o "$METADATA_TEST_OUTPUT/library-metadata-tests" 2>&1 | tee "$METADATA_TEST_OUTPUT/compile.txt"
shasum -a 256 -c "$METADATA_TEST_OUTPUT/source-sha256.txt"
"$METADATA_TEST_OUTPUT/library-metadata-tests" | tee "$METADATA_TEST_OUTPUT/results.txt"

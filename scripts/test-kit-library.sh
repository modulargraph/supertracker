#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
KIT_LIBRARY_TEST_OUTPUT="${1:-$PWD/test-output/kit-library}"
mkdir -p "$KIT_LIBRARY_TEST_OUTPUT" .build/clang-module-cache
shasum -a 256 Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift \
  Sources/SuperTracker/NKSMetadata.swift tests/KitLibraryTests.swift tests/KitLibraryIndexTests.swift \
  > "$KIT_LIBRARY_TEST_OUTPUT/source-sha256.txt"
xcrun swiftc -swift-version 5 -module-cache-path .build/clang-module-cache \
  Sources/SuperTracker/KitLibrary.swift tests/KitLibraryTests.swift \
  -o "$KIT_LIBRARY_TEST_OUTPUT/package-tests"
"$KIT_LIBRARY_TEST_OUTPUT/package-tests" | tee "$KIT_LIBRARY_TEST_OUTPUT/package-results.txt"
xcrun swiftc -D SELFTESTS -swift-version 5 -module-cache-path .build/clang-module-cache \
  Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift Sources/SuperTracker/NKSMetadata.swift \
  tests/KitLibraryIndexTests.swift -o "$KIT_LIBRARY_TEST_OUTPUT/index-tests"
"$KIT_LIBRARY_TEST_OUTPUT/index-tests" | tee "$KIT_LIBRARY_TEST_OUTPUT/index-results.txt"
shasum -a 256 -c "$KIT_LIBRARY_TEST_OUTPUT/source-sha256.txt"

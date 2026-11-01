#!/bin/bash
# Kit library cache migration: private generated cache and library roots only; no explicit
# library scan, installed content, native host or audio.
set -euo pipefail
cd "$(dirname "$0")/.."

mkdir -p test-output .build/clang-module-cache
build=$(mktemp -d "$PWD/test-output/kit-library-migration.XXXXXX")
# Remove the private build on success; keep it for debugging when a step fails.
trap 'status=$?; if [ "$status" -eq 0 ]; then rm -rf "$build"; else echo "Kept for debugging: $build" >&2; fi' EXIT
xcrun swiftc -D SELFTESTS -swift-version 5 -module-cache-path "$PWD/.build/clang-module-cache" \
  Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift \
  Sources/SuperTracker/NKSMetadata.swift tests/KitLibraryMigrationTests.swift \
  -o "$build/kit-library-migration-tests" 2>&1 | tee "$build/compile.log"
"$build/kit-library-migration-tests" 2>&1 | tee "$build/results.log"

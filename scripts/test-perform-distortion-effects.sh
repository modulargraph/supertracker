#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_build=$(mktemp -d /private/tmp/supertracker-perform-distortion.XXXXXX)
trap 'rm -rf "$test_build"' EXIT
for family in perform distortion; do
  clang++ -std=c++17 -O2 -Wall -Wextra -Werror "tests/effects_${family}_tests.cpp" -o "$test_build/$family"
  "$test_build/$family"
done

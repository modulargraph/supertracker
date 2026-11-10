#!/bin/bash
# Build the Rust engine (rust/tracker-engine) as a static library.
#   scripts/build-engine.sh          release library for the app: rust/target/release/libtracker_engine.a
#   scripts/build-engine.sh tests    library for the C++ and Swift test programs:
#                                    .build/rust-tests/release/libtracker_engine.a
# The test build routes Rust allocations through C++ operator new (feature cxx-alloc), so the test
# programs that count render-thread allocations by replacing operator new also see the engine's.
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
  echo "The engine is written in Rust and needs cargo. Install Rust (https://rustup.rs), or build the" >&2
  echo "previous C++ engine instead with SUPERTRACKER_ENGINE=cpp scripts/build.sh." >&2
  exit 69
fi
# Match the app's deployment target so the linker does not warn about newer object files.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
case "${1:-app}" in
  app)
    cargo build --release --manifest-path rust/Cargo.toml -p tracker-engine
    echo "rust/target/release/libtracker_engine.a" ;;
  tests)
    CARGO_TARGET_DIR="$PWD/.build/rust-tests" cargo build --release --manifest-path rust/Cargo.toml -p tracker-engine --features cxx-alloc
    echo ".build/rust-tests/release/libtracker_engine.a" ;;
  *) echo "Usage: scripts/build-engine.sh [app|tests]" >&2; exit 64 ;;
esac

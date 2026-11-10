#!/bin/bash
# Build a debug host and run it from the repository (the interface is loaded from ui/).
#   scripts/run.sh             the window
#   scripts/run.sh --serve     headless: prints a URL to open in a browser
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build -p st-app
CARGO_TARGET_DIR="$TARGET" cargo build --manifest-path crates/plugin-host/Cargo.toml -p stpc-container
exec "$TARGET/debug/supertracker" "$@"

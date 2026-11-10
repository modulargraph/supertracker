#!/bin/bash
# Build SuperTracker.
#   scripts/build.sh            release build
#   scripts/build.sh debug      debug build
# macOS: build/SuperTracker.app (ad-hoc signed). Elsewhere: build/supertracker/ with the executable,
# the plug-in container and the interface folder side by side.
set -euo pipefail
cd "$(dirname "$0")/.."
CONFIGURATION="${1:-release}"
case "$CONFIGURATION" in
  release) FLAG=(--release) ;;
  debug) FLAG=() ;;
  *) echo "Usage: scripts/build.sh [release|debug]" >&2; exit 64 ;;
esac
if ! command -v cargo >/dev/null 2>&1; then
  echo "SuperTracker is written in Rust and needs cargo. Install it from https://rustup.rs." >&2
  exit 69
fi
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
TARGET="${CARGO_TARGET_DIR:-$PWD/target}"
# The host, then the plug-in container (its own workspace), into one target folder.
cargo build ${FLAG[@]+"${FLAG[@]}"} -p st-app
CARGO_TARGET_DIR="$TARGET" cargo build ${FLAG[@]+"${FLAG[@]}"} --manifest-path crates/plugin-host/Cargo.toml -p stpc-container
OUT="$TARGET/$CONFIGURATION"
if [[ "$(uname)" == Darwin ]]; then
  APP="$PWD/build/SuperTracker.app"
  mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
  cp "$OUT/supertracker" "$APP/Contents/MacOS/SuperTracker"
  # The host looks for the container next to its own executable.
  cp "$OUT/supertracker-plugin-container" "$APP/Contents/MacOS/supertracker-plugin-container"
  cp packaging/macos/Info.plist "$APP/Contents/Info.plist"
  rsync -a --delete ui/ "$APP/Contents/Resources/ui/"
  codesign --force --deep --sign - "$APP"
  echo "Built $APP"
else
  DIR="$PWD/build/supertracker"
  mkdir -p "$DIR"
  cp "$OUT/supertracker" "$OUT/supertracker-plugin-container" "$DIR/"
  rm -rf "$DIR/ui" && cp -R ui "$DIR/ui"
  echo "Built $DIR"
fi

#!/bin/bash
# Build a release app and install it as /Applications/SuperTracker.app.
# The running copy keeps working until it quits (the old bundle is moved aside, not overwritten in place);
# the previous install is kept zipped as build/SuperTracker-Applications-backup.zip (a zip, so Spotlight
# doesn't offer it as a second SuperTracker).
set -euo pipefail
cd "$(dirname "$0")/.."
DEST="${1:-/Applications/SuperTracker.app}"
STAGE="$PWD/build/SuperTracker-install.app"
./scripts/build.sh release "$STAGE"
if [[ -d "$DEST" ]]; then
  rm -f build/SuperTracker-Applications-backup.zip
  ditto -c -k --keepParent "$DEST" build/SuperTracker-Applications-backup.zip
  rm -rf "$DEST"
fi
mv "$STAGE" "$DEST"
codesign --verify --deep "$DEST"
# The repo's build/SuperTracker.app can also be opened (Spotlight, Open With, the MCP bridge); keep it the same build.
if [[ "$DEST" == /Applications/SuperTracker.app && -d build/SuperTracker.app ]]; then
  rm -rf build/SuperTracker.app.previous; mv build/SuperTracker.app build/SuperTracker.app.previous
  ditto "$DEST" build/SuperTracker.app && rm -rf build/SuperTracker.app.previous
fi
echo "Installed $DEST (previous copy: build/SuperTracker-Applications-backup.zip). Quit and reopen SuperTracker to use it."

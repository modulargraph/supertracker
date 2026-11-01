#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
CONFIGURATION="${1:-release}"
APP="${2:-$PWD/build/SuperTracker.app}"
case "$APP" in
  /*) ;;
  *) APP="$PWD/$APP" ;;
esac
if [[ "$APP" != *.app ]]; then
  echo "The output path must end in .app" >&2
  exit 64
fi
if [[ ! -f Vendor/vst3sdk/pluginterfaces/base/funknown.h || ! -f Vendor/vst3sdk/LICENSE.txt ]]; then
  echo "The VST3 SDK is missing. Run: git submodule update --init --recursive" >&2
  exit 66
fi
# One warm, shared build tree: debug and release both live in .build (.build/debug, .build/release), so switching
# configuration or output path never starts from scratch. Do not point builds at fresh scratch paths.
# selftest = optimised build with the in-app self-tests compiled in (#if SELFTESTS); it has its own tree so the
# shipping release objects in .build/release stay warm. debug builds always include the self-tests.
SCRATCH=.build
SWIFT_FLAGS=()
case "$CONFIGURATION" in
  debug|release) SWIFT_CONFIGURATION="$CONFIGURATION" ;;
  selftest) SWIFT_CONFIGURATION=release; SCRATCH=.build-selftest; SWIFT_FLAGS=(-Xswiftc -DSELFTESTS) ;;
  *) echo "Usage: scripts/build.sh [debug|release|selftest] [path/to/App.app]" >&2; exit 64 ;;
esac
export CLANG_MODULE_CACHE_PATH="$PWD/.build/clang-module-cache"
build_started=$SECONDS
swift build --disable-sandbox -c "$SWIFT_CONFIGURATION" --scratch-path "$SCRATCH" ${SWIFT_FLAGS[@]+"${SWIFT_FLAGS[@]}"}
echo "Compiled $CONFIGURATION in $((SECONDS - build_started))s"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$SCRATCH/$SWIFT_CONFIGURATION/SuperTracker" "$APP/Contents/MacOS/SuperTracker"
cp Resources/Info.plist "$APP/Contents/Info.plist"
mkdir -p "$APP/Contents/Resources/Licenses"
cp Vendor/vst3sdk/LICENSE.txt "$APP/Contents/Resources/Licenses/VST3-SDK.txt"
rsync -a --delete Resources/Web/ "$APP/Contents/Resources/Web/"
codesign --force --deep --sign - "$APP"
echo "Built $APP"
# The standard build also refreshes the installed copy so Launchpad/Spotlight always open the latest version.
# Only a release build of the main checkout: a debug build (scripts/run.sh) or a linked git worktree's branch build
# must not silently replace the app the user runs (scripts/install.sh installs any build deliberately).
refreshes_installed_copy() { # configuration app-path
  [[ "$1" == release && "$2" == "$PWD/build/SuperTracker.app" ]] || return 1
  local dirs; dirs=$(git rev-parse --path-format=absolute --git-dir --git-common-dir 2>/dev/null) || return 1
  [[ "$(sed -n 1p <<<"$dirs")" == "$(sed -n 2p <<<"$dirs")" ]]
}
if refreshes_installed_copy "$CONFIGURATION" "$APP" && [[ -d /Applications/SuperTracker.app && ! -L /Applications/SuperTracker.app ]]; then
  rsync -a --delete "$APP/" /Applications/SuperTracker.app/
  echo "Updated /Applications/SuperTracker.app"
fi

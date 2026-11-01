#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
CONFIGURATION="${1:-debug}"
QA_APP="$PWD/build/KeyboardQA/SuperTrackerKeyboardQA.app"
./scripts/build.sh "$CONFIGURATION" "$QA_APP"
python3 - "$QA_APP" "$PWD/test-output/keyboard-native" <<'PY'
import pathlib, plistlib, sys
bundle = pathlib.Path(sys.argv[1])
info = bundle / 'Contents/Info.plist'
with info.open('rb') as stream:
    data = plistlib.load(stream)
data.update(CFBundleName='SuperTracker Keyboard QA',
            CFBundleDisplayName='SuperTracker Keyboard QA',
            CFBundleIdentifier='studio.supertracker.keyboardqa',
            CFBundleExecutable='SuperTrackerKeyboardQA',
            SuperTrackerKeyboardAcceptance=True,
            SuperTrackerKeyboardAcceptanceOutput=sys.argv[2])
data.pop('CFBundleDocumentTypes', None)
data.pop('UTExportedTypeDeclarations', None)
with info.open('wb') as stream:
    plistlib.dump(data, stream, sort_keys=False)
(bundle / 'Contents/MacOS/SuperTracker').replace(bundle / 'Contents/MacOS/SuperTrackerKeyboardQA')
PY
codesign --force --deep --sign - "$QA_APP"
echo "Prepared isolated keyboard QA app without launching: $QA_APP"

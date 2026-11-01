#!/bin/bash
# VST2 host: CC 120/123 and reset() release held notes explicitly (original fixture, no device).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p test-output
build=$(mktemp -d "$PWD/test-output/vst2-midi-panic.XXXXXX")
# Remove the private build on success; keep it for debugging when a step fails.
trap 'status=$?; if [ "$status" -eq 0 ]; then rm -rf "$build"; else echo "Kept for debugging: $build" >&2; fi' EXIT
mkdir -p "$build/MIDINotes.vst/Contents/MacOS"
python3 - "$build/MIDINotes.vst/Contents/Info.plist" <<'PY'
import pathlib, plistlib, sys
pathlib.Path(sys.argv[1]).write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'studio.supertracker.tests.midi-notes', 'CFBundleExecutable': 'MIDINotes',
    'CFBundlePackageType': 'BNDL', 'CFBundleName': 'Original MIDI Notes VST2 Fixture', 'CFBundleVersion': '1'}))
PY
clang++ -std=c++17 -O1 -g -bundle -DORIGINAL_MIDI_NOTES tests/vst2_debug_fixture.cpp -o "$build/MIDINotes.vst/Contents/MacOS/MIDINotes"
clang++ -std=c++17 -O1 -g -fobjc-arc -fsanitize=address,undefined -I Sources/VST2Host/include \
  Sources/VST2Host/VST2Host.mm tests/vst2_midi_panic_tests.mm \
  -framework AppKit -framework Foundation -framework CoreFoundation -o "$build/vst2-midi-panic-tests"
"$build/vst2-midi-panic-tests" "$build/MIDINotes.vst"

#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_build="$PWD/test-output/freeze-media/build"
mkdir -p "$test_build/module/TrackerEngine" "$test_build/module-cache"
cat > "$test_build/module/TrackerEngine/module.modulemap" <<MODULE
module TrackerEngine {
  header "$PWD/Sources/TrackerEngine/include/TrackerEngine.h"
  export *
}
MODULE
# Pure timing math and temporary files; no audio engine, devices or vendor code.
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/TrackerClock.cpp -o "$test_build/clock.o"
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/ClockTimingMap.cpp -o "$test_build/map.o"
swiftc -O -swift-version 5 -module-cache-path "$test_build/module-cache" -I "$test_build/module" \
  Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRouting.swift \
  Sources/SuperTracker/AudioFreezeDurationProof.swift Sources/SuperTracker/AudioFreezeProject.swift Sources/SuperTracker/AudioFreezeTrackFingerprint.swift Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/ProjectMigration.swift Sources/SuperTracker/KitGroups.swift Sources/SuperTracker/BuiltinWavetables.swift Sources/SuperTracker/WavetableFormat.swift \
  Sources/SuperTracker/MediaRelinker.swift tests/FreezeMediaTests.swift \
  "$test_build/clock.o" "$test_build/map.o" -Xlinker -lc++ -o "$test_build/freeze-media-tests"
"$test_build/freeze-media-tests"

#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build/test-bin .build/clang-module-cache .build/test-modules/TrackerEngine
export CLANG_MODULE_CACHE_PATH="$PWD/.build/clang-module-cache"
cat > .build/test-modules/TrackerEngine/module.modulemap <<'MODULE'
module TrackerEngine {
  header "../../../Sources/TrackerEngine/include/TrackerEngine.h"
  export *
}
MODULE
# Objective-C render blocks for the Swift Audio Units (Sources/AURenderShim), as a module for swiftc.
mkdir -p .build/test-modules/AURenderShim
cat > .build/test-modules/AURenderShim/module.modulemap <<'MODULE'
module AURenderShim {
  header "../../../Sources/AURenderShim/include/AURenderShim.h"
  export *
}
MODULE
clang -fobjc-arc -fmodules -fmodules-cache-path="$CLANG_MODULE_CACHE_PATH" -O2 -I Sources/AURenderShim/include -c Sources/AURenderShim/AURenderShim.m -o .build/test-bin/au-render-shim.o
# Project persistence validates freeze policy using only immutable Clock math.
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/TrackerClock.cpp -o .build/test-bin/freeze-clock.o
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/ClockTimingMap.cpp -o .build/test-bin/freeze-clock-map.o
freeze_test_support=(-I .build/test-modules Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/ProjectMigration.swift Sources/SuperTracker/KitGroups.swift Sources/SuperTracker/BuiltinWavetables.swift Sources/SuperTracker/WavetableFormat.swift Sources/SuperTracker/AudioFreezeDurationProof.swift Sources/SuperTracker/AudioFreezeProject.swift Sources/SuperTracker/AudioFreezeTrackFingerprint.swift .build/test-bin/freeze-clock.o .build/test-bin/freeze-clock-map.o -Xlinker -lc++)
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioMIDITimebase.swift tests/AudioMIDITimebaseTests.swift -o .build/test-bin/midi-timebase
.build/test-bin/midi-timebase
bash scripts/test-audio-freeze-duration-proof.sh
# Project migration (tempo maps, built-in synths), built-in wavetables and wavetable file detection.
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectMigration.swift Sources/SuperTracker/KitGroups.swift Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/BuiltinWavetables.swift Sources/SuperTracker/WavetableFormat.swift tests/ProjectMigrationTests.swift -o .build/test-bin/project-migration
.build/test-bin/project-migration
bash scripts/test-audio-recording-location.sh
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioDeviceSelection.swift Sources/SuperTracker/AudioLineInputTopology.swift tests/AudioLineInputTopologyTests.swift -o .build/test-bin/line-input-topology
.build/test-bin/line-input-topology
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioDeviceSelection.swift Sources/SuperTracker/AudioDevicePreferences.swift tests/AudioDevicePreferencesTests.swift -o .build/test-bin/audio-device-preferences
.build/test-bin/audio-device-preferences
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/MIDIHost.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MIDIClockOutput.swift tests/MIDIClockOutputTests.swift -o .build/test-bin/midi-clock
.build/test-bin/midi-clock
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioStallWatch.swift tests/AudioStallWatchTests.swift -o .build/test-bin/audio-stall-watch
.build/test-bin/audio-stall-watch
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/BridgeDelivery.swift tests/BridgeDeliveryTests.swift -o .build/test-bin/bridge-delivery
.build/test-bin/bridge-delivery
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift tests/ProjectStoreTests.swift "${freeze_test_support[@]}" -o .build/test-bin/store
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift tests/ProjectStoreBoundaryTests.swift "${freeze_test_support[@]}" -o .build/test-bin/store-boundaries
.build/test-bin/store-boundaries
.build/test-bin/store
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/KeyboardAcceptancePolicy.swift tests/KeyboardAcceptancePolicyTests.swift "${freeze_test_support[@]}" -o .build/test-bin/keyboard-policy
.build/test-bin/keyboard-policy
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/PluginSnapshotCapture.swift tests/PluginSnapshotCaptureTests.swift "${freeze_test_support[@]}" -o .build/test-bin/plugin-snapshot
.build/test-bin/plugin-snapshot
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ExportDestination.swift tests/ExportDestinationTests.swift -o .build/test-bin/export-destination
.build/test-bin/export-destination
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioExportFormat.swift tests/AudioExportFormatTests.swift -o .build/test-bin/export-format
.build/test-bin/export-format
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRenderSelection.swift tests/AudioRenderSelectionTests.swift -o .build/test-bin/render-selection
.build/test-bin/render-selection
swiftc -O -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioExportTransaction.swift tests/AudioExportTransactionTests.swift -o .build/test-bin/export-transaction
.build/test-bin/export-transaction "$PWD/.build/test-output/export-transaction"
swiftc -O -module-cache-path .build/clang-module-cache -framework AVFoundation Sources/SuperTracker/AudioExportFormat.swift Sources/SuperTracker/AudioExportDelivery.swift tests/AudioExportDeliveryTests.swift -o .build/test-bin/export-delivery
export_delivery_test_output="$(mktemp -d "$PWD/.build/test-output/export-delivery.XXXXXX")"
.build/test-bin/export-delivery "$export_delivery_test_output"; rm -rf "$export_delivery_test_output"
swiftc -O -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioSamples.swift Sources/SuperTracker/AudioStretch.swift tests/AudioStretchTests.swift -o .build/test-bin/stretch
.build/test-bin/stretch
swiftc -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioSamples.swift Sources/SuperTracker/AudioStretch.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/SampleSlicing.swift tests/SampleSlicingTests.swift -o .build/test-bin/slicing
.build/test-bin/slicing
swiftc -O -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioMeters.swift tests/AudioMetersTests.swift -o .build/test-bin/audio-meters
.build/test-bin/audio-meters
# Browser previews: crossfades, cache, streamed tails and an offline before/after render (never the output device).
swiftc -O -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioPreview.swift tests/AudioPreviewTests.swift -o .build/test-bin/audio-preview
.build/test-bin/audio-preview
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MIDIFile.swift Sources/SuperTracker/ProjectMigration.swift Sources/SuperTracker/KitGroups.swift Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/BuiltinWavetables.swift Sources/SuperTracker/WavetableFormat.swift tests/MIDIFileTests.swift -o .build/test-bin/midi
.build/test-bin/midi
swiftc -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/MIDISustainState.swift tests/MIDISustainStateTests.swift -o .build/test-bin/midi-sustain
swiftc -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MaschineMK3Protocol.swift Sources/SuperTracker/MaschineMK3Screen.swift tests/MaschineMK3ProtocolTests.swift -o .build/test-bin/mk3-protocol
.build/test-bin/mk3-protocol
bash scripts/test-mk3-lifecycle.sh
bash scripts/test-plugin-parameter-metadata.sh
bash scripts/test-builtin-effect-adapter.sh
bash tests/run_maschine_core_mapping_tests.sh
bash tests/run_maschine_additional_mapping_tests.sh
bash tests/run_maschine_distortion_mapping_tests.sh
bash scripts/test-maschine-drum-mapping.sh
bash scripts/test-maschine-sampler-mapping.sh
bash scripts/test-maschine-group-import.sh
bash scripts/test-kit-groups-migration.sh
# Sixteen inserts per channel: render-endpoint and automation-offset layouts never collide.
swiftc -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/MixerChannel.swift tests/MixerChannelTests.swift -o .build/test-bin/mixer-channel
.build/test-bin/mixer-channel
swiftc -O -swift-version 5 Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift tests/KitProjectTests.swift -o .build/test-bin/kit-project
.build/test-bin/kit-project
.build/test-bin/midi-sustain
bash scripts/test-kit-library.sh
bash scripts/test-kit-library-migration.sh
bash scripts/test-maschine-plugin-state.sh
bash scripts/test-vst2-bank-state.sh
bash scripts/test-vst2-midi-panic.sh
bash scripts/test-legacy-vst2-bank.sh
bash scripts/test-vst3-legacy-bank.sh
bash scripts/test-vst3-module-exit.sh
swiftc -D SELFTESTS -O -module-cache-path .build/clang-module-cache Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift Sources/SuperTracker/NKSMetadata.swift tests/LibraryStressTests.swift -o .build/test-bin/library
.build/test-bin/library
swiftc -D SELFTESTS -module-cache-path .build/clang-module-cache Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift Sources/SuperTracker/NKSMetadata.swift tests/LibraryTests.swift -o .build/test-bin/library-functional
.build/test-bin/library-functional
swiftc -D SELFTESTS -module-cache-path .build/clang-module-cache Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift Sources/SuperTracker/NKSMetadata.swift tests/LibraryFilterTests.swift -o .build/test-bin/library-filters
.build/test-bin/library-filters
bash scripts/test-library-metadata.sh
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/NKSMetadata.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/NKSHostPreset.swift tests/NKSHostPresetTests.swift -o .build/test-bin/nks-host
.build/test-bin/nks-host
# AI control: JSON-RPC line framing, control-socket client and MCP stdio server.
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AgentControlProtocol.swift Sources/SuperTracker/MCPServer.swift tests/AgentMCPTests.swift -o .build/test-bin/agent-mcp
.build/test-bin/agent-mcp
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/PluginRecoveryStore.swift tests/PluginRecoveryStoreTests.swift -o .build/test-bin/plugin-recovery
.build/test-bin/plugin-recovery
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/VST3Catalogue.swift tests/VST3CatalogueTests.swift -o .build/test-bin/vst3-catalogue
.build/test-bin/vst3-catalogue
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/VST3Compatibility.swift tests/VST3CompatibilityTests.swift -o .build/test-bin/vst3-compatibility
.build/test-bin/vst3-compatibility
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/VST3StatePreparation.swift tests/VST3StatePreparationTests.swift -o .build/test-bin/vst3-state-preparation
.build/test-bin/vst3-state-preparation
swiftc -O -swift-version 5 -D ROUTING_AUTOMATION_TEST -I .build/test-modules -module-cache-path .build/clang-module-cache Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/AudioRoutingAutomation.swift tests/audio_routing_tests.swift -o .build/test-bin/routing
.build/test-bin/routing
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/TrackerEngine.cpp -o .build/test-bin/plugin-step-engine.o
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/TrackerClock.cpp -o .build/test-bin/plugin-step-clock.o
clang++ -std=c++17 -O2 -I Sources/TrackerEngine/include -c Sources/TrackerEngine/ClockTimingMap.cpp -o .build/test-bin/plugin-step-clock-map.o
swiftc -O -swift-version 5 -I .build/test-modules -module-cache-path .build/clang-module-cache Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/AudioRoutingAutomation.swift Sources/SuperTracker/AudioPluginStepFX.swift Sources/SuperTracker/AudioProject.swift tests/PluginStepFXTests.swift .build/test-bin/plugin-step-engine.o .build/test-bin/plugin-step-clock.o .build/test-bin/plugin-step-clock-map.o -Xlinker -lc++ -o .build/test-bin/plugin-step-adapter
.build/test-bin/plugin-step-adapter
swiftc -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MIDIHost.swift Sources/SuperTracker/AudioRenderControl.swift tests/audio_render_control_tests.swift -o .build/test-bin/render-control
swiftc -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MIDIHost.swift Sources/SuperTracker/AudioRenderControl.swift tests/AudioRenderUtilizationTests.swift -o .build/test-bin/render-utilization
.build/test-bin/render-utilization
.build/test-bin/render-control
python3 tests/vst3_runner_tests.py
python3 tests/nks_ui_diagnostic_tests.py
python3 tests/nks_diagnostic_build_tests.py
python3 tests/native_regression_runner_tests.py
python3 tests/native_app_registration_tests.py
python3 tests/routing_diagnostic_build_tests.py
bash tests/vst3_fixture_tests.sh
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/MediaRelinker.swift tests/MediaRelinkerTests.swift "${freeze_test_support[@]}" -o .build/test-bin/media
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/MediaRelinker.swift tests/MediaRelinkerAliasTests.swift "${freeze_test_support[@]}" -o .build/test-bin/media-aliases
.build/test-bin/media-aliases
.build/test-bin/media
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/MediaRelinker.swift tests/MediaPortableTests.swift "${freeze_test_support[@]}" -o .build/test-bin/media-portable
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/MediaRelinker.swift tests/TakesMediaTests.swift "${freeze_test_support[@]}" -o .build/test-bin/takes-media
.build/test-bin/takes-media
.build/test-bin/media-portable
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/MediaRelinker.swift Sources/SuperTracker/KitLibrary.swift tests/KitMediaPackageRecoveryTests.swift "${freeze_test_support[@]}" -o .build/test-bin/kit-media-package
.build/test-bin/kit-media-package
swiftc -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/MediaRelinker.swift tests/TrackFreezeMediaTests.swift "${freeze_test_support[@]}" -o .build/test-bin/track-freeze-media
.build/test-bin/track-freeze-media
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/MediaRelinker.swift Sources/SuperTracker/MediaRelinkService.swift tests/MediaRelinkServiceTests.swift "${freeze_test_support[@]}" -o .build/test-bin/media-service
.build/test-bin/media-service
bash scripts/test-audio-freeze-project.sh
bash scripts/test-freeze-track-fingerprint.sh
bash scripts/test-freeze-media.sh
for SCRIPT in Resources/Web/*.js; do node --check "$SCRIPT"; done
for SCRIPT in tests/*_tests.js; do node "$SCRIPT"; done
bash scripts/test-sample-inspection.sh
bash scripts/test-sampler.sh

# Offline routing graph assembly: strips, PDC, sidechains, sends and solo gates.
swiftc -O -swift-version 5 -I .build/test-modules -module-cache-path .build/clang-module-cache Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/AudioMixAutomation.swift Sources/SuperTracker/AudioDelayCompensation.swift Sources/SuperTracker/AudioRoutingCapture.swift Sources/SuperTracker/AudioRoutingGraph.swift tests/AudioRoutingGraphTests.swift Sources/SuperTracker/AudioRenderUnit.swift .build/test-bin/au-render-shim.o -o .build/test-bin/graph-assembly
.build/test-bin/graph-assembly
# Incremental routing: owners added/removed while rendering, compared with full rebuilds.
swiftc -I .build/test-modules -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/AudioMixAutomation.swift Sources/SuperTracker/AudioDelayCompensation.swift Sources/SuperTracker/AudioRoutingCapture.swift Sources/SuperTracker/AudioRoutingGraph.swift tests/AudioRoutingIncrementalTests.swift Sources/SuperTracker/AudioRenderUnit.swift .build/test-bin/au-render-shim.o -o .build/test-bin/routing-incremental
.build/test-bin/routing-incremental
# Per-stage capture taps, same offline graph sources.
swiftc -O -swift-version 5 -I .build/test-modules -module-cache-path .build/clang-module-cache Sources/SuperTracker/MixerChannel.swift Sources/SuperTracker/KitProject.swift Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/ProjectLayout.swift Sources/SuperTracker/AudioMixAutomation.swift Sources/SuperTracker/AudioDelayCompensation.swift Sources/SuperTracker/AudioRoutingCapture.swift Sources/SuperTracker/AudioRoutingGraph.swift tests/AudioRoutingStageCaptureTests.swift Sources/SuperTracker/AudioRenderUnit.swift .build/test-bin/au-render-shim.o -o .build/test-bin/graph-stage-capture
.build/test-bin/graph-stage-capture
# Graph building blocks on their own: delay compensation, mix automation, capture taps, endpoint store (all offline).
swiftc -I .build/test-modules -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioDelayCompensation.swift tests/audio_delay_unit_tests.swift Sources/SuperTracker/AudioRenderUnit.swift .build/test-bin/au-render-shim.o -o .build/test-bin/delay-compensation
.build/test-bin/delay-compensation
swiftc -I .build/test-modules -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioMixAutomation.swift tests/audio_mix_automation_tests.swift Sources/SuperTracker/AudioRenderUnit.swift .build/test-bin/au-render-shim.o -o .build/test-bin/mix-automation
.build/test-bin/mix-automation
swiftc -I .build/test-modules -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioRoutingCapture.swift Sources/SuperTracker/AudioRenderUnit.swift .build/test-bin/au-render-shim.o tests/audio_routing_capture_tests.swift -o .build/test-bin/routing-capture
.build/test-bin/routing-capture
swiftc -O -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioEndpointStore.swift tests/audio_endpoint_store_tests.swift -o .build/test-bin/endpoint-store
.build/test-bin/endpoint-store
# Recording: frame-window selection and the input boundary (pure, no devices).
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioRecordingWindow.swift tests/audio_recording_window_tests.swift -o .build/test-bin/recording-window
.build/test-bin/recording-window
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioRecordingBoundary.swift Sources/SuperTracker/AudioRecordingWindow.swift tests/audio_recording_boundary_tests.swift -o .build/test-bin/recording-boundary
.build/test-bin/recording-boundary
swiftc -swift-version 5 -D SELFTESTS -module-cache-path .build/clang-module-cache Sources/SuperTracker/AudioRecording.swift Sources/SuperTracker/AudioRecordingBoundary.swift Sources/SuperTracker/AudioRecordingWindow.swift Sources/SuperTracker/NativeAudioRecordingTests.swift tests/audio_recording_main.swift -o .build/test-bin/recording-channels
.build/test-bin/recording-channels "$PWD/.build/test-output/recording-channels"
# Private virtual CoreMIDI endpoints only (never hardware); exit 77 = CoreMIDI unavailable, a skip.
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MIDIHost.swift tests/MIDILoopbackTests.swift -o .build/test-bin/midi-loopback
.build/test-bin/midi-loopback || [ $? -eq 77 ]
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/PayloadValue.swift Sources/SuperTracker/MIDIHost.swift tests/MIDIHotplugTests.swift -o .build/test-bin/midi-hotplug
.build/test-bin/midi-hotplug || [ $? -eq 77 ]
# AI control socket: a closing connection never writes into a reused descriptor.
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache Sources/SuperTracker/AgentControlProtocol.swift Sources/SuperTracker/AgentControlServer.swift tests/AgentConnectionTests.swift -o .build/test-bin/agent-connection
.build/test-bin/agent-connection
python3 tests/testrun_parse_tests.py
# Only a release build of the main checkout refreshes /Applications (not run.sh's debug build or a worktree build).
python3 tests/build_install_guard_tests.py

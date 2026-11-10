// swift-tools-version: 5.9
import Foundation
import PackageDescription

// Build layout (see docs/TESTING.md, "Builds"):
//
// All Swift sources stay in Sources/SuperTracker, because many test scripts and diagnostic builders compile
// individual files from there with swiftc. SwiftPM splits that one directory into several modules with disjoint
// `sources:` lists, so an incremental build only recompiles the module that changed (and the modules above it),
// and release builds no longer run whole-module optimisation over the entire app for every edit:
//
//   SuperTrackerCore     project model, persistence, media relinking, export, samples, Maschine group mappings
//   SuperTrackerLibrary  sound library, NKS metadata, kit library               (uses Core)
//   SuperTrackerMK3      Maschine MK3 USB/HID/screen driver and probe          (uses Core)
//   SuperTrackerAgent    agent control protocol and MCP stdio proxy
//   SuperTracker         the app: audio host, plug-in hosting, bridge, UI       (uses all of the above)
//
// The libraries are imported implicitly with @testable (-testable-import-module), so code moved between modules needs
// no `public` annotations and no import lines; files still compile unchanged as one module with plain swiftc. The
// libraries are therefore built with -enable-testing in every configuration.
//
// In-app self-tests (Native*Tests.swift, NativeSelfTest.swift, NativeUIBridgeSelfTest.swift), the DevHarness UI
// driver and the LibraryService self-tests are wrapped in
// `#if SELFTESTS`, which is defined for debug builds only. Release builds skip them entirely;
// `scripts/build.sh selftest` makes an optimised build that includes them.

let core = [
    "AudioExportDelivery.swift", "AudioExportFormat.swift", "AudioExportTransaction.swift",
    "AudioFreezeDurationProof.swift", "AudioFreezeProject.swift", "AudioFreezeTrackFingerprint.swift",
    "ProjectLayout.swift", "AudioLegacyVST2Bank.swift", "AudioMeters.swift",
    "AudioPluginParameterMetadata.swift", "AudioPluginStepFX.swift", "AudioProject.swift",
    "AudioRecordingLocation.swift", "AudioRouting.swift", "AudioRoutingAutomation.swift", "AudioSamples.swift",
    "AudioStretch.swift", "BuiltinWavetables.swift", "ExportDestination.swift",
    "KitGroups.swift", "KitProject.swift", "MIDIFile.swift", "MIDISustainState.swift", "MaschineGroupAdditionalMapping.swift",
    "MaschineGroupArchive.swift", "MaschineGroupCoreMapping.swift", "MaschineGroupDistortionMapping.swift",
    "MaschineGroupDrumMapping.swift", "MaschineGroupPluginState.swift", "MaschineGroupSamplerMapping.swift",
    "MaschineGroupSpatialMapping.swift", "MediaRelinkService.swift", "MediaRelinker.swift", "MixerChannel.swift", "PayloadValue.swift",
    "PluginRecoveryStore.swift", "PluginSnapshotCapture.swift", "ProjectMigration.swift", "ProjectStore.swift",
    "SampleReferences.swift", "SampleSlicing.swift", "VST3Compatibility.swift", "VST3StatePreparation.swift", "VST3TransportContext.swift",
    "WavetableFormat.swift",
]
let library = ["KitLibrary.swift", "LibraryService.swift", "NKSHostPreset.swift", "NKSMetadata.swift"]
let mk3 = ["MaschineMK3.swift", "MaschineMK3Probe.swift", "MaschineMK3Protocol.swift", "MaschineMK3Screen.swift"]
let agent = ["AgentControlProtocol.swift", "MCPServer.swift"]

let appPath = "Sources/SuperTracker"
/// Every target shares `appPath`; each library excludes the files it does not own, so
/// SwiftPM reports no unhandled files.
let appFiles: [String] = {
    let directory = Context.packageDirectory + "/" + appPath
    return ((try? FileManager.default.contentsOfDirectory(atPath: directory)) ?? []).filter { !$0.hasPrefix(".") }.sorted()
}()
func excluding(_ owned: [String]) -> [String] { appFiles.filter { !owned.contains($0) } }
let testableLibrary: [SwiftSetting] = [.unsafeFlags(["-enable-testing"])]
func testableImports(_ modules: String...) -> SwiftSetting {
    .unsafeFlags(modules.flatMap { ["-Xfrontend", "-testable-import-module", "-Xfrontend", $0] })
}

// The engine (module TrackerEngine: the C headers in Sources/TrackerEngine/include) has two implementations:
//
//   rust  the static library built from rust/tracker-engine by scripts/build-engine.sh (scripts/build.sh runs it)
//   cpp   the original C++ sources in Sources/TrackerEngine, kept until the Rust engine has shipped
//
// SUPERTRACKER_ENGINE=rust|cpp chooses. Without it, a plain `swift build` links the Rust library when it has been
// built and compiles the C++ sources otherwise, so the package always builds.
let rustEngineDirectory = Context.packageDirectory + "/rust/target/release"
let usesRustEngine: Bool = {
    switch ProcessInfo.processInfo.environment["SUPERTRACKER_ENGINE"] {
    case "rust": return true
    case "cpp": return false
    default: return FileManager.default.fileExists(atPath: rustEngineDirectory + "/libtracker_engine.a")
    }
}()
let engineTarget: Target = usesRustEngine
    ? .target(name: "TrackerEngine", path: "Sources/TrackerEngine", sources: ["RustEngineLink.c"], publicHeadersPath: "include",
              linkerSettings: [.unsafeFlags(["-L" + rustEngineDirectory]), .linkedLibrary("tracker_engine")])
    // SwiftPM builds C-family release targets at -Os, which leaves the realtime DSP's small
    // inline helpers as out-of-line calls; the engine and built-in effects are measured at -O2.
    : .target(name: "TrackerEngine", path: "Sources/TrackerEngine", publicHeadersPath: "include",
              cxxSettings: [.unsafeFlags(["-std=c++17"]), .unsafeFlags(["-O2"], .when(configuration: .release))])

let package = Package(
    name: "SuperTracker",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "SuperTracker", targets: ["SuperTracker"])],
    targets: [
        engineTarget,
        .target(name: "VST3Host", path: "Sources/VST3Host", publicHeadersPath: "include", cxxSettings: [
            .headerSearchPath("../../Vendor/vst3sdk")
        ], linkerSettings: [.linkedFramework("AppKit"), .linkedFramework("CoreFoundation")]),
        // Objective-C render blocks for the Swift Audio Units (see AudioRenderUnit.swift).
        .target(name: "AURenderShim", path: "Sources/AURenderShim", publicHeadersPath: "include",
                cSettings: [.unsafeFlags(["-fobjc-arc"])], linkerSettings: [.linkedFramework("AudioToolbox")]),
        .target(name: "VST2Host", path: "Sources/VST2Host", publicHeadersPath: "include",
                linkerSettings: [.linkedFramework("AppKit"), .linkedFramework("CoreFoundation")]),
        .target(name: "SuperTrackerCore", dependencies: ["TrackerEngine", "VST3Host", "VST2Host"], path: appPath,
                exclude: excluding(core), sources: core, swiftSettings: testableLibrary),
        .target(name: "SuperTrackerLibrary", dependencies: ["SuperTrackerCore", "TrackerEngine"], path: appPath,
                exclude: excluding(library), sources: library,
                swiftSettings: testableLibrary + [testableImports("SuperTrackerCore"), .define("SELFTESTS", .when(configuration: .debug))]),
        .target(name: "SuperTrackerMK3", dependencies: ["SuperTrackerCore"], path: appPath, exclude: excluding(mk3), sources: mk3,
                swiftSettings: testableLibrary + [testableImports("SuperTrackerCore")],
                linkerSettings: [.linkedFramework("IOKit"), .linkedFramework("IOUSBHost")]),
        .target(name: "SuperTrackerAgent", path: appPath, exclude: excluding(agent), sources: agent, swiftSettings: testableLibrary),
        .executableTarget(name: "SuperTracker",
                          dependencies: ["TrackerEngine", "VST3Host", "VST2Host", "AURenderShim", "SuperTrackerCore", "SuperTrackerLibrary",
                                         "SuperTrackerMK3", "SuperTrackerAgent"],
                          path: appPath, exclude: core + library + mk3 + agent,
                          swiftSettings: [
                            .define("SELFTESTS", .when(configuration: .debug)),
                            testableImports("SuperTrackerCore", "SuperTrackerLibrary", "SuperTrackerMK3", "SuperTrackerAgent"),
                          ],
                          linkerSettings: [
            .linkedFramework("AppKit"), .linkedFramework("WebKit"), .linkedFramework("AVFoundation"),
            .linkedFramework("AudioToolbox"), .linkedFramework("CoreAudio"), .linkedFramework("CoreMIDI"),
            .linkedFramework("IOKit"), .linkedFramework("IOUSBHost")
        ])
    ],
    swiftLanguageVersions: [.v5],
    cxxLanguageStandard: .cxx17
)

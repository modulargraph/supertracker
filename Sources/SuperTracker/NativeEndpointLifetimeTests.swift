// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox
import Darwin

/// Real Apple AU replacement plus an original deliberately failing AU render. No
/// third-party plug-in is instantiated and all audio is rendered offline.
enum NativeEndpointLifetimeTests {
    private static let failureDescription = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x656C6661, componentManufacturer: 0x53745473,
        componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(EndpointFailureUnit.self, as: failureDescription,
            name: "SuperTracker Test: Endpoint render failure", version: 1)
    }()

    @MainActor static func run(output: URL) async throws -> [String] {
        _ = registration
        let folder = output.appendingPathComponent("endpoint-lifetime", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let host = try AudioHost(midiInputIDs: [])
        defer { host.shutdown() }
        var observations: [[String: Any]] = []
        var errorDescription = ""
        var failureCallbacks: Int64 = 0
        var exportThrew = false
        var failedExports: [[String: Any]] = []
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Endpoint lifetime regression: " + message) }
        }
        func inspect(_ name: String, instances: Int) throws {
            let status = host.status()["endpointLifetime"] as? [String: Any] ?? [:]
            observations.append(["name": name, "status": status])
            try require((status["activeReaders"] as? Int) == 0, "\(name) left a registered render callback: \(status)")
            try require((status["retiredEndpoints"] as? Int) == 0 && (status["retiredUnits"] as? Int) == 0,
                        "\(name) retained a detached plug-in after quiescence: \(status)")
            try require((status["activeInstances"] as? Int) == instances, "\(name) has unexpected live instances: \(status)")
        }
        defer {
            let result: [String: Any] = ["observations": observations,
                "expectedRenderFailure": errorDescription, "failureCallbacks": failureCallbacks,
                "exportThrew": exportThrew, "failedExports": failedExports]
            if let data = try? JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        let rows: [Any] = [["note": 60, "instrument": 0, "velocity": 90, "gate": 0.7]]
        let project: [String: Any] = ["version": 1, "name": "Endpoint lifetime verification",
            "bpm": 120, "linesPerBeat": 4, "masterVolume": 0.5, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Native pulse", "instrument": 0, "volume": 0.7, "pan": 0]],
            "instruments": [["type": "synth", "volume": 0.5, "attack": 0, "release": 0.001]],
            "patterns": [["id": 0, "length": 1, "steps": [rows], "automation": []]], "sequence": [0],
            "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        let available = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
        guard let delay = available.first(where: { ($0["name"] as? String) == "AUDelay" }),
              let delayID = delay["id"] as? String else {
            throw AudioSampleError.invalid("Endpoint lifetime tests require the macOS AUDelay component.")
        }
        let target: [String: Any] = ["track": 0, "kind": "effect", "slot": 0]
        func load(_ component: String) async throws {
            var payload = target; payload["component"] = component
            _ = try await host.handle("plugin.load", payload: payload)
        }
        func render(_ name: String) async throws {
            _ = try await host.handle("audio.export", payload: ["path": folder.appendingPathComponent(name + ".wav").path,
                "mode": "song", "tail": 0])
        }
        for index in 0..<12 {
            try await load(delayID)
            try await render("replacement-\(index)")
            try inspect("replacement-\(index)", instances: 1)
        }
        _ = try await host.handle("plugin.remove", payload: target)
        try await render("after-remove")
        try inspect("after-remove", instances: 0)

        let description = failureDescription
        try await load("\(description.componentType):\(description.componentSubType):\(description.componentManufacturer)")
        for attempt in 0..<2 {
            let previousFailures = EndpointFailureUnit.failureCount
            exportThrew = false
            do { try await render("expected-render-failure-\(attempt)") }
            catch { exportThrew = true; errorDescription = error.localizedDescription }
            let callbacks = EndpointFailureUnit.failureCount - previousFailures
            failureCallbacks += callbacks
            failedExports.append(["attempt": attempt, "failureCallbacks": callbacks,
                "exportThrew": exportThrew, "error": errorDescription])
            try require(callbacks > 0, "The original failure AU was never called to render.")
            try require(exportThrew, "Export reported success after \(callbacks) upstream AU render failures.")
            try require(errorDescription.localizedCaseInsensitiveContains("Endpoint render failure") &&
                        errorDescription.localizedCaseInsensitiveContains("track 1"),
                        "The upstream render failure omitted the responsible plug-in or track: \(errorDescription)")
            try inspect("after-render-error-\(attempt)", instances: 1)
        }
        _ = try await host.handle("plugin.remove", payload: target)
        try await render("after-error-remove")
        try inspect("after-error-remove", instances: 0)
        return [
            "Twelve real AUDelay replacements and removal reclaim retired endpoints after offline rendering",
            "An upstream AU render failure aborts export, balances reader scopes, and permits safe subsequent removal"
        ]
    }
}

private final class EndpointFailureUnit: AUAudioUnit {
    private static let failures: UnsafeMutablePointer<Int64> = {
        let pointer = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
        pointer.initialize(to: 0)
        return pointer
    }()
    static var failureCount: Int64 { OSAtomicAdd64Barrier(0, failures) }
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let input = try AUAudioUnitBus(format: format), output = try AUAudioUnitBus(format: format)
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        maximumFramesToRender = 16_384
    }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 &&
            format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }
    override var internalRenderBlock: AUInternalRenderBlock {
        let failures = Self.failures
        return { _, _, _, _, _, _, _ in
            OSAtomicIncrement64Barrier(failures)
            return kAudioUnitErr_FailedInitialization
        }
    }
}

#endif

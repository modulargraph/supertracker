// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
import Foundation
import AVFoundation
#if SELFTESTS

extension NativeSelfTest {
    @MainActor static func run(output: URL) async throws {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let reportURL = output.appendingPathComponent("native-test-report.json")
        func report(_ status: String, error: String? = nil) throws {
            var value: [String: Any] = ["passed": false, "status": status,
                "generatedAt": ISO8601DateFormatter().string(from: Date())]
            if let error { value["error"] = error }
            try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys]).write(to: reportURL, options: .atomic)
        }
        try report("running")
        do { try await runChecks(output: output) }
        catch { try? report("failed", error: error.localizedDescription); throw error }
    }

    @MainActor private static func runChecks(output: URL) async throws {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        var checks: [[String: Any]] = []
        func record(_ name: String, _ details: [String: Any] = [:]) {
            var check = details; check["name"] = name; check["passed"] = true; checks.append(check)
            print("PASS \(name)")
            fflush(stdout)
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("SELF TEST: \(message)") }
        }
        for name in try LibraryService.selfTest() { record("Library: " + name) }
        for name in try NKSMetadata.selfTest() { record("NKS: " + name) }
        var project = testProject()
        let host = try AudioHost(midiInputIDs: [])
        defer { host.shutdown() }
        _ = try await host.handle("engine.project", payload: ["project": project])
        let synthURL = output.appendingPathComponent("native-synth.wav")
        _ = try await host.handle("audio.export", payload: ["path": synthURL.path, "mode": "song", "tail": 0.5])
        let native = try AudioSamples.decode(synthURL.path)
        let rms = energy(native.left)
        try require(rms > 0.001 && rms < 0.9, "Native synth export must contain bounded non-silent audio; RMS \(rms)")
        try require(abs(native.duration - 4.5) < 0.05, "Song export should last 4.5 seconds, got \(native.duration)")
        try require(native.left.allSatisfy { $0.isFinite && abs($0) <= 1 }, "Native render contains invalid audio")
        record("Native tracker arrangement and stereo WAV export", ["rms": rms, "duration": native.duration])

        let plugins = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
        try require(!plugins.isEmpty, "No installed Audio Units found")
        record("Audio Unit discovery", ["count": plugins.count])
        guard let dls = plugins.first(where: { ($0["name"] as? String ?? "").localizedCaseInsensitiveContains("DLSMusicDevice") }), let identifier = dls["id"] as? String else {
            throw TrackerError.message("SELF TEST: Apple DLSMusicDevice is unavailable for instrument render verification.")
        }
        print("Loading AU instrument: \(dls["name"] ?? identifier)"); fflush(stdout)
        _ = try await host.handle("plugin.load", payload: ["track": 0, "kind": "instrument", "slot": 0, "component": identifier])
        var instruments = project["instruments"] as! [[String: Any]]
        instruments[0]["type"] = "au"; project["instruments"] = instruments
        let snapshot = try await host.handle("engine.snapshot", payload: [:]) as! [String: Any]
        project["audioPlugins"] = snapshot["audioPlugins"]
        _ = try await host.handle("engine.project", payload: ["project": project])
        let auURL = output.appendingPathComponent("audio-unit-instrument.wav")
        _ = try await host.handle("audio.export", payload: ["path": auURL.path, "mode": "song", "tail": 0.5])
        let auSample = try AudioSamples.decode(auURL.path)
        let auEnergy = energy(auSample.left)
        try require(auEnergy > 0.0001, "AU instrument export is silent (RMS \(auEnergy))")
        let auMeterStatus = host.status()
        try require((auMeterStatus["peakL"] as? Float ?? 0) > 0, "Post-AU master meter must see rendered instrument audio")
        try require(((auMeterStatus["trackPeaks"] as? [Float])?.first ?? 0) > 0, "Post-AU track meter must see rendered instrument audio")
        record("Real AU instrument sequenced and rendered", ["plugin": dls["name"] ?? identifier, "rms": auEnergy])

        if let delay = plugins.first(where: { ($0["name"] as? String ?? "").localizedCaseInsensitiveContains("AUDelay") && $0["type"] as? String == "effect" }), let delayID = delay["id"] as? String {
            _ = try await host.handle("plugin.load", payload: ["track": 0, "kind": "effect", "slot": 0, "component": delayID])
            let parameters = try await host.handle("plugin.parameters", payload: ["track": 0, "kind": "effect", "slot": 0])
            record("Real AU effect loaded with parameters", ["plugin": delay["name"] ?? delayID, "parameters": parameters])
            let effectSnapshot = try await host.handle("engine.snapshot", payload: [:]) as! [String: Any]
            project["audioPlugins"] = effectSnapshot["audioPlugins"]
            let store = try ProjectStore(directory: output.appendingPathComponent("support"))
            try store.update(project)
            _ = try store.save(to: output.appendingPathComponent("Integration.supertracker"))
            let reopened = try store.load(output.appendingPathComponent("Integration.supertracker"))
            _ = try await host.handle("engine.project", payload: ["project": reopened])
            let fxURL = output.appendingPathComponent("audio-unit-restored.wav")
            _ = try await host.handle("audio.export", payload: ["path": fxURL.path, "mode": "song", "tail": 0.5])
            let restored = try AudioSamples.decode(fxURL.path)
            try require(energy(restored.left) > 0.0001, "Restored AU project is silent")
            record("AU instrument and effect state survive project save/reopen")
        } else { throw TrackerError.message("SELF TEST: Apple AUDelay unavailable for effect test.") }
        let devices = try await host.handle("midi.devices", payload: [:])
        record("CoreMIDI device discovery", ["devices": devices])
        for name in try await NativeAudioTests.run(output: output) { record(name) }
        for name in try await NativeRenderRestartTests.run(output: output) { record(name) }
        for name in try await NativePluginRecoveryTests.run(output: output) { record(name) }
        for name in try await NativeExportFormatTests.run(output: output) { record(name) }
        for name in try await NativeExportDeliveryTests.run(output: output) { record(name) }
        for name in try await NativeMIDISustainTests.run(output: output) { record(name) }
        let report: [String: Any] = ["passed": true, "checks": checks, "generatedAt": ISO8601DateFormatter().string(from: Date())]
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: output.appendingPathComponent("native-test-report.json"), options: .atomic)
        print("Native integration tests passed. Evidence: \(output.path)")
    }

    static func energy(_ values: [Float]) -> Double {
        sqrt(values.reduce(0.0) { $0 + Double($1) * Double($1) } / Double(max(1, values.count)))
    }
}
#endif

// The fixture project is also used by the keyboard acceptance session (--ui-keyboard-session), so it is in every build.
enum NativeSelfTest {
    static func testProject() -> [String: Any] {
        func pattern(_ id: Int, _ offset: Int) -> [String: Any] {
            var steps = [Any](repeating: NSNull(), count: 16)
            for row in stride(from: 0, to: 16, by: 4) {
                steps[row] = ["note": 60 + offset + row / 4 * 2, "instrument": 0, "velocity": 90, "gate": 2.0, "chord": [0, 4, 7]] as [String: Any]
            }
            return ["id": id, "name": "Test \(id + 1)", "length": 16, "steps": [steps], "automation": [["id": "vol\(id)", "track": 0, "target": "volume", "name": "Volume", "interpolation": "linear", "points": [["row": 0, "value": 0.4], ["row": 15, "value": 0.8]]]]]
        }
        return ["version": 1, "name": "Native integration", "bpm": 120.0, "linesPerBeat": 4, "swing": 0, "masterVolume": 0.5, "activePattern": 0,
                "tracks": [["id": 0, "name": "Instrument", "volume": 0.7, "pan": 0, "instrument": 0, "delaySend": 0.1, "reverbSend": 0.1]],
                "instruments": [["id": 0, "name": "Sine", "type": "synth", "waveform": "sine", "volume": 0.5, "attack": 0.005, "decay": 0.2, "sustain": 0.5, "release": 0.05]],
                "patterns": [pattern(0, 0), pattern(1, 5)], "sequence": [0, 1], "loopSong": false,
                "delay": ["time": 0.25, "feedback": 0.2, "mix": 0.2], "reverb": ["size": 0.3, "damping": 0.4, "mix": 0.15]]
    }
}

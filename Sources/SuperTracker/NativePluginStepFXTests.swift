// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Real AU render coverage for instance-addressed tracker parameter locks. The
/// instrument is original test code; this suite never opens an editor, starts
/// real-time playback, scans vendor plug-ins, or connects a MIDI input.
enum NativePluginStepFXTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        NativeUINKSAutomationFixture.register()
        let folder = output.appendingPathComponent("plugin-step-fx", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let address = String(NativeUINKSAutomationFixture.wideAddress)
        let owner = "track:native-plugin-step"
        var passed: [String] = [], renders: [[String: Any]] = []
        func require(_ result: Bool, _ message: String) throws {
            if !result { throw AudioSampleError.invalid("Plug-in step FX integration: " + message) }
        }
        func makeHost(_ name: String, graph: AVAudioEngine? = nil) throws -> AudioHost {
            var configuration = VST3Catalogue.Configuration()
            configuration.roots = []
            configuration.cacheURL = folder.appendingPathComponent(name + "-private-vst3-cache.json")
            return try AudioHost(recoveryStore: nil, audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: configuration), midiInputIDs: [])
        }
        func report(_ success: Bool, _ error: String? = nil) throws {
            var result: [String: Any] = ["passed": success, "checks": passed, "renders": renders,
                "generatedAt": ISO8601DateFormatter().string(from: Date()),
                "scope": "Original in-process AU; offline PCM; instance-addressed step locks",
                "installedPluginsLoaded": false, "physicalKeyboardTested": false,
                "realtimePlaybackTested": false, "midiInputsConnected": false,
                "parameterAddress": address, "sampleRate": 48_000, "framesPerRow": 6_000]
            if let error { result["error"] = error }
            try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                .write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func apply(_ project: [String: Any], to host: AudioHost, replace: Bool = false) async throws {
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": replace])
            await host.pluginRestorationBarrier()?.value
        }
        func render(_ name: String, host: AudioHost) async throws -> DecodedSample {
            let url = folder.appendingPathComponent(name + ".wav")
            _ = try await host.handle("audio.export", payload: ["path": url.path, "mode": "song", "tail": 0])
            let pcm = try AudioSamples.decode(url.path)
            try require(pcm.sampleRate == 48_000 && pcm.left.count == 24_000 && pcm.right.count == 24_000,
                        "\(name) has the wrong sample rate or duration.")
            try require(pcm.left.allSatisfy(\.isFinite) && pcm.right.allSatisfy(\.isFinite), "\(name) contains non-finite PCM.")
            return pcm
        }
        func verify(_ pcm: DecodedSample, name: String, reference: DecodedSample, gains: [Float]) throws {
            let left = reference.left[0], right = reference.right[0]
            try require(left > 0.001 && right > 0.001, "\(name) reference is silent.")
            var maximumDifference: Float = 0
            for frame in 0..<24_000 {
                let multiplier = gains[frame / 6_000] / 0.25
                maximumDifference = max(maximumDifference, abs(pcm.left[frame] - left * multiplier))
                maximumDifference = max(maximumDifference, abs(pcm.right[frame] - right * multiplier))
            }
            renders.append(["name": name, "frames": pcm.left.count, "expectedRowGains": gains,
                "referenceLeft": left, "referenceRight": right, "maximumExpectedPCMDifference": maximumDifference])
            try require(maximumDifference <= 0.000001,
                        "\(name) differs from the exact row-boundary envelope by \(maximumDifference).")
        }
        func step(_ instanceID: String, value: Double) -> [String: Any] {
            ["type": "plugin", "instanceID": instanceID, "address": address, "value": value]
        }
        func pattern(rows: [[Any]], lanes: [[String: Any]]) -> [String: Any] {
            ["id": 0, "name": "Instance parameter locks", "length": 4, "steps": rows, "automation": lanes]
        }
        try report(false)
        do {
            for standard in [false, true] {
                let mode = standard ? "standard" : "legacy"
                let host = try makeHost(mode)
                defer { host.shutdown() }
                var rows = [Any](repeating: NSNull(), count: 4)
                rows[0] = ["note": 60, "instrument": 0, "velocity": 127, "gate": 3]
                var project: [String: Any] = ["version": 1, "name": "Native plug-in step FX \(mode)",
                    "bpm": 120.0, "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0,
                    "loopSong": false, "metronome": false,
                    "tracks": [["id": 0, "routingID": "native-plugin-step", "name": "Original AU",
                                "instrument": 0, "volume": 1, "pan": 0, "delaySend": 0, "reverbSend": 0]],
                    "instruments": [["id": 0, "type": "au", "volume": 1, "pan": 0]],
                    "patterns": [pattern(rows: [rows], lanes: [])], "sequence": [0], "audioPlugins": [],
                    "audioClips": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
                if standard {
                    project["routing"] = ["version": 1, "mixModel": "standard", "buses": [],
                        "outputs": [owner: "master"], "sends": [], "sidechains": []]
                }
                try await apply(project, to: host, replace: true)
                guard let descriptor = try await host.handle("plugin.load", payload: ["track": 0, "kind": "instrument", "slot": 0,
                    "component": NativeUINKSAutomationFixture.wideIdentifier]) as? [String: Any],
                    let instanceID = descriptor["instanceID"] as? String, !instanceID.isEmpty else {
                    throw AudioSampleError.invalid("The original AU did not return an instance identity.")
                }
                project["audioPlugins"] = [descriptor]
                try await apply(project, to: host)
                let reference = try await render(mode + "-reference", host: host)
                try verify(reference, name: mode + "-constant-reference", reference: reference, gains: [0.25, 0.25, 0.25, 0])
                var lane: [String: Any] = ["id": "wide-parameter-lane", "track": 0,
                    "target": "plugin:instrument:0:\(address)", "interpolation": "step",
                    "points": [["row": 0, "value": 0.25], ["row": 1, "value": 0.375],
                               ["row": 2, "value": 0.5], ["row": 3, "value": 0.75]]]
                if standard { lane["owner"] = owner }
                project["patterns"] = [pattern(rows: [rows], lanes: [lane])]
                try await apply(project, to: host)
                let laneOnly = try await render(mode + "-lane-only", host: host)
                try verify(laneOnly, name: mode + "-lane-only", reference: reference, gains: [0.25, 0.375, 0.5, 0])
                // The second FX column has the same identity and address, so its
                // value must win for this row without disabling the next row.
                rows[1] = ["fx1": step(instanceID, value: 0.875), "fx2": step(instanceID, value: 1)]
                project["patterns"] = [pattern(rows: [rows], lanes: [lane])]
                try await apply(project, to: host)
                let locked = try await render(mode + "-locked-row", host: host)
                try verify(locked, name: mode + "-locked-row", reference: reference, gains: [0.25, 1, 0.5, 0])
                passed.append("\(mode.capitalized) routing: exact UInt64 AU step locks override the lane at frame 6000, FX2 wins, and lane automation resumes at frame 12000")

                guard let captured = try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] else {
                    throw AudioSampleError.invalid("The native project snapshot is missing.")
                }
                let store = try ProjectStore(directory: folder.appendingPathComponent(mode + "-private-project-store"))
                let file = folder.appendingPathComponent(mode + ".supertracker")
                _ = try store.save(to: file, snapshot: captured, collectSamples: false)
                let reopened = try store.load(file)
                let persistedRows = ((reopened["patterns"] as? [[String: Any]])?.first?["steps"] as? [[Any]])?.first
                let persistedFX = (persistedRows?[1] as? [String: Any])?["fx2"] as? [String: Any]
                try require(persistedFX?["address"] as? String == address && persistedFX?["instanceID"] as? String == instanceID,
                            "Save/Open changed the exact UInt64 string or plug-in identity.")
                let fresh = try makeHost(mode + "-fresh")
                defer { fresh.shutdown() }
                try await apply(reopened, to: fresh, replace: true)
                let freshPCM = try await render(mode + "-fresh-reopened", host: fresh)
                try verify(freshPCM, name: mode + "-fresh-reopened", reference: reference, gains: [0.25, 1, 0.5, 0])
                try require(freshPCM.left == locked.left && freshPCM.right == locked.right,
                            "Fresh-host restored PCM differs from the pre-save render.")
                passed.append("\(mode.capitalized) routing: native snapshot, disk Save/Open and a fresh AU preserve instance/address identity and byte-identical rendered PCM")

                // A stale identity must not be rebound to the plug-in occupying
                // the same slot; copying a valid lock to another track is also
                // deliberately unavailable rather than targeting its old owner.
                var safe = reopened
                var staleRows = rows
                staleRows[1] = ["fx1": step("removed-instance-do-not-rebind", value: 1)]
                safe["patterns"] = [pattern(rows: [staleRows], lanes: [])]
                try await apply(safe, to: fresh)
                let stalePCM = try await render(mode + "-stale-instance", host: fresh)
                try verify(stalePCM, name: mode + "-stale-instance", reference: reference, gains: [0.25, 0.25, 0.25, 0])
                var tracks = safe["tracks"] as! [[String: Any]]
                tracks.append(["id": 1, "routingID": "native-plugin-step-other", "name": "Pasted lock",
                    "instrument": 0, "volume": 1, "pan": 0, "delaySend": 0, "reverbSend": 0])
                safe["tracks"] = tracks
                var copiedRows = [Any](repeating: NSNull(), count: 4)
                copiedRows[1] = ["fx1": step(instanceID, value: 1)]
                var cleanRows = rows; cleanRows[1] = NSNull()
                safe["patterns"] = [pattern(rows: [cleanRows, copiedRows], lanes: [])]
                if standard {
                    var routing = safe["routing"] as! [String: Any]
                    routing["outputs"] = [owner: "master", "track:native-plugin-step-other": "master"]
                    safe["routing"] = routing
                }
                try await apply(safe, to: fresh)
                let copiedPCM = try await render(mode + "-cross-track-paste", host: fresh)
                try verify(copiedPCM, name: mode + "-cross-track-paste", reference: reference, gains: [0.25, 0.25, 0.25, 0])
                passed.append("\(mode.capitalized) routing: missing identities and cross-track pasted locks leave the existing AU's actual audio unchanged")
            }
            // This is the exact pending-project window: applyProject publishes
            // the C project synchronously and schedules restoration on MainActor.
            // No await follows that call until the manual render has crossed the
            // locked row, so the original same-slot AU must still be installed.
            // A slot-addressed lock would audibly change that unrelated old unit.
            do {
                let graph = AVAudioEngine()
                let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
                try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 512)
                let host = try makeHost("pending-flat-identity", graph: graph)
                defer { host.shutdown() }
                var rows = [Any](repeating: NSNull(), count: 4)
                rows[0] = ["note": 60, "instrument": 0, "velocity": 127, "gate": 3]
                var original: [String: Any] = ["version": 1, "name": "Pending flat step identity",
                    "bpm": 120.0, "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0,
                    "loopSong": false, "metronome": false,
                    "tracks": [["id": 0, "routingID": "native-plugin-step", "name": "Original pending AU",
                                "instrument": 0, "volume": 1, "pan": 0, "delaySend": 0, "reverbSend": 0]],
                    "instruments": [["id": 0, "type": "au", "volume": 1, "pan": 0]],
                    "patterns": [pattern(rows: [rows], lanes: [])], "sequence": [0], "audioPlugins": [],
                    "audioClips": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
                try await apply(original, to: host, replace: true)
                guard let oldDescriptor = try await host.handle("plugin.load", payload: ["track": 0, "kind": "instrument", "slot": 0,
                    "component": NativeUINKSAutomationFixture.wideIdentifier]) as? [String: Any],
                    let oldID = oldDescriptor["instanceID"] as? String else {
                    throw AudioSampleError.invalid("The pending-identity AU fixture has no instance identity.")
                }
                original["audioPlugins"] = [oldDescriptor]
                try await apply(original, to: host)
                _ = try await host.handle("transport.play", payload: ["mode": "song", "fromStart": true])
                let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
                try require(try graph.renderOffline(512, to: buffer) == .success, "Cannot warm the pending-identity offline graph.")
                let referenceLeft = buffer.floatChannelData![0][0], referenceRight = buffer.floatChannelData![1][0]
                try require(referenceLeft > 0.001 && referenceRight > 0.001, "The old same-slot AU is silent before the pending update.")
                var pending = original, newDescriptor = oldDescriptor
                let desiredID = "pending-new-instance-\(oldID)"
                newDescriptor["instanceID"] = desiredID
                pending["audioPlugins"] = [newDescriptor]
                rows[1] = ["fx1": step(desiredID, value: 1)]
                pending["patterns"] = [pattern(rows: [rows], lanes: [])]
                try host.applyProject(pending)
                var maximumDifference: Float = 0
                for _ in 0..<15 {
                    try require(try graph.renderOffline(512, to: buffer) == .success,
                                "Cannot render the pending restoration window.")
                    for frame in 0..<512 {
                        maximumDifference = max(maximumDifference, abs(buffer.floatChannelData![0][frame] - referenceLeft))
                        maximumDifference = max(maximumDifference, abs(buffer.floatChannelData![1][frame] - referenceRight))
                    }
                }
                try require(maximumDifference <= 0.000001,
                            "A pending desired instance's frame-6000 lock changed the old same-slot AU by \(maximumDifference).")
                renders.append(["name": "pending-flat-instance-before-restore", "frames": 7_680,
                    "startFrame": 512, "endFrameExclusive": 8_192, "lockedRowFrame": 6_000,
                    "oldInstanceID": oldID, "desiredInstanceID": desiredID,
                    "referenceLeft": referenceLeft, "referenceRight": referenceRight,
                    "maximumExpectedPCMDifference": maximumDifference,
                    "restorationCouldRunDuringRender": false])
                graph.stop()
                try host.applyProject(original)
                await host.pluginRestorationBarrier()?.value
                passed.append("A pending flat-project replacement cannot send its exact-ID step lock to the still-live old AU in the same slot before asynchronous restoration")
            }
            try report(true)
        } catch {
            try? report(false, error.localizedDescription)
            throw error
        }
        return passed
    }
}

#endif

// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Transport restoration is verified through the real host with an injected offline
/// graph. Even fixtures whose graph is running never open a physical audio device.
enum NativeExportTransportTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("export-transport", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, midiInputIDs: [])
        defer { host.shutdown() }
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
        var passed: [String] = []
        var measurements: [[String: Any]] = []
        var cancellationError: String?
        defer {
            let report: [String: Any] = ["checks": passed, "observations": measurements,
                "physicalPlayback": false, "graphMode": "offline", "cancellationError": cancellationError ?? ""]
            if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }

        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Export transport regression: " + message) }
        }
        func number(_ state: [String: Any], _ key: String) -> Double {
            (state[key] as? NSNumber)?.doubleValue ?? -.infinity
        }
        func transport() -> [String: Any] {
            let status = host.status()
            return Dictionary(uniqueKeysWithValues: ["playing", "running", "songMode", "pattern", "row", "sequence", "position", "seconds"].map {
                ($0, status[$0] ?? NSNull())
            })
        }
        func observe(_ name: String) -> [String: Any] {
            let state = transport()
            measurements.append(["name": name, "transport": state])
            return state
        }
        func sameTransport(_ expected: [String: Any], _ name: String, forceStopped: Bool = false) throws {
            let actual = observe(name)
            for key in ["pattern", "row", "sequence"] {
                try require(number(actual, key) == number(expected, key), "\(name) changed \(key): \(expected) → \(actual).")
            }
            for key in ["position", "seconds"] {
                try require(abs(number(actual, key) - number(expected, key)) < 0.000000001,
                            "\(name) lost the exact \(key): \(expected) → \(actual).")
            }
            for key in ["running", "songMode"] {
                try require(actual[key] as? Bool == expected[key] as? Bool,
                            "\(name) changed \(key): \(expected) → \(actual).")
            }
            let playing = forceStopped ? false : expected["playing"] as? Bool
            try require(actual["playing"] as? Bool == playing, "\(name) restored the wrong playback state: \(actual).")
            try require(graph.isInManualRenderingMode && graph.manualRenderingMode == .offline,
                        "\(name) failed to restore the original offline graph configuration.")
            try require(graph.manualRenderingMaximumFrameCount == 4096 && graph.manualRenderingFormat.sampleRate == 48_000,
                        "\(name) changed the original manual render format or quantum limit.")
        }
        @discardableResult func render(_ frames: AVAudioFrameCount) throws -> Float {
            try require(graph.isRunning && graph.isInManualRenderingMode && graph.manualRenderingMode == .offline,
                        "The manual render fixture must have a running offline graph.")
            var retries = 0
            while true {
                let result = try graph.renderOffline(frames, to: buffer)
                if result == .success { break }
                retries += 1
                try require(retries < 100 && (result == .cannotDoInCurrentContext || result == .insufficientDataFromInputNode),
                            "The offline fixture failed to render: \(result.rawValue).")
            }
            var peak: Float = 0
            for channel in 0..<2 {
                for frame in 0..<Int(buffer.frameLength) { peak = max(peak, abs(buffer.floatChannelData![channel][frame])) }
            }
            return peak
        }
        func project(active: Int = 0, long: Bool = false) -> [String: Any] {
            func pattern(_ id: Int, length: Int) -> [String: Any] {
                var rows = [Any](repeating: NSNull(), count: length)
                for row in stride(from: 0, to: length, by: 3) {
                    rows[row] = ["note": 60 + id * 7, "instrument": 0, "velocity": 90, "gate": 1.5]
                }
                return ["id": id, "length": length, "steps": [rows], "automation": []]
            }
            return ["version": 1, "name": "Export transport fixture", "bpm": 120, "linesPerBeat": 4,
                "masterVolume": 0.5, "activePattern": active, "loopSong": true, "metronome": true,
                "tracks": [["name": "Native oscillator", "instrument": 0, "volume": 0.5, "pan": 0, "mute": false, "solo": false]],
                "instruments": [["type": "synth", "waveform": "sine", "volume": 0.5, "attack": 0,
                    "decay": 0, "sustain": 1, "release": 0.001]],
                "patterns": [pattern(0, length: 8), pattern(1, length: long ? 256 : 16)],
                "sequence": [0, 1], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        }
        func apply(_ value: [String: Any]) async throws {
            _ = try await host.handle("engine.project", payload: ["project": value, "replace": true])
            await host.pluginRestorationBarrier()?.value
        }
        func export(_ name: String, tail: Double, mode: String = "song") async throws {
            _ = try await host.handle("audio.export", payload: ["path": folder.appendingPathComponent(name + ".wav").path,
                "mode": mode, "tail": tail])
        }
        func stopAndConsume() async throws {
            _ = try await host.handle("transport.stop", payload: [:])
            _ = try await host.handle("engine.panic", payload: [:])
            try render(128)
            try require(host.status()["playing"] as? Bool == false, "The stopped fixture still has a pending Play command.")
        }

        try await apply(project())
        try require(!graph.isRunning && host.status()["playing"] as? Bool == false,
                    "The idle export fixture must begin with graph and transport stopped.")
        // This seek is deliberately still queued: export must capture control intent
        // before taking its snapshot, even when no callback has ever consumed it.
        _ = try await host.handle("transport.seek", payload: ["pattern": 0, "row": 3, "sequence": 0])
        let expectedIdle: [String: Any] = ["playing": false, "running": false, "songMode": false,
            "pattern": 0, "row": 3, "sequence": 0, "position": 3.0, "seconds": 0.375]
        try await export("idle-zero-tail", tail: 0)
        try sameTransport(expectedIdle, "idle-zero-tail")
        passed.append("Zero-tail export preserves an idle graph and consumes the queued seek before capturing its cursor")

        try await export("idle-positive-tail", tail: 0.125)
        try sameTransport(expectedIdle, "idle-positive-tail")
        try await export("idle-repeated-zero-tail", tail: 0)
        try sameTransport(expectedIdle, "idle-repeated-zero-tail")
        passed.append("Positive tails and repeated exports do not retain the export's playback flag or ending cursor")

        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        try render(128)
        try await stopAndConsume()
        let runningStopped = observe("running-stopped-before-export")
        try require(runningStopped["running"] as? Bool == true && runningStopped["playing"] as? Bool == false,
                    "This fixture requires a running graph with stopped transport.")
        for (name, tail) in [("running-stopped-zero-tail", 0.0), ("running-stopped-positive-tail", 0.125)] {
            try await export(name, tail: tail)
            try sameTransport(runningStopped, name)
            let peak = try render(512)
            measurements.append(["name": name + "-next-buffer", "peak": peak])
            try require(peak < 0.000001, "\(name) produced unrequested music in the next callback (peak \(peak)).")
            try sameTransport(runningStopped, name + "-after-next-buffer")
        }
        passed.append("A running graph with stopped transport remains stopped and silent after exports with either tail length")

        try await apply(project(active: 1))
        _ = try await host.handle("transport.seek", payload: ["pattern": 1, "row": 3, "sequence": 1])
        _ = try await host.handle("transport.play", payload: ["mode": "pattern"])
        let playingPeak = try render(1234)
        let playingPattern = observe("playing-pattern-before-export")
        try require(playingPattern["running"] as? Bool == true && playingPattern["playing"] as? Bool == true &&
                    playingPattern["songMode"] as? Bool == false && number(playingPattern, "pattern") == 1,
                    "The playing fixture must be rendering Pattern mode on pattern 1.")
        try require(playingPeak > 0.001 && number(playingPattern, "position") > 3 && number(playingPattern, "position") < 4,
                    "The playing fixture must have an audible note and a genuinely fractional cursor.")
        for (name, tail) in [("playing-pattern-positive-tail", 0.125), ("playing-pattern-zero-tail", 0.0)] {
            try await export(name, tail: tail)
            try sameTransport(playingPattern, name)
        }
        passed.append("Song export restores a playing Pattern session's exact fractional cursor, time, sequence and playback mode")

        func invalidDestination(_ name: String) async throws {
            let before = observe(name + "-before")
            let invalidPath = folder.appendingPathComponent("uncreated-" + UUID().uuidString, isDirectory: true).appendingPathComponent("failed.wav")
            var failed = false
            do { _ = try await host.handle("audio.export", payload: ["path": invalidPath.path, "mode": "song", "tail": 0]) }
            catch { failed = true; measurements.append(["name": name, "error": error.localizedDescription]) }
            try require(failed, "The invalid output fixture unexpectedly created an export.")
            try sameTransport(before, name)
        }
        try requireRunningOfflineGraph(graph)
        try await invalidDestination("invalid-path-playing-pattern")
        try await stopAndConsume()
        graph.stop()
        try require(!graph.isRunning && host.status()["playing"] as? Bool == false,
                    "The second invalid-path fixture must have a stopped graph and transport.")
        try await invalidDestination("invalid-path-idle")
        passed.append("Output errors after the export queues Play restore the exact prior running or stopped session")

        func cancelExport(_ name: String, originallyPlaying: Bool, mode: String) async throws {
            let before = observe(name + "-before")
            let originalProject = try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            try require(before["playing"] as? Bool == originallyPlaying,
                        "The cancellation fixture has the wrong initial transport state.")
            var requested = false
            var cancelTask: Task<Void, Never>?
            var snapshotDuringExport: [String: Any]?
            host.onEvent = { event, _ in
                guard event == "audio.exportProgress", !requested else { return }
                requested = true
                // Export yields immediately after its progress callback. The higher
                // priority task requests Stop at that boundary without timing sleeps.
                cancelTask = Task(priority: .high) { @MainActor in
                    do {
                        snapshotDuringExport = try await host.handle("engine.snapshot", payload: [:]) as? [String: Any]
                        _ = try await host.handle("transport.stop", payload: [:])
                    }
                    catch { cancellationError = error.localizedDescription }
                }
            }
            var errorText: String?
            do { try await export(name, tail: 0.125, mode: mode) }
            catch { errorText = error.localizedDescription }
            await cancelTask?.value
            host.onEvent = nil
            try require(requested && cancellationError == nil, "The progress callback never successfully requested export cancellation.")
            try require(errorText?.localizedCaseInsensitiveContains("cancel") == true,
                        "The export did not report cancellation: \(errorText ?? "completed successfully").")
            try require(snapshotDuringExport != nil, "The export progress task did not capture a project snapshot.")
            let keys = ["sequence", "tracks", "metronome", "loopSong"]
            let originalFields = Dictionary(uniqueKeysWithValues: keys.map { ($0, originalProject[$0] ?? NSNull()) })
            let duringFields = Dictionary(uniqueKeysWithValues: keys.map { ($0, snapshotDuringExport?[$0] ?? NSNull()) })
            let originalJSON = try JSONSerialization.data(withJSONObject: originalFields, options: .sortedKeys)
            let duringJSON = try JSONSerialization.data(withJSONObject: duringFields, options: .sortedKeys)
            try require(originalJSON == duringJSON,
                        "A snapshot during \(mode) export saved temporary sequence, mute/solo or metronome changes.")
            measurements.append(["name": name + "-project-during-export", "mode": mode, "projectFields": duringFields])
            try sameTransport(before, name, forceStopped: true)
            if graph.isRunning {
                let peak = try render(512)
                try require(peak < 0.000001, "A canceled export resumed music (peak \(peak)).")
                try sameTransport(before, name + "-after-next-buffer", forceStopped: true)
            }
        }

        try await apply(project(active: 1, long: true))
        _ = try await host.handle("transport.seek", payload: ["pattern": 1, "row": 5, "sequence": 1])
        _ = try await host.handle("transport.play", payload: ["mode": "pattern"])
        try render(1234)
        try await stopAndConsume()
        graph.stop()
        try await cancelExport("cancel-idle-export", originallyPlaying: false, mode: "pattern")
        passed.append("Canceling Pattern export restores the idle cursor and saves the original sequence and metronome during progress")

        _ = try await host.handle("transport.play", payload: ["mode": "pattern"])
        try render(1234)
        try await cancelExport("cancel-playing-export", originallyPlaying: true, mode: "stems")
        passed.append("Stop during stem export stops prior playback, preserves its cursor and keeps temporary mute/solo changes out of snapshots")
        return passed
    }

    private static func requireRunningOfflineGraph(_ graph: AVAudioEngine) throws {
        guard graph.isRunning && graph.isInManualRenderingMode && graph.manualRenderingMode == .offline else {
            throw AudioSampleError.invalid("The failed-output fixture must still have its running offline graph.")
        }
    }
}

#endif

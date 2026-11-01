// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// "Render selection to sample" on a private manual-rendering host (no device, no MIDI):
/// sample-accurate start, clean start, only the selected tracks, tail detection that stops
/// at silence or caps at the maximum, Through master vs Track only, mono / 44.1 kHz /
/// normalize, the default Recordings location and Save collecting the file, and a render
/// while the live host is playing that never pauses it.
enum NativeRenderSelectionTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("render-selection", isDirectory: true)
        try? FileManager.default.removeItem(at: folder)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], metrics: [String: Any] = [:]
        defer {
            metrics["checks"] = checks
            if let data = try? JSONSerialization.data(withJSONObject: metrics, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        func require(_ condition: Bool, _ message: @autoclosure () -> String) throws {
            if !condition { throw AudioSampleError.invalid("Render selection: " + message()) }
        }
        let rate = 48_000.0, rowFrames = 6_000 // 120 BPM, 4 lines per beat
        func write(_ name: String, _ values: [Float]) throws -> String {
            let url = folder.appendingPathComponent(name)
            try AudioSamples.write(DecodedSample(path: url.path, sampleRate: rate, left: values, right: values, originalChannels: 2), to: url)
            return url.path
        }
        // Long ringing hit (above −70 dBFS for about 0.6 s) and a short negative hit.
        let long = try write("long.wav", (0..<96_000).map { Float(0.5 * exp(-Double($0) / 4_000)) })
        let other = try write("other.wav", (0..<4_800).map { Float(-0.4 * exp(-Double($0) / 900)) })
        func instrument(_ path: String, _ name: String, release: Double = 0.005) -> [String: Any] {
            ["type": "sample", "name": name, "samplePath": path, "rootNote": 60, "volume": 1.0, "mode": "oneshot",
             "attack": 0, "decay": 0, "sustain": 1, "release": release]
        }
        func note(_ instrument: Int? = nil) -> [String: Any] {
            var step: [String: Any] = ["note": 60, "velocity": 127, "gate": 1.0]
            if let instrument { step["instrument"] = instrument }
            return step
        }
        let rows = 16
        var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: rows), count: 4)
        steps[0][0] = note()          // Kick: long note before the 1–3 selection (must not be chased)
        steps[0][4] = note()          // Kick onset at the first row of 4–7
        steps[1][2] = note()          // Snare inside 1–3
        steps[1][6] = note()
        for row in stride(from: 0, to: rows, by: 2) { steps[2][row] = note() } // Other: never selected
        steps[3][7] = note()          // Ring: long sample on the last row of 4–7
        let tracks: [[String: Any]] = [
            ["name": "Kick", "routingID": "kick", "instrument": 0, "volume": 1.0, "pan": 0, "groupID": "kit"],
            ["name": "Snare", "routingID": "snare", "instrument": 1, "volume": 1.0, "pan": 0, "groupID": "kit"],
            ["name": "Other", "routingID": "other", "instrument": 2, "volume": 1.0, "pan": 0],
            ["name": "Ring", "routingID": "ring", "instrument": 3, "volume": 1.0, "pan": 0]]
        let base: [String: Any] = ["id": "render-selection", "version": 1, "name": "Render selection", "bpm": 120.0, "linesPerBeat": 4,
            "masterVolume": 0.5, "tracks": tracks,
            "instruments": [instrument(long, "Kick"), instrument(other, "Snare"), instrument(other, "Other"), instrument(long, "Ring", release: 1.0)],
            "groups": [["id": "kit", "name": "Kit", "color": "#e2a44f", "collapsed": false, "mixerCollapsed": false, "busID": "kitbus"]],
            "patterns": [["id": 0, "name": "P0", "length": rows, "steps": steps, "automation": []]], "sequence": [0], "activePattern": 0,
            "loopSong": true, "audioPlugins": [], "audioClips": [], "delay": ["mix": 0], "reverb": ["mix": 0],
            "routing": ["version": 1, "mixModel": "standard",
                        "buses": [["id": "kitbus", "name": "Kit", "type": "group", "groupID": "kit", "volume": 1, "pan": 0, "mute": false, "solo": false]],
                        "outputs": ["track:kick": "bus:kitbus", "track:snare": "bus:kitbus", "track:other": "master", "track:ring": "master", "bus:kitbus": "master"],
                        "sends": [], "sidechains": []] as [String: Any]]

        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, midiInputIDs: [], midiInputSourceProvider: { [] }, midiOutputDestinationProvider: { [] })
        defer { host.shutdown() }
        func load(_ project: [String: Any]) async throws {
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
            await host.awaitProjectAssets()
        }
        var index = 0
        func render(_ tracks: [Int], _ first: Int, _ last: Int, _ extra: [String: Any] = [:]) async throws -> (result: [String: Any], sample: DecodedSample) {
            index += 1
            var payload: [String: Any] = ["pattern": 0, "tracks": tracks, "firstRow": first, "lastRow": last, "mode": "master", "tail": 10, "name": "Test \(index)"]
            payload.merge(extra) { _, new in new }
            let url = folder.appendingPathComponent("render-\(index).wav")
            let result = try await host.renderSelection(payload, destination: url)
            return (result, try AudioSamples.decode(url.path))
        }
        func firstAbove(_ values: [Float], _ threshold: Float = 1e-4) -> Int { values.firstIndex { abs($0) > threshold } ?? -1 }
        func maxDiff(_ a: [Float], _ b: [Float]) -> Float {
            guard a.count == b.count else { return .infinity }
            return zip(a, b).reduce(0) { max($0, abs($1.0 - $1.1)) }
        }
        try await load(base)

        // Sample-accurate start: the onset of the first selected row is output frame 0.
        let onset = try await render([0], 4, 6)
        try require(firstAbove(onset.sample.left) == 0, "the row-4 onset starts at frame \(firstAbove(onset.sample.left)), not 0")
        try require(onset.result["musicalFrames"] as? Int64 == Int64(3 * rowFrames), "musical length \(onset.result["musicalFrames"] ?? "nil") is not 3 rows")
        checks.append("The first selected row starts at output frame 0 (sample-accurate), musical length = rows × row length")

        // Clean start: the Kick note sounding since row 0 is not chased; the Snare at row 2 starts at frame 6000.
        let clean = try await render([0, 1], 1, 3)
        try require(abs(firstAbove(clean.sample.left) - rowFrames) <= 1, "the row-2 snare starts at \(firstAbove(clean.sample.left)), expected \(rowFrames)")
        try require(clean.sample.left.prefix(rowFrames - 1).allSatisfy { abs($0) < 1e-6 }, "a note sounding before the selection was rendered")
        checks.append("Clean start: only notes starting inside the selection sound; a row-2 onset lands exactly one row (6000 frames) in")

        // Only the selected tracks: Other plays on every even row but adds nothing.
        var silentOther = base
        var otherless = steps; otherless[2] = [Any](repeating: NSNull(), count: rows)
        silentOther["patterns"] = [["id": 0, "name": "P0", "length": rows, "steps": otherless, "automation": []]]
        let withOther = try await render([0, 1], 4, 7, ["tail": 0.5])
        let both = try await render([0, 1, 2], 4, 7, ["tail": 0.5])
        try await load(silentOther)
        let without = try await render([0, 1], 4, 7, ["tail": 0.5])
        try await load(base)
        let exclusion = maxDiff(withOther.sample.left, without.sample.left)
        try require(exclusion < 1e-6, "an unselected track was audible (max difference \(exclusion))")
        try require(both.sample.left.count != withOther.sample.left.count || maxDiff(both.sample.left, withOther.sample.left) > 0.01, "selecting Other did not add it")
        metrics["exclusionMaxDiff"] = exclusion
        checks.append("Unselected tracks are silent: the render equals the same project with the other track emptied (max diff \(exclusion))")

        // Tail: the note held on the last row is released at the selection end; its 1 s release
        // is rendered past the selection until the output falls below −70 dBFS.
        let threshold = AudioRenderSelection.linear(dB: -70)
        let ring = try await render([3], 4, 7)
        let ringFrames = ring.sample.left.count, musical = 4 * rowFrames
        try require(ring.result["tailEnd"] as? String == "silence", "the ringing tail did not end at silence: \(ring.result)")
        try require(ringFrames > musical + 10_000 && ringFrames < musical + 40_000, "ringing tail length \(ringFrames - musical) frames is implausible")
        try require(max(abs(ring.sample.left.last!), abs(ring.sample.right.last!)) >= threshold * 0.99, "the render was not trimmed at the last frame above the threshold")
        metrics["ringTailFrames"] = ringFrames - musical
        // A short hit that ends inside the selection: the sample is exactly the selection.
        let quiet = try await render([1], 6, 7)
        try require(quiet.sample.left.count == 2 * rowFrames && quiet.result["tailEnd"] as? String == "silence",
            "a short hit rendered \(quiet.sample.left.count) frames, expected the 2-row selection")
        checks.append("Tail detection renders a ringing last note \(ringFrames - musical) frames past the selection and stops below −70 dBFS")

        // Maximum tail caps the render (and fades it).
        let capped = try await render([3], 4, 7, ["tail": 0.1])
        try require(capped.result["tailEnd"] as? String == "maximum", "the 0.1 s maximum tail was not reached: \(capped.result)")
        try require(capped.sample.left.count == musical + 4_800, "capped length \(capped.sample.left.count) ≠ selection + 0.1 s")
        try require(abs(capped.sample.left.last!) < 1e-3, "the capped end is not faded")
        checks.append("A tail longer than the maximum is cut at selection + maximum tail, with a short fade")

        // Through master (master fader 0.5) vs Track only (master at unity).
        let master = try await render([0], 4, 6, ["mode": "master"])
        let track = try await render([0], 4, 6, ["mode": "track"])
        let ratio = (master.result["peak"] as? Float ?? 0) / max(1e-9, track.result["peak"] as? Float ?? 1)
        try require(abs(ratio - 0.5) < 0.02, "Through master / Track only peak ratio \(ratio), expected the 0.5 master fader")
        metrics["masterRatio"] = ratio
        checks.append("Through master applies the master fader (ratio \(String(format: "%.3f", ratio))); Track only renders before it")

        // Mono, 44.1 kHz and normalize.
        let mono = try await render([0, 1], 4, 7, ["channels": 1, "sampleRate": 44_100, "normalize": true, "tail": 0.5])
        try require(mono.sample.originalChannels == 1 && mono.sample.sampleRate == 44_100, "mono 44.1 kHz file has \(mono.sample.originalChannels) channels at \(mono.sample.sampleRate) Hz")
        let peak = mono.sample.left.reduce(Float(0)) { max($0, abs($1)) }
        try require(abs(peak - AudioRenderSelection.linear(dB: -0.3)) < 0.02, "normalized peak \(peak) is not −0.3 dBFS")
        checks.append("Mono, 44.1 kHz and normalize to −0.3 dBFS")

        // Default location: Recordings in the support folder; Save collects it into the project's media.
        let stored = try await host.handle("render.selection", payload: ["pattern": 0, "tracks": [0, 1], "firstRow": 4, "lastRow": 7, "name": "Kick+Snare 04–07", "tail": 0.5]) as! [String: Any]
        let path = stored["path"] as! String
        try require(path.contains("/Recordings/Kick-Snare 04-07-") && path.hasSuffix(".wav") && FileManager.default.fileExists(atPath: path), "unexpected render location \(path)")
        var document = base
        var instruments = document["instruments"] as! [[String: Any]]
        instruments.append(["type": "sample", "name": "Kick+Snare 04–07", "samplePath": path, "rootNote": 60, "mode": "oneshot"])
        document["instruments"] = instruments
        let store = try ProjectStore(directory: folder.appendingPathComponent("store"))
        try store.update(document)
        let saved = folder.appendingPathComponent("Saved.supertracker")
        _ = try store.save(to: saved)
        let media = (try? FileManager.default.contentsOfDirectory(atPath: folder.appendingPathComponent("Saved.media").path)) ?? []
        try require(media.contains { $0.hasSuffix(URL(fileURLWithPath: path).lastPathComponent) }, "Save did not collect the rendered sample (\(media))")
        checks.append("Renders are written to <support>/Recordings and Save collects them into <project>.media")

        // Refusal: a selection without notes.
        var refusal = ""
        do { _ = try await render([2], 1, 1) } catch { refusal = error.localizedDescription }
        try require(refusal.contains("no notes"), "an empty selection was not refused: \(refusal)")
        checks.append("A selection without notes is refused with a reason")

        // Plug-ins: the group bus and master inserts on the path load; the unselected track's does not.
        let catalogue = BuiltinEffectsAudioUnit.catalogue().compactMap { $0["id"] as? String }
        func effect(_ name: String) -> String { catalogue.first { $0 == "stfx:" + name } ?? catalogue[0] }
        var withPlugins = base
        withPlugins["audioPlugins"] = [
            ["owner": "bus:kitbus", "kind": "effect", "slot": 0, "component": effect("eq"), "instanceID": "bus-eq"],
            ["owner": "track:other", "kind": "effect", "slot": 0, "component": effect("saturator"), "instanceID": "other-saturator"],
            ["owner": "master", "kind": "effect", "slot": 0, "component": effect("dynamics"), "instanceID": "master-dynamics"]]
        try await load(withPlugins)
        let pathPlugins = try await render([0], 4, 6, ["mode": "master"]).result["plugins"] as? Int
        let preMasterPlugins = try await render([0], 4, 6, ["mode": "track"]).result["plugins"] as? Int
        try require(pathPlugins == 2 && preMasterPlugins == 1, "path plug-ins \(pathPlugins ?? -1) (expected bus + master = 2), pre-master \(preMasterPlugins ?? -1) (expected 1)")
        try await load(base)
        checks.append("Only plug-ins on the selection's path load: group bus and master (Through master) or the bus alone (Track only)")

        // While playing: the live host keeps rendering 512-frame blocks with no pause and an exact clock.
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
        func seconds() -> Double { (host.status()["seconds"] as? NSNumber)?.doubleValue ?? -1 }
        func block() throws {
            let before = seconds()
            try require(try host.renderOfflineQuantum(frames: 512, to: buffer) == .success, "live render failed")
            try require(abs((seconds() - before) * rate - 512) < 0.001, "a live block did not advance 512 frames")
        }
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        for _ in 0..<8 { try block() }
        let pausesBefore = host.rack.enginePauses, nodesBefore = Set(graph.attachedNodes.map(ObjectIdentifier.init))
        let playing = try await render([0, 1], 4, 7, ["tail": 0.5])
        for _ in 0..<8 { try block() }
        try require(host.rack.enginePauses == pausesBefore, "rendering the selection paused the live graph")
        try require(Set(graph.attachedNodes.map(ObjectIdentifier.init)) == nodesBefore, "rendering the selection changed live graph nodes")
        try require(graph.isRunning && host.status()["playing"] as? Bool == true, "playback stopped")
        try require(maxDiff(playing.sample.left, withOther.sample.left) < 1e-6, "a render during playback differs from a stopped render")
        _ = try await host.handle("transport.stop", payload: [:])
        checks.append("Rendering during playback never pauses the live graph and equals the stopped render")
        return checks
    }
}
#endif

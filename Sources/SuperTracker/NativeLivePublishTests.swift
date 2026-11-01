// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import Darwin

/// Regression for "playback hangs every few steps" with a kit and VST3 hardware
/// editors: every note edit published the project and every autosave captured each
/// VST3 state by pausing the graph (one audible gap and IO restart per instance), and
/// the interface adopting those captured states made the next note edit restore them
/// again. Editing notes, autosaving and adopting a snapshot while playing must neither
/// pause the graph nor rebuild routing; transport must advance exactly once per block.
/// Private offline graph, the original VST3 fixture and built-in effects only.
enum NativeLivePublishTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("live-publish", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], metrics: [String: Any] = ["hardware": false, "vendors": false, "mode": "manual-offline"]
        defer {
            metrics["checks"] = checks
            if let data = try? JSONSerialization.data(withJSONObject: metrics, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        func require(_ condition: Bool, _ message: @autoclosure () -> String) throws {
            if !condition { throw AudioSampleError.invalid("Live publish: " + message()) }
        }
        let bundle = URL(fileURLWithPath: ProcessInfo.processInfo.environment["SUPERTRACKER_VST3_FIXTURE"]
            ?? FileManager.default.currentDirectoryPath + "/test-output/Fixture.vst3").standardizedFileURL
        try require(FileManager.default.fileExists(atPath: bundle.path), "Build the original VST3 fixture first: \(bundle.path)")
        var configuration = VST3Catalogue.Configuration()
        configuration.roots = [bundle.deletingLastPathComponent()]
        configuration.cacheURL = folder.appendingPathComponent("vst3.json")
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: configuration),
            midiInputIDs: [], midiInputSourceProvider: { [] }, midiOutputDestinationProvider: { [] })
        defer { host.shutdown() }

        // Hardware-template-like tracks hold the fixture VST3; a sixteen-pad group with
        // per-pad and bus built-in inserts stands in for an imported Maschine kit.
        let catalogue = BuiltinEffectsAudioUnit.catalogue().compactMap { $0["id"] as? String }
        func effect(_ name: String) -> String { catalogue.first { $0 == "stfx:" + name } ?? catalogue[0] }
        let padEffects = [effect("saturator"), effect("eq"), effect("reverb")], busEffects = [effect("dynamics"), effect("metaverb")]
        let fixture = "vst3:0123456789ABCDEF0123456789ABCDEF"
        let rate = 48_000.0
        let hit = (0..<4800).map { Float(sin(Double($0) * 2 * Double.pi * 110 / rate) * exp(-Double($0) / 900) * 0.5) }
        let samplePath = folder.appendingPathComponent("hit.wav")
        try AudioSamples.write(DecodedSample(path: samplePath.path, sampleRate: rate, left: hit, right: hit, originalChannels: 2), to: samplePath)
        let rows = 256
        var tracks: [[String: Any]] = [], instruments: [[String: Any]] = [], plugins: [[String: Any]] = []
        var outputs: [String: Any] = [:]
        for index in 0..<3 {
            tracks.append(["name": "HW \(index)", "routingID": "hw\(index)", "instrument": instruments.count, "volume": 0.5, "pan": 0])
            instruments.append(["type": "au", "volume": 0.5, "pan": 0])
            plugins.append(["owner": "track:hw\(index)", "kind": "instrument", "slot": 0, "component": fixture, "instanceID": "hw-\(index)"])
        }
        for pad in 0..<16 {
            let routingID = "kit-pad\(pad)"
            tracks.append(["name": "Pad \(pad + 1)", "routingID": routingID, "instrument": instruments.count, "volume": 0.8, "pan": 0, "groupID": "kit"])
            instruments.append(["type": "sample", "samplePath": samplePath.path, "rootNote": 60, "volume": 0.8, "mode": "oneshot",
                                "attack": 0, "decay": 0, "sustain": 1, "release": 0.01])
            outputs["track:" + routingID] = "bus:kitbus"
            for (slot, component) in padEffects.enumerated() {
                plugins.append(["owner": "track:" + routingID, "kind": "effect", "slot": slot, "component": component, "instanceID": "pad\(pad)-fx\(slot)"])
            }
        }
        for (slot, component) in busEffects.enumerated() {
            plugins.append(["owner": "bus:kitbus", "kind": "effect", "slot": slot, "component": component, "instanceID": "bus-fx\(slot)"])
        }
        outputs["bus:kitbus"] = "master"
        var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: rows), count: tracks.count)
        for row in stride(from: 0, to: rows, by: 4) { steps[3][row] = ["note": 60, "velocity": 110, "gate": 1.0] }       // kick
        for row in stride(from: 4, to: rows, by: 8) { steps[4][row] = ["note": 60, "velocity": 100, "gate": 1.0] }      // snare
        for row in stride(from: 0, to: rows, by: 16) { steps[0][row] = ["note": 48, "instrument": 0, "velocity": 90, "gate": 2.0] }
        var project: [String: Any] = ["id": "live-publish", "version": 1, "name": "Live publish", "bpm": 120.0, "linesPerBeat": 4,
            "masterVolume": 0.0, "tracks": tracks, "instruments": instruments,
            "groups": [["id": "kit", "name": "Kit", "color": "#e2a44f", "collapsed": false, "mixerCollapsed": false, "busID": "kitbus"]],
            "patterns": [["id": 0, "name": "P0", "length": rows, "steps": steps, "automation": []]], "sequence": [0], "activePattern": 0,
            "loopSong": true, "audioPlugins": plugins, "audioClips": [], "delay": ["mix": 0], "reverb": ["mix": 0],
            "routing": ["version": 1, "mixModel": "standard",
                        "buses": [["id": "kitbus", "name": "Kit", "type": "group", "groupID": "kit", "volume": 1, "pan": 0, "mute": false, "solo": false]],
                        "outputs": outputs, "sends": [], "sidechains": []] as [String: Any]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        let loaded = host.rack.snapshot(fresh: false)
        try require(loaded.count == plugins.count, "expected \(plugins.count) loaded plug-ins, got \(loaded.count)")

        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
        func seconds() -> Double { (host.status()["seconds"] as? NSNumber)?.doubleValue ?? -1 }
        func render(_ quanta: Int = 1) throws {
            for _ in 0..<quanta {
                let before = seconds()
                try require(try host.renderOfflineQuantum(frames: 512, to: buffer) == .success, "offline render failed")
                let advanced = (seconds() - before) * rate
                try require(abs(advanced - 512) < 0.001, "a 512-frame block advanced transport by \(advanced) frames")
            }
        }
        func routing() -> [String: Any] { host.status()["routing"] as? [String: Any] ?? [:] }
        var timebase = mach_timebase_info_data_t(); mach_timebase_info(&timebase)
        func milliseconds(_ start: UInt64) -> Double { Double(mach_absolute_time() - start) * Double(timebase.numer) / Double(timebase.denom) / 1e6 }
        func snapshot() async throws -> [String: Any] { try await host.handle("engine.snapshot", payload: [:]) as! [String: Any] }
        func fixtureState(_ value: [String: Any], _ id: String) -> String? {
            (value["audioPlugins"] as? [[String: Any]])?.first { $0["instanceID"] as? String == id }?["state"] as? String
        }

        _ = try await host.handle("transport.recordArm", payload: ["armed": true])
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        try render(8)
        let pausesBefore = host.rack.enginePauses, capturesBefore = host.rack.concurrentStateCaptures
        let patternOnlyBefore = host.patternDataPublications
        let rebuildsBefore = routing()["fullRebuilds"] as? Int ?? 0
        let nodesBefore = Set(graph.attachedNodes.map(ObjectIdentifier.init))
        var publishTimes: [Double] = [], snapshotTimes: [Double] = []
        func publish() async throws {
            let start = mach_absolute_time()
            _ = try await host.handle("engine.project", payload: ["project": project, "awaitPlugins": true])
            publishTimes.append(milliseconds(start))
        }
        let initialState = fixtureState(try await snapshot(), "hw-0")
        try require(initialState != nil, "the fixture state was not captured")
        for edit in 0..<24 {
            // A note edit, published the way the pattern editor does.
            var patterns = project["patterns"] as! [[String: Any]], grid = patterns[0]["steps"] as! [[Any]]
            let row = 2 + edit * 8
            grid[5][row] = edit % 3 == 2 ? NSNull() : ["note": 62, "velocity": 90 + edit, "gate": 1.0] as [String: Any]
            patterns[0]["steps"] = grid; project["patterns"] = patterns
            try await publish()
            try render(2)
            if edit % 4 == 3 {
                // Autosave captures every plug-in while playing; the interface then adopts
                // those descriptors and publishes them with its next edit.
                let start = mach_absolute_time()
                let captured = try await snapshot()
                snapshotTimes.append(milliseconds(start))
                project["audioPlugins"] = captured["audioPlugins"]
                try render(1)
                try await publish()
                try render(1)
            }
        }
        // A control edit on the playing VST3 reaches the processor on the next block,
        // and the following live capture contains it without pausing.
        _ = try await host.handle("plugin.parameter", payload: ["owner": "track:hw0", "kind": "instrument", "slot": 0, "address": "42", "value": 0.8])
        try render(1)
        let edited = try await snapshot()
        try require(fixtureState(edited, "hw-0") != nil && fixtureState(edited, "hw-0") != initialState,
            "a live capture after a delivered VST3 edit did not contain the edit")
        project["audioPlugins"] = edited["audioPlugins"]
        try await publish()
        try render(2)

        let pauses = host.rack.enginePauses - pausesBefore, captures = host.rack.concurrentStateCaptures - capturesBefore
        let rebuilds = (routing()["fullRebuilds"] as? Int ?? 0) - rebuildsBefore
        let sorted = publishTimes.sorted(), snapshots = snapshotTimes.sorted()
        metrics["enginePauses"] = Int(pauses); metrics["concurrentCaptures"] = Int(captures); metrics["fullRebuilds"] = rebuilds
        metrics["publishMedianMs"] = sorted[sorted.count / 2]; metrics["publishMaxMs"] = sorted.last ?? 0
        metrics["snapshotMedianMs"] = snapshots[snapshots.count / 2]; metrics["plugins"] = plugins.count
        print("live publish: \(publishTimes.count) publications median \(String(format: "%.1f", sorted[sorted.count / 2])) ms, \(snapshots.count) live captures median \(String(format: "%.1f", snapshots[snapshots.count / 2])) ms, pauses \(pauses)")
        try require(pauses == 0, "note edits, autosave captures and snapshot adoption paused the playing graph \(pauses) times")
        try require(captures >= UInt64(3 * (snapshots.count + 1)), "VST3 states were not captured while rendering continued (\(captures))")
        try require(rebuilds == 0, "note edits rebuilt the routing graph \(rebuilds) times")
        let patternOnly = host.patternDataPublications - patternOnlyBefore
        metrics["patternDataPublications"] = Int(patternOnly)
        try require(patternOnly >= 24, "note-only edits re-planned routing and plug-ins (\(patternOnly) of 24 took the pattern-data path)")
        try require(Set(graph.attachedNodes.map(ObjectIdentifier.init)) == nodesBefore, "note edits replaced live graph nodes")
        try require(graph.isRunning && host.status()["playing"] as? Bool == true, "playback stopped")
        checks.append("Note edits, live autosave captures and adopted snapshots never pause the playing graph or rebuild routing")
        checks.append("Transport advances exactly 512 frames per block across every publication and capture")
        checks.append("A VST3 edit delivered by the render thread is in the next live capture")
        checks.append("Note-only edits publish pattern data without re-planning routing or reconciling plug-ins")
        _ = try await host.handle("transport.stop", payload: [:])
        return checks
    }
}
#endif

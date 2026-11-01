// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox
import TrackerEngine

/// Host regressions for stem export, MIDI input ownership, pad reorder note
/// ownership and AU transport position. Private offline graphs, generated PCM
/// and original note-counting units only; no device, MIDI endpoint or vendor.
enum NativeAudioHostFixTests {
    private static let countingSubtype: OSType = 0x6E637474 // nctt
    private static let manufacturer: OSType = 0x53744678 // StFx
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(NoteCountingUnit.self,
            as: AudioComponentDescription(componentType: kAudioUnitType_MusicDevice, componentSubType: countingSubtype,
                componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0),
            name: "SuperTracker Test: note counter", version: 1)
    }()
    private static var component: String { "\(kAudioUnitType_MusicDevice):\(countingSubtype):\(manufacturer)" }

    @MainActor static func run(output: URL) async throws -> [String] {
        _ = registration
        let folder = output.appendingPathComponent("audio-host-fixes", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], failures: [String] = []
        func attempt(_ name: String, _ body: () async throws -> Void) async {
            do { try await body(); checks.append(name) } catch { failures.append("\(name): \(error.localizedDescription)") }
        }
        await attempt("Legacy stems keep the user's mute and solo, so stems sum to the song mix") { try await stems(folder) }
        await attempt("Unowned MIDI note-offs are dropped and unchanged input settings keep held notes") { try await midiInput(folder) }
        await attempt("A MIDI route published while plug-ins restore starts audio only after they have loaded") { try await midiRouteDuringRestore(folder) }
        await attempt("Pad reorder releases held AU notes on the moved plug-ins without stray note-offs") { try await padReorder(folder) }
        await attempt("AU transport reports song position, not graph frames, without unit timing") { try await transportPosition(folder) }
        await attempt("VST3 kLatencyChanged reactivates off the render thread and refreshes PDC; sequenced notes clear kIsLive") { try await vst3Restart(folder) }
        await attempt("A VST3 loaded with saved state while playing pauses the graph once, for its wiring only") { try await vst3StatefulLoad(folder) }
        await attempt("An insert removed while a plug-in restore defers rebuilds leaves the routed graph at once") { try await removalDuringBatch(folder) }
        await attempt("Live sampler parameters reach a held note and the sample editor playhead snapshot follows it") { try await samplerLive(folder) }
        let report: [String: Any] = ["checks": checks, "failures": failures, "hardware": false, "vendors": false]
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
        }
        NoteCountingRegistry.shared.removeAll()
        guard failures.isEmpty else { throw AudioSampleError.invalid("Audio host fixes: " + failures.joined(separator: " | ")) }
        return checks
    }

    fileprivate static func require(_ condition: Bool, _ message: String) throws {
        if !condition { throw AudioSampleError.invalid(message) }
    }

    @MainActor private static func offlineHost() throws -> (AudioHost, AVAudioEngine, AVAudioPCMBuffer) {
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, midiInputIDs: [], midiInputSourceProvider: { [] }, midiOutputDestinationProvider: { [] })
        return (host, graph, AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!)
    }

    @MainActor fileprivate static func render(_ host: AudioHost, _ buffer: AVAudioPCMBuffer, frames: Int = 4096) throws {
        var remaining = frames
        while remaining > 0 {
            let count = AVAudioFrameCount(min(4096, remaining))
            let status = try host.renderOfflineQuantum(frames: count, to: buffer)
            try require(status == .success, "Offline render returned \(status.rawValue).")
            remaining -= Int(count)
        }
    }

    // MARK: Item 1 — legacy stems follow the mix
    @MainActor private static func stems(_ folder: URL) async throws {
        let (host, _, _) = try offlineHost()
        defer { host.shutdown() }
        let rate = 48_000.0, sourceURL = folder.appendingPathComponent("stem-source.wav")
        let wave = (0..<9600).map { Float(sin(Double($0) * 2 * Double.pi * 220 / rate) * 0.4) }
        try AudioSamples.write(DecodedSample(path: sourceURL.path, sampleRate: rate, left: wave, right: wave, originalChannels: 2), to: sourceURL)
        var steps = Array(repeating: [Any](repeating: NSNull(), count: 8), count: 3)
        for (track, note) in [60, 64, 67].enumerated() { steps[track][0] = ["note": note, "instrument": 0, "velocity": 100, "gate": 0.8] }
        func project(_ flags: [(mute: Bool, solo: Bool)]) -> [String: Any] {
            ["version": 1, "name": "Stem mix", "bpm": 120, "linesPerBeat": 4, "masterVolume": 0.6, "activePattern": 0, "loopSong": false,
             "tracks": flags.enumerated().map { ["name": "T\($0.offset)", "instrument": 0, "volume": 0.6, "pan": 0.2 * Double($0.offset), "mute": $0.element.mute, "solo": $0.element.solo] },
             "instruments": [["type": "sample", "samplePath": sourceURL.path, "rootNote": 60, "volume": 0.7, "mode": "oneshot", "attack": 0, "decay": 0, "sustain": 1, "release": 0.001]],
             "patterns": [["id": 0, "length": 8, "steps": steps, "automation": []]], "sequence": [0], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        }
        for (label, flags, audible) in [("muted", [(false, false), (true, false), (false, false)], [true, false, true]),
                                        ("solo", [(false, true), (false, false), (true, true)], [true, false, false])] {
            _ = try await host.handle("engine.project", payload: ["project": project(flags)])
            let mix = folder.appendingPathComponent("stem-\(label)-mix.wav")
            _ = try await host.handle("audio.export", payload: ["path": mix.path, "mode": "song", "tail": 0, "encoding": "float32"])
            let result = try await host.handle("audio.export", payload: ["path": folder.appendingPathComponent("stems-\(label)").path,
                "mode": "stems", "tail": 0, "encoding": "float32"]) as? [String: Any] ?? [:]
            let paths = result["paths"] as? [String] ?? []
            try require(paths.count == 3, "\(label): legacy stems must keep one file per track, got \(paths.count).")
            let decoded = try paths.map { try AudioSamples.decode($0) }, song = try AudioSamples.decode(mix.path)
            for (index, stem) in decoded.enumerated() {
                let peak = stem.left.map(abs).max() ?? 0
                try require(audible[index] ? peak > 0.005 : peak < 0.000001, "\(label): stem \(index) audible=\(peak) but the mix \(audible[index] ? "plays" : "silences") it.")
            }
            let error = (0..<song.left.count).map { frame in
                abs(decoded.reduce(Float(0)) { $0 + $1.left[frame] } - song.left[frame]) + abs(decoded.reduce(Float(0)) { $0 + $1.right[frame] } - song.right[frame])
            }.max() ?? .infinity
            try require(decoded.allSatisfy { $0.left.count == song.left.count } && error < 0.00001, "\(label): stems do not sum to the song mix (max error \(error)).")
        }
    }

    // MARK: Live sampler parameters and playheads (engine.instrumentParameter, sampler.playheads)
    @MainActor private static func samplerLive(_ folder: URL) async throws {
        let (host, _, buffer) = try offlineHost()
        defer { host.shutdown() }
        let project: [String: Any] = ["version": 1, "id": "sampler-live", "name": "Sampler live", "bpm": 120, "linesPerBeat": 4,
            "masterVolume": 1, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Grains", "instrument": 0, "volume": 1, "pan": 0]],
            "instruments": [["type": "sample", "mode": "granular", "volume": 1, "pan": 0, "attack": 0, "sustain": 1, "grainDensity": 2]],
            "patterns": [["id": 0, "length": 16, "steps": [[Any]](), "automation": []]], "sequence": [0],
            "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        let pcm = (0..<48_000).map { Float(sin(Double($0) * 0.05) * 0.5) }
        try require(pcm.withUnsafeBufferPointer { st_engine_set_sample(host.engine, 0, $0.baseAddress, nil, UInt64($0.count), 48_000) } != 0, "The test sample was not accepted.")
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 60, "velocity": 100])
        try render(host, buffer)
        let watching = try await host.handle("sampler.playheads", payload: ["enabled": true, "instrument": 0]) as? [String: Any]
        try require(watching?["watching"] as? Int == 0, "The playhead watch did not start for instrument 0.")
        let accepted = try await host.handle("engine.instrumentParameter", payload: ["instrument": 0, "target": "grainDensity", "value": 5]) as? [String: Any]
        try require(accepted?["ok"] as? Bool == true, "A live granular parameter was refused.")
        _ = try await host.handle("engine.instrumentParameter", payload: ["instrument": 0, "target": "cutoff", "value": 900])
        try render(host, buffer)
        var voices = [STVoicePosition](repeating: STVoicePosition(), count: 8)
        let count = voices.withUnsafeMutableBufferPointer { st_engine_voice_positions(host.engine, $0.baseAddress, 8, nil) }
        try require(count == 1 && voices[0].grainCount == 5 && voices[0].mode == 6, "The held note did not take the live density (count \(count), grains \(voices.first?.grainCount ?? -1)).")
        let payload = AudioHost.playheadPayload(voices[0])
        try require((payload["grains"] as? [[Double]])?.count == 5 && payload["frames"] as? Double == 48_000, "The playhead event payload is incomplete.")
        do { _ = try await host.handle("engine.instrumentParameter", payload: ["instrument": 0, "target": "volume", "value": 1]); try require(false, "A track mix target was accepted as a live instrument parameter.") }
        catch let error as AudioSampleError where !"\(error.localizedDescription)".contains("track mix") {}
        _ = try await host.handle("sampler.playheads", payload: ["enabled": false])
        try render(host, buffer)
        let after = voices.withUnsafeMutableBufferPointer { st_engine_voice_positions(host.engine, $0.baseAddress, 8, nil) }
        try require(after == 0 && host.playheadTimer == nil, "Hiding the sample editor did not stop the playhead snapshot.")
    }

    // MARK: Item 2 — MIDI input ownership
    @MainActor private static func midiInput(_ folder: URL) async throws {
        let (host, _, buffer) = try offlineHost()
        defer { host.shutdown() }
        let project: [String: Any] = ["version": 1, "id": "midi-ownership", "name": "MIDI ownership", "bpm": 120, "linesPerBeat": 4,
            "masterVolume": 1, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Counter", "instrument": 0, "volume": 1, "pan": 0]], "instruments": [["type": "au", "volume": 1, "pan": 0]],
            "patterns": [["id": 0, "length": 16, "steps": [[Any]](), "automation": []]], "sequence": [0],
            "audioPlugins": [["track": 0, "kind": "instrument", "slot": 0, "component": component, "instanceID": "midi-counter"]],
            "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        _ = try await host.handle("midi.configure", payload: ["track": 0, "instrument": 0])
        // Start the private graph through an ordinary audition, then forget it.
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 1, "velocity": 1])
        _ = try await host.handle("note.off", payload: ["track": 0, "instrument": 0, "note": 1])
        try render(host, buffer)
        guard let unit = NoteCountingRegistry.shared.units.last else { throw AudioSampleError.invalid("The note counter was not instantiated.") }
        unit.clearCounts()
        host.receiveMIDI(0x90, 60, 100, source: 2); try render(host, buffer)
        try require(unit.held(60) == 1, "Device B's note did not reach the counter.")
        host.receiveMIDI(0x80, 60, 0, source: 1); try render(host, buffer)
        try require(unit.held(60) == 1 && unit.strayOffs == 0, "A note-off from device A without a recorded note-on released device B's held note.")
        _ = try await host.handle("midi.configure", payload: ["inputChannel": 0, "enabled": true]); try render(host, buffer)
        try require(unit.held(60) == 1, "An unchanged MIDI input configuration released held input notes.")
        _ = try await host.handle("midi.configure", payload: ["inputChannel": 2]); try render(host, buffer)
        try require(unit.held(60) == 0, "Changing the MIDI input channel did not release held input notes.")
        _ = try await host.handle("midi.configure", payload: ["inputChannel": 0])
        host.receiveMIDI(0x80, 60, 0, source: 2); try render(host, buffer)
        try require(unit.strayOffs == 0, "A release after the route-change release reached the instrument again.")
    }

    // The interface publishes its MIDI route (midi.configure) right after the launch project is
    // applied, while its plug-ins still restore; that used to start the graph mid-restore.
    @MainActor private static func midiRouteDuringRestore(_ folder: URL) async throws {
        let (host, graph, _) = try offlineHost()
        defer { host.shutdown() }
        let project: [String: Any] = ["version": 1, "id": "midi-restore", "name": "MIDI during restore", "bpm": 120, "linesPerBeat": 4,
            "masterVolume": 1, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Counter", "instrument": 0, "volume": 1, "pan": 0]], "instruments": [["type": "au", "volume": 1, "pan": 0]],
            "patterns": [["id": 0, "length": 16, "steps": [[Any]](), "automation": []]], "sequence": [0],
            "audioPlugins": [["track": 0, "kind": "instrument", "slot": 0, "component": component, "instanceID": "restore-counter"]],
            "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        try require(host.pendingPluginRestore != nil, "The project's plug-in was restored synchronously; the test needs a pending restore.")
        _ = try await host.handle("midi.configure", payload: ["track": 0, "instrument": 0])
        try require(!graph.isRunning, "Publishing the MIDI route started audio while the project's plug-ins were restoring.")
        try require(host.midiAutoStart, "The MIDI route did not defer its audio start until the restore finished.")
        await host.awaitPluginRestoration()
        try require(host.pendingPluginRestore == nil && host.rack.snapshot(fresh: false).count == 1, "The plug-in did not finish restoring.")
        _ = try await host.handle("midi.configure", payload: ["track": 0, "instrument": 0])
        try require(graph.isRunning, "Publishing the MIDI route after the restore did not start audio.")
    }

    // MARK: Item 3 — reordering group members keeps held AU notes with their plug-in
    @MainActor private static func padReorder(_ folder: URL) async throws {
        let (host, _, buffer) = try offlineHost()
        defer { host.shutdown() }
        // Two pads of a kit group: each member track has its own AU instrument.
        let tracks: [[String: Any]] = (0..<2).map { pad in ["routingID": "m\(pad)", "name": "Pad \(pad + 1)", "groupID": "k0", "pad": pad,
            "instrument": pad, "volume": 1, "pan": 0] }
        var project: [String: Any] = ["version": 1, "id": "pad-reorder", "name": "Pad reorder", "bpm": 120, "linesPerBeat": 4, "masterVolume": 1,
            "tracks": tracks, "instruments": [["type": "au", "volume": 1, "pan": 0], ["type": "au", "volume": 1, "pan": 0]],
            "groups": [["id": "k0", "name": "Kit", "busID": "g0"]],
            "patterns": [["id": 0, "length": 4, "steps": [[Any]](), "automation": []]], "sequence": [0], "activePattern": 0, "loopSong": false,
            "audioPlugins": (0..<2).map { ["owner": "track:m\($0)", "track": $0, "kind": "instrument", "slot": 0, "component": component, "instanceID": "counter-m\($0)"] },
            "delay": ["mix": 0], "reverb": ["mix": 0],
            "routing": ["version": 1, "mixModel": "standard", "buses": [["id": "g0", "name": "Kit", "type": "group", "groupID": "k0", "volume": 1]],
                        "outputs": ["track:m0": "bus:g0", "track:m1": "bus:g0"], "sends": [], "sidechains": []]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        for (owner, tag) in [("track:m0", 0.25), ("track:m1", 0.75)] {
            _ = try await host.handle("plugin.parameter", payload: ["owner": owner, "kind": "instrument", "slot": 0, "address": 0, "value": tag])
        }
        func unit(_ high: Bool) throws -> NoteCountingUnit {
            guard let found = NoteCountingRegistry.shared.units.last(where: { ($0.tag > 0.5) == high && $0.tag > 0 }) else {
                throw AudioSampleError.invalid("A tagged pad note counter is missing.")
            }
            return found
        }
        let first = try unit(false), second = try unit(true)
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 60, "velocity": 100])
        try render(host, buffer)
        try require(first.held(60) == 1 && second.held(60) == 0, "The pad note did not reach only its own plug-in.")
        // Move the first member after the second (Move track right inside the group).
        project = try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? project
        var moved = project["tracks"] as! [[String: Any]]; moved.swapAt(0, 1); project["tracks"] = moved
        project["audioPlugins"] = (project["audioPlugins"] as! [[String: Any]]).map { plugin in
            var copy = plugin; copy["track"] = plugin["owner"] as? String == "track:m0" ? 1 : 0; return copy
        }
        _ = try await host.handle("engine.project", payload: ["project": project, "awaitPlugins": true])
        try render(host, buffer)
        _ = try await host.handle("note.off", payload: ["track": 1, "instrument": 0, "note": 60])
        try render(host, buffer, frames: 8192)
        try require(try unit(false) === first && unit(true) === second, "Member reorder replaced a pad plug-in.")
        try require(first.held(60) == 0, "The moved member's plug-in kept its note after its key was released.")
        try require(second.strayOffs == 0, "The plug-in now on the old member channel received a note-off it never played.")
        _ = try await host.handle("note.on", payload: ["track": 1, "instrument": 0, "note": 62, "velocity": 100]); try render(host, buffer)
        _ = try await host.handle("note.off", payload: ["track": 1, "instrument": 0, "note": 62]); try render(host, buffer)
        try require(first.held(62) == 0 && first.ons(62) == 1 && second.ons(62) == 0 && second.strayOffs == 0 && first.strayOffs == 0,
            "After the reorder the member's notes no longer reach exactly its own plug-in.")
    }

    // MARK: Item 8 — AU transport position is song position
    @MainActor private static func transportPosition(_ folder: URL) async throws {
        let (host, _, buffer) = try offlineHost()
        defer { host.shutdown() }
        let project: [String: Any] = ["version": 1, "id": "transport-position", "name": "Transport position", "bpm": 120, "linesPerBeat": 4,
            "masterVolume": 1, "activePattern": 0, "loopSong": false, "metronome": false,
            "tracks": [["name": "Probe", "instrument": 0, "volume": 1, "pan": 0]], "instruments": [["type": "au", "volume": 1, "pan": 0]],
            "patterns": [["id": 0, "length": 64, "steps": [[Any]](), "automation": []]], "sequence": [0],
            "audioPlugins": [["track": 0, "kind": "instrument", "slot": 0, "component": component, "instanceID": "transport-probe"]],
            "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 1, "velocity": 1])
        _ = try await host.handle("note.off", payload: ["track": 0, "instrument": 0, "note": 1])
        try render(host, buffer, frames: 12_288) // Graph time advances while the song is stopped.
        guard let probe = NoteCountingRegistry.shared.units.last else { throw AudioSampleError.invalid("The transport probe was not instantiated.") }
        _ = try await host.handle("transport.play", payload: ["mode": "song", "fromStart": true])
        probe.clearCounts()
        try render(host, buffer, frames: 4096 * 3)
        let moving = probe.positions().filter { $0.moving }
        try require(moving.count >= 2, "The probe never saw a moving transport.")
        let sample = moving.map(\.sample), beat = moving.map(\.beat)
        try require(sample[0] >= 0 && sample[0] < 4096, "The first moving AU callback reported sample position \(sample[0]) instead of the song start.")
        try require(zip(sample, sample.dropFirst()).allSatisfy { $1 - $0 == 4096 }, "AU sample positions do not advance with the song: \(sample).")
        try require(zip(sample, beat).allSatisfy { abs($1 - $0 / 24_000) < 1e-9 }, "AU beat position disagrees with the song sample position: \(beat).")
        _ = try await host.handle("transport.stop", payload: [:])
    }
}

extension NativeAudioHostFixTests {
    // MARK: Item 5 — VST3 latency restart and kIsLive (original fixture only)
    @MainActor fileprivate static func vst3Restart(_ folder: URL) async throws {
        let bundle = URL(fileURLWithPath: ProcessInfo.processInfo.environment["SUPERTRACKER_VST3_FIXTURE"]
            ?? FileManager.default.currentDirectoryPath + "/test-output/Fixture.vst3").standardizedFileURL
        try require(FileManager.default.fileExists(atPath: bundle.path), "Build the original fixture first (tests/vst3_fixture_tests.sh): \(bundle.path)")
        let trace = folder.appendingPathComponent("vst3-restart-trace.log")
        try Data().write(to: trace)
        setenv("ST_VST3_FIXTURE_LATENCY_RESTART", "1", 1); setenv("ST_VST3_FIXTURE_TRACE", trace.path, 1)
        defer { unsetenv("ST_VST3_FIXTURE_LATENCY_RESTART"); unsetenv("ST_VST3_FIXTURE_TRACE") }
        var configuration = VST3Catalogue.Configuration()
        configuration.roots = [bundle.deletingLastPathComponent()]
        configuration.cacheURL = folder.appendingPathComponent("vst3-restart-catalogue.json")
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: configuration), midiInputIDs: [],
            midiInputSourceProvider: { [] }, midiOutputDestinationProvider: { [] })
        defer { host.shutdown() }
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
        var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: 16), count: 1)
        steps[0][0] = ["note": 60, "instrument": 0, "velocity": 100, "gate": 0.5]
        let project: [String: Any] = ["version": 1, "id": "vst3-restart", "name": "VST3 restart", "bpm": 120, "linesPerBeat": 4,
            "masterVolume": 1, "activePattern": 0, "loopSong": false, "metronome": false,
            "tracks": [["name": "Fixture", "instrument": 0, "volume": 1, "pan": 0]], "instruments": [["type": "au", "volume": 1, "pan": 0]],
            "patterns": [["id": 0, "length": 16, "steps": steps, "automation": []]], "sequence": [0],
            "audioPlugins": [["track": 0, "kind": "instrument", "slot": 0, "component": "vst3:0123456789ABCDEF0123456789ABCDEF", "instanceID": "restart-fixture"]],
            "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 72, "velocity": 100])
        try render(host, buffer)
        _ = try await host.handle("note.off", payload: ["track": 0, "instrument": 0, "note": 72])
        try render(host, buffer)
        // The adapter's controller timer and the host's 50 ms timer run on this run loop.
        var compensated = 0
        for _ in 0..<40 {
            try await Task.sleep(nanoseconds: 50_000_000)
            compensated = (host.status()["latencyCompensation"] as? [String: Any])?["frames"] as? Int ?? 0
            if compensated == 256 { break }
        }
        let lines = (try? String(contentsOf: trace, encoding: .utf8))?.split(separator: "\n").map(String.init) ?? []
        try require(lines.contains("component.inactive") && lines.lastIndex(of: "component.active")! > lines.lastIndex(of: "component.inactive")!,
            "The latency change never deactivated and reactivated the component.")
        try require(compensated == 256, "Latency compensation did not follow the restarted fixture's 256 frames (got \(compensated)).")
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        try render(host, buffer, frames: 8192)
        _ = try await host.handle("transport.stop", payload: [:])
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 74, "velocity": 100])
        try render(host, buffer)
        _ = try await host.handle("note.off", payload: ["track": 0, "instrument": 0, "note": 74])
        try render(host, buffer)
        let events = ((try? String(contentsOf: trace, encoding: .utf8)) ?? "").split(separator: "\n").filter { $0.hasPrefix("event.on.") }
        try require(events == ["event.on.sequenced", "event.on.live"], "Sequenced and live note-ons must differ in kIsLive: \(events).")
    }
}

extension NativeAudioHostFixTests {
    /// Restoring a candidate's saved state (preset, undo, project open) suspended the whole running
    /// graph although the candidate is not attached yet: one extra audible gap per plug-in, a full
    /// device restart with live inputs. Only the wiring that follows may pause.
    @MainActor fileprivate static func vst3StatefulLoad(_ folder: URL) async throws {
        let bundle = URL(fileURLWithPath: ProcessInfo.processInfo.environment["SUPERTRACKER_VST3_FIXTURE"]
            ?? FileManager.default.currentDirectoryPath + "/test-output/Fixture.vst3").standardizedFileURL
        try require(FileManager.default.fileExists(atPath: bundle.path), "Build the original fixture first (tests/vst3_fixture_tests.sh): \(bundle.path)")
        var configuration = VST3Catalogue.Configuration()
        configuration.roots = [bundle.deletingLastPathComponent()]
        configuration.cacheURL = folder.appendingPathComponent("vst3-stateful-load-catalogue.json")
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: configuration), midiInputIDs: [],
            midiInputSourceProvider: { [] }, midiOutputDestinationProvider: { [] })
        defer { host.shutdown() }
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
        let fixture = "vst3:0123456789ABCDEF0123456789ABCDEF"
        let project: [String: Any] = ["version": 1, "id": "vst3-stateful-load", "name": "VST3 stateful load", "bpm": 120, "linesPerBeat": 4,
            "masterVolume": 1, "activePattern": 0, "loopSong": false, "metronome": false,
            "tracks": [["name": "Fixture", "instrument": 0, "volume": 1, "pan": 0]], "instruments": [["type": "au", "volume": 1, "pan": 0]],
            "patterns": [["id": 0, "length": 16, "steps": [[Any]](), "automation": []]], "sequence": [0],
            "audioPlugins": [["track": 0, "kind": "instrument", "slot": 0, "component": fixture, "instanceID": "stateful-a"]],
            "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        let slot: [String: Any] = ["track": 0, "kind": "instrument", "slot": 0]
        guard let state = (try host.rack.state(slot) as? [String: Any])?["state"] as? String else {
            throw AudioSampleError.invalid("The fixture did not report its state.")
        }
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 60, "velocity": 100])
        try render(host, buffer)
        try require(graph.isRunning, "The audition did not start the graph.")
        let before = host.rack.enginePauses
        var request = slot; request["component"] = fixture; request["state"] = state; request["instanceID"] = "stateful-b"
        _ = try await host.rack.load(request)
        try render(host, buffer)
        let pauses = host.rack.enginePauses - before
        try require(graph.isRunning, "The graph did not resume after the plug-in was replaced.")
        try require(pauses == 1, "Replacing a VST3 with saved state paused the running graph \(pauses) times (the wiring needs one).")
    }
}

extension NativeAudioHostFixTests {
    /// A batch restore (project open, undo) coalesces routed rebuilds while it loads. Removing an
    /// insert inside it (a slot whose plug-in changed, or a user removal meanwhile) disconnected only
    /// the unit's outputs and deferred the rebuild, then detached the still-wired unit in the
    /// resumed graph: the strip went silent until the batch ended (and with live inputs
    /// AVAudioEngine raises on that disconnect).
    @MainActor fileprivate static func removalDuringBatch(_ folder: URL) async throws {
        let (host, _, buffer) = try offlineHost()
        defer { host.shutdown() }
        let rate = 48_000.0, sourceURL = folder.appendingPathComponent("batch-removal-source.wav")
        let wave = (0..<96_000).map { Float(sin(Double($0) * 2 * Double.pi * 220 / rate) * 0.4) }
        try AudioSamples.write(DecodedSample(path: sourceURL.path, sampleRate: rate, left: wave, right: wave, originalChannels: 2), to: sourceURL)
        // Apple's low-pass filter: an ordinary Audio Unit insert with its own graph node.
        let lowPass = "\(kAudioUnitType_Effect):\(kAudioUnitSubType_LowPassFilter):\(kAudioUnitManufacturer_Apple)"
        let project: [String: Any] = ["version": 1, "id": "batch-removal", "name": "Batch removal", "bpm": 120, "linesPerBeat": 4,
            "masterVolume": 1, "activePattern": 0, "loopSong": false, "metronome": false,
            "tracks": [["name": "Sine", "routingID": "sine", "instrument": 0, "volume": 1, "pan": 0]],
            "instruments": [["type": "sample", "samplePath": sourceURL.path, "rootNote": 60, "volume": 1, "mode": "oneshot",
                             "attack": 0, "decay": 0, "sustain": 1, "release": 0.001]],
            "patterns": [["id": 0, "length": 16, "steps": [[Any]](), "automation": []]], "sequence": [0],
            "audioPlugins": [["owner": "track:sine", "kind": "effect", "slot": 0, "component": lowPass, "instanceID": "batch-lowpass"]],
            "delay": ["mix": 0], "reverb": ["mix": 0],
            "routing": ["version": 1, "mixModel": "standard", "buses": [], "outputs": ["track:sine": "master"], "sends": [], "sidechains": []]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        try require(host.rack.snapshot(fresh: false).count == 1, "The low-pass insert did not load.")
        func peak() throws -> Float {
            try render(host, buffer)
            let data = buffer.floatChannelData!
            return (0..<Int(buffer.frameLength)).reduce(Float(0)) { max($0, abs(data[0][$1])) }
        }
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 60, "velocity": 127])
        _ = try peak()
        try require(try peak() > 0.01, "The track is silent through its insert before the removal.")
        guard let track = host.rack.ownerIndices["track:sine"] else { throw AudioSampleError.invalid("The track has no channel.") }
        var during: Float = 0
        try await host.rack.coalescingRoutingRebuilds {
            _ = try host.rack.removeTarget(AudioPluginRack.Target(track: track, kind: "effect", slot: 0))
            _ = try peak()
            during = try peak()
        }
        try require(during > 0.01, "Removing the insert during a batch restore silenced the track until the batch ended (peak \(during)).")
        try require(try peak() > 0.01, "The track is silent after the batch.")
        _ = try await host.handle("note.off", payload: ["track": 0, "instrument": 0, "note": 60])
    }
}

/// Construction/control-thread registry. Each unit writes only its own
/// preallocated counters on render; tests read them after synchronous offline pulls.
final class NoteCountingRegistry: @unchecked Sendable {
    static let shared = NoteCountingRegistry()
    private let lock = NSLock()
    private var values: [NoteCountingUnit] = []
    var units: [NoteCountingUnit] { lock.lock(); defer { lock.unlock() }; return values }
    func register(_ unit: NoteCountingUnit) { lock.lock(); values.append(unit); lock.unlock() }
    func removeAll() { lock.lock(); values.removeAll(); lock.unlock() }
}

final class NoteCountingUnit: AUAudioUnit {
    struct Position { var sample = 0.0, beat = 0.0, moving = false }
    private var inputs: AUAudioUnitBusArray!, outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree!
    private let held = UnsafeMutablePointer<Int32>.allocate(capacity: 128)
    private let onCounts = UnsafeMutablePointer<Int32>.allocate(capacity: 128)
    private let stray = UnsafeMutablePointer<Int32>.allocate(capacity: 1)
    private let trace = UnsafeMutablePointer<Position>.allocate(capacity: 256)
    private let traceCount = UnsafeMutablePointer<Int>.allocate(capacity: 1)
    private let scratch = UnsafeMutablePointer<Float>.allocate(capacity: 2 * 16_384)
    private(set) var tag: AUValue = 0

    override init(componentDescription: AudioComponentDescription, options: AudioComponentInstantiationOptions = []) throws {
        held.initialize(repeating: 0, count: 128); onCounts.initialize(repeating: 0, count: 128)
        stray.initialize(to: 0); trace.initialize(repeating: Position(), count: 256); traceCount.initialize(to: 0)
        scratch.initialize(repeating: 0, count: 2 * 16_384)
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let output = try AUAudioUnitBus(format: format); output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        let level = AUParameterTree.createParameter(withIdentifier: "tag", name: "Tag", address: 0, min: 0, max: 1, unit: .generic,
            unitName: nil, flags: [.flag_IsReadable, .flag_IsWritable], valueStrings: nil, dependentParameters: nil)
        parameters = AUParameterTree.createTree(withChildren: [level])
        parameters.implementorValueObserver = { [weak self] _, value in self?.tag = value }
        parameters.implementorValueProvider = { [weak self] _ in self?.tag ?? 0 }
        maximumFramesToRender = 16_384
        NoteCountingRegistry.shared.register(self)
    }
    deinit {
        held.deallocate(); onCounts.deallocate(); stray.deallocate(); trace.deallocate(); traceCount.deallocate(); scratch.deallocate()
    }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var parameterTree: AUParameterTree? { get { parameters } set { parameters = newValue } }
    override var channelCapabilities: [NSNumber]? { [0, 2] }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = max(16_384, newValue) }
    }
    func held(_ note: Int) -> Int { Int(held[note]) }
    func ons(_ note: Int) -> Int { Int(onCounts[note]) }
    var strayOffs: Int { Int(stray.pointee) }
    func positions() -> [Position] { Array(UnsafeBufferPointer(start: trace, count: traceCount.pointee)) }
    func clearCounts() {
        held.update(repeating: 0, count: 128); onCounts.update(repeating: 0, count: 128); stray.pointee = 0; traceCount.pointee = 0
    }
    override var internalRenderBlock: AUInternalRenderBlock {
        let held = held, onCounts = onCounts, stray = stray, trace = trace, traceCount = traceCount, scratch = scratch
        unowned(unsafe) let unit = self
        return { _, _, frames, _, output, events, _ in
            var event = events
            while let current = event {
                if current.pointee.head.eventType == .MIDI {
                    let message = current.pointee.MIDI, status = message.data.0 & 0xF0, note = Int(message.data.1 & 0x7F)
                    if status == 0x90 && message.data.2 > 0 { held[note] += 1; onCounts[note] += 1 }
                    else if status == 0x80 || status == 0x90 { if held[note] > 0 { held[note] -= 1 } else { stray.pointee += 1 } }
                    else if status == 0xB0 && (message.data.1 == 123 || message.data.1 == 120) { held.update(repeating: 0, count: 128) }
                }
                event = current.pointee.head.next.map { UnsafePointer($0) }
            }
            var flags = AUHostTransportStateFlags(), sample = 0.0, beat = 0.0
            if unit.transportStateBlock?(&flags, &sample, nil, nil) == true,
               unit.musicalContextBlock?(nil, nil, nil, &beat, nil, nil) == true, traceCount.pointee < 256 {
                trace[traceCount.pointee] = Position(sample: sample, beat: beat, moving: flags.contains(.moving)); traceCount.pointee += 1
            }
            let buffers = UnsafeMutableAudioBufferListPointer(output)
            guard frames <= 16_384 else { return kAudioUnitErr_TooManyFramesToProcess }
            for index in 0..<min(2, buffers.count) {
                if buffers[index].mData == nil { buffers[index].mData = UnsafeMutableRawPointer(scratch.advanced(by: index * 16_384)) }
                buffers[index].mDataByteSize = frames * 4; memset(buffers[index].mData, 0, Int(frames) * 4)
            }
            return noErr
        }
    }
}

#endif

// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Instrument insert chains through the production host (private offline graph, built-in effects only):
/// - every track that plays an instrument runs its own instance of the instrument's chain, ahead of the
///   track's own inserts, and consecutive built-ins (instrument + track) share one insert-chain node;
/// - parameter edits fan out to every instance; snapshots report the chain once, owned by the instrument;
/// - adding, reordering and removing instrument effects while playing updates chains in place (no pause);
/// - reassigning a track's instrument carries the chain; automation lanes and step-FX locks address it;
/// - kit files keep a pad instrument's chain.
///   --instrument-effects-self-test <output>
enum NativeInstrumentEffectsTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("instrument-effects", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = []
        func require(_ condition: Bool, _ message: @autoclosure () -> String) throws {
            if !condition { throw AudioSampleError.invalid("Instrument effects: " + message()) }
        }
        let rate = 48_000.0, format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        let samplePath = folder.appendingPathComponent("tone.wav")
        let tone: [Float] = (0..<24_000).map { Float(sin(Double($0) * 2 * Double.pi * 220 / rate) * 0.5) }
        try AudioSamples.write(DecodedSample(path: samplePath.path, sampleRate: rate, left: tone, right: tone, originalChannels: 2), to: samplePath)
        let rows = 32
        var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: rows), count: 3)
        for row in stride(from: 0, to: rows, by: 4) { for track in 0..<3 { steps[track][row] = ["note": 60, "velocity": 100, "gate": 1.0, "instrument": track == 2 ? 1 : 0] } }
        let instruments: [[String: Any]] = (0..<2).map { _ in ["type": "sample", "samplePath": samplePath.path, "rootNote": 60, "volume": 0.8, "mode": "oneshot",
                                                                   "attack": 0, "decay": 0, "sustain": 1, "release": 0.01] }
        let plugins: [[String: Any]] = [
            ["owner": "instrument:0", "kind": "effect", "slot": 0, "component": "stfx:filter", "format": "Builtin", "instanceID": "keys-filter"],
            ["owner": "instrument:0", "kind": "effect", "slot": 1, "component": "stfx:saturator", "format": "Builtin", "instanceID": "keys-drive"],
            ["owner": "track:a", "kind": "effect", "slot": 0, "component": "stfx:chorus", "format": "Builtin", "instanceID": "track-a-0"]]
        var project: [String: Any] = ["id": "instrument-effects", "version": 1, "name": "Instrument effects", "bpm": 120.0, "linesPerBeat": 4, "masterVolume": 0.7,
            "tracks": [["name": "A", "routingID": "a", "instrument": 0], ["name": "B", "routingID": "b", "instrument": 0], ["name": "C", "routingID": "c", "instrument": 1]],
            "instruments": instruments, "patterns": [["id": 0, "length": rows, "steps": steps, "automation": []]],
            "sequence": [0], "activePattern": 0, "loopSong": true, "audioPlugins": plugins, "audioClips": [],
            "routing": ["version": 1, "mixModel": "standard", "buses": [], "outputs": ["track:a": "master", "track:b": "master", "track:c": "master"],
                        "sends": [], "sidechains": []] as [String: Any]]

        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 512)
        let host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: .init(roots: [], cacheURL: folder.appendingPathComponent("vst3.json"))),
            midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration: .init(roots: [], cacheURL: folder.appendingPathComponent("vst2.json"))))
        defer { host.shutdown() }
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
        func render(_ blocks: Int) throws -> [Float] {
            var left: [Float] = []
            for _ in 0..<blocks {
                guard try host.renderOfflineQuantum(frames: 256, to: buffer) == .success else { throw AudioSampleError.invalid("Instrument effects: render failed") }
                left += UnsafeBufferPointer(start: buffer.floatChannelData![0], count: 256)
            }
            return left
        }
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        let rack: AudioPluginRack = host.rack
        func instanceIDs(_ owner: String) -> [String] {
            guard let channel = rack.ownerIndices[owner] else { return [] }
            return rack.instances.values.filter { $0.target.track == channel && $0.target.kind == "effect" }
                .sorted { MixerChannel.chainOrder($0.target.slot) < MixerChannel.chainOrder($1.target.slot) }.map(\.instanceID)
        }

        // One instance per track that plays the instrument, ahead of the track's own inserts, sharing its chain node.
        try require(instanceIDs("track:a") == ["keys-filter-on-a", "keys-drive-on-a", "track-a-0"], "track A chain \(instanceIDs("track:a"))")
        try require(instanceIDs("track:b") == ["keys-filter-on-b", "keys-drive-on-b"], "track B chain \(instanceIDs("track:b"))")
        try require(instanceIDs("track:c").isEmpty, "track C plays another instrument: \(instanceIDs("track:c"))")
        try require(rack.insertChainLayout() == [["keys-filter-on-a", "keys-drive-on-a", "track-a-0"], ["keys-filter-on-b", "keys-drive-on-b"]],
                    "insert-chain nodes \(rack.insertChainLayout())")
        let edges = try AudioRouting.parse(project: project).graph(pluginLayouts: rack.routingLayouts(AudioRouting.parse(project: project))).edges
        try require(edges.contains { $0.id == "insert:keys-filter-on-a" && $0.source == AudioRouting.nodeID(owner: "track:a", stage: .input) }
                    && edges.contains { $0.id == "insert:track-a-0" && $0.source == AudioRouting.pluginNodeID(instanceID: "keys-drive-on-a") },
                    "the routing graph runs the instrument chain first")
        try require(try render(80).contains { $0 != 0 }, "plays")
        checks.append("instances: tracks A and B each run the chain (C does not); A's chain precedes its insert in one chain node")

        // Edits fan out; snapshots report the chain once, owned by the instrument.
        _ = try await host.handle("plugin.parameter", payload: ["owner": "instrument:0", "kind": "effect", "slot": 0, "address": 1, "value": 0.25])
        let values = ["track:a", "track:b"].compactMap { owner -> Float? in
            guard let channel = rack.ownerIndices[owner] else { return nil }
            return rack.instances[.init(track: channel, kind: "effect", slot: MixerChannel.instrumentSlotBase)]?.node.auAudioUnit.parameterTree?.parameter(withAddress: 1)?.value
        }
        try require(values.count == 2 && values[0] == values[1], "parameter fan-out \(values)")
        _ = try await host.handle("plugin.parameter", payload: ["owner": "instrument:0", "kind": "effect", "slot": 1, "bypass": true])
        let bypassed = rack.instances.values.filter { $0.instanceID.hasPrefix("keys-drive-on-") }.map(\.node.auAudioUnit.shouldBypassEffect)
        try require(bypassed == [true, true], "bypass fan-out \(bypassed)")
        func snapshot() async throws -> [[String: Any]] {
            (try await host.handle("engine.snapshot", payload: [:]) as? [String: Any])?["audioPlugins"] as? [[String: Any]] ?? []
        }
        var saved = try await snapshot()
        let instrumentEntries = saved.filter { $0["owner"] as? String == "instrument:0" }
        try require(instrumentEntries.map { $0["instanceID"] as? String ?? "" } == ["keys-filter", "keys-drive"]
                    && instrumentEntries.allSatisfy { ($0["slot"] as? Int ?? 99) < 16 && $0["track"] == nil && $0["state"] is String },
                    "snapshot \(instrumentEntries.map { [$0["instanceID"] ?? "", $0["slot"] ?? ""] })")
        try require(!saved.contains { ($0["instanceID"] as? String ?? "").contains("-on-") || ($0["slot"] as? Int ?? 0) >= 16 }, "no per-track instances leak into the project")
        let state = try await host.handle("plugin.state", payload: ["owner": "instrument:0", "kind": "effect", "slot": 0]) as? [String: Any] ?? [:]
        try require(state["owner"] as? String == "instrument:0" && state["instanceID"] as? String == "keys-filter" && state["state"] is String, "plugin.state \(state.keys.sorted())")
        checks.append("edits: parameters and bypass reach both instances; snapshot and state report the instrument's effects")

        // Live edits while playing: no pause, no rebuild.
        let pauses = rack.enginePauses, rebuilds = rack.fullRoutingRebuilds
        func live(_ label: String) throws {
            try require(rack.enginePauses == pauses, "\(label) paused the graph")
            try require(rack.fullRoutingRebuilds == rebuilds, "\(label) rebuilt routing: \(rack.lastFullRebuildReason ?? "")")
            try require(try render(20).allSatisfy(\.isFinite), "\(label): output")
        }
        let added = try await host.handle("plugin.load", payload: ["owner": "instrument:0", "kind": "effect", "slot": 2, "component": "stfx:flanger"]) as? [String: Any] ?? [:]
        let addedID = added["instanceID"] as? String ?? ""
        try live("adding an instrument effect")
        try require(added["owner"] as? String == "instrument:0" && added["slot"] as? Int == 2 && !addedID.contains("-on-") && (added["parameters"] as? [Any])?.isEmpty == false, "load result \(added.keys.sorted())")
        try require(instanceIDs("track:b") == ["keys-filter-on-b", "keys-drive-on-b", "\(addedID)-on-b"], "added on B \(instanceIDs("track:b"))")
        let moved = try await host.handle("plugin.move", payload: ["owner": "instrument:0", "fromSlot": 0, "toSlot": 2]) as? [String: Any] ?? [:]
        try live("reordering instrument effects")
        try require(instanceIDs("track:a") == ["keys-drive-on-a", "\(addedID)-on-a", "keys-filter-on-a", "track-a-0"], "moved on A \(instanceIDs("track:a"))")
        try require((moved["plugins"] as? [[String: Any]])?.count == 3 && (moved["slotMap"] as? [[String: Any]])?.count == 3, "move result")
        saved = try await snapshot()
        try require(saved.filter { $0["owner"] as? String == "instrument:0" }.sorted { ($0["slot"] as? Int ?? 0) < ($1["slot"] as? Int ?? 0) }
                    .map { $0["instanceID"] as? String ?? "" } == ["keys-drive", addedID, "keys-filter"], "reordered project chain")
        _ = try await host.handle("plugin.remove", payload: ["owner": "instrument:0", "kind": "effect", "slot": 1])
        try live("removing an instrument effect")
        try require(instanceIDs("track:b") == ["keys-drive-on-b", "keys-filter-on-b"], "removed on B \(instanceIDs("track:b"))")
        checks.append("live: add, reorder and remove while playing update every instance in place (no pause, no rebuild)")

        // Reassigning a track's instrument carries the chain.
        project = try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? project
        var tracks = project["tracks"] as! [[String: Any]]; tracks[2]["instrument"] = 0; project["tracks"] = tracks
        _ = try await host.handle("engine.project", payload: ["project": project, "awaitPlugins": true])
        try require(instanceIDs("track:c") == ["keys-drive-on-c", "keys-filter-on-c"], "reassigned C \(instanceIDs("track:c"))")
        try require(try render(20).contains { $0 != 0 }, "plays after reassigning")
        checks.append("reassign: a track that takes the instrument runs the chain")

        // Automation lanes (owner instrument:0) reach every track's instance; step-FX locks resolve per track.
        var automated = project
        var patterns = automated["patterns"] as! [[String: Any]]
        patterns[0]["automation"] = [["owner": "instrument:0", "target": "plugin:effect:0:1", "interpolation": "linear", "points": [["row": 0, "value": 0.1], ["row": 16, "value": 0.9]]]]
        var grid = patterns[0]["steps"] as! [[Any]]
        grid[1][4] = ["note": 60, "velocity": 100, "instrument": 0, "fx1": ["type": "plugin", "instanceID": "keys-filter", "address": "1", "value": 0.3]]
        patterns[0]["steps"] = grid; automated["patterns"] = patterns
        let mapping = try AudioProject.routingAutomation(automated)
        let pluginEntries = Set(mapping.entries.filter { $0.kind == .plugin }.map(\.destination))
        let pluginPoints = mapping.points.filter { pluginEntries.contains($0.destination) }
        let pluginDestinations = Set(pluginPoints.map(\.destination))
        let owners = Set(mapping.entries.filter { pluginDestinations.contains($0.destination) }.map(\.owner))
        try require(mapping.unavailableLanes.isEmpty && owners == ["track:a", "track:b", "track:c"] && pluginPoints.count == 6, "automation destinations \(owners.sorted()) \(pluginPoints.count)")
        try require(mapping.entries.filter { pluginDestinations.contains($0.destination) }.allSatisfy { $0.slot == MixerChannel.instrumentSlotBase }, "lanes address the instrument chain slot")
        let locks = try AudioPluginStepFX(project: automated, definition: AudioRouting.parse(project: automated), hostEntries: mapping.entries)
        try require(locks.bindings.count == 1 && locks.bindings[0].track == 1 && locks.bindings[0].slot == MixerChannel.instrumentSlotBase + 2 && locks.unavailableFXs.isEmpty,
                    "step-FX lock \(locks.bindings.map { [$0.track, $0.slot] }) unavailable \(locks.unavailableFXs.map(\.reason))")
        _ = try await host.handle("engine.project", payload: ["project": automated, "awaitPlugins": true])
        try require(try render(40).allSatisfy(\.isFinite), "plays with lanes and locks")
        checks.append("automation: an instrument lane drives 3 instances; a step lock reaches its track's instance")

        // Kit files: a pad instrument's chain travels inside its Sound and returns on load.
        var kitSession = automated
        var kitTracks = kitSession["tracks"] as! [[String: Any]]
        for index in kitTracks.indices { kitTracks[index]["groupID"] = "kit"; kitTracks[index]["pad"] = index }
        kitSession["tracks"] = kitTracks
        kitSession["groups"] = [["id": "kit", "name": "Kit", "busID": "kitbus"]]
        var routing = kitSession["routing"] as! [String: Any]
        routing["buses"] = [["id": "kitbus", "name": "Kit", "type": "group", "groupID": "kit", "volume": 1, "pan": 0, "mute": false, "solo": false]]
        routing["outputs"] = ["track:a": "bus:kitbus", "track:b": "bus:kitbus", "track:c": "bus:kitbus", "bus:kitbus": "master"]
        kitSession["routing"] = routing
        let layout = try KitGroups.kitProject(fromGroup: kitSession, groupID: "kit")
        let library = try KitProject.libraryProject(layout.project, kitID: "kit")
        let reopened = try KitGroups.groupProject(fromKitProject: library.project)
        let chains = (reopened["audioPlugins"] as? [[String: Any]] ?? []).filter { InstrumentEffects.instrumentIndex($0["owner"]) != nil }
        let reopenedInstruments = Set(chains.compactMap { InstrumentEffects.instrumentIndex($0["owner"]) })
        try require(chains.count == 2 * 3 && reopenedInstruments.count == 3 && chains.allSatisfy { $0["state"] is String && ($0["slot"] as? Int ?? 99) < 16 },
                    "kit round trip \(chains.map { [$0["owner"] ?? "", $0["component"] ?? ""] })")
        checks.append("kits: Save kit / Load kit keep each pad instrument's chain (\(chains.count) effects on \(reopenedInstruments.count) instruments)")
        let report: [String: Any] = ["checks": checks]
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) { try? data.write(to: folder.appendingPathComponent("report.json")) }
        return checks
    }
}
#endif

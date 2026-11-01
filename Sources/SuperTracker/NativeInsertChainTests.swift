// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import Darwin

/// Built-in insert chains, direct track sources, folded solo gates and sparse send routes
/// through the production host (private offline graphs, built-in effects only):
/// - a chained graph sounds like the per-node graph (makeInsertChain nil, uncollapsed
///   inputs, separate solo gates) with automation lanes, a sidechained and a latent insert,
///   bypass and parameter changes, solo and meters;
/// - plug-in state, automation offsets and latency are per insert as before;
/// - loading, replacing, removing and reordering built-in inserts while playing updates the
///   chain nodes in place (no pause, no rebuild); a change of chain shape still rebuilds;
/// - delay/reverb send routes follow send usage (a return's first send rebuilds, later ones
///   join live).
///   --insert-chain-self-test <output>
enum NativeInsertChainTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("insert-chain", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = []
        func require(_ condition: Bool, _ message: @autoclosure () -> String) throws {
            if !condition { throw AudioSampleError.invalid("Insert chains: " + message()) }
        }
        let ids = BuiltinEffectsAudioUnit.catalogue().compactMap { $0["id"] as? String }
        func fx(_ name: String) throws -> String {
            guard let id = ids.first(where: { $0 == "stfx:" + name }) else { throw AudioSampleError.invalid("Insert chains: missing built-in effect \(name)") }
            return id
        }
        let rate = 48_000.0
        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        let hit: [Float] = (0..<9600).map { (frame: Int) -> Float in
            let t = Double(frame)
            let body: Double = sin(t * 2 * Double.pi * 90 / rate) * exp(-t / 2400) * 0.6
            let click: Double = sin(t * 0.9) * exp(-t / 300) * 0.2
            return Float(body + click)
        }
        let samplePath = folder.appendingPathComponent("hit.wav")
        try AudioSamples.write(DecodedSample(path: samplePath.path, sampleRate: rate, left: hit, right: hit, originalChannels: 2), to: samplePath)

        // Track a: a chain (filter, saturator), a sidechained compressor (its own node), a chain
        // with a latent limiter; track b: the sidechain source with a bypassed reverb; track c
        // straight to master; the bus has two chained inserts and a lane on its second.
        let rows = 64
        var plugins: [[String: Any]] = []
        func insert(_ owner: String, _ slot: Int, _ name: String, bypass: Bool = false) throws {
            plugins.append(["owner": owner, "kind": "effect", "slot": slot, "component": try fx(name), "format": "Builtin",
                            "instanceID": owner.replacingOccurrences(of: ":", with: "-") + "-\(slot)", "bypass": bypass])
        }
        try insert("track:a", 0, "filter"); try insert("track:a", 1, "saturator"); try insert("track:a", 2, "compressor")
        try insert("track:a", 3, "limiter"); try insert("track:a", 4, "chorus"); try insert("track:a", 5, "transient")
        try insert("track:b", 0, "filter"); try insert("track:b", 1, "reverb", bypass: true); try insert("track:b", 2, "phaser")
        try insert("bus:kb", 0, "compressor"); try insert("bus:kb", 1, "maximizer")
        var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: rows), count: 3)
        for row in 0..<rows {
            if row % 4 == 0 { steps[0][row] = ["note": 60, "velocity": 110, "gate": 1.0] }
            if row % 8 == 6 { steps[1][row] = ["note": 67, "velocity": 100, "gate": 1.0] }
            if row % 16 == 2 { steps[2][row] = ["note": 55, "velocity": 90, "gate": 2.0] }
        }
        let instruments: [[String: Any]] = (0..<3).map { _ in ["type": "sample", "samplePath": samplePath.path, "rootNote": 60, "volume": 0.8,
            "mode": "oneshot", "attack": 0, "decay": 0, "sustain": 1, "release": 0.01] }
        var project: [String: Any] = ["id": "insert-chains", "version": 1, "name": "Insert chains", "bpm": 120.0, "linesPerBeat": 4, "masterVolume": 0.7,
            "tracks": [["name": "A", "routingID": "a", "instrument": 0, "volume": 0.8, "pan": -0.2],
                       ["name": "B", "routingID": "b", "instrument": 1, "volume": 0.7, "pan": 0.3],
                       ["name": "C", "routingID": "c", "instrument": 2, "volume": 0.6, "pan": 0]],
            "instruments": instruments,
            "patterns": [["id": 0, "length": rows, "steps": steps, "automation": [
                ["owner": "track:a", "target": "plugin:effect:0:0", "interpolation": "linear", "points": [["row": 0, "value": 0.2], ["row": 32, "value": 0.9], ["row": 63, "value": 0.4]]],
                ["owner": "bus:kb", "target": "plugin:effect:1:0", "interpolation": "step", "points": [["row": 0, "value": 0.3], ["row": 16, "value": 0.7]]]]]],
            "sequence": [0], "activePattern": 0, "loopSong": true, "audioPlugins": plugins, "audioClips": [],
            "delay": ["mix": 0.4], "reverb": ["mix": 0.3],
            "routing": ["version": 1, "mixModel": "standard",
                        "buses": [["id": "kb", "name": "Bus", "type": "group", "volume": 0.9, "pan": 0, "mute": false, "solo": false]],
                        "outputs": ["track:a": "bus:kb", "track:b": "bus:kb", "track:c": "master", "bus:kb": "master"], "sends": [],
                        "sidechains": [["id": "sc", "source": "track:b", "tap": "postFader", "gain": 1.0, "enabled": true,
                                        "destination": ["instanceID": "track-a-2", "inputBus": 1, "layoutSignature": ["name": "Sidechain", "channels": 2]]]]] as [String: Any]]

        @MainActor final class Session {
            let graph = AVAudioEngine()
            let host: AudioHost
            let buffer: AVAudioPCMBuffer
            init(folder: URL, format: AVAudioFormat, name: String, nodes: Bool) throws {
                try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 512)
                host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: .init(roots: [], cacheURL: folder.appendingPathComponent(name + "-vst3.json"))),
                    midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration: .init(roots: [], cacheURL: folder.appendingPathComponent(name + "-vst2.json"))))
                if nodes {
                    // The per-node reference: every insert its own node, one-input input mixers, separate solo gates.
                    host.rack.routingGraph.makeInsertChain = nil
                    host.rack.routingGraph.collapsesTrackInputs = false
                    host.rack.routingGraph.foldsSoloGates = false
                }
                buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
            }
            func render(_ blocks: Int, frames: AVAudioFrameCount = 256) throws -> [[Float]] {
                var result: [[Float]] = [[], []]
                for _ in 0..<blocks {
                    guard try host.renderOfflineQuantum(frames: frames, to: buffer) == .success else { throw AudioSampleError.invalid("Insert chains: render failed") }
                    for channel in 0..<2 { result[channel] += UnsafeBufferPointer(start: buffer.floatChannelData![channel], count: Int(frames)) }
                }
                return result
            }
            var routing: [String: Any] { host.status()["routing"] as? [String: Any] ?? [:] }
        }
        func compare(_ a: [[Float]], _ b: [[Float]]) -> (difference: Float, energy: Float) {
            var worst: Float = 0, energy: Float = 0
            for channel in 0..<2 { for (x, y) in zip(a[channel], b[channel]) { worst = max(worst, abs(x - y)); energy += y * y } }
            for channel in 0..<2 where a[channel].contains(where: { !$0.isFinite }) || b[channel].contains(where: { !$0.isFinite }) { worst = .infinity }
            return (worst, energy)
        }

        let reference = try Session(folder: folder, format: format, name: "nodes", nodes: true)
        let chained = try Session(folder: folder, format: format, name: "chains", nodes: false)
        defer { reference.host.shutdown(); chained.host.shutdown() }
        for session in [reference, chained] {
            _ = try await session.host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
            await session.host.awaitProjectAssets()
            _ = try await session.host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        }
        // Structure: chains where expected, the sidechained compressor on its own node.
        let layout = chained.host.rack.insertChainLayout()
        try require(layout == [["bus-kb-0", "bus-kb-1"], ["track-a-0", "track-a-1"], ["track-a-3", "track-a-4", "track-a-5"], ["track-b-0", "track-b-1", "track-b-2"]],
                    "chain layout \(layout)")
        try require(reference.host.rack.insertChainLayout().isEmpty, "the reference renders every insert as its own node")
        let rendered = (NativeRenderPerformanceTests.renderedNodes(reference.graph).count, NativeRenderPerformanceTests.renderedNodes(chained.graph).count)
        try require(rendered.1 < rendered.0, "chained graph renders fewer nodes (\(rendered.1) vs \(rendered.0))")
        checks.append("chains: \(layout.count) chain nodes for 10 of 11 inserts; rendered nodes \(rendered.0) -> \(rendered.1)")
        // Per-insert timing: latency compensation and automation offsets are unchanged.
        let arrivals = (reference.routing["pluginInputArrival"] as? [String: Int] ?? [:], chained.routing["pluginInputArrival"] as? [String: Int] ?? [:])
        try require(!arrivals.0.isEmpty && arrivals.0 == arrivals.1, "plug-in input arrivals \(arrivals.1) vs \(arrivals.0)")
        try require((chained.routing["latencyFrames"] as? Int ?? -1) == (reference.routing["latencyFrames"] as? Int ?? -2) && (chained.routing["latencyFrames"] as? Int ?? 0) > 0,
                    "graph latency \(chained.routing["latencyFrames"] ?? "nil") vs \(reference.routing["latencyFrames"] ?? "nil")")
        checks.append("latency: \(chained.routing["latencyFrames"] ?? 0) frames, per-insert arrivals equal")

        // Output equivalence through automation lanes, a bypass toggle and a parameter change.
        var worst: Float = 0, energy: Float = 0
        for phase in 0..<4 {
            switch phase {
            case 1:
                for session in [reference, chained] {
                    _ = try await session.host.handle("plugin.parameter", payload: ["owner": "track:b", "kind": "effect", "slot": 1, "bypass": false])
                    _ = try await session.host.handle("plugin.parameter", payload: ["owner": "track:a", "kind": "effect", "slot": 4, "bypass": true])
                }
            case 2:
                for session in [reference, chained] {
                    _ = try await session.host.handle("plugin.parameter", payload: ["owner": "track:a", "kind": "effect", "slot": 1, "address": 0, "value": 0.85])
                }
            case 3:
                // Solo: folded gates mute like gate nodes; a soloed-out strip still meters.
                var soloed = project, tracks = project["tracks"] as! [[String: Any]]
                tracks[2]["solo"] = true; soloed["tracks"] = tracks
                for session in [reference, chained] { _ = try await session.host.handle("engine.project", payload: ["project": soloed, "awaitPlugins": true]) }
            default: break
            }
            let a = try reference.render(150), b = try chained.render(150)
            let result = compare(a, b)
            worst = max(worst, result.difference); energy += result.energy
        }
        try require(worst <= 1e-4 && energy > 1, "chained output differs from per-node output by \(worst) (energy \(energy))")
        checks.append(String(format: "equivalence: 600 blocks with lanes, sidechain, latency, bypass, parameters and solo; max difference %.2e", worst))
        func peaks(_ session: Session, _ owner: String) throws -> Float {
            guard let mix = session.host.rack.routingGraph.current?.strips[owner]?.mix.automation else { throw AudioSampleError.invalid("Insert chains: no strip \(owner)") }
            _ = mix.takePeaks(); _ = try session.render(100)
            let value = mix.takePeaks(); return max(value.left, value.right)
        }
        let meters = (try peaks(reference, "track:a"), try peaks(chained, "track:a"))
        try require(meters.1 > 0 && abs(meters.0 - meters.1) <= 1e-4 * max(1, meters.0), "a soloed-out strip meters before its gate: \(meters.1) vs \(meters.0)")
        checks.append("solo: folded gates mute like gate nodes; soloed-out strip meters \(meters.1)")
        for session in [reference, chained] { _ = try await session.host.handle("engine.project", payload: ["project": project, "awaitPlugins": true]) }

        // Saved state of chained inserts is per insert, exactly as before.
        func states(_ session: Session) async throws -> [String: String] {
            let snapshot = try await session.host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            return Dictionary(uniqueKeysWithValues: (snapshot["audioPlugins"] as? [[String: Any]] ?? []).compactMap { item in
                (item["instanceID"] as? String).map { ($0, item["state"] as? String ?? "") } })
        }
        let savedStates = (try await states(reference), try await states(chained))
        let differing = savedStates.0.keys.filter { key in !(savedStates.1[key].map { chained.host.rack.equivalentState($0, savedStates.0[key]!) } ?? false) }
        try require(savedStates.0.count == plugins.count && savedStates.1.count == plugins.count && differing.isEmpty, "chained insert states differ from per-node states: \(differing.sorted())")
        checks.append("state: \(savedStates.1.count) insert states identical")

        // In-place edits while playing: no pause, no rebuild, the chain members follow.
        let rack: AudioPluginRack = chained.host.rack
        let pauses = rack.enginePauses, rebuilds = rack.fullRoutingRebuilds, updates = rack.insertChainUpdates
        func edited(_ label: String, layout expected: [[String]]) throws {
            try require(rack.enginePauses == pauses, "\(label) paused the graph")
            try require(rack.fullRoutingRebuilds == rebuilds, "\(label) rebuilt routing: \(rack.lastFullRebuildReason ?? "")")
            try require(rack.insertChainLayout() == expected, "\(label): chain layout \(rack.insertChainLayout())")
            let block = try chained.render(40)
            try require(block.allSatisfy { $0.allSatisfy(\.isFinite) } && block[0].contains { $0 != 0 }, "\(label): output after the edit")
        }
        _ = try await chained.host.handle("plugin.load", payload: ["owner": "track:a", "kind": "effect", "slot": 6, "component": try fx("flanger"), "instanceID": "track-a-6"])
        try edited("adding a built-in insert", layout: [["bus-kb-0", "bus-kb-1"], ["track-a-0", "track-a-1"], ["track-a-3", "track-a-4", "track-a-5", "track-a-6"], ["track-b-0", "track-b-1", "track-b-2"]])
        _ = try await chained.host.handle("plugin.move", payload: ["owner": "track:a", "fromSlot": 3, "toSlot": 6])
        try edited("reordering built-in inserts", layout: [["bus-kb-0", "bus-kb-1"], ["track-a-0", "track-a-1"], ["track-a-4", "track-a-5", "track-a-6", "track-a-3"], ["track-b-0", "track-b-1", "track-b-2"]])
        _ = try await chained.host.handle("plugin.remove", payload: ["owner": "track:a", "kind": "effect", "slot": 0])
        try edited("removing a built-in insert", layout: [["bus-kb-0", "bus-kb-1"], ["track-a-1"], ["track-a-4", "track-a-5", "track-a-6", "track-a-3"], ["track-b-0", "track-b-1", "track-b-2"]])
        _ = try await chained.host.handle("plugin.load", payload: ["owner": "track:b", "kind": "effect", "slot": 2, "component": try fx("flanger"), "instanceID": "track-b-2x"])
        try edited("replacing a built-in insert", layout: [["bus-kb-0", "bus-kb-1"], ["track-a-1"], ["track-a-4", "track-a-5", "track-a-6", "track-a-3"], ["track-b-0", "track-b-1", "track-b-2x"]])
        try require(rack.insertChainUpdates >= updates + 4, "in-place updates \(rack.insertChainUpdates - updates)")
        // Per-insert automation offsets after the edits equal a freshly built graph's.
        let edits = try await chained.host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
        var editedProject = project
        editedProject["audioPlugins"] = edits["audioPlugins"]; editedProject["patterns"] = edits["patterns"] ?? project["patterns"]
        let fresh = try Session(folder: folder, format: format, name: "fresh", nodes: false)
        defer { fresh.host.shutdown() }
        _ = try await fresh.host.handle("engine.project", payload: ["project": editedProject, "replace": true, "awaitPlugins": true])
        await fresh.host.awaitProjectAssets()
        try require(fresh.host.rack.insertChainLayout() == rack.insertChainLayout(), "fresh layout \(fresh.host.rack.insertChainLayout())")
        try require((fresh.routing["pluginInputArrival"] as? [String: Int]) == (chained.routing["pluginInputArrival"] as? [String: Int]), "arrivals after in-place edits")
        checks.append("in place: add, reorder, remove and replace while playing with no pause or rebuild; offsets match a fresh graph")
        // Removing a chain's last insert changes the graph's shape: a (paused) rebuild, which still works.
        _ = try await chained.host.handle("plugin.remove", payload: ["owner": "track:a", "kind": "effect", "slot": 1])
        try require(rack.fullRoutingRebuilds == rebuilds + 1, "removing a chain's last insert rebuilds (\(rack.fullRoutingRebuilds - rebuilds))")
        let after = try chained.render(40)
        try require(after.allSatisfy { $0.allSatisfy(\.isFinite) } && after[0].contains { $0 != 0 }, "output after the rebuild")
        checks.append("shape change: removing a chain's last insert rebuilds and plays")

        // Send routes follow send usage: a return's first send rebuilds, a second sender joins live.
        let sendRebuilds = rack.fullRoutingRebuilds, sendPauses = rack.enginePauses
        func send(_ track: Int, _ key: String, _ value: Double) async throws {
            let snapshot = try await chained.host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            var next = snapshot; var tracks = next["tracks"] as! [[String: Any]]
            tracks[track][key] = value; next["tracks"] = tracks
            _ = try await chained.host.handle("engine.project", payload: ["project": next, "awaitPlugins": true])
        }
        try require((chained.routing["owners"] as? [String])?.isEmpty == false && (rack.routingGraph.current?.builtinSendEndpoints.isEmpty ?? false), "no send routes without sends")
        try await send(0, "delaySend", 0.5)
        try require(rack.fullRoutingRebuilds == sendRebuilds + 1 && rack.enginePauses > sendPauses, "the delay return's first send rebuilds")
        let echoed = try chained.render(100)
        let pausesAfterFirst = rack.enginePauses
        try await send(2, "delaySend", 0.4)
        try require(rack.fullRoutingRebuilds == sendRebuilds + 1 && rack.enginePauses == pausesAfterFirst, "a second send joins live: \(rack.lastFullRebuildReason ?? "")")
        try require(rack.routingGraph.current?.builtinSendEndpoints.count == 2, "two send routes")
        let more = try chained.render(100)
        try require(echoed[0].contains { $0 != 0 } && more.allSatisfy { $0.allSatisfy(\.isFinite) }, "sends sound")
        try await send(2, "delaySend", 0)
        try require(rack.fullRoutingRebuilds == sendRebuilds + 1 && rack.routingGraph.current?.builtinSendEndpoints.count == 2, "an unneeded send route stays until a rebuild")
        checks.append("sends: routed only when used; first send rebuilds, later sends join live")
        let report: [String: Any] = ["checks": checks, "renderedNodes": ["perNode": rendered.0, "chained": rendered.1], "maxDifference": Double(worst)]
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) { try? data.write(to: folder.appendingPathComponent("report.json")) }
        return checks
    }
}
#endif

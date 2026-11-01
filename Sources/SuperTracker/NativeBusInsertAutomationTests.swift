// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Pattern automation of bus and master inserts reaches the insert in the lane's slot, not a neighbour with the same
/// parameter address: a group bus with five built-in inserts (one insert-chain node), a bus mixing an Apple AU with
/// built-ins, and master inserts, each with a lane on a slot > 0; then the same while playing, after a return bus and
/// its insert are added live, and while a bus ahead of an automated one gets an insert (destination renumbering).
/// Private offline graphs, no audio device.
///   --bus-insert-automation-self-test <output>
enum NativeBusInsertAutomationTests {
    static let appleDelay = "1635083896:1684368505:1634758764"

    @MainActor static func run(output: URL) async throws -> [String] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        var checks: [String] = []
        checks += try await loaded(output: output)
        checks += try await live(output: output)
        checks += try await renumbered(output: output)
        return checks
    }

    static func require(_ condition: Bool, _ message: @autoclosure () -> String) throws {
        if !condition { throw AudioSampleError.invalid("Bus insert automation: " + message()) }
    }
    static let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!

    @MainActor static func makeHost(_ output: URL, _ name: String) throws -> AudioHost {
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 512)
        return try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: .init(roots: [], cacheURL: output.appendingPathComponent(name + "-vst3.json"))),
            midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration: .init(roots: [], cacheURL: output.appendingPathComponent(name + "-vst2.json"))))
    }
    static func insert(_ owner: String, _ slot: Int, _ component: String, format: String = "Builtin") -> [String: Any] {
        ["owner": owner, "kind": "effect", "slot": slot, "component": component, "format": format,
         "instanceID": owner.replacingOccurrences(of: ":", with: "-") + "-\(slot)"]
    }
    static func lane(_ owner: String, _ target: String, _ value: Double) -> [String: Any] {
        ["owner": owner, "target": target, "interpolation": "step", "points": [["row": 0, "value": value]]]
    }
    static func project(plugins: [[String: Any]], buses: [[String: Any]], outputs: [String: String], sends: [[String: Any]] = [],
                        lanes: [[String: Any]]) -> [String: Any] {
        let rows = 16
        let steps: [[Any]] = (0..<2).map { _ in (0..<rows).map { $0 % 4 == 0 ? ["note": 60, "velocity": 100, "gate": 1.0] as Any : NSNull() } }
        return ["id": "bus-insert-automation", "version": 1, "name": "Bus insert automation", "bpm": 120.0, "linesPerBeat": 4,
            "tracks": [["name": "Kick", "routingID": "kick", "instrument": 0, "volume": 0.8, "pan": 0],
                       ["name": "Keys", "routingID": "keys", "instrument": 0, "volume": 0.8, "pan": 0]],
            "instruments": [["type": "synth", "name": "Tone"]],
            "patterns": [["id": 0, "name": "A", "length": rows, "steps": steps, "automation": lanes]],
            "sequence": [0], "activePattern": 0, "loopSong": true, "audioPlugins": plugins, "audioClips": [],
            "routing": ["version": 1, "mixModel": "standard", "buses": buses, "outputs": outputs, "sends": sends, "sidechains": []] as [String: Any]]
    }
    static func bus(_ id: String, _ name: String, _ type: String = "group") -> [String: Any] {
        ["id": id, "name": name, "type": type, "volume": 1, "pan": 0, "mute": false, "solo": false]
    }
    @MainActor static func value(_ host: AudioHost, _ owner: String, _ slot: Int, _ address: String) async throws -> Double {
        let list = try await host.handle("plugin.parameters", payload: ["owner": owner, "kind": "effect", "slot": slot]) as? [[String: Any]] ?? []
        guard let item = list.first(where: { $0["address"] as? String == address }), let number = item["normalized"] as? NSNumber else {
            throw AudioSampleError.invalid("Bus insert automation: \(owner) slot \(slot) has no parameter \(address)")
        }
        return number.doubleValue
    }
    @MainActor static func render(_ host: AudioHost, _ quanta: Int) throws {
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
        for _ in 0..<quanta { try require(try host.renderOfflineQuantum(frames: 512, to: buffer) == .success, "offline render failed") }
    }
    /// Lanes on (owner, slot, address 9, value); each neighbour slot also has an address 9 at 0.5 that must not move.
    @MainActor static func verify(_ host: AudioHost, _ cases: [(String, Int, Double, Int)], _ label: String) async throws -> [String] {
        var checks: [String] = []
        for (owner, slot, expected, neighbour) in cases {
            let automated = try await value(host, owner, slot, "9"), other = try await value(host, owner, neighbour, "9")
            try require(abs(automated - expected) < 1e-3, "\(label): \(owner) plugin:effect:\(slot):9 should be \(expected), read \(automated) (slot \(neighbour) reads \(other))")
            try require(abs(other - 0.5) < 1e-3, "\(label): \(owner)'s lane on slot \(slot) moved slot \(neighbour) address 9 to \(other)")
            checks.append("\(label): \(owner)'s lane on slot \(slot) moves that insert (\(String(format: "%.2f", automated))), slot \(neighbour) stays 0.50")
        }
        return checks
    }

    /// Phaser address 9 is Mix, Saturator address 9 is Tube Treble: both default to 0.5 normalized.
    @MainActor static func loaded(output: URL) async throws -> [String] {
        let host = try makeHost(output, "loaded")
        defer { host.shutdown() }
        var plugins = ["filter", "phaser", "saturator", "reverb", "limiter"].enumerated().map { insert("bus:kit", $0.offset, "stfx:" + $0.element) }
        plugins += [insert("bus:mixed", 0, appleDelay, format: "AudioUnit"), insert("bus:mixed", 1, "stfx:phaser"), insert("bus:mixed", 2, "stfx:saturator"),
                    insert("master", 0, "stfx:phaser"), insert("master", 1, "stfx:saturator")]
        let data = project(plugins: plugins, buses: [bus("kit", "Kit"), bus("mixed", "Mixed")],
            outputs: ["track:kick": "bus:kit", "track:keys": "bus:mixed", "bus:kit": "master", "bus:mixed": "master"],
            lanes: [lane("bus:kit", "plugin:effect:2:9", 0.9), lane("bus:mixed", "plugin:effect:2:9", 0.8), lane("master", "plugin:effect:1:9", 0.85)])
        _ = try await host.handle("engine.project", payload: ["project": data, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        for plugin in plugins {
            let id = plugin["instanceID"] as! String
            try require(host.rack.instances.values.contains { $0.instanceID == id }, "\(id) did not load")
        }
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        try render(host, 24)
        return try await verify(host, [("bus:kit", 2, 0.9, 1), ("bus:mixed", 2, 0.8, 1), ("master", 1, 0.85, 0)], "loaded")
    }

    /// While playing: a return bus is added between two group buses (in destination order), an AU loads on it and is
    /// replaced by a built-in, as the mixer and the AI do; a lane written afterwards must still reach its own insert.
    @MainActor static func live(output: URL) async throws -> [String] {
        let host = try makeHost(output, "live")
        defer { host.shutdown() }
        let kit = ["filter", "phaser", "saturator", "reverb", "limiter"].enumerated().map { insert("bus:kit", $0.offset, "stfx:" + $0.element) }
        var buses = [bus("kit", "Kit"), bus("hats", "Hats")]
        var outputs = ["track:kick": "bus:kit", "track:keys": "bus:hats", "bus:kit": "master", "bus:hats": "bus:kit"]
        var data = project(plugins: kit + [insert("bus:hats", 0, "stfx:filter")], buses: buses, outputs: outputs, lanes: [])
        _ = try await host.handle("engine.project", payload: ["project": data, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        try host.start()
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        try render(host, 4)
        var checks: [String] = []
        func publish(_ next: [String: Any]) async throws {
            data = next
            _ = try await host.handle("engine.project", payload: ["project": data])
            try render(host, 2)
        }
        // "bus:space" sorts between "bus:hats" and "bus:kit".
        buses.append(bus("space", "Space", "return")); outputs["bus:space"] = "master"
        data["routing"] = (data["routing"] as! [String: Any]).merging(["buses": buses, "outputs": outputs,
            "sends": [["id": "hs", "source": "bus:hats", "destination": "bus:space", "gain": 0.5, "enabled": true, "tap": "postFader"]]]) { $1 }
        try await publish(data)
        for component in [appleDelay, "stfx:reverb"] {
            _ = try await host.handle("plugin.load", payload: ["owner": "bus:space", "kind": "effect", "slot": 0, "component": component])
            try render(host, 2)
            // The UI adopts the host's descriptors and publishes the project.
            data["audioPlugins"] = try await host.handle("plugin.state", payload: [:]) as? [[String: Any]] ?? []
            try await publish(data)
        }
        var patterns = data["patterns"] as! [[String: Any]]
        patterns[0]["automation"] = [lane("bus:kit", "plugin:effect:2:9", 0.9)]
        data["patterns"] = patterns
        try await publish(data)
        try render(host, 24)
        checks += try await verify(host, [("bus:kit", 2, 0.9, 1)], "live")
        return checks
    }

    /// While playing, a group bus that sorts before an automated bus gets an insert: every destination behind it moves
    /// up one. The routing table is republished before the engine project is (see AudioPluginRack.configureRouting),
    /// and audio renders in between. The automated bus's lane, still numbered for the old table, must not reach the new
    /// insert at the index it used to own (live: a fresh Neutron on a new "Basses" bus panned hard right by a Hats lane).
    @MainActor static func renumbered(output: URL) async throws -> [String] {
        let host = try makeHost(output, "renumbered")
        defer { host.shutdown() }
        var buses = [bus("kit", "Kit"), bus("hats", "Hats")]
        var outputs = ["track:kick": "bus:kit", "track:keys": "bus:hats", "bus:kit": "master", "bus:hats": "master"]
        // A linear lane is re-sent every 32 frames, so any render between the two publications delivers it.
        let hatsLane: [String: Any] = ["owner": "bus:hats", "target": "plugin:effect:0:9", "interpolation": "linear",
                                       "points": [["row": 0, "value": 0.9], ["row": 15, "value": 0.9]]]
        var data = project(plugins: [insert("bus:kit", 0, "stfx:phaser"), insert("bus:hats", 0, "stfx:phaser")],
                           buses: buses, outputs: outputs, lanes: [hatsLane])
        _ = try await host.handle("engine.project", payload: ["project": data, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        try host.start()
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        try render(host, 4)
        try require(abs(try await value(host, "bus:hats", 0, "9") - 0.9) < 1e-3, "renumbered: the Hats lane did not reach its insert before the edit")
        // "bus:basses" sorts before "bus:hats": create it (no inserts yet), as createBus does.
        buses.append(bus("basses", "Basses")); outputs["track:keys"] = "bus:basses"; outputs["bus:basses"] = "master"
        data["routing"] = (data["routing"] as! [String: Any]).merging(["buses": buses, "outputs": outputs]) { $1 }
        _ = try await host.handle("engine.project", payload: ["project": data])
        try render(host, 2)
        // loadPlugin, as the "plugin.load" command runs it: the rack loads the insert, the host adds its descriptor
        // and republishes (refreshRoutingProject), with the audio thread rendering between the routing table and the
        // engine project (offline rendering otherwise only runs between whole publications).
        let descriptor = try await host.rack.load(["owner": "bus:basses", "kind": "effect", "slot": 0, "component": "stfx:phaser"]) as? [String: Any] ?? [:]
        try require(descriptor["instanceID"] != nil, "renumbered: the new insert returned no descriptor")
        host.project["audioPlugins"] = host.storedPluginDescriptions(host.project) + [descriptor]
        var rendered = 0
        try host.rack.configureRouting(host.project, commit: {
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
            for _ in 0..<4 { if try host.renderOfflineQuantum(frames: 512, to: buffer) == .success { rendered += 1 } }
            try host.applyEngineProject(host.project)
        })
        try require(rendered == 4, "renumbered: the routing change paused rendering, so it did not exercise the live window (\(rendered) quanta)")
        try render(host, 8)
        let fresh = try await value(host, "bus:basses", 0, "9"), hats = try await value(host, "bus:hats", 0, "9")
        try require(abs(fresh - 0.5) < 1e-3, "renumbered: the Hats lane (old numbering) moved the new Basses insert's address 9 to \(fresh)")
        try require(abs(hats - 0.9) < 1e-3, "renumbered: the Hats lane no longer reaches its own insert after the edit (\(hats))")
        return ["renumbered: a bus added ahead of an automated bus keeps its new insert at 0.50 while the Hats lane holds 0.90"]
    }
}
#endif

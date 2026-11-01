import Foundation

#if ROUTING_AUTOMATION_TEST
private var appliedRoutingPoints: [(Int32, Int32, UInt64, Int32, Double, Double)] = []
/// Link-only substitute for the C entry point. The standalone test verifies exact
/// delivery without creating an audio engine, loading vendors, or touching a GUI.
@_cdecl("st_engine_add_host_automation_point")
func captureRoutingPoint(_ engine: OpaquePointer?, _ pattern: Int32, _ destination: Int32,
                         _ address: UInt64, _ interpolation: Int32, _ row: Double, _ value: Double) {
    appliedRoutingPoints.append((pattern, destination, address, interpolation, row, value))
}
#endif

@main struct AudioRoutingTests {
    struct Failure: Error { let message: String }
    static var checks = 0
    static func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try condition() else { throw Failure(message: message) }; checks += 1
    }
    static func rejects(_ message: String, _ operation: () throws -> Void) throws {
        do { try operation() } catch is AudioRouting.RoutingError { checks += 1; return }
        throw Failure(message: "Expected rejection: " + message)
    }
    static func project(_ routing: [String: Any] = [:]) -> [String: Any] {
        var value: [String: Any] = ["version": 1, "mixModel": "standard", "buses": [], "outputs": [:], "sends": [], "sidechains": []]
        value.merge(routing) { _, new in new }
        return ["tracks": [["routingID": "a", "name": "Drums", "volume": 0.8, "pan": -0.25, "mute": false, "solo": true],
                           ["routingID": "b", "name": "Key", "volume": 1.2]], "masterVolume": 0.75, "routing": value]
    }
    static func bus(_ id: String, _ type: String = "group") -> [String: Any] {
        ["id": id, "name": id.capitalized, "type": type, "volume": 1.1, "pan": 0.2, "mute": false, "solo": false]
    }
    static func key(source: String = "track:b", tap: String = "postFader", instance: String = "compressor", enabled: Bool = true) -> [String: Any] {
        ["id": "key", "source": source, "tap": tap, "gain": 1, "enabled": enabled,
         "destination": ["instanceID": instance, "inputBus": 1, "layoutSignature": ["name": "Detector", "channels": 2]]]
    }
    static func main() throws {
        let legacy: [String: Any] = ["tracks": [["name": "Old song"]]]
        let original = try JSONSerialization.data(withJSONObject: legacy, options: .sortedKeys)
        let disabled = try AudioRouting.parse(project: legacy)
        try require(!disabled.enabled && disabled.mixModel == .legacy && disabled.outputs.isEmpty, "Absent routing must stay legacy-disabled")
        try require(try JSONSerialization.data(withJSONObject: legacy, options: .sortedKeys) == original, "Parsing mutated the original project")
        try require(try disabled.graph(pluginLayouts: []).nodes.isEmpty, "Legacy parsing synthesized a graph")
        let definition = try AudioRouting.parse(project: project(["buses": [bus("drums"), bus("space", "return")],
            "outputs": ["track:a": "bus:drums", "track:b": NSNull()]]))
        try require(definition.owners.count == 5 && definition.owner("track:a")?.trackIndex == 0, "Owner identity or index missing")
        try require(definition.owner("track:a")?.pan == -0.25 && definition.owner("track:a")?.solo == true, "Track strip controls lost")
        try require(definition.owner("bus:drums")?.volume == 1.1 && definition.owner("master")?.volume == 0.75, "Bus/master gain source incorrect")
        try require(definition.outputs["track:a"]! == "bus:drums" && definition.outputs["bus:space"]! == "master", "Explicit/default destinations incorrect")
        try require(definition.outputs.keys.contains("track:b") && definition.outputs["track:b"]! == nil, "Explicit disconnection became default output")
        try require(definition.outputs["master"] == nil, "Master gained an outgoing route")

        let validation: [(String, [String: Any])] = [
            ("version", ["version": 2]), ("fractional version", ["version": 1.5]), ("Boolean version", ["version": true]),
            ("mix model", ["mixModel": "surprise"]), ("invalid bus name", ["buses": [["id": "g", "name": "\n", "type": "group"]]]),
            ("path-like bus ID", ["buses": [bus("../group")]]), ("duplicate bus", ["buses": [bus("g"), bus("g")]]),
            ("unknown owner", ["outputs": ["track:missing": "master"]]), ("unknown destination", ["outputs": ["track:a": "bus:missing"]]),
            ("master output", ["outputs": ["master": NSNull()]]),
            ("self output cycle", ["outputs": ["track:a": "track:a"]]),
            ("output cycle", ["outputs": ["track:a": "track:b", "track:b": "track:a"]]),
            ("nonfinite gain", ["sends": [["id": "s", "source": "track:a", "tap": "postFader", "destination": "master", "gain": Double.nan]]]),
            ("out-of-range gain", ["sends": [["id": "s", "source": "track:a", "tap": "postFader", "destination": "master", "gain": 3]]]),
            ("numeric Boolean", ["sends": [["id": "s", "source": "track:a", "tap": "postFader", "destination": "master", "enabled": 1]]]),
            ("master send", ["sends": [["id": "s", "source": "master", "tap": "postFader", "destination": "track:a"]]]),
            ("master key", ["sidechains": [key(source: "master")]]),
            ("unknown tap", ["sidechains": [key(tap: "automatic")]]),
            ("duplicate route ID", ["sidechains": [key(), key()]]),
            ("too many buses", ["buses": (0..<1025).map { bus("b\($0)") }]),
            ("too many routes", ["sends": (0..<129).map { ["id": "s\($0)", "source": "track:a", "tap": "postFader", "destination": "master"] }])
        ]
        for (name, input) in validation { try rejects(name) { _ = try AudioRouting.parse(project: project(input)) } }
        var badTracks = project(); badTracks["tracks"] = [["routingID": "same"], ["routingID": "same"]]
        try rejects("duplicate explicit track IDs") { _ = try AudioRouting.parse(project: badTracks) }
        badTracks["tracks"] = [["name": "No ID"]]
        try rejects("missing active track IDs") { _ = try AudioRouting.parse(project: badTracks) }
        // 17+ tracks route fine; only an absurd count trips the sanity limit.
        badTracks["tracks"] = (0..<200).map { ["routingID": "t\($0)"] }
        let wide = try AudioRouting.parse(project: badTracks)
        guard wide.owners.filter({ $0.kind == .track }).count == 200, wide.owner("track:t199")?.trackIndex == 199 else { throw Failure(message: "200 tracks route") }
        badTracks["tracks"] = (0..<1025).map { ["routingID": "t\($0)"] }
        try rejects("track limit") { _ = try AudioRouting.parse(project: badTracks) }
        badTracks["tracks"] = [["routingID": "a", "volume": Double.infinity]]
        try rejects("nonfinite track volume") { _ = try AudioRouting.parse(project: badTracks) }
        for addition in [["buses": [bus("g")]], ["outputs": ["track:a": NSNull()]],
                         ["sidechains": [key()]], ["sends": [["id": "s", "source": "track:a", "tap": "postFader", "destination": "master"]]]] as [[String: Any]] {
            var legacyRouting = addition; legacyRouting["mixModel"] = "legacy"
            do { _ = try AudioRouting.parse(project: project(legacyRouting)); throw Failure(message: "Legacy custom routing accepted") }
            catch let error as AudioRouting.RoutingError { try require(error.localizedDescription.contains("Enable standard routing"), "Legacy routing rejection was not actionable") }
        }
        let compatibleLegacy = try AudioRouting.parse(project: project(["mixModel": "legacy", "outputs": ["track:a": "master"]]))
        _ = try compatibleLegacy.graph(pluginLayouts: [.init(instanceID: "legacy-track-fx", owner: "track:a", slot: 0)]); checks += 1
        try rejects("legacy master inserts") { _ = try compatibleLegacy.graph(pluginLayouts: [.init(instanceID: "legacy-master-fx", owner: "master", slot: 0)]) }

        let detector = AudioRouting.InputBus(index: 1, name: "Detector", channels: 2)
        let main64 = AudioRouting.PluginLayout(instanceID: "main", owner: "track:a", slot: 0, latencyFrames: 64)
        let aux257 = AudioRouting.PluginLayout(instanceID: "aux", owner: "track:b", slot: 0, latencyFrames: 257)
        let compressor = AudioRouting.PluginLayout(instanceID: "compressor", owner: "track:a", slot: 1, latencyFrames: 64, inputBuses: [detector])
        let keyed = try AudioRouting.parse(project: project(["sidechains": [key()], "outputs": ["track:b": NSNull()]]))
        let graph = try keyed.graph(pluginLayouts: [main64, aux257, compressor])
        let plan = try graph.plan(maximumFrames: 96_000)
        try require(plan.inputArrival["plugin:compressor"] == 257 && plan.arrival["plugin:compressor"] == 321, "Slower key did not delay processor's main input")
        try require(plan.edgeDelays["insert:compressor"] == 193 && plan.edgeDelays["sidechain:key"] == 0, "Sidechain edge compensation incorrect")
        let master = AudioRouting.nodeID(owner: "master", stage: .postFader)
        try require(plan.arrival[master] == 321 && plan.total == 321, "Detector-only upstream latency omitted from master pre-roll")
        try require(1024 + plan.arrival[master]! - plan.total == 1024, "Trim did not preserve source musical time")
        try require(4095 + plan.inputArrival["plugin:compressor"]! == 4352, "Processor automation offset does not follow both inputs")
        let closure = try graph.dependencies(of: [master])
        try require(closure.contains(AudioRouting.nodeID(owner: "track:b", stage: .source)), "Sidechain-only source culled from stem dependencies")
        try require(!graph.edges.contains { $0.id == "output:track:b" }, "Detector-only source leaked an audible master route")
        try require(graph.resolvedSidechains.count == 1 && graph.unavailableSidechains.isEmpty, "Exact detector descriptor did not resolve")
        let swap = try keyed.graph(pluginLayouts: [
            .init(instanceID: "main", owner: "track:a", slot: 0, latencyFrames: 257),
            .init(instanceID: "aux", owner: "track:b", slot: 0, latencyFrames: 64), compressor]).plan(maximumFrames: 321)
        try require(swap.edgeDelays["insert:compressor"] == 0 && swap.edgeDelays["sidechain:key"] == 193 && swap.total == 321, "Swapped key/main latency reused stale edge delay")

        let missing = try keyed.graph(pluginLayouts: [main64, aux257])
        try require(missing.unavailableSidechains.first?.sidechain == keyed.sidechains.first && missing.resolvedSidechains.isEmpty, "Missing plug-in descriptor was discarded or remapped")
        let changedLayout = AudioRouting.PluginLayout(instanceID: "compressor", owner: "track:a", slot: 1, inputBuses: [.init(index: 2, name: "Detector", channels: 2)])
        let mismatch = try keyed.graph(pluginLayouts: [changedLayout])
        try require(mismatch.unavailableSidechains.count == 1 && !mismatch.edges.contains { $0.id == "sidechain:key" }, "Changed bus index was remapped silently")
        for wrongBus in [AudioRouting.InputBus(index: 1, name: "New Detector", channels: 2), .init(index: 1, name: "Detector", channels: 1)] {
            let wrong = try keyed.graph(pluginLayouts: [.init(instanceID: "compressor", owner: "track:a", slot: 1, inputBuses: [wrongBus])])
            try require(wrong.unavailableSidechains.count == 1, "Changed detector name/channels was accepted")
        }
        let mainBus = try keyed.graph(pluginLayouts: [.init(instanceID: "compressor", owner: "track:a", slot: 1,
            inputBuses: [.init(index: 1, name: "Detector", channels: 2, isAuxiliary: false)])])
        try require(mainBus.unavailableSidechains.count == 1, "Main input was silently accepted as an auxiliary input")
        let missingSource = try AudioRouting.parse(project: project(["sidechains": [key(source: "track:removed")]]))
        let missingSourceGraph = try missingSource.graph(pluginLayouts: [compressor])
        try require(missingSourceGraph.unavailableSidechains.first?.sidechain.source == "track:removed", "Removed detector source discarded saved routing intent")
        let disabledKey = try AudioRouting.parse(project: project(["sidechains": [key(enabled: false)]]))
        let disabledGraph = try disabledKey.graph(pluginLayouts: [compressor])
        try require(disabledGraph.resolvedSidechains.count == 1 && !disabledGraph.edges.contains { $0.id == "sidechain:key" }, "Disabled key became an active edge")
        let selfPre = try AudioRouting.parse(project: project(["sidechains": [key(source: "track:a", tap: "preInserts")]]))
        _ = try selfPre.graph(pluginLayouts: [compressor]); checks += 1
        for tap in ["preFader", "postFader"] {
            let selfPost = try AudioRouting.parse(project: project(["sidechains": [key(source: "track:a", tap: tap)]]))
            try rejects("same-owner \(tap) detector cycle") { _ = try selfPost.graph(pluginLayouts: [compressor]) }
        }
        try rejects("duplicate processor IDs") { _ = try keyed.graph(pluginLayouts: [main64, main64]) }
        try rejects("duplicate processor slots") { _ = try keyed.graph(pluginLayouts: [main64, .init(instanceID: "other", owner: "track:a", slot: 0)]) }
        // Track channels also hold their instrument's chain in slots 16..<32 (InstrumentEffects); nothing beyond.
        try rejects("processor slot limit") { _ = try keyed.graph(pluginLayouts: [.init(instanceID: "tooMany", owner: "track:a", slot: MixerChannel.channelSlotCount)]) }
        _ = try keyed.graph(pluginLayouts: [.init(instanceID: "instrumentChain", owner: "track:a", slot: MixerChannel.instrumentSlotBase)]); checks += 1
        // Slots 8...15 are valid inserts on every channel kind (tracks, buses, master).
        _ = try keyed.graph(pluginLayouts: (0..<MixerChannel.insertSlotCount).map { .init(instanceID: "deep\($0)", owner: "track:a", slot: $0) }); checks += 1

        let grouped = try AudioRouting.parse(project: project(["buses": [bus("g")], "outputs": ["track:a": "bus:g", "track:b": "bus:g"]]))
        let groupedPlan = try grouped.graph(pluginLayouts: [main64, aux257, .init(instanceID: "groupFX", owner: "bus:g", slot: 0, latencyFrames: 64)]).plan(maximumFrames: 321)
        try require(groupedPlan.edgeDelays["output:track:a"] == 193 && groupedPlan.edgeDelays["output:track:b"] == 0 && groupedPlan.arrival[master] == 321, "Group input compensation incorrect")
        let sent = try AudioRouting.parse(project: project(["buses": [bus("r", "return")], "sends": [["id": "s", "source": "track:a", "tap": "postFader", "destination": "bus:r", "gain": 0]]]))
        let sendPlan = try sent.graph(pluginLayouts: [main64, .init(instanceID: "sendFX", owner: "bus:r", slot: 0, latencyFrames: 257)]).plan(maximumFrames: 321)
        try require(sendPlan.edgeDelays["output:track:a"] == 257 && sendPlan.edgeDelays["send:s"] == 0 && sendPlan.total == 321, "Fan-out compensation moved before shared source or culled zero-gain send")
        try require(sendPlan.edgeDelays["output:track:b"] == 321, "Dry parallel track failed to follow send latency")
        let noSend = try AudioRouting.parse(project: project())
        try require(try noSend.graph(pluginLayouts: [main64]).plan(maximumFrames: 64).total == 64, "Removed routing retained stale latency")

        let n = [AudioRouting.Node(id: "a", latencyFrames: 64), .init(id: "b", latencyFrames: 257), .init(id: "mix"), .init(id: "out", latencyFrames: 64)]
        let e = [AudioRouting.Edge(id: "a-mix", source: "a", destination: "mix"), .init(id: "b-mix", source: "b", destination: "mix"), .init(id: "mix-out", source: "mix", destination: "out")]
        let generic = try AudioRouting.plan(nodes: n, edges: e, maximumFrames: 321)
        try require(generic == (try AudioRouting.plan(nodes: n.reversed(), edges: e.reversed(), maximumFrames: 321)), "Planner order depends on input dictionary ordering")
        try require(generic.order == ["a", "b", "mix", "out"] && generic.edgeDelays["a-mix"] == 193, "Generic DAG result incorrect")
        try rejects("capacity") { _ = try AudioRouting.plan(nodes: n, edges: e, maximumFrames: 320) }
        try rejects("negative capacity") { _ = try AudioRouting.plan(nodes: [], edges: [], maximumFrames: -1) }
        try rejects("negative latency") { _ = try AudioRouting.plan(nodes: [.init(id: "a", latencyFrames: -1)], edges: [], maximumFrames: 100) }
        try rejects("duplicate node") { _ = try AudioRouting.plan(nodes: n + [n[0]], edges: e, maximumFrames: 500) }
        try rejects("duplicate edge") { _ = try AudioRouting.plan(nodes: n, edges: e + [e[0]], maximumFrames: 500) }
        try rejects("unknown endpoint") { _ = try AudioRouting.plan(nodes: n, edges: [.init(id: "bad", source: "missing", destination: "out")], maximumFrames: 500) }
        try rejects("generic feedback") { _ = try AudioRouting.plan(nodes: n, edges: e + [.init(id: "feedback", source: "out", destination: "a")], maximumFrames: 500) }
        try rejects("overflow") { _ = try AudioRouting.plan(nodes: [.init(id: "a", latencyFrames: Int.max), .init(id: "b", latencyFrames: 1)], edges: [.init(id: "ab", source: "a", destination: "b")], maximumFrames: Int.max) }
        try require(try AudioRouting.plan(nodes: [], edges: [], maximumFrames: 0).total == 0, "Empty plan should have zero latency")
#if ROUTING_AUTOMATION_TEST
        try automationChecks()
#endif
        print("PASS: \(checks) pure routing validation and DAG timing checks")
    }

#if ROUTING_AUTOMATION_TEST
    static func automationChecks() throws {
        func lane(_ owner: String, _ target: String, _ value: Double, interpolation: String = "linear") -> [String: Any] {
            ["owner": owner, "target": target, "interpolation": interpolation,
             "points": [["row": 0, "value": value], ["row": 63.5, "value": value]]]
        }
        let send: [String: Any] = ["id": "s", "source": "track:a", "tap": "postFader", "destination": "bus:g", "gain": 0.3]
        var project = project(["buses": [bus("g")], "sends": [send], "sidechains": [key()]])
        let plugins = [AudioRouting.PluginLayout(instanceID: "compressor", owner: "track:a", slot: 0,
                                                inputBuses: [.init(index: 1, name: "Detector", channels: 2)])]
        let lanes: [[String: Any]] = [lane("bus:g", "volume", 1.5), lane("bus:g", "pan", -0.5),
            lane("master", "volume", 0.6), lane("track:a", "send:s", 1.7, interpolation: "step"),
            lane("track:b", "send:key", 0.9), lane("track:a", "plugin:effect:0:18446744073709551615", 0.4),
            ["track": 0, "target": "volume", "points": [["row": 0, "value": 1000]]]]
        project["patterns"] = [["id": 2, "length": 64, "automation": lanes],
                               ["id": 5, "length": 32, "automation": [["owner": "track:a", "target": "volume", "points": [["row": 31, "value": 1.9]]]]]]
        let definition = try AudioRouting.parse(project: project)
        let map = try AudioRoutingAutomation(project: project, definition: definition, plugins: plugins)
        try require(map.entries.count == 7 && map.points.count == 17, "Automation destinations/default points missing or native track lanes duplicated")
        try require(map.entries.enumerated().allSatisfy { $0.offset == $0.element.destination } && map.entries.count < 512, "Render destinations are not dense and bounded")
        let groupID = map.entries.first { $0.owner == "bus:g" && $0.kind == .mix }!.destination
        try require(Set(map.points.filter { $0.destination == groupID }.map(\.address)) == [0, 1], "Mix volume/pan do not share one dense destination")
        let pluginID = map.entries.first { $0.kind == .plugin }!.destination
        try require(map.points.first { $0.destination == pluginID }?.address == UInt64.max, "Parameter address was rounded through Double")
        let sendID = map.entries.first { $0.routeID == "s" }!.destination
        try require(map.points.filter { $0.destination == sendID }.allSatisfy { $0.address == 0 && $0.interpolation == 1 }, "Send gain or step interpolation changed")
        try require(map.points.filter { $0.pattern == 5 }.count == 5 && map.points.filter { $0.pattern == 5 }.allSatisfy { $0.row == 0 }, "Non-first pattern defaults missing or track mix duplicated")
        try require(map.points.allSatisfy { map.entries[$0.destination].kind != .mix || definition.owner(map.entries[$0.destination].owner)?.kind != .track }, "Generic track mix overrides native automation or step FX row locks")
        try require(map.defaults.first { $0.destination == groupID && $0.address == 0 }?.value == 1.1, "Bus default gain missing")
        try require(map.defaults.first { $0.destination == sendID }?.value == 0.3, "Send default gain missing")
        let reset = try map.defaults(pluginValues: ["compressor": [42: 0.8]])
        try require(reset.last?.destination == pluginID && reset.last?.address == 42 && reset.last?.value == 0.8, "Plugin reset snapshot failed to map")
        try rejects("invalid plugin reset default") { _ = try map.defaults(pluginValues: ["compressor": [42: Double.infinity]]) }
        appliedRoutingPoints.removeAll()
        map.apply(to: OpaquePointer(bitPattern: 1)!)
        try require(appliedRoutingPoints.count == map.points.count, "Host automation C bridge dropped points")
        for (received, expected) in zip(appliedRoutingPoints, map.points) {
            try require(received.0 == expected.pattern && received.1 == expected.destination && received.2 == expected.address &&
                        received.3 == expected.interpolation && received.4 == expected.row && received.5 == expected.value, "Host automation C delivery changed fields")
        }
        var reversed = project
        reversed["patterns"] = [["id": 2, "length": 64, "automation": lanes.reversed().map { $0 }]]
        let other = try AudioRoutingAutomation(project: reversed, definition: definition, plugins: plugins.reversed())
        try require(other.entries.map { "\($0.destination):\($0.owner):\($0.routeID ?? ""):\($0.instanceID ?? "")" } ==
                    map.entries.map { "\($0.destination):\($0.owner):\($0.routeID ?? ""):\($0.instanceID ?? "")" }, "Lane ordering changed dense mapping IDs")
        var missing = project
        missing["patterns"] = [["id": 0, "automation": [lane("track:a", "plugin:effect:3:42", 0.5), lane("track:a", "send:removed", 1)]]]
        let missingMap = try AudioRoutingAutomation(project: missing, definition: definition, plugins: plugins)
        try require(missingMap.unavailableLanes.count == 2 && missingMap.points.count == 5 &&
                    missingMap.points.allSatisfy { missingMap.entries[$0.destination].kind != .plugin }, "Missing targets were remapped or generic defaults were lost")
        let invalidLanes: [[String: Any]] = [lane("master", "pan", 0), lane("bus:missing", "volume", 1),
            lane("track:b", "send:s", 1), lane("track:a", "plugin:effect:0:42", 1.01), lane("bus:g", "volume", -0.01),
            lane("bus:g", "pan", 1.01), lane("track:a", "send:s", 2.01), lane("track:a", "cutoff", 0.5),
            lane("track:a", "volume", Double.nan), lane("track:a", "volume", 2.01), lane("track:a", "pan", 1.01),
            lane("track:a", "volume", 1, interpolation: "curve")]
        for invalid in invalidLanes {
            var bad = project; bad["patterns"] = [["id": 0, "automation": [invalid]]]
            try rejects("host lane validation \(invalid["target"] ?? "")") { _ = try AudioRoutingAutomation(project: bad, definition: definition, plugins: plugins) }
        }
        var bad = project
        bad["patterns"] = [["id": 0, "automation": [lane("bus:g", "volume", 1), lane("bus:g", "volume", 1)]]]
        try rejects("duplicate host lane") { _ = try AudioRoutingAutomation(project: bad, definition: definition, plugins: plugins) }
        bad["patterns"] = [["id": 0, "automation": [lane("track:a", "volume", 1), lane("track:a", "volume", 0.5)]]]
        try rejects("duplicate native owner mix lane") { _ = try AudioRoutingAutomation(project: bad, definition: definition, plugins: plugins) }
        bad["patterns"] = [["id": 0, "automation": [["owner": "bus:g", "target": "volume", "points": [["row": 65, "value": 1]]]]]]
        try rejects("point past pattern length") { _ = try AudioRoutingAutomation(project: bad, definition: definition, plugins: plugins) }
        bad["patterns"] = [["id": 0, "automation": [["owner": "bus:g", "target": "volume", "points": (0..<4097).map { ["row": Double($0) / 100, "value": 1] }]]]]
        try rejects("host point limit") { _ = try AudioRoutingAutomation(project: bad, definition: definition, plugins: plugins) }
        bad["patterns"] = [["id": 0, "automation": (0..<257).map { lane("track:a", "plugin:effect:0:\($0)", 0.5) }]]
        try rejects("host lane limit") { _ = try AudioRoutingAutomation(project: bad, definition: definition, plugins: plugins) }
        let legacy = try AudioRouting.parse(project: ["tracks": []])
        try require(try AudioRoutingAutomation(project: project, definition: legacy, plugins: []).entries.isEmpty, "Legacy-disabled project gained host automation")
        try require(try AudioRoutingAutomation(project: project, definition: legacy, plugins: []).hostMap == 0, "Legacy-disabled project gained a host destination map")
        // The destination map identity follows the numbering only: the same layout keeps it, a bus that sorts ahead
        // of an automated insert (renumbering it) changes it, so stale host events can be told apart.
        try require(map.hostMap != 0 && other.hostMap == map.hostMap && missingMap.hostMap == map.hostMap, "Host destination map changed without a renumbering")
        var ahead = project
        ahead["routing"] = (project["routing"] as! [String: Any]).merging(["buses": [bus("a"), bus("g")]]) { $1 }
        let aheadDefinition = try AudioRouting.parse(project: ahead)
        let aheadMap = try AudioRoutingAutomation(project: ahead, definition: aheadDefinition, plugins: plugins)
        try require(aheadMap.entries.first { $0.kind == .plugin }!.destination == pluginID + 1 && aheadMap.hostMap != map.hostMap, "A bus sorted ahead did not renumber, or kept the host destination map")
        let aheadInsert = plugins + [AudioRouting.PluginLayout(instanceID: "ahead", owner: "bus:a", slot: 0)]
        let insertMap = try AudioRoutingAutomation(project: ahead, definition: aheadDefinition, plugins: aheadInsert)
        try require(insertMap.entries[pluginID + 1].instanceID == "ahead" && insertMap.hostMap != aheadMap.hostMap, "An insert ahead took the old index without a new host destination map")

        var desiredProject = project
        desiredProject["patterns"] = []
        desiredProject["audioPlugins"] = [["owner": "bus:g", "kind": "effect", "slot": 3, "component": "missing-vendor"],
                                          ["track": 0, "kind": "effect", "slot": 0, "component": "missing-vendor-2", "instanceID": "z-first"]]
        let desired = try AudioRoutingAutomation.desiredPluginLayouts(project: desiredProject, definition: definition)
        try require(desired.count == 2 && desired.contains { $0.owner == "bus:g" && $0.slot == 3 && !$0.instanceID.contains(":") }, "Missing desired plug-in was dropped or given invalid fallback identity")
        let beforeLoad = try AudioRoutingAutomation(project: desiredProject, definition: definition, plugins: desired)
        desiredProject["audioPlugins"] = [["owner": "bus:g", "kind": "effect", "slot": 3, "component": "missing-vendor", "instanceID": "a-new-active-id"],
                                          ["track": 0, "kind": "effect", "slot": 0, "component": "missing-vendor-2", "instanceID": "z-first"]]
        let afterLoad = try AudioRoutingAutomation(project: desiredProject, definition: definition,
            plugins: AudioRoutingAutomation.desiredPluginLayouts(project: desiredProject, definition: definition))
        func dispatchKeys(_ map: AudioRoutingAutomation) -> [String] {
            map.entries.map { "\($0.destination):\($0.owner):\($0.kind.rawValue):\($0.pluginKind?.rawValue ?? ""):\($0.slot ?? -1):\($0.routeID ?? "")" }
        }
        try require(dispatchKeys(beforeLoad) == dispatchKeys(afterLoad), "Late instance identity changed owner/slot event destinations")
        var oldProject = project
        oldProject["patterns"] = []; oldProject["audioPlugins"] = nil
        oldProject["tracks"] = [["routingID": "a", "instrument": 0, "plugins": [NSNull(), ["component": "old-effect"]]], ["routingID": "b", "instrument": 1]]
        oldProject["instruments"] = [["plugin": ["component": "old-instrument"]], [:]]
        let oldDesired = try AudioRoutingAutomation.desiredPluginLayouts(project: oldProject, definition: definition)
        try require(oldDesired.count == 2 && oldDesired.contains { $0.kind == .instrument && $0.slot == 0 } && oldDesired.contains { $0.kind == .effect && $0.slot == 1 }, "Legacy desired descriptor extraction lost sparse effect or instrument slots")
        oldProject["audioPlugins"] = []
        try require(try AudioRoutingAutomation.desiredPluginLayouts(project: oldProject, definition: definition).isEmpty, "Explicit empty plug-in list fell back to stale legacy descriptors")

        var maximumProject = Self.project(["buses": (0..<16).map { bus("g\($0)") },
            "sends": (0..<128).map { ["id": "s\($0)", "source": "track:t0", "tap": "postFader", "destination": "master"] }])
        maximumProject["tracks"] = (0..<16).map { ["routingID": "t\($0)"] }
        let maximumDefinition = try AudioRouting.parse(project: maximumProject)
        var maximumPlugins: [AudioRouting.PluginLayout] = []
        for (index, owner) in maximumDefinition.owners.enumerated() {
            for slot in 0..<MixerChannel.insertSlotCount { maximumPlugins.append(.init(instanceID: "fx-\(index)-\(slot)", owner: owner.id, slot: slot)) }
            if owner.kind == .track { maximumPlugins.append(.init(instanceID: "inst-\(index)", owner: owner.id, kind: .instrument, slot: 0)) }
        }
        let maximumMap = try AudioRoutingAutomation(project: maximumProject, definition: maximumDefinition, plugins: maximumPlugins)
        try require(maximumMap.entries.count == 705 && maximumMap.entries.last?.destination == 704, "Full supported topology exceeds bounded dense destination space")

        var legacyRouted = Self.project(["mixModel": "legacy"])
        let legacyDefinition = try AudioRouting.parse(project: legacyRouted)
        for (owner, target) in [("track:a", "volume"), ("track:a", "pan"), ("master", "volume")] {
            legacyRouted["patterns"] = [["id": 0, "automation": [lane(owner, target, 0.5)]]]
            try rejects("legacy \(owner) owner-mix lane") { _ = try AudioRoutingAutomation(project: legacyRouted, definition: legacyDefinition, plugins: []) }
        }
        legacyRouted["patterns"] = [["id": 0, "automation": [["track": 0, "target": "volume", "points": [["row": 0, "value": 0.6]]]]]]
        let legacyMapped = try AudioRoutingAutomation(project: legacyRouted, definition: legacyDefinition, plugins: [])
        try require(legacyMapped.points.isEmpty && legacyMapped.defaults.isEmpty, "Legacy native track lanes/defaults were duplicated in the host mapper")
        legacyRouted["audioPlugins"] = [["owner": "master", "kind": "effect", "slot": 0, "component": "fake"]]
        try rejects("legacy desired master inserts") { _ = try AudioRoutingAutomation.desiredPluginLayouts(project: legacyRouted, definition: legacyDefinition) }

        // Defaults are C host lanes, not an immediate reset of already scheduled
        // delayed events. Explicit values replace exactly one corresponding key.
        var defaultProject = project
        defaultProject["patterns"] = [["id": 0, "length": 64, "automation": []]]
        let onlyDefaults = try AudioRoutingAutomation(project: defaultProject, definition: definition, plugins: plugins)
        try require(onlyDefaults.points.count == 5 && onlyDefaults.points.allSatisfy { $0.row == 0 && $0.interpolation == 1 }, "Absent generic lanes did not produce constant row-zero step defaults")
        try require(onlyDefaults.points.first { $0.destination == groupID && $0.address == 0 }?.value == 1.1, "Default volume was normalized rather than retained in raw units")
        let trackMixID = onlyDefaults.entries.first { $0.owner == "track:a" && $0.kind == .mix }!.destination
        try require(onlyDefaults.defaults.first { $0.destination == trackMixID && $0.address == 1 }?.value == -0.25 &&
                    !onlyDefaults.points.contains { $0.destination == trackMixID }, "Track default metadata lost signed units or leaked to the host lane output")
        try require(!onlyDefaults.points.contains { $0.destination == pluginID }, "Mapper invented a plug-in baseline")
        defaultProject["patterns"] = [["id": 0, "length": 64, "automation": [lane("bus:g", "volume", 1.5)]]]
        let explicit = try AudioRoutingAutomation(project: defaultProject, definition: definition, plugins: plugins)
        let explicitVolume = explicit.points.filter { $0.destination == groupID && $0.address == 0 }
        try require(explicitVolume.count == 2 && explicitVolume.allSatisfy { $0.value == 1.5 && $0.interpolation == 0 }, "Default and explicit lane were merged instead of replaced")
        defaultProject["patterns"] = [["id": 0, "automation": [["owner": "bus:g", "target": "volume", "points": []]]]]
        let emptied = try AudioRoutingAutomation(project: defaultProject, definition: definition, plugins: plugins)
        try require(emptied.points.filter { $0.destination == groupID && $0.address == 0 }.count == 1 &&
                    emptied.points.first { $0.destination == groupID && $0.address == 0 }?.value == 1.1, "Empty/removed lane did not restore its timed default")
        defaultProject["patterns"] = [["id": 0, "automation": (0..<251).map { lane("track:a", "plugin:effect:0:\($0)", 0.5) } +
            [lane("track:a", "volume", 1.5), lane("track:a", "pan", -0.2)]]]
        let atLimit = try AudioRoutingAutomation(project: defaultProject, definition: definition, plugins: plugins)
        try require(Set(atLimit.points.map { "\($0.destination):\($0.address)" }).count == 256, "Combined default/explicit lane capacity is incorrect")
        defaultProject["patterns"] = [["id": 0, "automation": (0..<252).map { lane("track:a", "plugin:effect:0:\($0)", 0.5) }]]
        do {
            _ = try AudioRoutingAutomation(project: defaultProject, definition: definition, plugins: plugins)
            throw Failure(message: "Expected combined default lane limit rejection")
        } catch let error as AudioRouting.RoutingError {
            try require(error.localizedDescription.contains("including routing defaults"), "Combined lane overflow lacks a clear explanation")
        }
    }
#endif
}

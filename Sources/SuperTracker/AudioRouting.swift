import Foundation
import CoreFoundation

/// Project routing and sample-domain delay planning. This module neither mutates
/// the project nor owns an audio graph; callers publish a validated graph atomically.
public enum AudioRouting {
    public enum RoutingError: LocalizedError, Equatable {
        case invalid(String)
        public var errorDescription: String? { if case .invalid(let message) = self { return message }; return nil }
    }
    public enum MixModel: String, Sendable { case legacy, standard }
    public enum OwnerKind: String, Sendable { case track, group, `return`, master }
    public enum Tap: String, Sendable { case preInserts, preFader, postFader }
    public enum Stage: String, Sendable { case source, input, preFader, postFader }
    public enum PluginKind: String, Sendable { case instrument, effect }

    public struct Owner: Equatable, Sendable {
        public let id: String
        public let name: String
        public let kind: OwnerKind
        public let trackIndex: Int?
        public let volume: Double
        public let pan: Double
        public let mute: Bool
        public let solo: Bool
    }
    public struct Send: Equatable, Sendable {
        public let id: String
        public let source: String
        public let tap: Tap
        public let destination: String
        public let gain: Double
        public let enabled: Bool
    }
    public struct LayoutSignature: Equatable, Sendable {
        public let name: String
        public let channels: Int
        public init(name: String, channels: Int) { self.name = name; self.channels = channels }
    }
    public struct SidechainDestination: Equatable, Sendable {
        public let instanceID: String
        public let inputBus: Int
        public let layoutSignature: LayoutSignature
    }
    public struct Sidechain: Equatable, Sendable {
        public let id: String
        public let source: String
        public let tap: Tap
        public let destination: SidechainDestination
        public let gain: Double
        public let enabled: Bool
    }
    public struct InputBus: Equatable, Sendable {
        public let index: Int
        public let name: String
        public let channels: Int
        public let isAuxiliary: Bool
        public init(index: Int, name: String, channels: Int, isAuxiliary: Bool? = nil) {
            self.index = index; self.name = name; self.channels = channels
            self.isAuxiliary = isAuxiliary ?? (index > 0)
        }
    }
    public struct PluginLayout: Equatable, Sendable {
        public let instanceID: String
        public let owner: String
        public let kind: PluginKind
        public let slot: Int
        public let latencyFrames: Int
        /// Actual current input buses, including their stable index and layout.
        public let inputBuses: [InputBus]
        public init(instanceID: String, owner: String, kind: PluginKind = .effect,
                    slot: Int, latencyFrames: Int = 0, inputBuses: [InputBus] = []) {
            self.instanceID = instanceID; self.owner = owner; self.kind = kind
            self.slot = slot; self.latencyFrames = latencyFrames; self.inputBuses = inputBuses
        }
    }
    public struct UnavailableSidechain: Equatable, Sendable {
        public let sidechain: Sidechain
        public let reason: String
    }
    public struct ResolvedSidechain: Equatable, Sendable {
        public let sidechain: Sidechain
        public let plugin: PluginLayout
        public let inputBus: InputBus
    }

    public struct Definition: Equatable, Sendable {
        /// False only when the project has no routing field. Legacy playback then
        /// retains its existing graph and mix policy; parsing never migrates it.
        public let enabled: Bool
        public let mixModel: MixModel
        public let owners: [Owner]
        /// Every non-master owner is present. An explicit optional nil is a
        /// disconnected output, rather than an omitted/default-master entry.
        public let outputs: [String: String?]
        public let sends: [Send]
        public let sidechains: [Sidechain]

        public func owner(_ id: String) -> Owner? { owners.first { $0.id == id } }

        /// Compile semantic stages, preserving auxiliary dependencies independently
        /// from audible edges. Muting and zero gain do not alter graph topology.
        public func graph(pluginLayouts: [PluginLayout]) throws -> Graph {
            guard enabled else { return Graph(nodes: [], edges: [], resolvedSidechains: [], unavailableSidechains: []) }
            let ownerIDs = Set(owners.map(\.id))
            var pluginIDs = Set<String>(), slots = Set<String>()
            var byOwner: [String: [PluginLayout]] = [:]
            for plugin in pluginLayouts {
                if mixModel == .legacy, plugin.owner == "master" {
                    throw RoutingError.invalid("Enable standard routing to use master inserts.")
                }
                try AudioRouting.identifier(plugin.instanceID, label: "Plug-in instance ID")
                // Track channels also hold their instrument's chain in slots 16..<32 (InstrumentEffects).
                guard ownerIDs.contains(plugin.owner), pluginIDs.insert(plugin.instanceID).inserted,
                      (0..<MixerChannel.insertSlotCount).contains(plugin.slot)
                        || (MixerChannel.insertSlotCount..<MixerChannel.channelSlotCount).contains(plugin.slot) && owner(plugin.owner)?.trackIndex != nil,
                      plugin.latencyFrames >= 0 else {
                    throw RoutingError.invalid("Invalid or duplicate plug-in layout: \(plugin.instanceID).")
                }
                if plugin.kind == .instrument {
                    guard owner(plugin.owner)?.trackIndex != nil, plugin.slot == 0 else {
                        throw RoutingError.invalid("Instruments require a track's instrument slot.")
                    }
                }
                guard slots.insert("\(plugin.owner)/\(plugin.kind.rawValue)/\(plugin.slot)").inserted else {
                    throw RoutingError.invalid("Two plug-ins occupy the same routing slot.")
                }
                var busIndices = Set<Int>()
                for bus in plugin.inputBuses {
                    try AudioRouting.text(bus.name, label: "Plug-in input name", allowEmpty: true)
                    guard (0..<32).contains(bus.index), (1...64).contains(bus.channels), busIndices.insert(bus.index).inserted else {
                        throw RoutingError.invalid("Invalid or duplicate plug-in input bus.")
                    }
                }
                byOwner[plugin.owner, default: []].append(plugin)
            }
            var nodes: [Node] = [], edges: [Edge] = []
            for owner in owners {
                let input = AudioRouting.nodeID(owner: owner.id, stage: .input)
                let pre = AudioRouting.nodeID(owner: owner.id, stage: .preFader)
                let post = AudioRouting.nodeID(owner: owner.id, stage: .postFader)
                nodes += [Node(id: input), Node(id: pre), Node(id: post)]
                let plugins = byOwner[owner.id] ?? []
                if owner.trackIndex != nil {
                    let source = AudioRouting.nodeID(owner: owner.id, stage: .source)
                    nodes.append(Node(id: source))
                    edges.append(Edge(id: "native:\(owner.id)", source: source, destination: input))
                    if let instrument = plugins.first(where: { $0.kind == .instrument }) {
                        let id = AudioRouting.pluginNodeID(instanceID: instrument.instanceID)
                        nodes.append(Node(id: id, latencyFrames: instrument.latencyFrames))
                        edges.append(Edge(id: "instrument:\(owner.id)", source: id, destination: input))
                    }
                }
                var previous = input
                for plugin in plugins.filter({ $0.kind == .effect }).sorted(by: { MixerChannel.chainOrder($0.slot) < MixerChannel.chainOrder($1.slot) }) {
                    let id = AudioRouting.pluginNodeID(instanceID: plugin.instanceID)
                    nodes.append(Node(id: id, latencyFrames: plugin.latencyFrames))
                    edges.append(Edge(id: "insert:\(plugin.instanceID)", source: previous, destination: id))
                    previous = id
                }
                edges.append(Edge(id: "preFader:\(owner.id)", source: previous, destination: pre))
                edges.append(Edge(id: "fader:\(owner.id)", source: pre, destination: post))
                if let optionalDestination = outputs[owner.id], let destination = optionalDestination {
                    edges.append(Edge(id: "output:\(owner.id)", source: post,
                                      destination: AudioRouting.nodeID(owner: destination, stage: .input)))
                }
            }
            for send in sends where send.enabled {
                edges.append(Edge(id: "send:\(send.id)", source: AudioRouting.tapNodeID(owner: send.source, tap: send.tap),
                                  destination: AudioRouting.nodeID(owner: send.destination, stage: .input)))
            }
            var resolved: [ResolvedSidechain] = [], unavailable: [UnavailableSidechain] = []
            let layouts = Dictionary(uniqueKeysWithValues: pluginLayouts.map { ($0.instanceID, $0) })
            for sidechain in sidechains {
                guard ownerIDs.contains(sidechain.source) else {
                    unavailable.append(UnavailableSidechain(sidechain: sidechain, reason: "The sidechain source is unavailable.")); continue
                }
                guard let plugin = layouts[sidechain.destination.instanceID] else {
                    unavailable.append(UnavailableSidechain(sidechain: sidechain, reason: "The destination plug-in is unavailable.")); continue
                }
                guard let bus = plugin.inputBuses.first(where: { $0.index == sidechain.destination.inputBus }), bus.isAuxiliary,
                      bus.name == sidechain.destination.layoutSignature.name,
                      bus.channels == sidechain.destination.layoutSignature.channels else {
                    unavailable.append(UnavailableSidechain(sidechain: sidechain, reason: "The saved input bus layout no longer matches the plug-in.")); continue
                }
                resolved.append(ResolvedSidechain(sidechain: sidechain, plugin: plugin, inputBus: bus))
                if sidechain.enabled {
                    edges.append(Edge(id: "sidechain:\(sidechain.id)",
                        source: AudioRouting.tapNodeID(owner: sidechain.source, tap: sidechain.tap),
                        destination: AudioRouting.pluginNodeID(instanceID: plugin.instanceID)))
                }
            }
            // Validate before the caller disconnects or attaches any real nodes.
            _ = try AudioRouting.plan(nodes: nodes, edges: edges, maximumFrames: Int.max)
            return Graph(nodes: nodes, edges: edges, resolvedSidechains: resolved, unavailableSidechains: unavailable)
        }
    }

    public struct Node: Equatable, Sendable {
        public let id: String
        public let latencyFrames: Int
        public init(id: String, latencyFrames: Int = 0) { self.id = id; self.latencyFrames = latencyFrames }
    }
    public struct Edge: Equatable, Sendable {
        public let id: String
        public let source: String
        public let destination: String
        public init(id: String, source: String, destination: String) {
            self.id = id; self.source = source; self.destination = destination
        }
    }
    public struct Plan: Equatable, Sendable {
        public let order: [String]
        public let arrival: [String: Int]
        public let inputArrival: [String: Int]
        public let edgeDelays: [String: Int]
        /// Maximum node arrival. A selected output's pre-roll is arrival[sinkID].
        public let total: Int
    }
    public struct Graph: Equatable, Sendable {
        public let nodes: [Node]
        public let edges: [Edge]
        public let resolvedSidechains: [ResolvedSidechain]
        public let unavailableSidechains: [UnavailableSidechain]
        public func plan(maximumFrames: Int) throws -> Plan {
            try AudioRouting.plan(nodes: nodes, edges: edges, maximumFrames: maximumFrames)
        }
        /// Computational closure includes auxiliary edges. A stem's audible mask
        /// must be applied separately, without muting these detector dependencies.
        public func dependencies(of sinks: Set<String>) throws -> Set<String> {
            let known = Set(nodes.map(\.id))
            guard sinks.isSubset(of: known) else { throw RoutingError.invalid("Unknown routing sink.") }
            let incoming = Dictionary(grouping: edges, by: \.destination)
            var result = sinks, pending = Array(sinks)
            while let id = pending.popLast() {
                for edge in incoming[id] ?? [] where result.insert(edge.source).inserted { pending.append(edge.source) }
            }
            return result
        }
    }

    public static func nodeID(owner: String, stage: Stage) -> String { "\(owner)/\(stage.rawValue)" }
    public static func pluginNodeID(instanceID: String) -> String { "plugin:\(instanceID)" }
    public static func tapNodeID(owner: String, tap: Tap) -> String {
        nodeID(owner: owner, stage: tap == .preInserts ? .input : tap == .preFader ? .preFader : .postFader)
    }

    /// Longest-path planning uses main and auxiliary dependencies identically.
    /// Edge delays align each processor's inputs, while each node contributes its
    /// own algorithmic latency exactly once. All arithmetic is checked.
    public static func plan(nodes: [Node], edges: [Edge], maximumFrames: Int) throws -> Plan {
        guard maximumFrames >= 0 else { throw RoutingError.invalid("The latency capacity cannot be negative.") }
        var byID: [String: Node] = [:], incoming: [String: [Edge]] = [:], outgoing: [String: [Edge]] = [:]
        var degrees: [String: Int] = [:], edgeIDs = Set<String>()
        for node in nodes {
            guard !node.id.isEmpty, node.latencyFrames >= 0, byID.updateValue(node, forKey: node.id) == nil else {
                throw RoutingError.invalid("Invalid or duplicate routing node: \(node.id).")
            }
            degrees[node.id] = 0
        }
        for edge in edges {
            guard !edge.id.isEmpty, edgeIDs.insert(edge.id).inserted, byID[edge.source] != nil, byID[edge.destination] != nil else {
                throw RoutingError.invalid("Invalid or duplicate routing connection: \(edge.id).")
            }
            incoming[edge.destination, default: []].append(edge)
            outgoing[edge.source, default: []].append(edge)
            degrees[edge.destination, default: 0] += 1
        }
        var ready = degrees.filter { $0.value == 0 }.map(\.key).sorted()
        var order: [String] = [], arrival: [String: Int] = [:], inputArrival: [String: Int] = [:], edgeDelays: [String: Int] = [:]
        var total = 0
        while !ready.isEmpty {
            let id = ready.removeFirst()
            let atInput = (incoming[id] ?? []).map { arrival[$0.source]! }.max() ?? 0
            let (atOutput, overflow) = atInput.addingReportingOverflow(byID[id]!.latencyFrames)
            guard !overflow, atOutput <= maximumFrames else {
                throw RoutingError.invalid("Routing latency exceeds the \(maximumFrames)-frame compensation capacity.")
            }
            inputArrival[id] = atInput; arrival[id] = atOutput; total = max(total, atOutput); order.append(id)
            for edge in incoming[id] ?? [] { edgeDelays[edge.id] = atInput - arrival[edge.source]! }
            for edge in outgoing[id] ?? [] {
                degrees[edge.destination]! -= 1
                if degrees[edge.destination] == 0 { ready.append(edge.destination) }
            }
            ready.sort()
        }
        guard order.count == nodes.count else {
            let remaining = degrees.filter { $0.value > 0 }.map(\.key).sorted().joined(separator: ", ")
            throw RoutingError.invalid("Routing feedback is unsupported. Cycle includes: \(remaining).")
        }
        return Plan(order: order, arrival: arrival, inputArrival: inputArrival, edgeDelays: edgeDelays, total: total)
    }

    public static func parse(project: [String: Any]) throws -> Definition {
        let tracks = try objects(project["tracks"], label: "Tracks", defaultEmpty: true)
        guard tracks.count <= ProjectLayout.trackSanityLimit else { throw RoutingError.invalid("Routing supports at most \(ProjectLayout.trackSanityLimit) tracks.") }
        let enabled = project["routing"] != nil
        var owners: [Owner] = [], explicitTrackIDs = Set<String>()
        for (index, track) in tracks.enumerated() {
            let id: String
            if let raw = track["routingID"] {
                guard let value = raw as? String else { throw RoutingError.invalid("Track routingID must be text.") }
                try identifier(value, label: "Track routingID")
                guard explicitTrackIDs.insert(value).inserted else { throw RoutingError.invalid("Duplicate track routingID: \(value).") }
                id = value
            } else {
                guard !enabled else { throw RoutingError.invalid("Track \(index + 1) has no stable routingID.") }
                id = "legacy-\(index)"
            }
            let name = try string(track["name"], label: "Track name", fallback: "Track \(index + 1)")
            owners.append(Owner(id: "track:\(id)", name: name, kind: .track, trackIndex: index,
                volume: try number(track["volume"], label: "Track volume", range: 0...2, fallback: 0.8),
                pan: try number(track["pan"], label: "Track pan", range: -1...1, fallback: 0),
                mute: try boolean(track["mute"], label: "Track mute", fallback: false),
                solo: try boolean(track["solo"], label: "Track solo", fallback: false)))
        }
        owners.append(Owner(id: "master", name: "Master", kind: .master, trackIndex: nil,
            volume: try number(project["masterVolume"], label: "Master volume", range: 0...2, fallback: 0.7),
            pan: 0, mute: false, solo: false))
        guard enabled else {
            return Definition(enabled: false, mixModel: .legacy, owners: owners, outputs: [:], sends: [], sidechains: [])
        }
        guard let raw = project["routing"] as? [String: Any],
              try integer(raw["version"], label: "Routing version", range: 1...1) == 1,
              let modelText = raw["mixModel"] as? String, let model = MixModel(rawValue: modelText) else {
            throw RoutingError.invalid("Routing requires version 1 and a legacy or standard mixModel.")
        }
        let buses = try objects(raw["buses"], label: "Routing buses", defaultEmpty: true)
        // Bus slots are an index range in the rack (AudioPluginRack.busCapacity), not nodes.
        guard buses.count <= ProjectLayout.trackSanityLimit else { throw RoutingError.invalid("Routing supports at most \(ProjectLayout.trackSanityLimit) additional buses.") }
        var busIDs = Set<String>()
        for bus in buses {
            let id = try string(bus["id"], label: "Bus ID")
            try identifier(id, label: "Bus ID")
            guard busIDs.insert(id).inserted, let kindText = bus["type"] as? String,
                  let kind = OwnerKind(rawValue: kindText), kind == .group || kind == .return else {
                throw RoutingError.invalid("Bus IDs must be unique and bus type must be group or return.")
            }
            owners.insert(Owner(id: "bus:\(id)", name: try string(bus["name"], label: "Bus name"), kind: kind, trackIndex: nil,
                volume: try number(bus["volume"], label: "Bus volume", range: 0...2, fallback: 1),
                pan: try number(bus["pan"], label: "Bus pan", range: -1...1, fallback: 0),
                mute: try boolean(bus["mute"], label: "Bus mute", fallback: false),
                solo: try boolean(bus["solo"], label: "Bus solo", fallback: false)), at: owners.count - 1)
        }
        let known = Set(owners.map(\.id))
        guard known.count == owners.count else { throw RoutingError.invalid("Routing owner IDs are not unique.") }
        func ownerID(_ value: Any?, label: String, allowMaster: Bool = true) throws -> String {
            guard let id = value as? String, known.contains(id), allowMaster || id != "master" else {
                throw RoutingError.invalid("\(label) must reference an existing \(allowMaster ? "owner" : "track or bus").")
            }
            return id
        }
        var outputs: [String: String?] = [:]
        for owner in owners where owner.id != "master" { outputs[owner.id] = .some("master") }
        if let value = raw["outputs"] {
            guard let values = value as? [String: Any] else { throw RoutingError.invalid("Routing outputs must be an object.") }
            for (source, destination) in values {
                _ = try ownerID(source, label: "Output source", allowMaster: false)
                outputs[source] = destination is NSNull ? .some(nil) : .some(try ownerID(destination, label: "Output destination"))
            }
        }
        let rawSends = try objects(raw["sends"], label: "Sends", defaultEmpty: true)
        let rawSidechains = try objects(raw["sidechains"], label: "Sidechains", defaultEmpty: true)
        guard rawSends.count + rawSidechains.count <= 128 else { throw RoutingError.invalid("Routing supports at most 128 sends and sidechains combined.") }
        var routeIDs = Set<String>()
        func route(_ value: [String: Any], allowMissingSource: Bool = false) throws -> (String, String, Tap, Double, Bool) {
            let id = try string(value["id"], label: "Route ID")
            try identifier(id, label: "Route ID")
            guard routeIDs.insert(id).inserted else { throw RoutingError.invalid("Duplicate route ID: \(id).") }
            let source: String
            if allowMissingSource {
                guard let rawSource = value["source"] as? String else { throw RoutingError.invalid("Sidechain source must be an owner ID.") }
                let parts = rawSource.split(separator: ":", omittingEmptySubsequences: false)
                guard parts.count == 2 && (parts[0] == "track" || parts[0] == "bus") else {
                    throw RoutingError.invalid("Sidechain source must reference a track or bus ID.")
                }
                for part in parts.dropFirst() { try identifier(String(part), label: "Sidechain source ID") }
                source = rawSource
            } else { source = try ownerID(value["source"], label: "Route source", allowMaster: false) }
            guard let text = value["tap"] as? String, let tap = Tap(rawValue: text) else { throw RoutingError.invalid("Unknown routing tap.") }
            return (id, source, tap, try number(value["gain"], label: "Route gain", range: 0...2, fallback: 1),
                    try boolean(value["enabled"], label: "Route enabled", fallback: true))
        }
        let sends = try rawSends.map { value -> Send in
            let (id, source, tap, gain, enabled) = try route(value)
            return Send(id: id, source: source, tap: tap, destination: try ownerID(value["destination"], label: "Send destination"), gain: gain, enabled: enabled)
        }
        let sidechains = try rawSidechains.map { value -> Sidechain in
            let (id, source, tap, gain, enabled) = try route(value, allowMissingSource: true)
            guard let destination = value["destination"] as? [String: Any], let signature = destination["layoutSignature"] as? [String: Any] else {
                throw RoutingError.invalid("Sidechains require a destination and saved input layout.")
            }
            let instanceID = try string(destination["instanceID"], label: "Sidechain instance ID")
            try identifier(instanceID, label: "Sidechain instance ID")
            let name = try string(signature["name"], label: "Sidechain input name", allowEmpty: true)
            return Sidechain(id: id, source: source, tap: tap,
                destination: SidechainDestination(instanceID: instanceID,
                    inputBus: try integer(destination["inputBus"], label: "Sidechain input bus", range: 0...31),
                    layoutSignature: LayoutSignature(name: name, channels: try integer(signature["channels"], label: "Sidechain input channels", range: 1...64))),
                gain: gain, enabled: enabled)
        }
        let definition = Definition(enabled: true, mixModel: model, owners: owners, outputs: outputs, sends: sends, sidechains: sidechains)
        if model == .legacy, !buses.isEmpty || !sends.isEmpty || !sidechains.isEmpty || outputs.values.contains(where: { $0 != "master" }) {
            throw RoutingError.invalid("Enable standard routing to use buses, custom sends or sidechains.")
        }
        // Validate ordinary output/send cycles now. Plug-in layouts subsequently
        // add the actual auxiliary dependencies and validate their stage cycles.
        _ = try definition.graph(pluginLayouts: [])
        return definition
    }

    private static func objects(_ value: Any?, label: String, defaultEmpty: Bool) throws -> [[String: Any]] {
        if value == nil, defaultEmpty { return [] }
        guard let result = value as? [[String: Any]] else { throw RoutingError.invalid("\(label) must be an array of objects.") }
        return result
    }
    private static func identifier(_ value: String, label: String) throws {
        let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.")
        guard !value.isEmpty, value.utf8.count <= 128, value.unicodeScalars.allSatisfy({ allowed.contains($0) }) else {
            throw RoutingError.invalid("\(label) must contain 1–128 letters, digits, hyphens, underscores, or periods.")
        }
    }
    private static func text(_ value: String, label: String, allowEmpty: Bool = false) throws {
        guard value.utf8.count <= 512, !value.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
              allowEmpty || !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw RoutingError.invalid("\(label) must be valid non-control text.")
        }
    }
    private static func string(_ value: Any?, label: String, fallback: String? = nil, allowEmpty: Bool = false) throws -> String {
        if value == nil, let fallback { return fallback }
        guard let result = value as? String else { throw RoutingError.invalid("\(label) must be text.") }
        try text(result, label: label, allowEmpty: allowEmpty); return result
    }
    private static func number(_ value: Any?, label: String, range: ClosedRange<Double>, fallback: Double? = nil) throws -> Double {
        if value == nil, let fallback { return fallback }
        guard PayloadValue.numeric(value) != nil else { throw RoutingError.invalid("\(label) must be numeric.") }
        guard let result = PayloadValue.number(value, in: range) else { throw RoutingError.invalid("\(label) is outside its allowed range.") }
        return result
    }
    private static func integer(_ value: Any?, label: String, range: ClosedRange<Int>) throws -> Int {
        let result = try number(value, label: label, range: Double(range.lowerBound)...Double(range.upperBound))
        guard PayloadValue.isWhole(result) else { throw RoutingError.invalid("\(label) must be an integer.") }
        return Int(result)
    }
    private static func boolean(_ value: Any?, label: String, fallback: Bool) throws -> Bool {
        guard let value else { return fallback }
        guard let result = PayloadValue.boolean(value) else { throw RoutingError.invalid("\(label) must be Boolean.") }
        return result
    }
}

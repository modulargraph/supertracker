import Foundation
import CoreFoundation
import TrackerEngine

/// Control-thread mapping from persistent owner/route identities to bounded render
/// destinations. Audio callbacks index `entries`; they never resolve strings.
struct AudioRoutingAutomation {
    enum Kind: String { case mix, send, plugin }
    struct Entry {
        let destination: Int
        let owner: String
        let kind: Kind
        let routeID: String?
        let instanceID: String?
        let requiresExactIdentity: Bool
        let slot: Int?
        let pluginKind: AudioRouting.PluginKind?
        /// Nil for mix/plugin destinations whose events carry their own address.
        let address: UInt64?
    }
    struct Point {
        let pattern: Int
        let destination: Int
        let address: UInt64
        let interpolation: Int
        let row: Double
        let value: Double
    }
    struct DefaultValue {
        let destination: Int
        let address: UInt64
        let value: Double
    }
    struct UnavailableLane {
        let pattern: Int
        let owner: String
        let target: String
        let reason: String
    }
    let entries: [Entry]
    /// Identity of this destination numbering, never 0 while routing is on (0 = no
    /// routed destinations). The engine stamps it on every host event of a project
    /// published with this mapping, and the render table built from it drops events
    /// stamped with another one: adding a bus or insert renumbers the destinations
    /// sorted after it, and the table and the engine project are replaced one after
    /// the other while audio keeps rendering.
    let hostMap: UInt64
    let points: [Point]
    let defaults: [DefaultValue]
    let unavailableLanes: [UnavailableLane]

    /// `plugins` describes desired project slots, including missing/quarantined
    /// processors. Use desiredPluginLayouts below, never the live instance list.
    init(project: [String: Any], definition: AudioRouting.Definition, plugins: [AudioRouting.PluginLayout]) throws {
        guard definition.enabled else { entries = []; hostMap = 0; points = []; defaults = []; unavailableLanes = []; return }
        _ = try definition.graph(pluginLayouts: plugins)
        var entries: [Entry] = [], defaults: [DefaultValue] = [], points: [Point] = [], unavailable: [UnavailableLane] = []
        var mixIDs: [String: Int] = [:], routeIDs: [String: Int] = [:], pluginIDs: [String: Int] = [:]
        let owners = Dictionary(uniqueKeysWithValues: definition.owners.map { ($0.id, $0) })
        let layouts = Dictionary(grouping: plugins, by: \.owner)
        let explicitPluginIDs = Set(try Self.projectPluginDescriptions(project).compactMap { $0["instanceID"] as? String })
        func entry(owner: String, kind: Kind, routeID: String? = nil, instanceID: String? = nil, slot: Int? = nil,
                   pluginKind: AudioRouting.PluginKind? = nil, address: UInt64? = nil, requiresExactIdentity: Bool = false) throws -> Int {
            guard entries.count < 2048 else { throw AudioRouting.RoutingError.invalid("Host automation exceeds 2048 destinations.") }
            let id = entries.count
            entries.append(Entry(destination: id, owner: owner, kind: kind, routeID: routeID, instanceID: instanceID, requiresExactIdentity: requiresExactIdentity, slot: slot, pluginKind: pluginKind, address: address))
            return id
        }
        for owner in definition.owners.sorted(by: { $0.id < $1.id }) {
            let id = try entry(owner: owner.id, kind: .mix); mixIDs[owner.id] = id
            // Legacy projects already apply track/master mix in their sources.
            // Reserve the dense ID, but leave those host strip faders at unity.
            if definition.mixModel == .legacy, owner.kind == .track || owner.kind == .master { continue }
            defaults.append(DefaultValue(destination: id, address: 0, value: owner.volume))
            if owner.kind != .master { defaults.append(DefaultValue(destination: id, address: 1, value: owner.pan)) }
        }
        var routes: [(id: String, owner: String, gain: Double)] = definition.sends.map { ($0.id, $0.source, $0.gain) }
        routes += definition.sidechains.map { ($0.id, $0.source, $0.gain) }
        for route in routes.sorted(by: { $0.id < $1.id }) where owners[route.owner] != nil {
            let id = try entry(owner: route.owner, kind: .send, routeID: route.id, address: 0)
            routeIDs[route.id] = id; defaults.append(DefaultValue(destination: id, address: 0, value: route.gain))
        }
        for plugin in plugins.sorted(by: {
            if $0.owner != $1.owner { return $0.owner < $1.owner }
            if $0.kind != $1.kind { return $0.kind.rawValue < $1.kind.rawValue }
            return $0.slot < $1.slot
        }) {
            let id = try entry(owner: plugin.owner, kind: .plugin, instanceID: plugin.instanceID, slot: plugin.slot, pluginKind: plugin.kind,
                               requiresExactIdentity: explicitPluginIDs.contains(plugin.instanceID))
            pluginIDs[plugin.instanceID] = id
        }
        let patterns = try Self.objects(project["patterns"], label: "Patterns")
        guard patterns.count <= 256 else { throw AudioRouting.RoutingError.invalid("Host automation supports at most 256 patterns.") }
        var patternIDs = Set<Int>()
        for (patternIndex, pattern) in patterns.enumerated() {
            let id = try Self.integer(pattern["id"], label: "Pattern ID", range: 0...255, fallback: patternIndex)
            guard patternIDs.insert(id).inserted else { throw AudioRouting.RoutingError.invalid("Duplicate automation pattern ID.") }
            let length = try Self.integer(pattern["length"], label: "Pattern length", range: 1...256, fallback: 64)
            var laneKeys = Set<String>()
            var explicitValueKeys = Set<String>()
            var nativeMixKeys = Set<String>()
            // Instrument-chain lanes become one lane per track that plays the instrument.
            for lane in InstrumentEffects.expandLanes(try Self.objects(pattern["automation"], label: "Automation lanes"), project: project) {
                // Existing track-index lanes remain owned by AudioProject/C++.
                guard let rawOwner = lane["owner"] else { continue }
                guard let owner = rawOwner as? String, owners[owner] != nil,
                      let target = lane["target"] as? String else {
                    throw AudioRouting.RoutingError.invalid("Host automation requires an existing owner and a target.")
                }
                if definition.mixModel == .legacy, target == "volume" || target == "pan",
                   owners[owner]?.kind == .track || owners[owner]?.kind == .master {
                    throw AudioRouting.RoutingError.invalid("Legacy routing does not support owner-based track or master mix automation. Use existing track lanes or Standard routing.")
                }
                if definition.mixModel == .standard, owners[owner]?.trackIndex != nil,
                   target != "volume", target != "pan", let range=ProjectLayout.nativeAutomationRanges[target] {
                    let nativeKey="native:\(owner):\(target)"
                    guard nativeMixKeys.insert(nativeKey).inserted else { throw AudioRouting.RoutingError.invalid("Duplicate channel automation destination.") }
                    guard ["linear","step"].contains(lane["interpolation"] as? String ?? "linear") else { throw AudioRouting.RoutingError.invalid("Unknown automation interpolation.") }
                    let rawPoints=try Self.objects(lane["points"],label:"Channel automation points")
                    guard rawPoints.count <= 4096 else { throw AudioRouting.RoutingError.invalid("Channel automation exceeds 4096 points.") }
                    var rows=Set<Double>()
                    for point in rawPoints {
                        let row=try Self.number(point["row"],label:"Automation row",range:0...Double(length))
                        _ = try Self.number(point["value"],label:"Automation value",range:range)
                        guard rows.insert(row).inserted else { throw AudioRouting.RoutingError.invalid("Duplicate channel automation point row.") }
                    }
                    continue
                }
                let interpolation: Int
                switch lane["interpolation"] as? String ?? "linear" {
                case "linear": interpolation = 0
                case "step": interpolation = 1
                default: throw AudioRouting.RoutingError.invalid("Unknown host automation interpolation.")
                }
                var destination: Int?, address: UInt64 = 0, range: ClosedRange<Double> = 0...2, missing: String?
                switch target {
                case "volume": destination = mixIDs[owner]
                case "pan":
                    guard owner != "master" else { throw AudioRouting.RoutingError.invalid("Master pan automation is unsupported.") }
                    destination = mixIDs[owner]; address = 1; range = -1...1
                default:
                    let components = target.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
                    if components.count == 2, components[0] == "send", !components[1].isEmpty {
                        if let route = routes.first(where: { $0.id == components[1] }) {
                            guard route.owner == owner else { throw AudioRouting.RoutingError.invalid("Send automation owner does not match its source.") }
                            destination = routeIDs[route.id]
                        } else { missing = "The automated send or sidechain is unavailable." }
                    } else if components.count == 4, components[0] == "plugin",
                              let kind = AudioRouting.PluginKind(rawValue: components[1]),
                              let slot = Int(components[2]), (0..<MixerChannel.insertSlotCount).contains(slot)
                                || owners[owner]?.trackIndex != nil && (0..<MixerChannel.channelSlotCount).contains(slot),
                              let parameter = UInt64(components[3]) {
                        guard kind != .instrument || slot == 0 else { throw AudioRouting.RoutingError.invalid("Invalid instrument automation slot.") }
                        address = parameter; range = 0...1
                        if let plugin = layouts[owner]?.first(where: { $0.kind == kind && $0.slot == slot }) {
                            destination = pluginIDs[plugin.instanceID]
                        } else { missing = "The automated plug-in slot is unavailable." }
                    } else { throw AudioRouting.RoutingError.invalid("Unknown host automation target: \(target).") }
                }
                let rawPoints = try Self.objects(lane["points"], label: "Automation points")
                guard rawPoints.count <= 4096 else { throw AudioRouting.RoutingError.invalid("Host automation supports at most 4096 points per lane.") }
                var validated: [(Double, Double)] = []
                var rows = Set<Double>()
                for point in rawPoints {
                    let row = try Self.number(point["row"], label: "Automation row", range: 0...Double(length))
                    let value = try Self.number(point["value"], label: "Automation value", range: range)
                    guard rows.insert(row).inserted else { throw AudioRouting.RoutingError.invalid("Duplicate host automation point row.") }
                    validated.append((row, value))
                }
                guard let destination else {
                    unavailable.append(UnavailableLane(pattern: id, owner: owner, target: target, reason: missing ?? "The automation destination is unavailable.")); continue
                }
                // Track mix remains on C's existing targets 0/1, where step FX
                // row locks and native defaults already arbitrate values. The
                // caller maps these validated owner lanes to their track index;
                // emitting a second generic lane would overwrite that decision.
                if entries[destination].kind == .mix, owners[owner]?.trackIndex != nil {
                    guard nativeMixKeys.insert("\(destination):\(address)").inserted else {
                        throw AudioRouting.RoutingError.invalid("Duplicate track mix automation destination.")
                    }
                    continue
                }
                guard laneKeys.insert("\(destination):\(address)").inserted, laneKeys.count <= 256 else {
                    throw AudioRouting.RoutingError.invalid("Host automation has duplicate destinations or exceeds 256 lanes per pattern.")
                }
                if !validated.isEmpty { explicitValueKeys.insert("\(destination):\(address)") }
                for (row, value) in validated.sorted(by: { $0.0 < $1.0 }) {
                    points.append(Point(pattern: id, destination: destination, address: address, interpolation: interpolation, row: row, value: value))
                }
            }
            // A removed/empty lane restores its strip value at the next uncached
            // musical frame through the C scheduler. Do not flush host gain queues:
            // they may still hold correctly timed events behind plugin latency.
            // Plug-in baselines are deliberately absent; their current values are
            // owned by the vendor/caller rather than the routing schema.
            for value in defaults {
                let entry = entries[value.destination]
                if entry.kind == .mix, owners[entry.owner]?.trackIndex != nil { continue }
                let key = "\(value.destination):\(value.address)"
                guard !explicitValueKeys.contains(key) else { continue }
                laneKeys.insert(key)
                guard laneKeys.count <= 256 else {
                    throw AudioRouting.RoutingError.invalid("Pattern \(id) exceeds 256 host automation lanes including routing defaults.")
                }
                points.append(Point(pattern: id, destination: value.destination, address: value.address,
                                    interpolation: 1, row: 0, value: value.value))
            }
        }
        self.entries = entries; self.defaults = defaults; self.points = points; unavailableLanes = unavailable
        hostMap = Self.identity(entries)
    }

    /// FNV-1a over every field that decides where a destination index delivers.
    static func identity(_ entries: [Entry]) -> UInt64 {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        func mix(_ text: String) {
            for byte in text.utf8 { hash = (hash ^ UInt64(byte)) &* 0x0000_0100_0000_01b3 }
            hash = (hash ^ 0x1f) &* 0x0000_0100_0000_01b3
        }
        for entry in entries {
            mix("\(entry.destination)"); mix(entry.owner); mix(entry.kind.rawValue); mix(entry.routeID ?? ""); mix(entry.instanceID ?? "")
            mix(entry.requiresExactIdentity ? "1" : "0"); mix("\(entry.slot ?? -1)"); mix(entry.pluginKind?.rawValue ?? ""); mix("\(entry.address.map { String($0) } ?? "")")
        }
        return hash == 0 ? 1 : hash
    }

    /// Match project plug-in extraction without requiring an instantiated vendor.
    /// Explicit audioPlugins, even an empty array, is authoritative. Legacy track
    /// and instrument descriptors are considered only when that field is absent.
    static func desiredPluginLayouts(project: [String: Any], definition: AudioRouting.Definition) throws -> [AudioRouting.PluginLayout] {
        guard definition.enabled else { return [] }
        let descriptions = try projectPluginDescriptions(project)
        let ownerOrder = definition.owners.map(\.id).sorted()
        let result = try descriptions.map { plugin -> AudioRouting.PluginLayout in
            let owner: String
            if let rawOwner = plugin["owner"] {
                guard let id = rawOwner as? String, definition.owner(id) != nil else {
                    throw AudioRouting.RoutingError.invalid("Plug-in owner \(rawOwner as? String ?? "?") does not exist.")
                }
                owner = id
            } else {
                let track = try integer(plugin["track"], label: "Plug-in track", range: 0...15, fallback: 0)
                guard let found = definition.owners.first(where: { $0.trackIndex == track }) else { throw AudioRouting.RoutingError.invalid("Plug-in track does not exist.") }
                owner = found.id
            }
            guard let kind = AudioRouting.PluginKind(rawValue: plugin["kind"] as? String ?? "effect") else { throw AudioRouting.RoutingError.invalid("Unknown plug-in slot kind.") }
            let slot = try integer(plugin["slot"], label: "Plug-in slot", range: 0...(MixerChannel.channelSlotCount - 1), fallback: 0)
            let instanceID: String
            if let rawID = plugin["instanceID"] {
                guard let value = rawID as? String else { throw AudioRouting.RoutingError.invalid("Plug-in instance ID must be text.") }
                instanceID = value
            } else {
                // Bounded collision-free slot identity within this definition;
                // automation dispatch still uses owner/kind/slot, never this ID.
                instanceID = "desired-\(ownerOrder.firstIndex(of: owner)!)-\(kind.rawValue)-\(slot)"
            }
            return AudioRouting.PluginLayout(instanceID: instanceID, owner: owner, kind: kind, slot: slot)
        }
        _ = try definition.graph(pluginLayouts: result)
        return result
    }

    /// Persistent descriptors including missing instances; the authoritative array
    /// remains authoritative even when empty. Shared by lane and step resolution.
    /// Instrument effects appear as their per-track instances (InstrumentEffects.expand).
    static func projectPluginDescriptions(_ project: [String: Any]) throws -> [[String: Any]] {
        if project["audioPlugins"] != nil {
            return InstrumentEffects.expand(try objects(project["audioPlugins"], label: "Project plug-ins"), project: project)
        } else {
            let tracks = try objects(project["tracks"], label: "Tracks")
            let instruments = try objects(project["instruments"], label: "Instruments")
            var collected: [[String: Any]] = []
            for (track, item) in tracks.enumerated() {
                let instrument = try integer(item["instrument"], label: "Track instrument", range: 0...63, fallback: 0)
                if instruments.indices.contains(instrument), var plugin = instruments[instrument]["plugin"] as? [String: Any], plugin["component"] != nil {
                    plugin["track"] = track; plugin["kind"] = "instrument"; plugin["slot"] = 0; collected.append(plugin)
                }
                if let rawPlugins = item["plugins"] {
                    guard let effects = rawPlugins as? [Any], effects.count <= MixerChannel.insertSlotCount else { throw AudioRouting.RoutingError.invalid("Track plug-ins must be an array of at most \(MixerChannel.insertSlotCount) inserts.") }
                    for (slot, raw) in effects.enumerated() {
                        guard var plugin = raw as? [String: Any], plugin["component"] != nil else { continue }
                        plugin["track"] = track; plugin["kind"] = "effect"; plugin["slot"] = slot; collected.append(plugin)
                    }
                }
            }
            return collected
        }
    }

    /// Call inside the caller's project update after AudioProject has cleared old
    /// lanes. This does not begin/commit an update, mutate DSP, or reset transport.
    func apply(to engine: OpaquePointer, skipping reused: Set<Int> = []) {
        for point in points where !reused.contains(point.pattern) {
            st_engine_add_host_automation_point(engine, Int32(point.pattern), Int32(point.destination), point.address,
                                                Int32(point.interpolation), point.row, point.value)
        }
    }

    /// Plug-in defaults come from the caller's current parameter snapshot, since
    /// the routing schema stores no parameter tree. Values are normalized like lanes.
    func defaults(pluginValues: [String: [UInt64: Double]]) throws -> [DefaultValue] {
        var result = defaults
        for entry in entries where entry.kind == .plugin {
            guard let instanceID = entry.instanceID, let values = pluginValues[instanceID] else { continue }
            for address in values.keys.sorted() {
                let value = values[address]!
                guard value.isFinite, (0...1).contains(value) else { throw AudioRouting.RoutingError.invalid("Plug-in automation defaults must be normalized finite values.") }
                result.append(DefaultValue(destination: entry.destination, address: address, value: value))
            }
        }
        return result
    }

    private static func objects(_ value: Any?, label: String) throws -> [[String: Any]] {
        guard let value else { return [] }
        guard let result = value as? [[String: Any]] else { throw AudioRouting.RoutingError.invalid("\(label) must be an array of objects.") }
        return result
    }
    private static func number(_ value: Any?, label: String, range: ClosedRange<Double>) throws -> Double {
        guard let result = PayloadValue.number(value, in: range) else {
            throw AudioRouting.RoutingError.invalid("\(label) must be a finite number in \(range).")
        }
        return result
    }
    private static func integer(_ value: Any?, label: String, range: ClosedRange<Int>, fallback: Int) throws -> Int {
        guard let value else { return fallback }
        let number = try number(value, label: label, range: Double(range.lowerBound)...Double(range.upperBound))
        guard PayloadValue.isWhole(number) else { throw AudioRouting.RoutingError.invalid("\(label) must be an integer.") }
        return Int(number)
    }
}

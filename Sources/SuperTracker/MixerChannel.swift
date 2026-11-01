import Foundation

/// Insert slots per mixer channel: tracks, buses, kit Sounds, kit Groups and Master
/// all accept effect slots 0..<insertSlotCount. Raising this never invalidates a
/// saved project (older projects simply use fewer slots); every native limit
/// (endpoint indexing, automation, step FX, freeze, import) derives from it.
/// `MaschineGroupPluginState`, the engine's `ST_MAX_INSERTS` and the web UI's
/// `INSERT_SLOTS` constants mirror this value.
///
/// A track channel also runs its instrument's insert chain (see `InstrumentEffects`):
/// instrument slot n occupies channel slot `instrumentSlotBase + n`, and those slots
/// render ahead of the track's own inserts (`chainOrder`). Only the host addresses
/// them; projects and the interface keep instrument slots 0..<insertSlotCount.
enum MixerChannel {
    static let insertSlotCount = 16
    /// First channel slot of the instrument chain on a track channel.
    static let instrumentSlotBase = insertSlotCount
    /// Slots a channel can hold in the host: its own inserts and its instrument's.
    static let channelSlotCount = insertSlotCount * 2

    /// The plug-in rack's render endpoints per channel: the instrument at 0 and
    /// channel slot n at n + 1.
    static let endpointStride = channelSlotCount + 1
    /// The plug-in rack's automation time offsets per track: instrument output time
    /// at 0, instrument parameter time at 1 (always zero) and channel slot n's input
    /// time at n + 2. The common send time follows the last track.
    static let offsetStride = channelSlotCount + 2

    @inline(__always) static func endpointIndex(channel: Int, kind: String, slot: Int) -> Int {
        channel * endpointStride + (kind == "instrument" ? 0 : slot + 1)
    }
    @inline(__always) static func offsetIndex(track: Int, slot: Int) -> Int { track * offsetStride + slot + 2 }

    /// Whether a channel slot belongs to the instrument chain.
    @inline(__always) static func isInstrumentSlot(_ slot: Int) -> Bool { slot >= instrumentSlotBase }
    /// Signal-order key of a channel slot: the instrument chain first, then the channel's own inserts.
    @inline(__always) static func chainOrder(_ slot: Int) -> Int { slot >= instrumentSlotBase ? slot - channelSlotCount : slot }
    /// Channel slots in signal order.
    static let slotsInChainOrder: [Int] = Array(instrumentSlotBase..<channelSlotCount) + Array(0..<insertSlotCount)
}

/// Instrument insert chains. An instrument's effects are project plug-in descriptors owned by
/// `instrument:<index>` (kind `effect`, slots 0..<16). The host runs one instance of the chain
/// on every track that plays the instrument (the track's assigned instrument), at the start
/// of that track's strip: instrument slot n becomes channel slot 16 + n of the track's owner,
/// with a per-track instance ID. Voices of other instruments a track triggers by step share
/// the track's channel, so they pass through the assigned instrument's chain too.
enum InstrumentEffects {
    static let ownerPrefix = "instrument:"
    /// Keys an expanded (per-track) descriptor carries back to its instrument descriptor.
    static let ownerKey = "instrumentOwner", slotKey = "instrumentSlot", instanceKey = "instrumentInstanceID"

    static func owner(_ instrument: Int) -> String { ownerPrefix + String(instrument) }
    /// The instrument index of an `instrument:<index>` owner.
    static func instrumentIndex(_ owner: Any?) -> Int? {
        guard let text = owner as? String, text.hasPrefix(ownerPrefix) else { return nil }
        let digits = text.dropFirst(ownerPrefix.count)
        guard !digits.isEmpty, digits.allSatisfy({ $0.isASCII && $0.isNumber }), let index = Int(digits), index < 4096 else { return nil }
        return index
    }
    static func isInstrumentDescriptor(_ descriptor: [String: Any]) -> Bool { instrumentIndex(descriptor["owner"]) != nil }
    static func isExpanded(_ descriptor: [String: Any]) -> Bool { descriptor[ownerKey] != nil }

    /// Owner (`track:<routingID>`) or flat track index of every track whose assigned instrument is `instrument`.
    static func tracks(playing instrument: Int, project: [String: Any]) -> [(index: Int, owner: String?)] {
        let routed = project["routing"] != nil
        return (project["tracks"] as? [[String: Any]] ?? []).enumerated().compactMap { index, track in
            guard (integer(track["instrument"]) ?? 0) == instrument else { return nil }
            if routed { guard let id = track["routingID"] as? String, !id.isEmpty else { return nil }; return (index, "track:" + id) }
            return (index, nil)
        }
    }

    /// The per-track descriptor of one instrument effect on one track.
    static func expansion(_ descriptor: [String: Any], instrument: Int, track: (index: Int, owner: String?)) -> [String: Any] {
        let slot = integer(descriptor["slot"]) ?? 0
        var item = descriptor
        item.removeValue(forKey: "owner"); item.removeValue(forKey: "track")
        if let owner = track.owner { item["owner"] = owner } else { item["track"] = track.index }
        item["kind"] = "effect"; item["slot"] = MixerChannel.instrumentSlotBase + slot
        let base = (descriptor["instanceID"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        if let base { item[instanceKey] = base }
        let suffix = track.owner.map { String($0.dropFirst("track:".count)) } ?? String(track.index)
        item["instanceID"] = String(((base ?? "ifx-\(instrument)-\(slot)") + "-on-" + suffix).prefix(128))
        item[ownerKey] = owner(instrument); item[slotKey] = slot
        return item
    }

    /// Replaces each instrument effect by one descriptor per track that plays the instrument,
    /// in place. `keepUnused` keeps the effects of instruments no track plays (for snapshots).
    static func expand(_ descriptors: [[String: Any]], project: [String: Any], keepUnused: Bool = false) -> [[String: Any]] {
        guard descriptors.contains(where: isInstrumentDescriptor) else { return descriptors }
        var cache: [Int: [(index: Int, owner: String?)]] = [:]
        var result: [[String: Any]] = []
        for descriptor in descriptors {
            guard let instrument = instrumentIndex(descriptor["owner"]) else { result.append(descriptor); continue }
            guard descriptor["kind"] as? String ?? "effect" == "effect", let slot = integer(descriptor["slot"]),
                  (0..<MixerChannel.insertSlotCount).contains(slot) else { continue }
            let tracks = cache[instrument] ?? tracks(playing: instrument, project: project)
            cache[instrument] = tracks
            if tracks.isEmpty { if keepUnused { result.append(descriptor) }; continue }
            for track in tracks { result.append(expansion(descriptor, instrument: instrument, track: track)) }
        }
        return result
    }

    /// The instrument descriptor an expanded descriptor (live or desired) stands for.
    static func collapse(_ expanded: [String: Any]) -> [String: Any] {
        var item = expanded
        item["owner"] = expanded[ownerKey]; item["slot"] = expanded[slotKey]; item["kind"] = "effect"
        if let base = expanded[instanceKey] { item["instanceID"] = base } else { item.removeValue(forKey: "instanceID") }
        for key in ["track", ownerKey, slotKey, instanceKey] { item.removeValue(forKey: key) }
        return item
    }

    /// Instrument automation lanes (`owner: instrument:<n>`, `plugin:effect:<slot>:<address>`) as one
    /// lane per track that plays the instrument, addressed to that track's instrument chain slot.
    static func expandLanes(_ lanes: [[String: Any]], project: [String: Any]) -> [[String: Any]] {
        guard lanes.contains(where: { instrumentIndex($0["owner"]) != nil }) else { return lanes }
        var result: [[String: Any]] = []
        for lane in lanes {
            guard let instrument = instrumentIndex(lane["owner"]) else { result.append(lane); continue }
            let parts = (lane["target"] as? String ?? "").split(separator: ":", omittingEmptySubsequences: false).map(String.init)
            guard parts.count == 4, parts[0] == "plugin", parts[1] == "effect", let slot = Int(parts[2]),
                  (0..<MixerChannel.insertSlotCount).contains(slot) else { continue }
            for track in tracks(playing: instrument, project: project) {
                guard let owner = track.owner else { continue }
                var item = lane
                item["owner"] = owner; item.removeValue(forKey: "track")
                item["target"] = "plugin:effect:\(MixerChannel.instrumentSlotBase + slot):\(parts[3])"
                result.append(item)
            }
        }
        return result
    }

    private static func integer(_ value: Any?) -> Int? {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
        let double = number.doubleValue
        guard double.isFinite, double.rounded(.towardZero) == double, abs(double) < 1e9 else { return nil }
        return Int(double)
    }
}

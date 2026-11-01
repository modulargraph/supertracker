import Foundation

/// Sixteen inserts per channel: the plug-in rack's render-endpoint and automation-offset
/// layouts must give every (channel, kind, slot) its own index inside the allocated store.
@main
struct MixerChannelTests {
    static func main() {
        var checks = 0
        func check(_ condition: Bool, _ message: String) { precondition(condition, message); checks += 1 }
        check(MixerChannel.insertSlotCount == 16, "A channel holds sixteen inserts")
        // AudioPluginRack: 16 source channels + 16 audio buses + master.
        let trackCount = 16, stripCapacity = trackCount + 17
        let endpointCapacity = stripCapacity * MixerChannel.endpointStride
        var seen = Set<Int>()
        for channel in 0..<stripCapacity {
            let instrument = MixerChannel.endpointIndex(channel: channel, kind: "instrument", slot: 0)
            check(seen.insert(instrument).inserted, "Instrument endpoint of channel \(channel) collides")
            for slot in 0..<MixerChannel.channelSlotCount {
                let index = MixerChannel.endpointIndex(channel: channel, kind: "effect", slot: slot)
                check((0..<endpointCapacity).contains(index), "Endpoint \(channel)/\(slot) lies outside the store")
                check(seen.insert(index).inserted, "Endpoint of channel \(channel) slot \(slot) collides")
                if channel + 1 < stripCapacity {
                    check(index < MixerChannel.endpointIndex(channel: channel + 1, kind: "instrument", slot: 0), "Channel \(channel) slot \(slot) reaches into channel \(channel + 1)")
                }
            }
        }
        check(seen.count == endpointCapacity, "The endpoint store is exactly as large as its layout")
        // The former nine-endpoint stride sent track 0 slot 13 to track 1 slot 4.
        check(0 * 9 + 13 + 1 == 1 * 9 + 4 + 1, "Regression premise")
        check(MixerChannel.endpointIndex(channel: 0, kind: "effect", slot: 13) != MixerChannel.endpointIndex(channel: 1, kind: "effect", slot: 4), "Track 0 slot 13 collides with track 1 slot 4")
        var offsets = Set<Int>()
        for track in 0..<trackCount {
            check(offsets.insert(track * MixerChannel.offsetStride).inserted && offsets.insert(track * MixerChannel.offsetStride + 1).inserted, "Instrument offsets of track \(track) collide")
            for slot in 0..<MixerChannel.channelSlotCount {
                let index = MixerChannel.offsetIndex(track: track, slot: slot)
                check(index < trackCount * MixerChannel.offsetStride, "Offset \(track)/\(slot) overlaps the common send time")
                check(offsets.insert(index).inserted, "Automation offset of track \(track) slot \(slot) collides")
            }
        }
        check(!offsets.contains(trackCount * MixerChannel.offsetStride), "The common send time has its own entry")
        // Instrument chains: slots 16..<32 of a track channel, rendered first.
        check(MixerChannel.channelSlotCount == 32 && MixerChannel.instrumentSlotBase == 16, "Track channels hold 16 instrument-chain slots")
        check(MixerChannel.slotsInChainOrder == Array(16..<32) + Array(0..<16), "The instrument chain renders first")
        check((0..<32).sorted { MixerChannel.chainOrder($0) < MixerChannel.chainOrder($1) } == MixerChannel.slotsInChainOrder, "chainOrder sorts like slotsInChainOrder")
        check(InstrumentEffects.instrumentIndex("instrument:12") == 12 && InstrumentEffects.instrumentIndex("instrument:") == nil
              && InstrumentEffects.instrumentIndex("instrument:-1") == nil && InstrumentEffects.instrumentIndex("track:a") == nil, "instrument owners")
        let project: [String: Any] = ["routing": ["version": 1], "tracks": [["routingID": "a", "instrument": 1], ["routingID": "b"], ["routingID": "c", "instrument": 1]]]
        let descriptors: [[String: Any]] = [["owner": "track:a", "kind": "effect", "slot": 0, "component": "x"],
            ["owner": "instrument:1", "kind": "effect", "slot": 2, "component": "stfx:eq", "instanceID": "eq", "state": "s"],
            ["owner": "instrument:2", "kind": "effect", "slot": 0, "component": "stfx:eq", "instanceID": "unused"]]
        let expanded = InstrumentEffects.expand(descriptors, project: project)
        check(expanded.map { $0["owner"] as? String ?? "" } == ["track:a", "track:a", "track:c"], "one instance per track that plays the instrument, in place")
        check(expanded[1]["slot"] as? Int == 18 && expanded[1]["instanceID"] as? String == "eq-on-a" && expanded[2]["instanceID"] as? String == "eq-on-c"
              && expanded[1]["state"] as? String == "s" && expanded[1][InstrumentEffects.ownerKey] as? String == "instrument:1", "expanded descriptor")
        check(InstrumentEffects.expand(descriptors, project: project, keepUnused: true).last?["instanceID"] as? String == "unused", "keepUnused keeps an unplayed instrument's effects")
        let collapsed = InstrumentEffects.collapse(expanded[2])
        check(collapsed["owner"] as? String == "instrument:1" && collapsed["slot"] as? Int == 2 && collapsed["instanceID"] as? String == "eq" && collapsed["track"] == nil
              && collapsed[InstrumentEffects.ownerKey] == nil, "collapse restores the instrument descriptor")
        let flat = InstrumentEffects.expand(descriptors, project: ["tracks": [["instrument": 1], [:]]])
        check(flat.count == 2 && flat[1]["track"] as? Int == 0 && flat[1]["owner"] == nil && flat[1]["instanceID"] as? String == "eq-on-0", "flat projects use track indices")
        let lanes = InstrumentEffects.expandLanes([["owner": "instrument:1", "target": "plugin:effect:2:7", "points": []], ["owner": "instrument:1", "target": "volume"],
                                                   ["track": 0, "target": "volume"]], project: project)
        check(lanes.map { "\($0["owner"] ?? "-") \($0["target"] ?? "")" } == ["track:a plugin:effect:18:7", "track:c plugin:effect:18:7", "- volume"], "instrument lanes per track")
        print("PASS \(checks) mixer channel layout checks")
    }
}

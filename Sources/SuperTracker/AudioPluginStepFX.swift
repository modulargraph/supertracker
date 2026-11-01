import Foundation
import CoreFoundation
import TrackerEngine

/// Resolve persistent plug-in identities before publishing the immutable engine
/// project. No strings or plug-in lookups cross into the render callback.
struct AudioPluginStepFX {
    struct FlatDestination {
        let destination: Int, track: Int, slot: Int
        let instanceID: String
    }
    struct Binding {
        let pattern: Int, track: Int, row: Int, column: Int, slot: Int, destination: Int
        let address: UInt64
        let value: Double
    }
    struct UnavailableFX {
        let pattern: Int, track: Int, row: Int, column: Int
        let instanceID: String, address: String, reason: String
        var dictionary: [String: Any] {
            ["pattern": pattern, "track": track, "row": row, "column": column,
             "instanceID": instanceID, "address": address, "reason": reason]
        }
    }
    let bindings: [Binding]
    let unavailableFXs: [UnavailableFX]
    let flatDestinations: [FlatDestination]

    init(project: [String: Any], definition: AudioRouting.Definition,
         hostEntries: [AudioRoutingAutomation.Entry]) throws {
        let descriptors = try AudioRoutingAutomation.projectPluginDescriptions(project)
        let byID = Dictionary(grouping: descriptors.compactMap { descriptor -> (String, [String: Any])? in
            guard let id = descriptor["instanceID"] as? String, !id.isEmpty else { return nil }
            return (id, descriptor)
        }, by: { $0.0 })
        let hostByID = Dictionary(grouping: hostEntries.filter { $0.kind == .plugin }, by: { $0.instanceID ?? "" })
        func location(_ descriptor: [String: Any]) -> (owner: AudioRouting.Owner, slot: Int)? {
            let owner: AudioRouting.Owner?
            if let ownerID = descriptor["owner"] as? String { owner = definition.owner(ownerID) }
            else {
                let index = (descriptor["track"] as? NSNumber)?.intValue ?? 0
                owner = definition.owners.first { $0.trackIndex == index }
            }
            guard let owner, owner.trackIndex != nil,
                  let kind = descriptor["kind"] as? String, ["instrument", "effect"].contains(kind),
                  let rawSlot = descriptor["slot"] as? NSNumber, CFGetTypeID(rawSlot) != CFBooleanGetTypeID(),
                  rawSlot.doubleValue.rounded(.towardZero) == rawSlot.doubleValue,
                  (0..<MixerChannel.insertSlotCount).contains(rawSlot.intValue)
                    || descriptor[InstrumentEffects.ownerKey] != nil && (0..<MixerChannel.channelSlotCount).contains(rawSlot.intValue),
                  kind != "instrument" || rawSlot.intValue == 0,
                  descriptor["component"] as? String != nil else { return nil }
            return (owner, kind == "instrument" ? -1 : rawSlot.intValue)
        }
        // Flat projects need the same identity-safe endpoint indirection as routed
        // projects. Number desired descriptors, never the set of live processors:
        // asynchronous replacement cannot turn a pending lock into an old-slot edit.
        var flat: [FlatDestination] = []
        if !definition.enabled {
            for id in byID.keys.sorted() {
                guard let matches = byID[id], matches.count == 1, id.count <= 128,
                      let location = location(matches[0].1) else { continue }
                guard flat.count < 512 else { throw AudioRouting.RoutingError.invalid("Plug-in step FX exceeds 512 destinations.") }
                flat.append(FlatDestination(destination: flat.count, track: location.owner.trackIndex!, slot: location.slot, instanceID: id))
            }
        }
        let flatIDs = Dictionary(uniqueKeysWithValues: flat.map { ($0.instanceID, $0.destination) })
        // A lock on an instrument effect names the instrument's instance; on each track it addresses
        // that track's instance of the chain.
        var instrumentInstances: [String: [String: Any]] = [:]
        for descriptor in descriptors {
            guard let base = descriptor[InstrumentEffects.instanceKey] as? String, let location = location(descriptor),
                  let track = location.owner.trackIndex else { continue }
            instrumentInstances["\(base)#\(track)"] = descriptor
        }
        var bindings: [Binding] = [], unavailable: [UnavailableFX] = []
        let patterns = project["patterns"] as? [[String: Any]] ?? []
        for (patternIndex, pattern) in patterns.prefix(256).enumerated() {
            let patternID = (pattern["id"] as? NSNumber)?.intValue ?? patternIndex
            let rows = pattern["steps"] as? [[Any]] ?? []
            for (track, steps) in rows.prefix(ProjectLayout.trackSanityLimit).enumerated() {
                for (row, raw) in steps.prefix(256).enumerated() {
                    guard let step = raw as? [String: Any] else { continue }
                    for (column, key) in ["fx1", "fx2"].enumerated() {
                        guard let fx = step[key] as? [String: Any], fx["type"] as? String == "plugin" else { continue }
                        // JSON numbers lose integer precision above 2^53. Reject
                        // numeric, signed, padded and overflowing addresses rather
                        // than silently automating parameter zero or a neighbor.
                        guard let addressText = fx["address"] as? String,
                              let address = UInt64(addressText), String(address) == addressText else {
                            throw AudioRouting.RoutingError.invalid("Plug-in step FX address must be an exact UInt64 decimal string.")
                        }
                        guard let value = fx["value"] as? NSNumber, CFGetTypeID(value) != CFBooleanGetTypeID(),
                              value.doubleValue.isFinite, (0...1).contains(value.doubleValue) else {
                            throw AudioRouting.RoutingError.invalid("Plug-in step FX value must be a finite normalized number from 0 to 1.")
                        }
                        let lockedID = fx["instanceID"] as? String ?? ""
                        let instanceID = instrumentInstances["\(lockedID)#\(track)"]?["instanceID"] as? String ?? lockedID
                        func missing(_ reason: String) {
                            unavailable.append(UnavailableFX(pattern: patternID, track: track, row: row, column: column,
                                                             instanceID: instanceID, address: addressText, reason: reason))
                        }
                        guard !instanceID.isEmpty, instanceID.count <= 128,
                              let matches = byID[instanceID], matches.count == 1 else {
                            missing("The step FX plug-in instance is missing or its identity is ambiguous."); continue
                        }
                        let descriptor = matches[0].1
                        guard let location = location(descriptor) else {
                            missing("The step FX plug-in descriptor is unavailable."); continue
                        }
                        guard location.owner.trackIndex == track else {
                            missing("The step FX plug-in belongs to a different track. Choose a parameter on this track."); continue
                        }
                        let slot = location.slot
                        var destination = flatIDs[instanceID] ?? -1
                        if definition.enabled {
                            guard let entries = hostByID[instanceID], entries.count == 1,
                                  entries[0].owner == location.owner.id, entries[0].slot == max(0, slot),
                                  entries[0].pluginKind?.rawValue == (slot < 0 ? "instrument" : "effect") else {
                                missing("The step FX plug-in routing destination is unavailable."); continue
                            }
                            destination = entries[0].destination
                        }
                        bindings.append(Binding(pattern: patternID, track: track, row: row, column: column,
                                                slot: slot, destination: destination, address: address, value: value.doubleValue))
                    }
                }
            }
        }
        self.bindings = bindings; unavailableFXs = unavailable; flatDestinations = flat
    }

    /// Call after all ordinary steps have been installed, within the same project
    /// update. The C setter validates bounds and never changes the supplied address.
    func apply(to engine: OpaquePointer, skipping reused: Set<Int> = []) {
        for binding in bindings where !reused.contains(binding.pattern) {
            _ = st_engine_set_plugin_step_fx(engine, Int32(clamping: binding.pattern), Int32(binding.track),
                Int32(binding.row), Int32(binding.column), Int32(binding.slot), Int32(binding.destination),
                binding.address, binding.value)
        }
    }
}

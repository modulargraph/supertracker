import Foundation
import CoreFoundation

/// The kit file layout (.stkit packages and Maschine group imports): one kit of sixteen
/// Sound slots with its group bus, read and written as a small project. Projects no
/// longer contain kits: kits load as track groups (KitGroups), and older projects with
/// kit lanes are converted on open (ProjectMigration). This model validates and exports
/// the file layout only.
struct KitProject {
    static let maximumKits = 4
    static let soundsPerKit = 16
    struct Invalid: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    struct Sound {
        let owner: String
        let kitID: String
        let soundID: String
        let instrument: [String: Any]
        let channel: [String: Any]
        let groupOwner: String
        let name: String
        let pad: Int
    }
    struct Lane {
        let index: Int
        let kitID: String
        let muted: Bool
        let defaultSoundID: String?
    }
    let sounds: [Sound]
    let lanes: [Lane]
    let kits: [[String: Any]]
    var isEmpty: Bool { kits.isEmpty }

    init(project: [String: Any]) throws {
        guard project["kits"] == nil || project["kits"] is [[String: Any]] else {
            throw Invalid(message: "Kits must be a list of kit definitions.")
        }
        kits = project["kits"] as? [[String: Any]] ?? []
        guard kits.count <= Self.maximumKits else { throw Invalid(message: "A project supports up to four kits, each with sixteen Sounds.") }
        var foundSounds: [Sound] = [], foundLanes: [Lane] = []
        var kitIDs = Set<String>(), groupIDs = Set<String>(), soundSets: [String: Set<String>] = [:]
        for kit in kits {
            let kitID = try Self.identifier(kit["id"], label: "Kit ID")
            let busID = try Self.identifier(kit["busID"], label: "Kit group ID")
            guard kitIDs.insert(kitID).inserted, groupIDs.insert(busID).inserted else {
                throw Invalid(message: "Each kit needs a unique ID and its own group bus.")
            }
            guard let slots = kit["sounds"] as? [[String: Any]], slots.count == Self.soundsPerKit else {
                throw Invalid(message: "A kit must contain exactly sixteen Sound slots, including empty slots.")
            }
            var ids = Set<String>()
            for (pad, slot) in slots.enumerated() {
                let id = try Self.identifier(slot["id"], label: "Sound ID")
                guard ids.insert(id).inserted else { throw Invalid(message: "Sound IDs must be unique within their kit.") }
                guard var instrument = slot["instrument"] as? [String: Any] else { throw Invalid(message: "Every Sound needs an instrument definition.") }
                if let raw = slot["chokeGroup"] ?? instrument["chokeGroup"] {
                    guard let number = raw as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(),
                          number.doubleValue.isFinite, number.doubleValue.rounded() == number.doubleValue,
                          (0...16).contains(number.intValue) else { throw Invalid(message: "A Sound choke group must be an integer from 0 to 16.") }
                    instrument["chokeGroup"] = number.intValue
                }
                guard instrument["zones"] == nil || instrument["zones"] is [[String: Any]] else { throw Invalid(message: "Sound sample zones must be a list of zone definitions.") }
                guard slot["channel"] == nil || slot["channel"] is [String: Any] else { throw Invalid(message: "Sound channel controls must be an object.") }
                guard (instrument["zones"] as? [Any] ?? []).count <= 128 else { throw Invalid(message: "A Sound supports up to 128 sample zones.") }
                let channel = slot["channel"] as? [String: Any] ?? [:]
                for (key, range) in [("volume", 0.0...2.0), ("pan", -1.0...1.0), ("delaySend", 0.0...1.0), ("reverbSend", 0.0...1.0)] {
                    if let raw = channel[key] {
                        guard let number = raw as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite, range.contains(number.doubleValue) else {
                            throw Invalid(message: "Sound \(key) is outside its supported range.")
                        }
                    }
                }
                foundSounds.append(Sound(owner: "sound:\(kitID):\(id)", kitID: kitID, soundID: id,
                    instrument: instrument, channel: channel,
                    groupOwner: "bus:\(busID)", name: slot["name"] as? String ?? "Sound \(pad + 1)", pad: pad))
            }
            soundSets[kitID] = ids
        }
        for (index, track) in (project["tracks"] as? [[String: Any]] ?? []).enumerated() {
            guard track["kind"] as? String == "kit" else { continue }
            guard let id = track["kitID"] as? String, let valid = soundSets[id] else { throw Invalid(message: "A kit lane refers to a kit that is missing.") }
            if let raw = track["soundID"], !(raw is String) { throw Invalid(message: "A kit lane Sound ID must be text.") }
            let defaultID = track["soundID"] as? String
            if let defaultID, !valid.contains(defaultID) { throw Invalid(message: "A kit lane's selected Sound is missing.") }
            foundLanes.append(Lane(index: index, kitID: id, muted: track["sequencerMute"] as? Bool ?? false, defaultSoundID: defaultID))
        }
        for pattern in project["patterns"] as? [[String: Any]] ?? [] {
            let steps = pattern["steps"] as? [[Any]] ?? []
            for lane in foundLanes where steps.indices.contains(lane.index) {
                for cell in steps[lane.index] {
                    guard let cell = cell as? [String: Any] else { continue }
                    if let raw = cell["kitEvents"], !(raw is [[String: Any]]) { throw Invalid(message: "A kit cell's extra hits must be a list.") }
                    let extra = cell["kitEvents"] as? [[String: Any]] ?? []
                    guard extra.count <= 7, extra.allSatisfy({ $0["kitEvents"] == nil }) else { throw Invalid(message: "A kit cell supports up to eight hits without nested hit lists.") }
                    for hit in [cell] + extra {
                        if let raw = hit["soundID"], !(raw is String) { throw Invalid(message: "A kit hit Sound ID must be text.") }
                        if let id = hit["soundID"] as? String, soundSets[lane.kitID]?.contains(id) != true {
                            throw Invalid(message: "A pattern references a Sound that is missing from its kit.")
                        }
                    }
                }
            }
        }
        sounds = foundSounds
        lanes = foundLanes
    }

    func sound(kitID: String, soundID: String) -> Sound? { sounds.first { $0.kitID == kitID && $0.soundID == soundID } }
    func lane(_ index: Int) -> Lane? { lanes.first { $0.index == index } }
    func sound(lane index: Int, step: [String: Any]) -> Sound? {
        guard let lane = lane(index), let id = step["soundID"] as? String ?? lane.defaultSoundID else { return nil }
        return sound(kitID: lane.kitID, soundID: id)
    }
    private static func identifier(_ value: Any?, label: String) throws -> String {
        guard let id = value as? String, !id.isEmpty, id.utf8.count <= 80,
              id.unicodeScalars.allSatisfy({ CharacterSet.alphanumerics.contains($0) || "_-".unicodeScalars.contains($0) }) else {
            throw Invalid(message: "\(label) must be a short, stable identifier containing letters, numbers, underscores or hyphens.")
        }
        return id
    }
}

extension KitProject {
    /// Save only this kit and the buses it feeds. External sidechain inputs are
    /// explicitly reported because a reusable kit cannot own another track.
    static func libraryProject(_ project: [String: Any], kitID: String) throws -> (project: [String: Any], metadata: [String: Any], warnings: [String]) {
        let model = try KitProject(project: project)
        guard let kit = model.kits.first(where: { $0["id"] as? String == kitID }), let busID = kit["busID"] as? String else { throw Invalid(message: "Select a kit to save.") }
        let routing = project["routing"] as? [String: Any] ?? [:]
        let buses = routing["buses"] as? [[String: Any]] ?? []
        let outputs = routing["outputs"] as? [String: Any] ?? [:]
        let sends = routing["sends"] as? [[String: Any]] ?? []
        var owners = Set(model.sounds.filter { $0.kitID == kitID }.map(\.owner))
        owners.insert("bus:" + busID)
        let busOwners = Set(buses.compactMap { ($0["id"] as? String).map { "bus:" + $0 } })
        var changed = true
        while changed {
            let old = owners
            for owner in old {
                if let destination = outputs[owner] as? String, busOwners.contains(destination) { owners.insert(destination) }
                for send in sends where send["source"] as? String == owner {
                    if let destination = send["destination"] as? String, busOwners.contains(destination) { owners.insert(destination) }
                }
            }
            changed = owners != old
        }
        let plugins = (project["audioPlugins"] as? [[String: Any]] ?? []).filter { owners.contains($0["owner"] as? String ?? "") }
        // Sound instruments carry their own insert chains ("effects"), whose page settings travel too.
        let instrumentEffectIDs = (kit["sounds"] as? [[String: Any]] ?? []).flatMap { (($0["instrument"] as? [String: Any])?["effects"] as? [[String: Any]]) ?? [] }
            .compactMap { $0["instanceID"] as? String }
        let pluginIDs = Set(plugins.compactMap { $0["instanceID"] as? String }).union(instrumentEffectIDs)
        var warnings: [String] = [], mappedOutputs: [String: Any] = [:]
        for owner in owners {
            if let target = outputs[owner] {
                if let id = target as? String, id != "master", !owners.contains(id) {
                    mappedOutputs[owner] = "master"; warnings.append("An output to another track was changed to Master in the saved kit.")
                } else { mappedOutputs[owner] = target }
            } else { mappedOutputs[owner] = owner.hasPrefix("sound:") ? "bus:" + busID : "master" }
        }
        let selectedSends = sends.filter { owners.contains($0["source"] as? String ?? "") }
        let keptSends = selectedSends.filter { owners.contains($0["destination"] as? String ?? "") || $0["destination"] as? String == "master" }
        if selectedSends.count != keptSends.count { warnings.append("Sends to other tracks are not included in this reusable kit.") }
        let relevantSidechains = (routing["sidechains"] as? [[String: Any]] ?? []).filter {
            let target = $0["destination"] as? [String: Any] ?? [:]
            return pluginIDs.contains(target["instanceID"] as? String ?? "")
        }
        let keptSidechains = relevantSidechains.filter { owners.contains($0["source"] as? String ?? "") }
        if relevantSidechains.count != keptSidechains.count { warnings.append("Sidechains driven by other tracks are not included in this reusable kit.") }
        // The session's record of where this kit was loaded or saved is not part of the reusable kit.
        var savedKit = kit; savedKit.removeValue(forKey: "libraryPath")
        let laneID = "kit-preview-" + kitID
        let lane: [String: Any] = ["name": kit["name"] as? String ?? "Kit", "routingID": laneID, "kind": "kit", "kitID": kitID,
            "soundID": model.sounds.first(where: { $0.kitID == kitID })?.soundID ?? "", "instrument": 0, "volume": 1, "pan": 0]
        mappedOutputs["track:" + laneID] = "master"
        var result: [String: Any] = ["version": 1, "name": kit["name"] as? String ?? "Kit", "bpm": project["bpm"] ?? 120,
            "linesPerBeat": project["linesPerBeat"] ?? 4, "masterVolume": 0.7, "tracks": [lane], "instruments": [], "kits": [savedKit],
            "patterns": [["id": 0, "length": 16, "steps": [Array(repeating: NSNull(), count: 16)]]], "sequence": [0], "audioPlugins": plugins,
            "routing": ["version": 1, "mixModel": "standard", "buses": buses.filter { owners.contains("bus:" + ($0["id"] as? String ?? "")) },
                "outputs": mappedOutputs, "sends": keptSends, "sidechains": keptSidechains]]
        if let pages = project["pluginPages"] as? [String: Any] { result["pluginPages"] = pages.filter { pluginIDs.contains($0.key) } }
        let metadata: [String: Any] = ["id": kitID, "name": kit["name"] as? String ?? "Kit", "type": "Kit", "tags": kit["tags"] ?? [], "source": "User", "warnings": warnings]
        return (result, metadata, warnings)
    }
}

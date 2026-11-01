import Foundation

/// Kits are track groups (docs/KIT_GROUPS_DESIGN.md). A group is a contiguous run of
/// ordinary member tracks routed to one group bus; `project.groups` describes it and each
/// member carries `groupID`.
///
/// This file converts the retired kit-lane model (project `kits`, `kind: "kit"` lanes,
/// stacked `kitEvents`, `sound:<kit>:<sound>` owners) into groups, for opening older
/// projects (ProjectMigration) and for reading `.stkit` / Maschine kit files, and writes
/// a group back into the `.stkit` kit layout (sixteen Sound slots) so kit files stay
/// readable by older versions.
enum KitGroups {
    static let groupColors = ["#e2a44f", "#7fb7d9", "#9fc97a", "#d98a9e", "#b39ddb", "#80cbc4", "#e6d36f", "#ef8a5b"]
    static let soundsPerKit = 16

    /// True when a document still uses kit lanes or kit definitions.
    static func needsMigration(_ project: [String: Any]) -> Bool {
        if project["kits"] != nil { return true }
        return (project["tracks"] as? [[String: Any]] ?? []).contains { $0["kind"] as? String == "kit" }
    }

    /// Member track routing identity for a former Sound: deterministic, so converting the
    /// same document twice gives the same project.
    static func memberRoutingID(kitID: String, soundID: String) -> String {
        let joined = "\(kitID)-\(soundID)"
        if joined.utf8.count <= 128 { return joined }
        var hash: UInt64 = 1469598103934665603
        for byte in joined.utf8 { hash = (hash ^ UInt64(byte)) &* 1099511628211 }
        return "kit-sound-" + String(hash, radix: 36)
    }

    struct Invalid: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    private static func int(_ value: Any?, _ fallback: Int) -> Int {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite else { return fallback }
        return Int(clamping: Int64(number.doubleValue.rounded(.towardZero)))
    }
    private static func number(_ value: Any?) -> Double? {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite else { return nil }
        return number.doubleValue
    }

    /// A Sound plays something when it has a sample, zones, an instrument plug-in or a
    /// hardware (MIDI) destination.
    static func soundHasSource(_ sound: [String: Any], owner: String, plugins: [[String: Any]]) -> Bool {
        let instrument = sound["instrument"] as? [String: Any] ?? [:]
        let type = instrument["type"] as? String ?? "sample"
        if type == "au" || type == "midi" { return true }
        if !(instrument["samplePath"] as? String ?? "").isEmpty { return true }
        if !(instrument["zones"] as? [Any] ?? []).isEmpty { return true }
        return plugins.contains { $0["owner"] as? String == owner }
    }

    struct Migration {
        var project: [String: Any]
        var notes: [String]
    }
    struct Member { var track: [String: Any]; var instrument: [String: Any]; var owner: String; var soundID: String }

    /// Converts every kit lane and kit definition into track groups.
    ///
    /// - Each kit becomes a group (same ID) whose bus is the kit's group bus.
    /// - Each Sound with a source or hits becomes a member track: its instrument becomes an
    ///   ordinary instrument, its channel mix and inserts move to the member's channel, its
    ///   choke group becomes the member's `chokeGroup`, and its pad position is kept as `pad`.
    /// - Every hit (primary and stacked) moves to the member track of its Sound on the same
    ///   row, keeping note, chord, velocity, gate and both FX (microtiming, plug-in locks …).
    ///   A lane-wide note-off reaches every member the lane played.
    /// - The members replace the kit's first lane in track order; later lanes of the same kit
    ///   merge into the same members (simultaneous hits of one pad keep the first).
    static func migrate(_ input: [String: Any]) -> Migration {
        guard needsMigration(input) else { return Migration(project: input, notes: []) }
        var project = input
        var notes: [String] = []
        let kits = (project["kits"] as? [[String: Any]] ?? []).filter { $0["id"] is String && $0["sounds"] is [[String: Any]] }
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        var instruments = project["instruments"] as? [[String: Any]] ?? []
        var plugins = project["audioPlugins"] as? [[String: Any]] ?? []
        var patterns = project["patterns"] as? [[String: Any]] ?? []
        var routing = project["routing"] as? [String: Any] ?? ["version": 1, "mixModel": "standard", "buses": [], "outputs": [:], "sends": [], "sidechains": []]
        var outputs = routing["outputs"] as? [String: Any] ?? [:]
        var sends = routing["sends"] as? [[String: Any]] ?? []
        var sidechains = routing["sidechains"] as? [[String: Any]] ?? []
        var buses = routing["buses"] as? [[String: Any]] ?? []
        var groups = project["groups"] as? [[String: Any]] ?? []
        let existingRoutingIDs = Set(tracks.compactMap { $0["routingID"] as? String })

        // New track order: every original track (kit lanes are replaced by their kit's
        // members at the first lane, later lanes vanish), then kits without lanes.
        enum Slot { case track(Int), kit(String) }
        var slots: [Slot] = [], placedKits = Set<String>()
        var lanesByKit: [String: [Int]] = [:]
        for (index, track) in tracks.enumerated() {
            if track["kind"] as? String == "kit" {
                let kitID = track["kitID"] as? String ?? ""
                guard kits.contains(where: { $0["id"] as? String == kitID }) else {
                    notes.append("A kit lane referred to a missing kit and was removed."); continue
                }
                lanesByKit[kitID, default: []].append(index)
                if placedKits.insert(kitID).inserted { slots.append(.kit(kitID)) }
            } else { slots.append(.track(index)) }
        }
        for kit in kits { if let id = kit["id"] as? String, placedKits.insert(id).inserted { slots.append(.kit(id)) } }

        var kitMembers: [String: [Member]] = [:]
        // Old Sound owner -> member routing ID, per kit.
        var memberOwners: [String: String] = [:]
        for (kitIndex, kit) in kits.enumerated() {
            let kitID = kit["id"] as! String
            let sounds = kit["sounds"] as! [[String: Any]]
            let lanes = lanesByKit[kitID] ?? []
            var hitSounds = Set<String>()
            for pattern in patterns {
                let steps = pattern["steps"] as? [Any] ?? []
                for lane in lanes where lane < steps.count {
                    for cell in steps[lane] as? [Any] ?? [] {
                        guard let cell = cell as? [String: Any] else { continue }
                        for hit in [cell] + (cell["kitEvents"] as? [[String: Any]] ?? []) {
                            if let id = hit["soundID"] as? String ?? tracks[lane]["soundID"] as? String { hitSounds.insert(id) }
                        }
                    }
                }
            }
            var members: [Member] = []
            for (pad, sound) in sounds.enumerated() {
                guard let soundID = sound["id"] as? String else { continue }
                let owner = "sound:\(kitID):\(soundID)"
                guard soundHasSource(sound, owner: owner, plugins: plugins) || hitSounds.contains(soundID) else { continue }
                members.append(member(kitID: kitID, sound: sound, soundID: soundID, owner: owner, pad: pad, existing: existingRoutingIDs))
            }
            // A kit with nothing loaded keeps one empty member so its bus and inserts survive.
            if members.isEmpty, let sound = sounds.first, let soundID = sound["id"] as? String {
                members.append(member(kitID: kitID, sound: sound, soundID: soundID, owner: "sound:\(kitID):\(soundID)", pad: 0, existing: existingRoutingIDs))
            }
            kitMembers[kitID] = members
            for item in members { memberOwners[item.owner] = item.track["routingID"] as? String }
            let laneMuted = !lanes.isEmpty && lanes.allSatisfy { tracks[$0]["sequencerMute"] as? Bool ?? false }
            var group: [String: Any] = ["id": kitID, "name": kit["name"] as? String ?? "Kit", "color": groupColors[(groups.count + kitIndex) % groupColors.count],
                                        "collapsed": false, "mixerCollapsed": false, "busID": kit["busID"] as? String ?? "\(kitID)-bus"]
            if let path = kit["libraryPath"] as? String { group["libraryPath"] = path }
            if let imported = kit["importNotes"] as? [String], !imported.isEmpty { group["importNotes"] = imported }
            groups.append(group)
            let busID = group["busID"] as! String
            if let index = buses.firstIndex(where: { $0["id"] as? String == busID }) {
                buses[index]["type"] = "group"; buses[index]["groupID"] = kitID
                if laneMuted { buses[index]["mute"] = true }
            } else {
                buses.append(["id": busID, "name": group["name"]!, "type": "group", "groupID": kitID, "volume": 1, "pan": 0, "mute": laneMuted, "solo": false])
            }
            if outputs["bus:\(busID)"] == nil { outputs["bus:\(busID)"] = "master" }
            if laneMuted { notes.append("\(group["name"]!): its kit lanes were muted, so the group bus is muted.") }
        }

        // Build the new track list and the old -> new index map for ordinary tracks.
        var newTracks: [[String: Any]] = [], trackMap: [Int: Int] = [:]
        var memberIndex: [String: Int] = [:]   // Sound owner -> new track index
        for slot in slots {
            switch slot {
            case .track(let old):
                trackMap[old] = newTracks.count
                newTracks.append(tracks[old])
            case .kit(let kitID):
                for var item in kitMembers[kitID] ?? [] {
                    let instrumentIndex = instruments.count
                    item.instrument["id"] = instrumentIndex
                    // A kit file keeps a Sound's instrument effects with its instrument; in a project
                    // they are plug-ins owned by the instrument (InstrumentEffects).
                    if let effects = item.instrument.removeValue(forKey: "effects") as? [[String: Any]] {
                        for effect in effects.prefix(MixerChannel.insertSlotCount) {
                            var descriptor = effect
                            descriptor.removeValue(forKey: "track")
                            descriptor["owner"] = InstrumentEffects.owner(instrumentIndex); descriptor["kind"] = "effect"
                            plugins.append(descriptor)
                        }
                    }
                    instruments.append(item.instrument)
                    item.track["instrument"] = instrumentIndex
                    memberIndex[item.owner] = newTracks.count
                    newTracks.append(item.track)
                }
            }
        }
        for (index, _) in newTracks.enumerated() { newTracks[index]["id"] = index }
        guard !newTracks.isEmpty else { return Migration(project: input, notes: ["The kit project has no tracks to convert."]) }

        // Patterns: ordinary columns move with their tracks; hits move to their member.
        var merged = 0
        for (patternIndex, var pattern) in patterns.enumerated() {
            let length = max(1, int(pattern["length"], 64))
            let old = pattern["steps"] as? [Any] ?? []
            var steps: [[Any]] = Array(repeating: Array(repeating: NSNull(), count: length), count: newTracks.count)
            for (oldIndex, newIndex) in trackMap where oldIndex < old.count { steps[newIndex] = old[oldIndex] as? [Any] ?? steps[newIndex] }
            for (kitID, lanes) in lanesByKit {
                let members = kitMembers[kitID] ?? []
                for lane in lanes where lane < old.count {
                    let defaultSound = tracks[lane]["soundID"] as? String
                    let played = Set((old[lane] as? [Any] ?? []).compactMap { $0 as? [String: Any] }.flatMap { [$0] + ($0["kitEvents"] as? [[String: Any]] ?? []) }
                        .compactMap { ($0["soundID"] as? String ?? defaultSound).map { "sound:\(kitID):\($0)" } })
                    for (row, raw) in (old[lane] as? [Any] ?? []).prefix(length).enumerated() {
                        guard let cell = raw as? [String: Any] else { continue }
                        var primary = cell; primary.removeValue(forKey: "kitEvents")
                        let hits = [primary] + (cell["kitEvents"] as? [[String: Any]] ?? [])
                        for (n, hit) in hits.enumerated() {
                            let soundID = hit["soundID"] as? String ?? (n == 0 ? defaultSound : nil)
                            var targets: [String] = []
                            if let soundID { targets = ["sound:\(kitID):\(soundID)"] }
                            else if n == 0, int(hit["note"], 0) < 0 {
                                // A lane-wide OFF/CUT stops every Sound the lane played.
                                targets = members.map(\.owner).filter { played.contains($0) }
                            }
                            for owner in targets {
                                guard let index = memberIndex[owner] else { continue }
                                if !(steps[index][row] is NSNull) { merged += 1; continue }
                                var step = hit
                                step.removeValue(forKey: "soundID"); step.removeValue(forKey: "kitEvents")
                                step["instrument"] = newTracks[index]["instrument"] ?? 0
                                steps[index][row] = step
                            }
                        }
                    }
                }
            }
            pattern["steps"] = steps
            // Automation: Sound owners become member owners; index lanes follow their track.
            var lanes: [[String: Any]] = []
            for var lane in pattern["automation"] as? [[String: Any]] ?? [] {
                if let owner = lane["owner"] as? String, owner.hasPrefix("sound:") {
                    guard let routingID = memberOwners[owner], let index = memberIndex[owner] else { continue }
                    lane["owner"] = "track:" + routingID; lane["track"] = index
                } else if lane["owner"] == nil, let track = lane["track"] as? Int, track >= 0 {
                    guard let index = trackMap[track] else { continue }
                    lane["track"] = index
                } else if let track = lane["track"] as? Int, track >= 0, let index = trackMap[track] {
                    lane["track"] = index
                }
                lanes.append(lane)
            }
            pattern["automation"] = lanes
            patterns[patternIndex] = pattern
        }
        if merged > 0 { notes.append("\(merged) simultaneous hit\(merged == 1 ? "" : "s") of one pad from different kit lanes were merged.") }

        // Plug-ins: Sound owners become member tracks; index-based entries follow their track.
        plugins = plugins.compactMap { plugin -> [String: Any]? in
            var item = plugin
            if let owner = item["owner"] as? String, owner.hasPrefix("sound:") {
                guard let routingID = memberOwners[owner], let index = memberIndex[owner] else { return nil }
                item["owner"] = "track:" + routingID; item["track"] = index
            } else if let track = item["track"] as? Int, track >= 0 {
                guard let index = trackMap[track] else { return nil }
                item["track"] = index
            }
            return item
        }
        for (index, track) in newTracks.enumerated() {
            guard var instrument = instruments[safe: int(track["instrument"], 0)], instrument["type"] as? String == "au",
                  let owner = track["routingID"] as? String,
                  let plugin = plugins.first(where: { $0["owner"] as? String == "track:" + owner && $0["kind"] as? String == "instrument" }) else { continue }
            instrument["plugin"] = plugin
            instruments[int(track["instrument"], 0)] = instrument
            _ = index
        }
        // Routing: Sound outputs and sends become member routes.
        var mappedOutputs: [String: Any] = [:]
        for (source, destination) in outputs {
            let key = source.hasPrefix("sound:") ? memberOwners[source].map { "track:" + $0 } : source
            guard let key else { continue }
            if let target = destination as? String, target.hasPrefix("sound:") {
                mappedOutputs[key] = memberOwners[target].map { "track:" + $0 } ?? "master"
            } else { mappedOutputs[key] = destination }
        }
        sends = sends.compactMap { send -> [String: Any]? in
            var item = send
            for key in ["source", "destination"] {
                if let owner = item[key] as? String, owner.hasPrefix("sound:") {
                    guard let routingID = memberOwners[owner] else { return nil }
                    item[key] = "track:" + routingID
                }
            }
            return item
        }
        sidechains = sidechains.compactMap { route -> [String: Any]? in
            var item = route
            if let owner = item["source"] as? String, owner.hasPrefix("sound:") {
                guard let routingID = memberOwners[owner] else { return nil }
                item["source"] = "track:" + routingID
            }
            return item
        }
        for track in newTracks {
            guard let routingID = track["routingID"] as? String, track["groupID"] != nil else { continue }
            if mappedOutputs["track:" + routingID] == nil,
               let group = groups.first(where: { $0["id"] as? String == track["groupID"] as? String }), let bus = group["busID"] as? String {
                mappedOutputs["track:" + routingID] = "bus:" + bus
            }
        }
        // Audio clips on removed lanes have no track left.
        if let clips = project["audioClips"] as? [[String: Any]] {
            project["audioClips"] = clips.compactMap { clip -> [String: Any]? in
                var item = clip
                guard let index = trackMap[int(clip["track"], 0)] else { return nil }
                item["track"] = index; return item
            }
        }
        if let clips = project["patternClips"] as? [[String: Any]] {
            project["patternClips"] = clips.compactMap { clip -> [String: Any]? in
                var item = clip
                guard let index = trackMap[int(clip["track"], 0)] else { return nil }
                item["track"] = index; return item
            }
        }
        routing["version"] = 1
        routing["mixModel"] = "standard"
        routing["buses"] = buses
        routing["outputs"] = mappedOutputs
        routing["sends"] = sends
        routing["sidechains"] = sidechains
        project["routing"] = routing
        project["tracks"] = newTracks
        project["instruments"] = instruments
        project["patterns"] = patterns
        project["audioPlugins"] = plugins
        if groups.isEmpty { project.removeValue(forKey: "groups") } else { project["groups"] = groups }
        project.removeValue(forKey: "kits")
        let converted = kits.count
        if converted > 0 {
            let count = kitMembers.values.reduce(0) { $0 + $1.count }
            notes.insert("Converted \(converted) kit\(converted == 1 ? "" : "s") to track group\(converted == 1 ? "" : "s") with \(count) track\(count == 1 ? "" : "s").", at: 0)
        }
        return Migration(project: project, notes: notes)
    }

    private static func member(kitID: String, sound: [String: Any], soundID: String, owner: String, pad: Int, existing: Set<String>) -> Member {
        let name = sound["name"] as? String ?? "Sound \(pad + 1)"
        var instrument = sound["instrument"] as? [String: Any] ?? ["type": "sample"]
        instrument["name"] = name
        let choke = number(sound["chokeGroup"]) ?? number(instrument["chokeGroup"]) ?? 0
        instrument.removeValue(forKey: "chokeGroup")
        // Mono / legato become the member track's voice mode (a per-track property).
        let voiceMode = instrument.removeValue(forKey: "voiceMode") as? String
        let channel = sound["channel"] as? [String: Any] ?? [:]
        var routingID = memberRoutingID(kitID: kitID, soundID: soundID)
        var suffix = 2
        while existing.contains(routingID) { routingID = memberRoutingID(kitID: kitID, soundID: "\(soundID)-\(suffix)"); suffix += 1 }
        var track: [String: Any] = ["name": name, "routingID": routingID, "groupID": kitID, "pad": pad, "padNote": 60,
            "volume": number(channel["volume"]) ?? 0.8, "pan": number(channel["pan"]) ?? 0,
            "mute": channel["mute"] as? Bool ?? false, "solo": channel["solo"] as? Bool ?? false,
            "delaySend": number(channel["delaySend"]) ?? 0, "reverbSend": number(channel["reverbSend"]) ?? 0, "plugins": []]
        if choke > 0 { track["chokeGroup"] = Int(choke) }
        if let voiceMode, ["mono", "legato"].contains(voiceMode) { track["voiceMode"] = voiceMode }
        if let color = sound["color"] as? String { track["color"] = color }
        return Member(track: track, instrument: instrument, owner: owner, soundID: soundID)
    }

    // MARK: - Kit files

    /// A `.stkit` / Maschine kit project (one kit, sixteen Sound slots) as a group project:
    /// the members of the kit's group with their instruments, inserts, bus and routes.
    static func groupProject(fromKitProject kitProject: [String: Any]) throws -> [String: Any] {
        guard let kit = (kitProject["kits"] as? [[String: Any]])?.first, kit["id"] is String else {
            throw Invalid(message: "The kit has no kit definition.")
        }
        var source = kitProject
        // Kit files keep their Sounds without lanes; a lane-free conversion keeps every loaded pad.
        source["tracks"] = [["id": 0, "name": "Kit preview", "routingID": "kit-preview-lane", "instrument": 0, "kind": "kit", "kitID": kit["id"]!]]
        source["instruments"] = []
        source["patterns"] = [["id": 0, "length": 1, "steps": [[NSNull()]], "automation": []]]
        source["sequence"] = [0]
        if var routing = source["routing"] as? [String: Any], var outputs = routing["outputs"] as? [String: Any] {
            outputs.removeValue(forKey: "track:kit-preview-lane")
            for key in outputs.keys where key.hasPrefix("track:kit-preview") { outputs.removeValue(forKey: key) }
            routing["outputs"] = outputs; source["routing"] = routing
        }
        // ProjectMigration also brings retired Sound instruments (built-in synths) up to date.
        return ProjectMigration.migrated(source)
    }

    /// The `.stkit` layout of a group: one kit whose sixteen Sound slots are the members
    /// by pad (members without a pad fill free pads in track order), its bus, inserts and
    /// routes. Members beyond sixteen pads are left out and reported.
    static func kitProject(fromGroup project: [String: Any], groupID: String) throws -> (project: [String: Any], warnings: [String]) {
        guard let group = (project["groups"] as? [[String: Any]])?.first(where: { $0["id"] as? String == groupID }),
              let busID = group["busID"] as? String else { throw Invalid(message: "Select a group to save as a kit.") }
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        let instruments = project["instruments"] as? [[String: Any]] ?? []
        let members = tracks.enumerated().filter { $0.element["groupID"] as? String == groupID }
        var warnings: [String] = []
        var slots: [Int: (index: Int, track: [String: Any])] = [:], unplaced: [(Int, [String: Any])] = []
        for (index, track) in members {
            if let pad = track["pad"] as? Int, (0..<soundsPerKit).contains(pad), slots[pad] == nil { slots[pad] = (index, track) } else { unplaced.append((index, track)) }
        }
        for (index, track) in unplaced {
            guard let pad = (0..<soundsPerKit).first(where: { slots[$0] == nil }) else {
                warnings.append("\(track["name"] as? String ?? "A track") was left out: a kit file holds sixteen pads."); continue
            }
            slots[pad] = (index, track)
        }
        let kitID = sanitized(groupID)
        var ownerMap: [String: String] = [:]
        var sounds: [[String: Any]] = []
        for pad in 0..<soundsPerKit {
            let soundID = String(format: "pad%02d", pad + 1)
            guard let (index, track) = slots[pad] else {
                sounds.append(["id": soundID, "name": "Sound \(String(format: "%02d", pad + 1))", "instrument": ["type": "sample", "name": "Sound \(pad + 1)", "volume": 1, "samplePath": ""],
                               "channel": ["volume": 0.8, "pan": 0, "mute": false, "solo": false, "delaySend": 0, "reverbSend": 0], "chokeGroup": 0])
                continue
            }
            _ = index
            if let routingID = track["routingID"] as? String { ownerMap["track:" + routingID] = "sound:\(kitID):\(soundID)" }
            var instrument = instruments[safe: int(track["instrument"], 0)] ?? ["type": "sample"]
            instrument.removeValue(forKey: "id")
            // The pad's instrument effects travel inside its Sound's instrument (see migrate).
            let effects = (project["audioPlugins"] as? [[String: Any]] ?? []).filter { InstrumentEffects.instrumentIndex($0["owner"]) == int(track["instrument"], 0) }
                .map { plugin -> [String: Any] in var item = plugin; item.removeValue(forKey: "owner"); item.removeValue(forKey: "track"); return item }
            if !effects.isEmpty { instrument["effects"] = effects } else { instrument.removeValue(forKey: "effects") }
            if let mode = track["voiceMode"] as? String { instrument["voiceMode"] = mode }
            let choke = int(track["chokeGroup"], 0)
            sounds.append(["id": soundID, "name": track["name"] as? String ?? "Sound \(pad + 1)", "instrument": instrument,
                           "channel": ["volume": number(track["volume"]) ?? 0.8, "pan": number(track["pan"]) ?? 0, "mute": track["mute"] as? Bool ?? false,
                                       "solo": track["solo"] as? Bool ?? false, "delaySend": number(track["delaySend"]) ?? 0, "reverbSend": number(track["reverbSend"]) ?? 0],
                           "chokeGroup": max(0, min(16, choke))])
        }
        var kit: [String: Any] = ["id": kitID, "name": group["name"] as? String ?? "Kit", "busID": sanitized(busID), "sounds": sounds]
        if let notes = group["importNotes"] as? [String] { kit["importNotes"] = notes }
        // A legacy kit project with the group's bus; KitProject.libraryProject keeps only the
        // kit's own owners, the buses it feeds, and the routes and plug-ins between them.
        var legacy = project
        legacy["kits"] = [kit]
        legacy["tracks"] = [["id": 0, "name": kit["name"]!, "routingID": "kit-save-lane", "kind": "kit", "kitID": kitID, "instrument": 0]]
        legacy["instruments"] = []
        legacy["patterns"] = [["id": 0, "length": 16, "steps": [Array(repeating: NSNull(), count: 16)], "automation": []]]
        legacy.removeValue(forKey: "groups")
        let mapOwner: (String) -> String = { owner in
            if let mapped = ownerMap[owner] { return mapped }
            if owner == "bus:" + busID { return "bus:" + sanitized(busID) }
            return owner
        }
        legacy["audioPlugins"] = (project["audioPlugins"] as? [[String: Any]] ?? []).compactMap { plugin -> [String: Any]? in
            guard let owner = plugin["owner"] as? String, InstrumentEffects.instrumentIndex(owner) == nil else { return nil }
            if owner.hasPrefix("track:") && ownerMap[owner] == nil { return nil }
            var item = plugin; item["owner"] = mapOwner(owner); item.removeValue(forKey: "track"); return item
        }
        var routing = project["routing"] as? [String: Any] ?? [:]
        var outputs: [String: Any] = [:]
        for (source, destination) in routing["outputs"] as? [String: Any] ?? [:] {
            if source.hasPrefix("track:") && ownerMap[source] == nil { continue }
            outputs[mapOwner(source)] = (destination as? String).map(mapOwner) ?? destination
        }
        routing["outputs"] = outputs
        routing["buses"] = (routing["buses"] as? [[String: Any]] ?? []).map { bus in
            var item = bus
            if bus["id"] as? String == busID { item["id"] = sanitized(busID); item.removeValue(forKey: "groupID") }
            return item
        }
        routing["sends"] = (routing["sends"] as? [[String: Any]] ?? []).compactMap { send in
            var item = send
            for key in ["source", "destination"] { if let owner = item[key] as? String { item[key] = mapOwner(owner) } }
            return item
        }
        routing["sidechains"] = (routing["sidechains"] as? [[String: Any]] ?? []).map { route in
            var item = route
            if let owner = item["source"] as? String { item["source"] = mapOwner(owner) }
            return item
        }
        legacy["routing"] = routing
        return (legacy, warnings)
    }

    private static func sanitized(_ id: String) -> String {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "_-"))
        let cleaned = String(id.unicodeScalars.map { allowed.contains($0) && $0.isASCII ? Character($0) : "-" })
        return String(cleaned.prefix(80)).isEmpty ? "kit" : String(cleaned.prefix(80))
    }
}

private extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}

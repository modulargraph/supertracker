import Foundation

/// Kit lanes → track groups (KitGroups.migrate through ProjectMigration), on the kind of
/// session that motivated the redesign: an eight-track hardware session (three external-synth
/// editor plug-ins) plus a "1988 Kit" kit lane with stacked hits. When SUPERTRACKER_1988_KIT
/// names an installed "1988 Kit.mxgrp" (DJ Khalil expansion), it is imported for the kit;
/// otherwise a synthetic sixteen-pad kit with the same features is used. Also checks the
/// group → .stkit → group round trip used by Save kit / Load kit.
@main enum KitGroupsMigrationTests {
    struct Failure: Error, CustomStringConvertible { let message: String; var description: String { message } }
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !condition() { throw Failure(message: message) }
    }
    static func json(_ value: Any) throws -> String {
        String(data: try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), encoding: .utf8)!
    }
    static var serial = 0
    static func makeID() -> String { serial += 1; return String(format: "id%04d", serial) }

    /// Eight tracks: three play external synths through editor plug-ins (instrument plug-in plus the synth's
    /// line-input return), the other five are empty.
    static func hardwareSession() throws -> [String: Any] {
        var instruments: [[String: Any]] = [], plugins: [[String: Any]] = [], tracks: [[String: Any]] = [], outputs: [String: String] = [:]
        let editors: [(name: String, channels: [Int])] = [("Synth A Editor", [4, 5]), ("Synth B Editor", [3]), ("Synth C Editor", [6, 7])]
        for index in 0..<8 {
            let routingID = makeID()
            var track: [String: Any] = ["id": index, "routingID": routingID, "name": index < editors.count ? editors[index].name : "Track \(String(format: "%02d", index + 1))",
                "kind": "track", "instrument": index < editors.count ? index : -1, "volume": 0.8, "pan": 0.0, "mute": false, "solo": false,
                "delaySend": 0.0, "reverbSend": 0.0, "plugins": []]
            outputs["track:" + routingID] = "master"
            if index < editors.count {
                let editor = editors[index]
                let descriptor: [String: Any] = ["component": "vst3:FIXTURE-EDITOR-\(index)", "state": "c3RhdGU=", "bundlePath": "/Library/Audio/Plug-Ins/VST3/\(editor.name).vst3",
                    "track": index, "owner": "track:" + routingID, "kind": "instrument", "slot": 0, "instanceID": makeID(), "format": "VST3", "name": editor.name]
                plugins.append(descriptor)
                instruments.append(["id": index, "name": editor.name, "type": "au", "plugin": descriptor, "volume": 1.0, "pan": 0.0, "rootNote": 60, "tune": 0,
                    "attack": 0.0, "decay": 0.0, "sustain": 1.0, "release": 0.01, "midiInput": 0, "midiOutput": 0, "midiInputChannel": 0, "midiChannel": 1, "samplePath": ""])
                track["audioInput"] = ["enabled": true, "deviceUID": "fixture-interface", "channels": editor.channels, "gain": 1.0]
            }
            tracks.append(track)
        }
        let rows = [Any](repeating: NSNull(), count: 64)
        return ["version": 1, "name": "Hardware Editors", "bpm": 120.0, "linesPerBeat": 4, "timeSignature": ["numerator": 4, "denominator": 4],
            "masterVolume": 0.8, "swing": 0.0, "metronome": false, "tracks": tracks, "instruments": instruments, "audioPlugins": plugins, "audioClips": [],
            "patterns": [["id": 0, "name": "Pattern 01", "length": 64, "steps": Array(repeating: rows, count: 8), "automation": []]],
            "activePattern": 0, "sequence": [0], "routing": ["version": 1, "mixModel": "standard", "buses": [], "outputs": outputs, "sends": [], "sidechains": []]]
    }

    /// A sixteen-pad kit with the features the real group has: samples, plug-in instruments,
    /// choke groups, mono voices, Sound inserts and mix, and group-bus inserts.
    static func syntheticKit() -> (kit: [String: Any], plugins: [[String: Any]], buses: [[String: Any]]) {
        let names = ["Kick 1988 1", "Snare 1988 1", "ClosedHH 1988 1", "OpenHH 1988", "Kick 1988 2", "Snare 1988 2", "ClosedHH 1988 2", "Tom 1988",
                     "ClosedHH[150] 1988", "Lick Conga 1988", "Woodblock 1988", "Lick Perc 1988", "RevCymbal 1988", "Clap 1988", "Synth[150] Gm 1988", "Combo[150] Gm 1988"]
        var plugins: [[String: Any]] = []
        let sounds: [[String: Any]] = names.enumerated().map { pad, name in
            let id = "snd\(pad)"
            var instrument: [String: Any] = ["type": "sample", "name": name, "samplePath": pad == 12 ? "" : "/Samples/\(name).wav", "rootNote": 60, "voiceMode": pad < 4 ? "mono" : "poly", "volume": 1]
            if pad >= 14 {
                instrument = ["type": "au", "name": name, "rootNote": 60]
                plugins.append(["owner": "sound:k1988:\(id)", "kind": "instrument", "slot": 0, "instanceID": "inst\(pad)", "component": "aufx:fixture", "name": name, "state": "AAAA"])
            }
            return ["id": id, "name": name, "instrument": instrument, "chokeGroup": [2, 3].contains(pad) ? 1 : 0,
                    "channel": ["volume": 0.5 + Double(pad) / 100, "pan": pad == 1 ? -0.25 : 0, "mute": false, "solo": false, "delaySend": 0, "reverbSend": pad == 1 ? 0.3 : 0]]
        }
        plugins.append(["owner": "sound:k1988:snd1", "kind": "effect", "slot": 0, "instanceID": "snareComp", "component": "builtin:compressor", "name": "Compressor", "state": "AAAA"])
        for (slot, name) in ["Metaverb", "Beat Delay", "Peak Limiter"].enumerated() {
            plugins.append(["owner": "bus:b1988", "kind": "effect", "slot": slot, "instanceID": "bus\(slot)", "component": "builtin:\(name)", "name": name, "state": "AAAA"])
        }
        return (["id": "k1988", "name": "1988 Kit", "busID": "b1988", "sounds": sounds, "importNotes": ["Flanger on 1988 Kit is unavailable."]], plugins,
                [["id": "b1988", "name": "1988 Kit", "type": "group", "volume": 1, "pan": 0, "mute": false, "solo": false]])
    }

    /// The real group, read the way the kit browser reads it (only when SUPERTRACKER_1988_KIT names it).
    static func realKit() throws -> (kit: [String: Any], plugins: [[String: Any]], buses: [[String: Any]])? {
        guard let path = ProcessInfo.processInfo.environment["SUPERTRACKER_1988_KIT"], FileManager.default.fileExists(atPath: path) else { return nil }
        let contents = try MaschineGroupImporter.read(at: URL(fileURLWithPath: path))
        guard let project = contents["project"] as? [String: Any], let kit = (project["kits"] as? [[String: Any]])?.first else { return nil }
        let routing = project["routing"] as? [String: Any] ?? [:]
        return (kit, project["audioPlugins"] as? [[String: Any]] ?? [], routing["buses"] as? [[String: Any]] ?? [])
    }

    /// Hardware session + kit lane at track 9 (index 8) with hits, stacked hits, FX and a lane-wide OFF.
    static func userSession(kit source: (kit: [String: Any], plugins: [[String: Any]], buses: [[String: Any]])) throws -> [String: Any] {
        var project = try hardwareSession()
        let kit = source.kit, kitID = kit["id"] as! String, busID = kit["busID"] as! String
        let sounds = (kit["sounds"] as! [[String: Any]]).map { $0["id"] as! String }
        var tracks = project["tracks"] as! [[String: Any]]
        tracks.append(["id": 8, "name": kit["name"]!, "routingID": "lane1988", "kind": "kit", "kitID": kitID, "instrument": 0, "sequencerMute": false])
        project["tracks"] = tracks
        project["kits"] = [kit]
        var patterns = project["patterns"] as! [[String: Any]]
        var steps = patterns[0]["steps"] as! [[Any]]
        let length = patterns[0]["length"] as? Int ?? 64
        var lane = [Any](repeating: NSNull(), count: length)
        for row in stride(from: 0, to: length, by: 4) {
            var cell: [String: Any] = ["note": 60, "soundID": sounds[0], "velocity": 100 + row % 20, "gate": 0.8]
            var stacked: [[String: Any]] = [["note": 62, "soundID": sounds[2], "velocity": 70, "gate": 0.25, "fx1": ["type": "microtiming", "value": 0.3]]]
            if row % 8 == 4 { stacked.append(["note": 60, "soundID": sounds[1], "velocity": 110, "gate": 1.5, "chord": [0, 7], "fx2": ["type": "probability", "value": 0.5]]) }
            cell["kitEvents"] = stacked
            lane[row] = cell
        }
        lane[2] = ["note": 60, "soundID": sounds[3], "velocity": 90, "gate": 2.0]
        lane[3] = ["note": -1, "velocity": 100]
        steps.append(lane)
        patterns[0]["steps"] = steps
        patterns[0]["automation"] = [["id": "auto1", "owner": "sound:\(kitID):\(sounds[1])", "target": "volume", "points": [["row": 0, "value": 0.5]]],
                                     ["id": "auto2", "track": 1, "target": "cutoff", "points": [["row": 0, "value": 900]]]]
        project["patterns"] = patterns
        project["audioPlugins"] = (project["audioPlugins"] as! [[String: Any]]) + source.plugins
        var routing = project["routing"] as! [String: Any]
        var outputs = routing["outputs"] as! [String: Any]
        outputs["bus:\(busID)"] = "master"
        for sound in sounds { outputs["sound:\(kitID):\(sound)"] = "bus:\(busID)" }
        routing["outputs"] = outputs
        routing["buses"] = source.buses + [["id": "verb", "name": "Verb return", "type": "return", "volume": 1]]
        outputs["bus:verb"] = "master"; routing["outputs"] = outputs
        routing["sends"] = [["id": "send1", "source": "sound:\(kitID):\(sounds[1])", "destination": "bus:verb", "gain": 0.4, "tap": "postFader", "enabled": true]]
        routing["mixModel"] = "standard"
        project["routing"] = routing
        return project
    }

    static func main() throws {
        var passed = 0
        func pass(_ name: String) { passed += 1; print("PASS " + name) }
        let real = try realKit()
        let source = real ?? syntheticKit()
        print(real == nil ? "note: SUPERTRACKER_1988_KIT is not set; using the synthetic sixteen-pad kit" : "note: using the installed 1988 Kit.mxgrp")
        let kitID = source.kit["id"] as! String, busID = source.kit["busID"] as! String
        let sounds = source.kit["sounds"] as! [[String: Any]]
        let original = try userSession(kit: source)
        // For a visual check: SUPERTRACKER_KIT_MIGRATION_WRITE=<file> keeps the old session as a project file.
        if let target = ProcessInfo.processInfo.environment["SUPERTRACKER_KIT_MIGRATION_WRITE"] {
            try JSONSerialization.data(withJSONObject: original, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: target))
        }
        try check(ProjectMigration.needsMigration(original), "A project with kit lanes needs migration")
        let (project, notes) = ProjectMigration.migratedWithNotes(original)
        try check(project["kits"] == nil && !KitGroups.needsMigration(project), "no kits remain")
        try check(notes.first?.hasPrefix("Converted 1 kit to track group") == true, "the conversion is reported: \(notes)")
        let tracks = project["tracks"] as! [[String: Any]], instruments = project["instruments"] as! [[String: Any]]
        let groups = project["groups"] as! [[String: Any]]
        try check(groups.count == 1 && groups[0]["id"] as? String == kitID && groups[0]["name"] as? String == source.kit["name"] as? String && groups[0]["busID"] as? String == busID,
                  "the kit became one group with its name and bus")
        // The eight hardware tracks are untouched, in place.
        let before = original["tracks"] as! [[String: Any]]
        for index in 0..<8 { try check(try json(tracks[index]) == json(before[index]), "hardware track \(index + 1) is unchanged") }
        let beforePatterns = original["patterns"] as! [[String: Any]], afterPatterns = project["patterns"] as! [[String: Any]]
        for index in 0..<8 {
            try check(try json((beforePatterns[0]["steps"] as! [Any])[index]) == json((afterPatterns[0]["steps"] as! [Any])[index]), "hardware column \(index + 1) is unchanged")
        }
        pass("the hardware template tracks, columns and plug-ins stay as they were")

        // Members: every Sound with a source or hits, in pad order, after the hardware tracks.
        let members = tracks.enumerated().filter { $0.element["groupID"] as? String == kitID }
        let plugins = project["audioPlugins"] as! [[String: Any]]
        let expected = sounds.enumerated().filter { pad, sound in
            KitGroups.soundHasSource(sound, owner: "sound:\(kitID):\(sound["id"]!)", plugins: source.plugins) || [0, 1, 2, 3].contains(pad)
        }
        try check(members.count == expected.count && members.first?.offset == 8 && members.last?.offset == 8 + members.count - 1,
                  "one contiguous member per loaded or played pad (\(members.count) of \(expected.count))")
        try check(zip(members, expected).allSatisfy { $0.element["name"] as? String == $1.element["name"] as? String && $0.element["pad"] as? Int == $1.offset },
                  "members keep Sound names and pad positions")
        let routing = project["routing"] as! [String: Any], outputs = routing["outputs"] as! [String: Any]
        for (index, member) in members {
            let owner = "track:" + (member["routingID"] as! String)
            try check(outputs[owner] as? String == "bus:" + busID, "member \(index) outputs to the group bus")
            let instrument = instruments[member["instrument"] as! Int]
            let sound = expected.first { $0.element["name"] as? String == member["name"] as? String }!.element
            let channel = sound["channel"] as? [String: Any] ?? [:]
            try check(abs((member["volume"] as? Double ?? 0) - ((channel["volume"] as? NSNumber)?.doubleValue ?? 0.8)) < 1e-9, "member \(index) keeps the Sound level")
            let choke = (sound["chokeGroup"] as? NSNumber)?.intValue ?? ((sound["instrument"] as? [String: Any])?["chokeGroup"] as? NSNumber)?.intValue ?? 0
            try check((member["chokeGroup"] as? Int ?? 0) == choke && instrument["chokeGroup"] == nil, "member \(index) carries the choke group as a track property")
            if instrument["type"] as? String == "au" {
                try check(plugins.contains { $0["owner"] as? String == owner && $0["kind"] as? String == "instrument" && $0["track"] as? Int == index }, "member \(index) owns its instrument plug-in")
                try check((instrument["plugin"] as? [String: Any])?["owner"] as? String == owner, "member \(index) instrument descriptor follows its owner")
            }
        }
        try check(!plugins.contains { ($0["owner"] as? String)?.hasPrefix("sound:") == true }, "no Sound owners remain")
        try check(plugins.filter { $0["owner"] as? String == "bus:" + busID }.count == source.plugins.filter { $0["owner"] as? String == "bus:" + busID }.count,
                  "the group bus keeps its inserts")
        let bus = (routing["buses"] as! [[String: Any]]).first { $0["id"] as? String == busID }!
        try check(bus["type"] as? String == "group" && bus["groupID"] as? String == kitID, "the bus is the group's bus")
        let snare = members.first { $0.element["pad"] as? Int == 1 }!
        let snareOwner = "track:" + (snare.element["routingID"] as! String)
        try check((routing["sends"] as! [[String: Any]]).first?["source"] as? String == snareOwner, "a Sound send became the member's send")
        try check(((afterPatterns[0]["automation"] as! [[String: Any]]).first { $0["id"] as? String == "auto1" })?["owner"] as? String == snareOwner,
                  "Sound automation became member automation")
        pass("members carry their instruments, mix, inserts, choke groups, sends and automation; the bus keeps its inserts")

        // Hits: primary and stacked hits land on their member at the same row with everything kept.
        let steps = afterPatterns[0]["steps"] as! [[Any]]
        func step(_ pad: Int, _ row: Int) -> [String: Any]? {
            guard let member = members.first(where: { $0.element["pad"] as? Int == pad }) else { return nil }
            return steps[member.offset][row] as? [String: Any]
        }
        let lane = (beforePatterns[0]["steps"] as! [[Any]])[8]
        var moved = 0
        for (row, raw) in lane.enumerated() {
            guard let cell = raw as? [String: Any], (cell["note"] as? Int ?? 0) >= 0 else { continue }
            var primary = cell; primary.removeValue(forKey: "kitEvents")
            for hit in [primary] + (cell["kitEvents"] as? [[String: Any]] ?? []) {
                let pad = sounds.firstIndex { $0["id"] as? String == hit["soundID"] as? String }!
                guard let placed = step(pad, row) else { throw Failure(message: "hit of pad \(pad + 1) at row \(row) is missing") }
                for key in ["note", "velocity", "gate", "fx1", "fx2", "chord"] {
                    try check(try json([hit[key] ?? NSNull()]) == json([placed[key] ?? NSNull()]), "pad \(pad + 1) row \(row) keeps \(key)")
                }
                try check(placed["soundID"] == nil && placed["kitEvents"] == nil, "member steps are ordinary steps")
                let member = members.first { $0.element["pad"] as? Int == pad }!.element
                try check(placed["instrument"] as? Int == member["instrument"] as? Int, "a moved hit plays its member's instrument")
                moved += 1
            }
        }
        try check(moved > 30, "the fixture moved many hits (\(moved))")
        for pad in [0, 2, 3] { try check(step(pad, 3)?["note"] as? Int == -1, "the lane-wide OFF reaches pad \(pad + 1)") }
        try check(step(1, 3) == nil || step(1, 3)?["note"] as? Int == -1, "the OFF reaches the snare as well")
        pass("\(moved) primary and stacked hits moved to their members with pitch, velocity, gate, chords and FX (microtiming included)")

        // The converted project is a valid routed project without kit machinery.
        let definition = try AudioRouting.parse(project: project)
        try check(!definition.owners.contains { $0.id.hasPrefix("sound:") }, "routing has no Sound owners")
        try check(definition.owners.contains { $0.id == "bus:" + busID && $0.kind == .group }, "routing keeps the group bus")
        try check((try KitProject(project: project)).isEmpty, "the kit model sees no kits")
        try check(try json(ProjectMigration.migrated(project)) == json(project), "migration is idempotent")
        try check(try json(ProjectMigration.migrated(original)) == json(project), "migration is deterministic")
        pass("the converted project validates, routes members through the group bus and migrates idempotently")

        // Save kit / Load kit: group -> .stkit layout -> group.
        let saved = try KitGroups.kitProject(fromGroup: project, groupID: kitID)
        let library = try KitProject.libraryProject(saved.project, kitID: (saved.project["kits"] as! [[String: Any]])[0]["id"] as! String)
        try check(try KitProject(project: library.project).kits.count == 1, "the saved kit file is a valid sixteen-pad kit")
        let reloaded = try KitGroups.groupProject(fromKitProject: library.project)
        let reTracks = (reloaded["tracks"] as! [[String: Any]]).filter { $0["groupID"] != nil }
        try check(reTracks.count == members.count, "reloading the kit gives one member per pad (\(reTracks.count))")
        let reInstruments = reloaded["instruments"] as! [[String: Any]]
        for (old, new) in zip(members.map(\.element), reTracks) {
            try check(old["name"] as? String == new["name"] as? String && old["pad"] as? Int == new["pad"] as? Int && (old["chokeGroup"] as? Int ?? 0) == (new["chokeGroup"] as? Int ?? 0),
                      "\(old["name"]!) keeps name, pad and choke group through Save/Load")
            var a = instruments[old["instrument"] as! Int], b = reInstruments[new["instrument"] as! Int]
            for key in ["id", "plugin"] { a.removeValue(forKey: key); b.removeValue(forKey: key) }
            try check(try json(a) == json(b), "\(old["name"]!) keeps its instrument through Save/Load")
        }
        let reBus = (reloaded["groups"] as! [[String: Any]])[0]["busID"] as! String
        try check((reloaded["audioPlugins"] as! [[String: Any]]).filter { $0["owner"] as? String == "bus:" + reBus }.count == plugins.filter { $0["owner"] as? String == "bus:" + busID }.count,
                  "the group bus inserts survive Save/Load")
        pass("Save kit writes the group as a sixteen-pad kit file and Load kit reads it back as the same group")

        // Optional: a real session file (for example the user's recovery file) converts and validates.
        if let path = ProcessInfo.processInfo.environment["SUPERTRACKER_KIT_MIGRATION_FIXTURE"], FileManager.default.fileExists(atPath: path) {
            let data = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: path))) as! [String: Any]
            let file = data["project"] as? [String: Any] ?? data
            let result = ProjectMigration.migratedWithNotes(file)
            _ = try AudioRouting.parse(project: result.project)
            try check((try KitProject(project: result.project)).isEmpty, "the real session has no kits left")
            print("note: \(path): \(result.notes.joined(separator: " "))")
            pass("the real session file converts and routes")
        }
        print("kit groups migration: \(passed) checks passed")
    }
}

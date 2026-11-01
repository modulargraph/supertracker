import Foundation

private var groups = 0
private func check(_ value: @autoclosure () -> Bool, _ message: String) { precondition(value(), message) }
private func rejects(_ project: [String: Any], _ label: String) {
    do { _ = try KitProject(project: project); fatalError("Expected rejection: \(label)") } catch {}
}
private func kit(_ index: Int = 0) -> [String: Any] {
    ["id": "kit\(index)", "busID": "kitbus\(index)", "name": "Kit \(index)", "sounds": (0..<16).map { pad in
        ["id": "s\(pad)", "instrument": ["type": "sample", "samplePath": "/fixture/\(pad).wav"], "channel": ["volume": 1.0, "pan": 0.0]] as [String: Any]
    }]
}
private func project(_ kits: [[String: Any]]? = nil) -> [String: Any] {
    ["kits": kits ?? [kit()], "tracks": [["kind": "kit", "kitID": "kit0", "soundID": "s0", "routingID": "lane0"]],
     "patterns": [["steps": [[NSNull(), ["note": 60, "soundID": "s1"], NSNull()] as [Any]]]]]
}
private func editedSound(_ body: (inout [String: Any]) -> Void) -> [String: Any] {
    var k = kit(), sounds = k["sounds"] as! [[String: Any]]; body(&sounds[0]); k["sounds"] = sounds; return project([k])
}
private func editedCell(_ cell: Any) -> [String: Any] {
    var p = project(); p["patterns"] = [["steps": [[NSNull(), cell] as [Any]]]]; return p
}
@main struct KitProjectTests {
    static func main() throws {
        let absent = try KitProject(project: [:]); check(absent.isEmpty && absent.sounds.isEmpty, "legacy projects stay empty"); groups += 1
        let full = try KitProject(project: project((0..<4).map(kit))); check(full.sounds.count == 64, "four full kits");
        check(full.sounds.last?.owner == "sound:kit3:s15", "stable owner identity"); groups += 1
        for malformed: Any in [NSNull(), "kit", 4, [:] as [String: Any], [NSNull()] as [Any]] { var p = project(); p["kits"] = malformed; rejects(p, "malformed kits") }; groups += 1
        rejects(project((0..<5).map(kit)), "fifth kit")
        for count in [0, 15, 17] { var k = kit(); k["sounds"] = Array(repeating: ["id": "s", "instrument": ["type": "synth"]] as [String: Any], count: count); rejects(project([k]), "exact sixteen slots") }; groups += 1
        rejects(project([kit(), kit()]), "duplicate kit")
        var duplicateGroup = kit(1); duplicateGroup["busID"] = "kitbus0"; rejects(project([kit(), duplicateGroup]), "duplicate group")
        var duplicateSound = kit(); var sounds = duplicateSound["sounds"] as! [[String: Any]]; sounds[1]["id"] = "s0"; duplicateSound["sounds"] = sounds; rejects(project([duplicateSound]), "duplicate sound"); groups += 1
        for id: Any in ["", "owner:bad", String(repeating: "x", count: 81), 9, NSNull()] { rejects(editedSound { $0["id"] = id }, "invalid sound ID") }; groups += 1
        for value: Any in [true, Double.nan, Double.infinity, -0.1, 2.1, "1"] { rejects(editedSound { $0["channel"] = ["volume": value] }, "invalid volume") }
        for value: Any in [NSNull(), "channel", [1]] { rejects(editedSound { $0["channel"] = value }, "malformed channel") }; groups += 1
        let choke = try KitProject(project: editedSound { $0["chokeGroup"] = 4; $0["instrument"] = ["type":"sample", "chokeGroup":2] })
        check(choke.sounds[0].instrument["chokeGroup"] as? Int == 4, "Sound choke control reaches runtime instrument")
        let legacyChoke = try KitProject(project: editedSound { $0["instrument"] = ["type":"sample", "chokeGroup":3] })
        check(legacyChoke.sounds[0].instrument["chokeGroup"] as? Int == 3, "Legacy inline choke remains supported")
        for value: Any in [true, -1, 17, 2.5, "2", Double.nan] { rejects(editedSound { $0["chokeGroup"] = value }, "invalid choke") }; groups += 1
        for value: Any in [NSNull(), "zones", ["path": "x"]] { rejects(editedSound { $0["instrument"] = ["type": "sample", "zones": value] }, "malformed zones") }
        rejects(editedSound { $0["instrument"] = ["type": "sample", "zones": Array(repeating: [:] as [String: Any], count: 129)] }, "zone capacity"); groups += 1
        for selected: Any in ["missing", 7, NSNull()] { var p = project(); p["tracks"] = [["kind": "kit", "kitID": "kit0", "soundID": selected]]; rejects(p, "invalid default Sound") }
        var badLane = project(); badLane["tracks"] = [["kind": "kit", "kitID": "missing"]]; rejects(badLane, "unknown kit"); groups += 1
        let partial = try KitProject(project: project()); check(partial.sound(lane: 0, step: [:])?.soundID == "s0", "default selection")
        check(partial.sound(lane: 0, step: ["soundID": "s1"])?.soundID == "s1", "explicit identity selection"); groups += 1
        for value: Any in ["missing", 4, NSNull()] { rejects(editedCell(["note": 60, "soundID": value]), "invalid step Sound") }
        let extra: [String: Any] = ["soundID": "s2", "note": 60, "velocity": 100, "gate": 0.8]
        _ = try KitProject(project: editedCell(["note": 60, "soundID": "s0", "kitEvents": Array(repeating: extra, count: 7)]))
        rejects(editedCell(["note": 60, "soundID": "s0", "kitEvents": Array(repeating: extra, count: 8)]), "eighth extra event")
        rejects(editedCell(["note": 60, "soundID": "s0", "kitEvents": [["note": 60, "soundID": "missing"]]]), "extra missing Sound")
        rejects(editedCell(["note": 60, "soundID": "s0", "kitEvents": "bad"]), "malformed extra events"); groups += 1
        var located = kit(); located["libraryPath"] = "/Users/x/Kits/Kit 0.stkit"
        let library = try KitProject.libraryProject(project([located]), kitID: "kit0")
        let savedKits = library.project["kits"] as? [[String: Any]] ?? []
        check(savedKits.count == 1 && savedKits[0]["libraryPath"] == nil && savedKits[0]["name"] as? String == "Kit 0", "a saved kit leaves out the session's library location"); groups += 1
        print("KitProject: \(groups) groups passed; stable identity/capacity, strict type/range admission, partial cells and stacked events.")
    }
}

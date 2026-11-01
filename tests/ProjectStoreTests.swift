import Foundation

@main
struct ProjectStoreTests {
    static func tryJSON(_ value: [String: Any]) -> Data { try! JSONSerialization.data(withJSONObject: value, options: .sortedKeys) }
    static func main() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-Store-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let sample = root.appendingPathComponent("original.wav")
        let contents = Data([0, 1, 2, 3, 4, 5])
        try contents.write(to: sample)
        let project: [String: Any] = ["version": 1, "name": "Test", "bpm": 120.0, "tracks": [["id": 0]], "instruments": [["id": 0, "samplePath": sample.path]], "patterns": [["id": 0, "length": 64, "steps": [[]]]], "sequence": [0]]
        let store = try ProjectStore(directory: root.appendingPathComponent("support"))
        try store.update(project)
        assert(store.dirty)
        try store.autosave()
        let destination = root.appendingPathComponent("Test.supertracker")
        let saved = try store.save(to: destination)
        assert(!store.dirty)
        let path1 = (saved["instruments"] as! [[String: Any]])[0]["samplePath"] as! String
        assert(path1 != sample.path)
        let sourceAfterSave = try Data(contentsOf: sample)
        assert(sourceAfterSave == contents)
        let savedAgain = try store.save(to: destination)
        let path2 = (savedAgain["instruments"] as! [[String: Any]])[0]["samplePath"] as! String
        assert(path1 == path2, "Saving repeatedly must not create recursively copied media")
        // Native sample capture may canonicalize /var to /private/var, or use a
        // Finder alias of the same collected media directory.
        let mediaAlias = root.appendingPathComponent("media-alias")
        try FileManager.default.createSymbolicLink(at: mediaAlias, withDestinationURL: destination.deletingPathExtension().appendingPathExtension("media"))
        var aliased = savedAgain
        var aliasedInstruments = aliased["instruments"] as! [[String: Any]]
        aliasedInstruments[0]["samplePath"] = mediaAlias.appendingPathComponent(URL(fileURLWithPath: path1).lastPathComponent).path
        aliased["instruments"] = aliasedInstruments
        let savedAlias = try store.save(to: destination, snapshot: aliased)
        let aliasPath = (savedAlias["instruments"] as! [[String: Any]])[0]["samplePath"] as! String
        assert(aliasPath == path1, "Saving a canonicalized/aliased collected path must retain the existing media identity")
        let mediaFiles = try FileManager.default.contentsOfDirectory(atPath: destination.deletingPathExtension().appendingPathExtension("media").path)
        assert(mediaFiles.count == 1, "Repeated save must not add a second hash prefix or duplicate media")
        let stored = try JSONSerialization.jsonObject(with: Data(contentsOf: destination)) as! [String: Any]
        assert(!(((stored["instruments"] as! [[String: Any]])[0]["samplePath"] as! String).hasPrefix("/")))
        let fresh = try ProjectStore(directory: root.appendingPathComponent("support"))
        let loaded = try fresh.load(destination)
        assert((loaded["instruments"] as! [[String: Any]])[0]["samplePath"] as! String == path1)
        let recovery = try fresh.restore()
        assert(recovery != nil)
        try fresh.update(loaded, dirty: false)
        try fresh.update(loaded)
        assert(!fresh.dirty, "Resending unchanged state after a save must not make the document dirty")
        var complex = project
        complex["instruments"] = [["id": 0, "samplePath": sample.path, "zones": [["samplePath": sample.path, "rootNote": 60]]]]
        complex["audioClips"] = [["id": "clip", "track": 0, "path": sample.path, "startBeat": 0, "durationBeats": 4, "takes": [["path": sample.path]]]]
        let complexURL = root.appendingPathComponent("Complex.supertracker")
        _ = try fresh.save(to: complexURL, snapshot: complex)
        let complexRaw = try JSONSerialization.jsonObject(with: Data(contentsOf: complexURL)) as! [String: Any]
        let clips = complexRaw["audioClips"] as! [[String: Any]]
        assert(!(clips[0]["path"] as! String).hasPrefix("/"), "Clips must be collected")
        let zones = (complexRaw["instruments"] as! [[String: Any]])[0]["zones"] as! [[String: Any]]
        assert(!(zones[0]["samplePath"] as! String).hasPrefix("/"), "Zones must be collected")
        let reloaded = try fresh.load(complexURL)
        assert(((reloaded["audioClips"] as! [[String: Any]])[0]["path"] as! String).hasPrefix("/"))
        var malformed = project; malformed["bpm"] = -10.0
        do { try fresh.update(malformed); fatalError("Malformed tempo accepted") } catch {}
        malformed = project; malformed["version"] = 100
        do { try fresh.update(malformed); fatalError("Unknown version accepted") } catch {}
        // Sixteen inserts per channel: a track with twelve inserts and a kit Group with thirteen save and
        // reopen unchanged, and the routing graph accepts slots 0...15 on both but not slot 16.
        var deep = project
        deep["tracks"] = [["id": 0, "routingID": "t0"], ["id": 1, "routingID": "lane", "kind": "kit", "kitID": "k", "soundID": "s0"]]
        deep["patterns"] = [["id": 0, "length": 64, "steps": [[], []]]]
        deep["kits"] = [["id": "k", "name": "Deep", "busID": "g", "sounds": (0..<16).map { ["id": "s\($0)", "name": "Sound \($0)", "instrument": ["type": "sample"]] }]]
        deep["routing"] = ["version": 1, "mixModel": "standard", "buses": [["id": "g", "name": "Deep", "type": "group", "volume": 1.0]], "outputs": [String: Any](), "sends": [Any](), "sidechains": [Any]()]
        func insert(_ owner: String, _ slot: Int) -> [String: Any] {
            ["instanceID": owner.replacingOccurrences(of: ":", with: "-") + "-\(slot)", "owner": owner, "kind": "effect", "slot": slot, "component": "stfx:eq", "format": "Builtin", "state": "AA=="]
        }
        deep["audioPlugins"] = (0..<12).map { insert("track:t0", $0) } + (0..<13).map { insert("bus:g", $0) }
        try fresh.update(deep)
        let deepURL = root.appendingPathComponent("Deep.supertracker")
        _ = try fresh.save(to: deepURL, snapshot: deep)
        let deepLoaded = try fresh.load(deepURL)
        func slots(_ p: [String: Any], _ owner: String) -> [Int] { (p["audioPlugins"] as! [[String: Any]]).filter { $0["owner"] as? String == owner }.compactMap { $0["slot"] as? Int }.sorted() }
        precondition(slots(deepLoaded, "track:t0") == Array(0..<12) && slots(deepLoaded, "bus:g") == Array(0..<13), "Deep inserts must save and reopen unchanged")
        let deepDefinition = try AudioRouting.parse(project: deepLoaded)
        let full = ["track:t0", "bus:g", "master"].flatMap { owner in (0..<MixerChannel.insertSlotCount).map { AudioRouting.PluginLayout(instanceID: owner.replacingOccurrences(of: ":", with: "-") + "-\($0)", owner: owner, slot: $0) } }
        _ = try deepDefinition.graph(pluginLayouts: full)
        do { _ = try deepDefinition.graph(pluginLayouts: [AudioRouting.PluginLayout(instanceID: "beyond", owner: "bus:g", slot: MixerChannel.insertSlotCount)]); fatalError("Slot 16 accepted") } catch {}
        let templateSupport = root.appendingPathComponent("template-support")
        let templateStore = try ProjectStore(directory: templateSupport)
        try templateStore.update(project); try templateStore.autosave()
        let recoveryBefore = try Data(contentsOf: templateStore.recoveryURL)
        let currentBefore = try JSONSerialization.data(withJSONObject: templateStore.project!, options: .sortedKeys)
        var hardware = project
        hardware["name"] = "Eight tracks"
        hardware["tracks"] = (0..<8).map { ["id": $0, "name": "Track \($0 + 1)", "instrument": $0 < 3 ? $0 : -1] }
        hardware["instruments"] = (0..<3).map { ["id": $0, "name": ["Synth A Editor", "Synth B Editor", "Synth C Editor"][$0], "type": "midi"] }
        hardware["patterns"] = [["id": 0, "length": 64, "steps": (0..<8).map { _ in Array(repeating: NSNull(), count: 64) }, "automation": []]]
        let template = try templateStore.saveTemplate(project: hardware, name: "Hardware editors", id: "hardware-editors")
        try templateStore.setDefaultTemplate(id: "hardware-editors")
        assert(tryJSON(templateStore.project!) == currentBefore && templateStore.dirty)
        let recoveryAfter = try Data(contentsOf: templateStore.recoveryURL)
        assert(recoveryAfter == recoveryBefore, "Template saves must not change current recovery")
        let list = try templateStore.listTemplates()
        assert(list.count == 1 && list[0]["id"] as? String == "hardware-editors")
        let reopenedStore = try ProjectStore(directory: templateSupport)
        let recoveredProject = try reopenedStore.startupProject()
        assert(recoveredProject?["name"] as? String == "Test", "Recovery must take priority over the default template")
        try FileManager.default.removeItem(at: reopenedStore.recoveryURL)
        let startupStore = try ProjectStore(directory: templateSupport)
        let freshProject = try startupStore.startupProject()!
        assert((freshProject["tracks"] as! [Any]).count == 8 && startupStore.url == nil && !startupStore.dirty)
        assert((freshProject["patterns"] as! [[String: Any]])[0]["automation"] as? [Any] != nil)
        let blank = try startupStore.proposedNewProject(blank: true)
        assert(blank == nil, "Explicit blank/demo bypasses the default")
        do { try startupStore.setDefaultTemplate(id: "missing"); fatalError("Missing template became default") } catch {}
        let retainedDefault = try startupStore.defaultTemplateID()
        assert(retainedDefault == "hardware-editors")
        do { _ = try startupStore.saveTemplate(project: hardware, name: "Escape", id: "../escape"); fatalError("Template path traversal accepted") } catch {}
        try Data("broken".utf8).write(to: URL(fileURLWithPath: template["path"] as! String))
        do { _ = try startupStore.proposedNewProject(); fatalError("Corrupt default silently replaced the session") } catch {}
        assert(tryJSON(startupStore.project!) == tryJSON(freshProject), "Corrupt defaults must preserve the open session")
        try startupStore.setDefaultTemplate(id: nil)
        let cleared = try startupStore.proposedNewProject()
        assert(cleared == nil)
        try recoveryKeepsItsFile(root.appendingPathComponent("recovery-session"), project: project)
        try launchKeepsEarlierSessions(root.appendingPathComponent("launch-copies"), project: project)
        print("PASS project store: atomic persistence, portable media, repeated saves, recovery, recovered file identity, validation")
    }

    /// Each launch keeps the recovery file as it found it, before autosave or a New from the template can
    /// replace it: an unsaved session replaced on a relaunch stays openable from Recovery Backups.
    static func launchKeepsEarlierSessions(_ support: URL, project: [String: Any]) throws {
        func name(_ value: [String: Any]?) -> String? { value?["name"] as? String }
        var unsaved = project; unsaved["name"] = "Unsaved break"
        let first = try ProjectStore(directory: support)
        _ = try first.startupProject()
        try first.update(unsaved); try first.autosave()
        // Relaunch: the session comes back, then is replaced (here by a template-like project) and autosaved.
        let second = try ProjectStore(directory: support)
        let restored = try second.startupProject()
        assert(name(restored) == "Unsaved break")
        var template = project; template["name"] = "Template"
        try second.acceptNewProject(template); try second.autosave()
        let third = try ProjectStore(directory: support)
        let replaced = try third.startupProject()
        assert(name(replaced) == "Template")
        func copies() throws -> [URL] {
            try FileManager.default.contentsOfDirectory(at: third.recoveryBackupsURL, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "supertracker" }.sorted { $0.lastPathComponent > $1.lastPathComponent }
        }
        var kept = try copies()
        assert(kept.count == 2, "Each distinct session found at launch is kept")
        let older = try third.read(kept[1]), newer = try third.read(kept[0])
        assert(name(older) == "Unsaved break", "The replaced unsaved session can still be opened")
        assert(name(newer) == "Template")
        // Relaunching the same session adds no copy; old copies are pruned to the limit, newest first.
        _ = try ProjectStore(directory: support).startupProject()
        kept = try copies()
        assert(kept.count == 2, "An unchanged session is not copied again")
        for index in 0..<(ProjectStore.recoveryBackupLimit + 2) {
            var next = project; next["name"] = "Session \(index)"
            let store = try ProjectStore(directory: support)
            try store.update(next); try store.autosave()
            _ = try ProjectStore(directory: support).startupProject()
        }
        kept = try copies()
        assert(kept.count == ProjectStore.recoveryBackupLimit, "Launch copies are pruned to the limit")
        let newest = try third.read(kept[0])
        assert(name(newest) == "Session \(ProjectStore.recoveryBackupLimit + 1)", "The newest copy is kept")
    }

    /// After a relaunch, a session saved to a file reopens as that file: clean when nothing changed since the save,
    /// dirty when it had unsaved edits. If the file changed on disk (or the session record is missing or stale), it
    /// reopens untitled and dirty, and nothing ever writes the user's file except an explicit Save.
    static func recoveryKeepsItsFile(_ folder: URL, project: [String: Any]) throws {
        let support = folder.appendingPathComponent("support"), file = folder.appendingPathComponent("Song.supertracker")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let session = try ProjectStore(directory: support)
        try session.update(project)
        _ = try session.save(to: file, collectSamples: false)
        var reopened = try ProjectStore(directory: support)
        _ = try reopened.restore()
        assert(reopened.url?.standardizedFileURL == file.standardizedFileURL && !reopened.dirty, "A saved, unchanged session must reopen as its file, clean")
        var edited = project; edited["name"] = "Edited"
        try reopened.update(edited); try reopened.autosave()
        let savedBytes = try Data(contentsOf: file)
        reopened = try ProjectStore(directory: support)
        let recovered = try reopened.restore()
        assert(recovered?["name"] as? String == "Edited" && reopened.url?.standardizedFileURL == file.standardizedFileURL && reopened.dirty, "Unsaved edits reopen dirty on their file")
        try reopened.autosave()
        let afterAutosave = try Data(contentsOf: file)
        assert(afterAutosave == savedBytes, "Autosave after recovery must never write the user's file")
        // The file changed on disk since the session last saved it: recover untitled rather than offer to overwrite it.
        var external = project; external["name"] = "Saved elsewhere"
        try JSONSerialization.data(withJSONObject: external).write(to: file)
        reopened = try ProjectStore(directory: support)
        _ = try reopened.restore()
        assert(reopened.url == nil && reopened.dirty, "A changed file must not be reattached")
        // A record that does not describe this recovery file (crash between writes, older version) is ignored.
        _ = try session.save(to: file, collectSamples: false)
        var recovery = try JSONSerialization.jsonObject(with: Data(contentsOf: session.recoveryURL)) as! [String: Any]
        recovery["name"] = "Other session"
        try JSONSerialization.data(withJSONObject: recovery).write(to: session.recoveryURL)
        reopened = try ProjectStore(directory: support)
        _ = try reopened.restore()
        assert(reopened.url == nil && reopened.dirty, "A stale session record must not reattach a file")
        try FileManager.default.removeItem(at: session.recoverySessionURL)
        _ = try session.save(to: file, collectSamples: false); try FileManager.default.removeItem(at: session.recoverySessionURL)
        reopened = try ProjectStore(directory: support)
        _ = try reopened.restore()
        assert(reopened.url == nil && reopened.dirty, "Recovery without a session record keeps the old untitled, dirty behavior")
        // Untitled sessions stay untitled and dirty.
        try session.acceptNewProject(project); try session.autosave()
        reopened = try ProjectStore(directory: support)
        _ = try reopened.restore()
        assert(reopened.url == nil && reopened.dirty, "An untitled session reopens untitled and dirty")
    }
}

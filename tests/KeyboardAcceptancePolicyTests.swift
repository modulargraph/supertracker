import Foundation

@main struct KeyboardAcceptancePolicyTests {
    static func main() throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("KeyboardPolicy-\(UUID().uuidString)")
        try files.createDirectory(at:root,withIntermediateDirectories:true)
        defer { try? files.removeItem(at:root) }
        let policy = KeyboardAcceptancePolicy(directory:root)
        func rejected(_ command: String, _ payload: [String: Any] = [:]) {
            do { _ = try policy.prepare(command,payload:payload); fatalError("Unexpected allowed command: \(command)") }
            catch { }
        }
        for command in ["plugin.load","plugin.loadNKS","plugin.editor","plugins.recovery.retry","vst3.scan","project.open","project.new","dialog.openSample","project.saveAs","project.media.search","project.media.apply","audio.record","audio.configure","library.chooseRoot"] { rejected(command) }
        let external = root.deletingLastPathComponent().appendingPathComponent("outside.wav").path
        rejected("sample.load",["path":external])
        rejected("library.scan",["roots":[external]])
        rejected("sample.load",["path":root.path + "-sibling/sample.wav"])
        let symlink = root.appendingPathComponent("escape")
        try files.createSymbolicLink(at:symlink,withDestinationURL:root.deletingLastPathComponent())
        rejected("sample.load",["path":symlink.appendingPathComponent("outside.wav").path])
        let internalPath = root.appendingPathComponent("sample.wav").path
        _ = try policy.prepare("sample.load",payload:["path":internalPath])
        let midi = try policy.prepare("midi.configure",payload:["enabled":true,"output":"Hardware","channel":3])
        precondition(midi["enabled"] as? Bool == false && midi["output"] as? String == "none")
        let preview = try policy.prepare("sample.preview",payload:["path":internalPath,"volume":1.0])
        precondition(preview["volume"] as? Int == 0)
        let edit = try policy.prepare("sample.edit",payload:["path":internalPath,"outputDirectory":external])
        precondition(policy.contains(edit["outputDirectory"] as! String))
        let muted = try policy.prepare("engine.project",payload:["project":["masterVolume":1,"audioPlugins":[],"instruments":[["type":"sample","samplePath":internalPath]]]])
        precondition((muted["project"] as? [String: Any])?["masterVolume"] as? Int == 0)
        rejected("engine.project",["project":["audioPlugins":[["component":"vendor"]]]])
        rejected("engine.project",["project":["instruments":[["type":"midi"]]]])
        rejected("engine.project",["project":["instruments":[["type":"sample","zones":[["samplePath":external]]]]]])
        _ = try policy.prepare("transport.play",payload:[:])
        _ = try policy.prepare("audio.devices",payload:[:])
        let explicitSave = root.appendingPathComponent("fixture.supertracker").path
        let explicitPayload = try policy.prepare("project.save",payload:["path":explicitSave])
        precondition(explicitPayload["path"] as? String == explicitSave)
        rejected("project.save",["path":external])
        rejected("project.save",["path":symlink.appendingPathComponent("outside.supertracker").path])
        for invalid in ["", 42, NSNull()] as [Any] { rejected("project.save",["path":invalid]) }

        // Reproduce the production initialization order: an initial private save
        // writes Recovery, then a restore without a matching session record (older
        // recovery, file changed on disk) reopens untitled. Cmd-S must still supply
        // a destination, without consulting any file panel.
        let store = try ProjectStore(directory:root.appendingPathComponent("Support",isDirectory:true))
        let destination = root.appendingPathComponent("Keyboard fixture.supertracker")
        let project: [String:Any] = ["version":1,"name":"Private keyboard fixture","bpm":120.0,"masterVolume":0,
            "tracks":[["id":0]],"instruments":[["id":0,"type":"synth"]],"audioPlugins":[],
            "patterns":[["id":0,"length":4,"steps":[[["note":60]]]]],"sequence":[0]]
        try store.update(project)
        _ = try store.save(to:destination,collectSamples:false)
        try FileManager.default.removeItem(at:store.recoverySessionURL)
        _ = try store.restore()
        precondition(store.url == nil, "Fixture must exercise the restored untitled-document case")
        var edited = store.project!; edited["name"] = "Private edited take"; edited["bpm"] = 137.0
        let save = try policy.prepare("project.save",payload:["project":edited])
        guard let privatePath = save["path"] as? String else { fatalError("Cmd-S would fall through to a native save panel") }
        precondition(privatePath == destination.path && policy.contains(privatePath))
        _ = try store.save(to:URL(fileURLWithPath:privatePath),snapshot:save["project"] as? [String:Any],collectSamples:false)
        let reopened = try store.read(destination)
        precondition(reopened["name"] as? String == "Private edited take" && reopened["bpm"] as? Double == 137)
        precondition(store.url == destination && !store.dirty)
        let savedBytes = try Data(contentsOf:destination)
        rejected("project.save",["path":external,"project":edited])
        let bytesAfterRejection = try Data(contentsOf:destination)
        precondition(bytesAfterRejection == savedBytes)
        // The injected fallback is validated too, including an escaped symlink.
        try files.removeItem(at:destination)
        try files.createSymbolicLink(at:destination,withDestinationURL:URL(fileURLWithPath:external))
        rejected("project.save",["project":edited])
        print("PASS Cmd-S after recovery injects a checked private destination and persists edits without a file panel; explicit external and escaped default paths are rejected")
        print("PASS Keyboard QA policy: external paths/symlinks, plug-ins, hardware, file panels and capture blocked; fixture audio muted; navigation/transport/private save allowed")
    }
}

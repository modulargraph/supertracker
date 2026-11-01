import Foundation

/// This optional boundary belongs only to the keyboard fixture app. Production
/// Bridge instances have no policy and retain their normal command behavior.
struct KeyboardAcceptancePolicy {
    let directory: URL

    private func deny(_ operation: String) -> NSError {
        NSError(domain:"SuperTracker.KeyboardAcceptance",code:1,userInfo:[NSLocalizedDescriptionKey:"\(operation) is outside this isolated keyboard session. Use the normal app for plug-ins, external files, hardware and recording."])
    }

    func contains(_ path: String) -> Bool {
        let root = directory.standardizedFileURL.resolvingSymlinksInPath().path
        // Resolve the existing parent as well: realpath-style resolution of an
        // absent destination does not consistently follow its parent symlink.
        var ancestor = URL(fileURLWithPath:path).standardizedFileURL
        var suffix: [String] = []
        while !FileManager.default.fileExists(atPath:ancestor.path) && ancestor.path != "/" {
            if (try? FileManager.default.destinationOfSymbolicLink(atPath:ancestor.path)) != nil { return false }
            suffix.append(ancestor.lastPathComponent); ancestor.deleteLastPathComponent()
        }
        var resolved = ancestor.resolvingSymlinksInPath()
        for component in suffix.reversed() { resolved.appendPathComponent(component) }
        let candidate = resolved.path
        return candidate == root || candidate.hasPrefix(root + "/")
    }

    func prepare(_ command: String, payload: [String: Any]) throws -> [String: Any] {
        if command.hasPrefix("plugin.") || command == "plugins.recovery.retry" || command.hasPrefix("vst3.") {
            throw deny("Plug-in loading and scanning")
        }
        if command.hasPrefix("project.media.") { throw deny("Media relinking") }
        if ["project.open","project.new","project.saveAs","project.importMIDI","project.exportMIDI","dialog.openSample","dialog.export","project.media.chooseRoot","library.chooseRoot","library.addRoot","app.external","audio.record","audio.configure","audio.bounceTrack","audio.export"].contains(command) {
            throw deny(command)
        }
        var result = payload
        if command == "project.save" {
            // Bridge restores the fixture recovery document without a saved URL.
            // Always supply a private destination for Cmd-S so that normal save
            // handling cannot fall through to an unrestricted native file panel.
            if result["path"] == nil { result["path"] = directory.appendingPathComponent("Keyboard fixture.supertracker").path }
            guard let path = result["path"] as? String, !path.isEmpty else { throw deny("Invalid save destination") }
        }
        if command == "midi.configure" { result["enabled"] = false; result["output"] = "none" }
        if command == "sample.preview" { result["volume"] = 0 }
        if command == "sample.edit" { result["outputDirectory"] = directory.appendingPathComponent("Sample edits",isDirectory:true).path }
        if let path = result["path"] as? String, !contains(path) { throw deny("External file access") }
        if let roots = result["roots"] as? [String], roots.contains(where:{!contains($0)}) { throw deny("External library scanning") }
        if command == "engine.project" || command == "project.autosave" || command == "project.save" {
            var project = result["project"] as? [String: Any] ?? (command == "engine.project" ? result : [:])
            if !project.isEmpty {
                guard (project["audioPlugins"] as? [Any] ?? []).isEmpty else { throw deny("Plug-in projects") }
                for instrument in project["instruments"] as? [[String: Any]] ?? [] {
                    if ["au","midi"].contains(instrument["type"] as? String ?? "") || instrument["plugin"] as? [String: Any] != nil {
                        throw deny("Plug-in or MIDI instruments")
                    }
                }
                func checkMedia(_ value: Any) throws {
                    if let object = value as? [String: Any] {
                        for (key,item) in object {
                            if ["path","samplePath"].contains(key), let path = item as? String, !path.isEmpty, !BuiltinSamples.isBuiltin(path), !contains(path) { throw deny("External project media") }
                            try checkMedia(item)
                        }
                    } else if let array = value as? [Any] { for item in array { try checkMedia(item) } }
                }
                try checkMedia(project)
                project["masterVolume"] = 0
                if result["project"] != nil { result["project"] = project } else { result = project }
            }
        }
        return result
    }
}

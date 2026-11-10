import AppKit
import WebKit
import UniformTypeIdentifiers

@MainActor
final class Bridge: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
    weak var webView: WKWebView?
    weak var agentControl: AgentControlServer?
    let audio: AudioHost
    let library: LibraryService
    let store: ProjectStore
    private let keyboardAcceptance: KeyboardAcceptancePolicy?
    private let pluginDiscoveryEnabled: Bool
    private let audioSnapshotProvider: (@MainActor () async throws -> Any)?
    private let mediaRelinking = MediaRelinkService()
    private let contextMenu = NativeContextMenu()
    private var autosaveTimer: Timer?
    private var recoverySaveTask: Task<Void, Error>?
    private var foregroundPersistence = 0
    private var projectPublicationRevision: UInt64 = 0
    private var projectSessionRevision: UInt64 = 0
    private var preparedTerminationSession: (revision: UInt64, url: URL?)?
    private var isReady = false
    private var initialProject: [String: Any]?
    private var startupWarning: String?
    private var pendingOpenURL: URL?
    private var snapshotEvents = SnapshotEventGate()
    private static let flushEditsScript = """
    const timeout = new Promise(resolve => setTimeout(resolve, 3000));
    const work = (async () => {
        await window.STInstrumentEdits?.waitForIdle?.();
        await window.STKits?.waitForIdle?.();
        window.STMK3?.commitBursts?.({nativeCapture:true,render:false});
        await window.STMK3?.flushEdits?.({nativeCapture:true,render:false});
        window.STNKSControls?.controller?.finish?.();
        window.STAutomationRecord?.finishPass?.({nativeCapture:true,render:false});
        await window.STNKSControls?.flushEdits?.();
        await window.STPluginParameters?.flushEdits?.();
        await window.STAgent?.flushEdits?.();
    })().catch(() => {});
    await Promise.race([work, timeout]);
    return true;
    """
    private static let projectSnapshotScript = """
    (() => {
        if (window.ST?.isChangingSession) return {ok:false,reason:'Wait for the current save or project change to finish before quitting.'};
        if (window.STKits?.isBusy) return {ok:false,reason:'Wait for the kit change to finish before quitting.'};
        if (window.STInstrumentEdits?.isBusy) return {ok:false,reason:'Wait for the instrument or sample change to finish before quitting.'};
        if (window.STStudio?.isMovingEffect) return {ok:false,reason:'Wait for the effect move to finish before quitting.'};
        if (window.STRouting?.isApplying) return {ok:false,reason:'Wait for the routing change to finish before quitting.'};
        if (window.STTrackDuplicate?.isApplying) return {ok:false,reason:'Wait for the track duplication to finish before quitting.'};
        if (window.STMK3?.hasPendingEdits) return {ok:false,reason:'Finish or retry the pending Maschine control edit before quitting.'};
        if (window.STAutomationRecord?.hasPendingEdits) return {ok:false,reason:'Finish or retry the pending automation take before quitting.'};
        if (window.STInputCapture?.hasPendingEdits) return {ok:false,reason:'Wait for the pending input recording edge before quitting.'};
        if (window.STNKSControls?.hasPendingEdits || window.STPluginParameters?.hasPendingEdits || window.STAgent?.hasPendingEdits) return {ok:false,reason:'Finish or retry the pending plug-in control edit before quitting.'};
        if (window.STExport?.isBusy) return {ok:false,reason:'Finish or cancel the audio export before quitting.'};
        if (window.STRecording?.isBusy || window.STRecording?.isRecording) return {ok:false,reason:'Finish the audio recording before quitting.'};
        if (window.ST?.startupPending) return {ok:true,project:null};
        return {ok:true,project:window.__supertracker && window.__supertracker.getProject ? window.__supertracker.getProject() : null};
    })()
    """

    init(audio: AudioHost, library: LibraryService, store: ProjectStore, keyboardAcceptance: KeyboardAcceptancePolicy? = nil,
         pluginDiscoveryEnabled: Bool = true, audioSnapshotProvider: (@MainActor () async throws -> Any)? = nil) {
        self.audio = audio; self.library = library; self.store = store
        self.keyboardAcceptance = keyboardAcceptance
        self.pluginDiscoveryEnabled = pluginDiscoveryEnabled
        self.audioSnapshotProvider = audioSnapshotProvider
        super.init()
        audio.onEvent = { [weak self] type, payload in Task { @MainActor in self?.event(type, payload) } }
        library.onEvent = { [weak self] type, payload in Task { @MainActor in self?.event(type, payload) } }
        mediaRelinking.onProgress = { [weak self] payload in self?.event("project.media.progress", payload) }
        do {
            initialProject = try store.startupProject()
        } catch { startupWarning = "Could not restore the startup project: \(error.localizedDescription) Opening a blank session." }
        autosaveTimer = Timer.scheduledTimer(withTimeInterval: 20, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.store.project != nil else { return }
                do { _ = try await self.saveRecovery(project:nil) } catch { self.event("error", ["message": "Recovery save failed: \(error.localizedDescription)"]) }
            }
        }
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame, let body = message.body as? [String: Any], let command = body["command"] as? String else { return }
        let id = body["id"] ?? NSNull()
        let payload = body["payload"] as? [String: Any] ?? [:]
        Task {
            do {
                let result = try await handle(command, payload: payload)
                send("nativeReceive", ["id": id, "ok": true, "result": result])
            } catch {
                send("nativeReceive", ["id": id, "ok": false, "error": error.localizedDescription])
            }
        }
    }

    func event(_ type: String, _ payload: [String: Any]) {
        guard isReady, let token = snapshotEvents.begin(type, now: ProcessInfo.processInfo.systemUptime) else { return }
        send("nativeEvent", ["type": type, "payload": payload]) { [weak self] in self?.snapshotEvents.finish(type, token: token) }
    }

    func command(_ name: String) { event("command", ["name": name]) }

    func requestOpenProject(_ url: URL) {
        if isReady { event("project.openRequested", ["path": url.path]) }
        else { pendingOpenURL = url }
    }

    /// `delivered` runs once the page has handled the call, or at once when it cannot be sent.
    private func send(_ function: String, _ data: [String: Any], delivered: (@MainActor () -> Void)? = nil) {
        guard let webView else { delivered?(); return }
        guard JSONSerialization.isValidJSONObject(data), let encoded = try? JSONSerialization.data(withJSONObject: data, options: [.fragmentsAllowed]), let json = String(data: encoded, encoding: .utf8) else {
            // A result JSON cannot carry must still settle the page's request, as an error (which always encodes).
            if function == "nativeReceive", data["ok"] as? Bool == true, let id = data["id"], JSONSerialization.isValidJSONObject(["id": id]) {
                NSLog("Bridge delivery: a native reply could not be encoded as JSON")
                send(function, ["id": id, "ok": false, "error": "The native reply could not be encoded for the interface."], delivered: delivered)
            } else { delivered?() }
            return
        }
        webView.evaluateJavaScript("window.\(function) && window.\(function)(\(json));") { _, error in
            if let error { NSLog("Bridge delivery: %@", error.localizedDescription) }
            delivered?()
        }
    }

    func handle(_ command: String, payload incoming: [String: Any]) async throws -> Any {
        let payload = try keyboardAcceptance?.prepare(command,payload:incoming) ?? incoming
        if keyboardAcceptance != nil && command == "plugins.list" { return [[String: Any]]() }
        if command == "engine.project" {
            try store.validate(payload["project"] as? [String: Any] ?? payload)
            projectPublicationRevision &+= 1
        }
        let publicationRevision = projectPublicationRevision
        // A settled engine publication also owns the store checkpoint. Recovery
        // must not capture its new rack into the preceding stored project while
        // plug-in instantiation is suspended.
        let foreground = ["engine.project","project.save","project.saveAs","project.new","project.open","project.importMIDI","kit.save"].contains(command)
        if foreground { foregroundPersistence += 1 }
        defer { if foreground { foregroundPersistence -= 1 } }
        if foreground {
            // An existing recovery capture owns its store checkpoint until it
            // finishes. New background requests defer from this point onward.
            if let pending = recoverySaveTask { _ = try? await pending.value }
            try Task.checkCancellation()
            if command == "engine.project", publicationRevision != projectPublicationRevision {
                throw TrackerError.message("The session changed while its audio project was being prepared. The newer session was preserved.")
            }
        }
        switch command {
        case "ready":
            isReady = true
            // The window must receive the startup session even when its audio cannot be prepared: without it the
            // window keeps a placeholder while recovery still holds (often unsaved) work the user would then replace.
            var audioError: String?
            if let initialProject {
                do { _ = try await audio.handle("engine.project",payload:["project":initialProject,"replace":true,"forcePluginState":true]) }
                catch { audioError = error.localizedDescription }
            }
            let roots = try await library.handle("library.roots", payload: [:])
            let plugins: Any = keyboardAcceptance != nil ? [[String: Any]]() : try await audio.handle("plugins.list", payload: [:])
            let audioInfo = try await audio.handle("audio.devices", payload: [:])
            let midi = try await audio.handle("midi.devices", payload: [:])
            var result: [String: Any] = ["version": "0.1.0", "project": initialProject as Any? ?? NSNull(), "dirty": store.dirty, "roots": roots, "plugins": plugins, "audio": audioInfo, "midi": midi]
            result["pluginRecovery"] = try await audio.handle("plugins.recovery", payload: [:])
            if let startupWarning { result["warning"] = startupWarning }
            if let audioError { result["audioError"] = audioError }
            if let pendingOpenURL { result["openPath"] = pendingOpenURL.path; self.pendingOpenURL = nil }
            #if SELFTESTS
            if let webView { DevHarness.startIfRequested(webView: webView) }
            #endif
            if pluginDiscoveryEnabled && keyboardAcceptance == nil {
                // Discover new or updated plug-ins in isolated helpers once the UI is up.
                Task { @MainActor [weak self] in try? await Task.sleep(nanoseconds: 2_000_000_000); self?.startPluginScan() }
            }
            return result
        case "engine.project":
            // Validated once on entry to handle(), before the publication revision.
            let project = payload["project"] as? [String: Any] ?? payload
            let revision = publicationRevision
            let checkpoint = try projectCheckpoint()
            let restorePluginState = (payload["forcePluginState"] as? Bool ?? false) || (payload["replace"] as? Bool ?? false)
            let result = try await audio.handle(command, payload: ["project": project, "forcePluginState": restorePluginState, "replace":payload["replace"] as? Bool ?? false, "awaitPlugins":payload["awaitPlugins"] as? Bool ?? false])
            try Task.checkCancellation()
            guard revision == projectPublicationRevision, try projectCheckpoint() == checkpoint else {
                throw TrackerError.message("The session changed while its audio project was being prepared. The newer session was preserved.")
            }
            try store.update(project)
            return result
        case "project.autosave":
            return try await saveRecovery(project:payload["project"] as? [String:Any])
        case "project.templates.list":
            return ["templates": try store.listTemplates(), "defaultID": try store.defaultTemplateID() as Any? ?? NSNull()]
        case "project.templates.save":
            guard let project = payload["project"] as? [String: Any] ?? store.project,
                  let name = payload["name"] as? String else { throw TrackerError.message("Choose a project and name for the template.") }
            return try store.saveTemplate(project: project, name: name, id: payload["id"] as? String)
        case "project.templates.setDefault":
            guard payload["id"] is String || payload["id"] is NSNull else { throw TrackerError.message("Choose a template identifier, or null for a blank default.") }
            try store.setDefaultTemplate(id: payload["id"] as? String)
            return ["defaultID": try store.defaultTemplateID() as Any? ?? NSNull()]
        case "project.new":
            let newProject = try store.proposedNewProject(templateID: payload["templateID"] as? String, blank: payload["blank"] as? Bool ?? false)
            try audio.requireSessionReplacementAllowed()
            try store.autosave()
            projectPublicationRevision &+= 1
            projectSessionRevision &+= 1
            _ = try await audio.handle("engine.panic", payload: [:])
            _ = try await audio.handle("transport.recordArm", payload: ["armed":false])
            try store.acceptNewProject(newProject)
            if let newProject { return ["project": newProject] }
            return NSNull()
        case "project.open":
            try audio.requireSessionReplacementAllowed()
            let chosen: URL?
            if let path = payload["path"] as? String { chosen = URL(fileURLWithPath: path) }
            else { chosen = try await openPanel(directory: false, types: [UTType(filenameExtension: "supertracker") ?? .json, .json]) }
            guard let chosen else { return ["cancelled": true] }
            try audio.requireSessionReplacementAllowed()
            try store.autosave()
            let project = try store.read(chosen)
            projectPublicationRevision &+= 1
            projectSessionRevision &+= 1
            _ = try await audio.handle("engine.project", payload: ["project": project, "forcePluginState": true, "replace":true])
            _ = try await audio.handle("transport.recordArm", payload: ["armed":false])
            store.acceptLoaded(project, from: chosen)
            return ["project": project, "path": chosen.path, "migrated": store.lastReadWasMigrated, "migrationNotes": store.lastReadWasMigrated ? store.lastMigrationNotes : [], "dirty": store.dirty]
        case "project.importMIDI":
            try audio.requireSessionReplacementAllowed()
            let chosen: URL?
            if let path = payload["path"] as? String { chosen = URL(fileURLWithPath: path) }
            else { chosen = try await openPanel(directory: false, types: [UTType(filenameExtension: "mid") ?? .data, UTType(filenameExtension: "midi") ?? .data]) }
            guard let chosen else { return ["cancelled": true] }
            try audio.requireSessionReplacementAllowed()
            try store.autosave()
            let project = try MIDIFile.importProject(chosen)
            try store.validate(project)
            projectPublicationRevision &+= 1
            projectSessionRevision &+= 1
            _ = try await audio.handle("engine.project", payload: ["project": project, "forcePluginState": true, "replace":true])
            _ = try await audio.handle("transport.recordArm", payload: ["armed":false])
            store.newProject(); try store.update(project)
            return ["project": project, "path": chosen.path, "warnings": (project["midiImport"] as? [String: Any])?["warnings"] ?? []]
        case "project.exportMIDI":
            let project = payload["project"] as? [String: Any] ?? store.project ?? [:]
            try store.validate(project)
            let chosen: URL?
            if let path = payload["path"] as? String { chosen = URL(fileURLWithPath: path) }
            else { chosen = try await savePanel(name: (project["name"] as? String ?? "Song") + ".mid", type: UTType(filenameExtension: "mid") ?? .data) }
            guard let chosen else { return ["cancelled": true] }
            return try MIDIFile.export(project, to: chosen)
        case "project.media.inspect", "project.media.search", "project.media.apply":
            let candidate: [String: Any]
            if let supplied = payload["project"] {
                guard let project = supplied as? [String: Any] else { throw TrackerError.message("Provide the current project to inspect or relink its media.") }
                candidate = project
            } else if let project = store.project { candidate = project }
            else { throw TrackerError.message("Open a project before relinking media.") }
            try store.validate(candidate)
            // Freeze the unsaved UI snapshot and its path context before leaving the
            // main actor. These commands must never commit stale async work themselves.
            let projectURL = store.url
            if command == "project.media.inspect" {
                return try await mediaRelinking.inspect(candidate, projectURL: projectURL)
            }
            if command == "project.media.search" {
                guard payload["roots"] == nil || payload["roots"] is [String],
                      payload["relocations"] == nil || payload["relocations"] is [[String: Any]] else {
                    throw TrackerError.message("Media search folders and root mappings are invalid.")
                }
                return try await mediaRelinking.search(candidate, projectURL: projectURL,
                    roots: payload["roots"] as? [String] ?? [],
                    relocations: payload["relocations"] as? [[String: Any]] ?? [],
                    requestID: payload["requestId"] as? String ?? UUID().uuidString)
            }
            guard let selections = payload["selections"] as? [[String: Any]] else {
                throw TrackerError.message("Select replacement files before applying media relinks.")
            }
            let proposed = try await mediaRelinking.apply(candidate, selections: selections, projectURL: projectURL)
            guard let project = proposed["project"] as? [String: Any] else { throw TrackerError.message("The media relinker returned an invalid project.") }
            try store.validate(project)
            return proposed
        case "project.media.cancel": return mediaRelinking.cancel()
        case "project.media.chooseRoot":
            guard let chosen = try await openPanel(directory: true, types: []) else { return ["cancelled": true] }
            return ["path": chosen.path]
        case "project.save", "project.saveAs":
            if let project = payload["project"] as? [String: Any] { try store.update(project) }
            var destination = command == "project.saveAs" ? nil : store.url
            if let path = payload["path"] as? String { destination = URL(fileURLWithPath: path) }
            if destination == nil { destination = try await savePanel(name: (store.project?["name"] as? String ?? "Untitled") + ".supertracker", type: UTType(filenameExtension: "supertracker") ?? .json) }
            guard let destination else { return ["cancelled": true] }
            var snapshot = store.project ?? [:]
            let checkpoint = try projectCheckpoint()
            snapshot["audioPlugins"] = try await capturedPluginStates()
            try requireProjectCheckpoint(checkpoint)
            let saved = try store.save(to: destination, snapshot: snapshot, collectSamples: payload["collectSamples"] as? Bool ?? true)
            var result: [String: Any] = ["project": saved, "path": destination.path, "saved": true]
            if let warning = store.lastRecoveryWarning { result["recoveryWarning"] = warning }
            return result
        case "builtin.drumModels": return BuiltinEffectsAudioUnit.drumModels()
        case "builtin.effects": return BuiltinEffectsAudioUnit.effectMetadata()
        case "kit.read", "kit.detail":
            guard let path = payload["path"] as? String, path.hasPrefix("/") else { throw TrackerError.message("Choose an available kit from the browser.") }
            let source = URL(fileURLWithPath: path)
            let contents = try await Task.detached(priority: .userInitiated) { () throws -> [String: Any] in
                if source.pathExtension.lowercased() == "mxgrp" { return try MaschineGroupImporter.read(at: source) }
                return try KitLibrary.read(at: source)
            }.value
            guard let project = contents["project"] as? [String: Any] else { throw TrackerError.message("The kit has no readable project data.") }
            let kit = try KitProject(project: project)
            guard kit.kits.count == 1 else { throw TrackerError.message("A library kit must contain one kit definition.") }
            if command == "kit.read" {
                // Kits load as track groups: the kit's loaded pads as member tracks plus its group bus.
                var result = contents
                result["groupProject"] = try KitGroups.groupProject(fromKitProject: project)
                return result
            }
            var detail = contents["metadata"] as? [String: Any] ?? [:]
            let plugins = project["audioPlugins"] as? [[String: Any]] ?? []
            var warnings = contents["warnings"] as? [String] ?? []
            let available = (try? await audio.handle("plugins.list", payload: [:])) as? [[String: Any]] ?? []
            let componentIDs = Set(available.compactMap { $0["id"] as? String })
            let missingPlugins = plugins.filter { plugin in
                guard let component = plugin["component"] as? String else { return true }
                return !componentIDs.contains(component)
            }.map { $0["name"] as? String ?? $0["component"] as? String ?? "Unknown plug-in" }
            for name in Set(missingPlugins).sorted() { warnings.append("Plug-in unavailable on this Mac: " + name) }
            detail["sounds"] = kit.sounds.map { sound -> [String: Any] in
                let plugin = plugins.first { $0["owner"] as? String == sound.owner && $0["kind"] as? String == "instrument" }
                let sample = sound.instrument["samplePath"] as? String ?? ""
                return ["slot": sound.pad, "id": sound.soundID, "name": sound.name, "type": sound.instrument["type"] ?? "sample",
                        "plugin": plugin?["name"] ?? "", "empty": sample.isEmpty && (sound.instrument["type"] as? String ?? "sample") == "sample" && (sound.instrument["zones"] as? [Any] ?? []).isEmpty, "missing": !sample.isEmpty && !FileManager.default.fileExists(atPath: sample)]
            }
            detail["compatibility"] = ["summary": warnings.isEmpty ? "Ready to load" : "Review import notes", "loadable": true, "warnings": warnings, "missingPlugins": missingPlugins]
            detail["path"] = path
            if let preview = KitLibrary.previewURL(for: source) { detail["previewPath"] = preview.path }
            return detail
        case "kit.save":
            guard let groupID = payload["groupID"] as? String ?? payload["kitID"] as? String, var project = store.project else { throw TrackerError.message("Select a kit to save.") }
            // Save overwrites the kit's own library package (payload "path"); Save As names a new
            // package and reports an existing one instead of silently numbering a copy.
            let directory = KitLibrary.userDirectory
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let libraryRoot = directory.standardizedFileURL.resolvingSymlinksInPath().path + "/"
            let named = (payload["name"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
            if let named, named.isEmpty || named.count > 200 || named.hasPrefix(".") { throw TrackerError.message("Choose a kit name of up to 200 characters.") }
            let overwrite = payload["overwrite"] as? Bool ?? false
            var chosen: URL?
            if let path = payload["path"] as? String {
                let url = URL(fileURLWithPath: path).standardizedFileURL
                // Only packages in the user kit library are replaced in place; Save of any other package becomes Save As.
                if overwrite, !(path.hasPrefix("/") && url.pathExtension.lowercased() == "stkit" &&
                      url.deletingLastPathComponent().resolvingSymlinksInPath().path.appending("/").hasPrefix(libraryRoot)) { return ["needsName": true, "path": path] }
                chosen = url
            } else if let named {
                let url = directory.appendingPathComponent(named + ".stkit", isDirectory: true)
                if FileManager.default.fileExists(atPath: url.path), !overwrite { return ["exists": true, "path": url.path, "name": named] }
                chosen = url
            }
            let checkpoint = try projectCheckpoint()
            project["audioPlugins"] = try await capturedPluginStates()
            try requireProjectCheckpoint(checkpoint)
            // A group is written in the kit file layout (sixteen pads) so kit files stay compatible.
            let layout = try KitGroups.kitProject(fromGroup: project, groupID: groupID)
            let kitID = ((layout.project["kits"] as? [[String: Any]])?.first?["id"] as? String) ?? groupID
            let exported = try KitProject.libraryProject(layout.project, kitID: kitID)
            var metadata = exported.metadata
            var warnings = layout.warnings + exported.warnings
            if let named { metadata["name"] = named }
            let previewDirectory = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-KitPreview-" + UUID().uuidString, isDirectory: true)
            defer { try? FileManager.default.removeItem(at: previewDirectory) }
            if let preview = payload["previewPath"] as? String { metadata["previewPath"] = preview }
            else if payload["generatePreview"] as? Bool != false {
                do {
                    try FileManager.default.createDirectory(at: previewDirectory, withIntermediateDirectories: true)
                    let preview = previewDirectory.appendingPathComponent("Preview.wav")
                    let rendered = try await audio.handle("audio.kitPreview", payload: ["groupID": groupID, "path": preview.path]) as? [String: Any]
                    metadata["previewPath"] = preview.path
                    if let warning = rendered?["warning"] as? String { warnings.append(warning) }
                } catch { warnings.append("Kit saved without an audio preview: " + error.localizedDescription) }
                try requireProjectCheckpoint(checkpoint)
            }
            let safeName = (metadata["name"] as? String ?? "Kit").replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
            let destination: URL
            if let chosen { destination = chosen }
            else {
                var candidate = directory.appendingPathComponent(safeName + ".stkit", isDirectory: true), suffix = 2
                while FileManager.default.fileExists(atPath: candidate.path) { candidate = directory.appendingPathComponent(safeName + " \(suffix).stkit", isDirectory: true); suffix += 1 }
                destination = candidate
            }
            let relativeTo = store.url
            let collected = payload["copySamples"] as? Bool ?? true, replace = chosen != nil && overwrite
            var result = try await Task.detached(priority: .userInitiated) {
                try KitLibrary.save(project: exported.project, metadata: metadata, to: destination, copySamples: collected, relativeTo: relativeTo, overwrite: replace)
            }.value
            result["warnings"] = warnings
            _ = try? await library.handle("library.scan", payload: ["roots": [directory.path]])
            return result
        case "library.chooseRoot":
            guard let chosen = try await openPanel(directory: true, types: []) else { return ["cancelled": true] }
            return try await library.handle("library.scan", payload: ["roots": [chosen.path]])
        case "dialog.openSample":
            guard let chosen = try await openPanel(directory: false, types: [.audio]) else { return ["cancelled": true] }
            return ["path": chosen.path]
        case "dialog.export":
            if payload["mode"] as? String == "stems" {
                let panel = NSOpenPanel(); panel.canChooseDirectories = true; panel.canChooseFiles = false
                panel.allowsMultipleSelection = false; panel.canCreateDirectories = true
                panel.prompt = "Export here"; panel.message = "A new folder will be created here for this stem export."
                let chosen = try await present(panel)
                guard let chosen else { return ["cancelled": true] }
                let destination = try ExportDestination.reserveStemsFolder(parent: chosen, projectName: payload["name"] as? String ?? store.project?["name"] as? String ?? "Mixdown")
                return ["path": destination.path, "mode": "stems"]
            }
            guard let chosen = try await savePanel(name: (store.project?["name"] as? String ?? "Mixdown") + ".wav", type: .wav) else { return ["cancelled": true] }
            return ["path": chosen.path]
        case "plugins.scan":
            guard pluginDiscoveryEnabled else { throw TrackerError.message("Plug-in discovery is disabled for this isolated session.") }
            startPluginScan()
            return ["scanning": true]
        case "mk3.configure":
            guard let enabled = payload["enabled"] as? Bool else { throw TrackerError.message("Choose whether Maschine MK3 support is on.") }
            try setMaschineEnabled(enabled)
            return maschineStatus(enabled ? await MaschineMK3Driver.shared.connect(exclusive: false) : await MaschineMK3Driver.shared.disconnect())
        case "mk3.connect", "mk3.disconnect", "mk3.status", "mk3.leds", "mk3.screen":
            if command == "mk3.connect", !maschineEnabled {
                throw TrackerError.message("Maschine MK3 support is in development and off. Turn it on in Settings first.")
            }
            let result = try await MaschineMK3Driver.shared.handle(command, payload: payload)
            return (result as? [String: Any]).map(maschineStatus) ?? result
        case "ui.contextMenu":
            guard let webView, let items = payload["items"] as? [[String: Any]] else { return ["id": NSNull()] }
            let point = NSPoint(x: (payload["x"] as? NSNumber)?.doubleValue ?? 0, y: (payload["y"] as? NSNumber)?.doubleValue ?? 0)
            let chosen = contextMenu.show(items, at: point, in: webView)
            if let chosen, chosen.hasPrefix("native:") {
                // Standard editing goes through the responder chain, like the Edit menu.
                let name = String(chosen.dropFirst(7))
                if ["cut", "copy", "paste", "selectAll"].contains(name) { NSApp.sendAction(Selector(name + ":"), to: nil, from: nil) }
                return ["id": NSNull()]
            }
            return ["id": chosen as Any? ?? NSNull()]
        case "agent.status":
            return agentControl?.status ?? ["available": false, "enabled": false, "clients": 0]
        case "agent.configure":
            guard let agentControl else { throw TrackerError.message("AI control is not available in this window.") }
            if let enabled = payload["enabled"] as? Bool { agentControl.setEnabled(enabled) }
            return agentControl.status
        case "app.external":
            if let path = payload["path"] as? String, payload["reveal"] as? Bool == true {
                let url = URL(fileURLWithPath: path)
                guard FileManager.default.fileExists(atPath: url.path) else { throw TrackerError.message("This file is unavailable. Reconnect its drive or rescan the library.") }
                NSWorkspace.shared.activateFileViewerSelecting([url])
                return ["opened": true]
            }
            guard let path = payload["url"] as? String, let url = URL(string: path), ["https", "http"].contains(url.scheme ?? "") else { throw TrackerError.message("Only web links can be opened here.") }
            NSWorkspace.shared.open(url)
            return ["opened": true]
        default:
            if command.hasPrefix("library.") { return try await library.handle(command, payload: payload) }
            return try await audio.handle(command, payload: payload)
        }
    }

    /// Maschine MK3 support is in development: off by default, turned on in Settings (`mk3.configure`).
    /// Stored as {"enabled": Bool} in maschine-mk3.json in the support folder.
    var maschineSettingsURL: URL { store.support.appendingPathComponent("maschine-mk3.json") }
    var maschineEnabled: Bool {
        guard let data = try? Data(contentsOf: maschineSettingsURL),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return false }
        return object["enabled"] as? Bool ?? false
    }
    func setMaschineEnabled(_ on: Bool) throws {
        try FileManager.default.createDirectory(at: maschineSettingsURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONSerialization.data(withJSONObject: ["enabled": on]).write(to: maschineSettingsURL, options: .atomic)
    }
    func maschineStatus(_ status: [String: Any]) -> [String: Any] {
        var value = status; value["enabled"] = maschineEnabled; return value
    }

    /// Maschine MK3 in shared mode (buttons, pads, LEDs), only when turned on in Settings.
    /// Exclusive screen take-over only via mk3.connect.
    func startMaschine() {
        let driver = MaschineMK3Driver.shared
        driver.onInput = { [weak self] payload in self?.event("mk3.input", payload) }
        driver.onStatus = { [weak self] payload in self?.event("mk3.status", self?.maschineStatus(payload) ?? payload) }
        Task { @MainActor in
            // Restores NI agents a crashed earlier exclusive session left stopped, even when MK3 support is off.
            await driver.recoverAgentsIfNeeded()
            guard maschineEnabled, ProcessInfo.processInfo.environment["SUPERTRACKER_NO_MK3"] == nil else {
                event("mk3.status", maschineStatus(driver.status)); return
            }
            await driver.connect(exclusive: false)
        }
    }

    private var pluginScanState: [String: [String: Any]] = [:]
    private var pluginScanRunning = false

    /// Refreshes the VST3 and VST2 catalogues together and reports combined progress.
    func startPluginScan() {
        guard pluginDiscoveryEnabled, !pluginScanRunning else { return }
        pluginScanRunning = true
        let publish: @MainActor (String, [String: Any]) -> Void = { [weak self] format, value in
            guard let self else { return }
            self.pluginScanState[format] = value
            let states = Array(self.pluginScanState.values)
            self.event("plugins.catalogue", [
                "scanning": states.contains { $0["scanning"] as? Bool == true },
                "completed": states.reduce(0) { $0 + ($1["completed"] as? Int ?? 0) },
                "total": states.reduce(0) { $0 + ($1["total"] as? Int ?? 0) }])
        }
        VST3Catalogue.shared.onProgress = { value in Task { @MainActor in publish("VST3", value) } }
        VST2Catalogue.shared.onProgress = { value in Task { @MainActor in publish("VST2", value) } }
        Task { @MainActor [weak self] in
            async let vst3 = VST3Catalogue.shared.refresh()
            async let vst2: Void = VST2Catalogue.shared.refresh()
            _ = await (vst3, vst2)
            self?.pluginScanRunning = false
            self?.event("plugins.catalogue", ["scanning": false, "completed": 0, "total": 0, "finished": true])
        }
    }

    private func saveRecovery(project: [String:Any]?) async throws -> [String:Any] {
        guard foregroundPersistence == 0, recoverySaveTask == nil else { return ["saved":false,"deferred":true] }
        let task = Task { @MainActor in
            defer { self.recoverySaveTask = nil }
            try Task.checkCancellation()
            if let project { try self.store.update(project) }
            try await self.capturePluginState()
            try self.store.autosave()
        }
        recoverySaveTask = task
        try await task.value
        return ["saved":true]
    }

    private func capturedPluginStates() async throws -> [[String: Any]] {
        let response: Any
        if let audioSnapshotProvider { response = try await audioSnapshotProvider() }
        else { response = try await audio.handle("engine.snapshot", payload: [:]) }
        return try PluginSnapshotCapture.states(from: response)
    }

    private struct ProjectCheckpoint: Equatable {
        let project: Data?
        let url: URL?
    }

    private func projectCheckpoint() throws -> ProjectCheckpoint {
        try ProjectCheckpoint(project: store.project.map { try JSONSerialization.data(withJSONObject: $0, options: .sortedKeys) }, url: store.url)
    }

    private func requireProjectCheckpoint(_ expected: ProjectCheckpoint) throws {
        guard try projectCheckpoint() == expected else {
            throw TrackerError.message("The project changed while capturing plug-in state. Retry saving or quitting after the current edit finishes.")
        }
    }

    private func capturePluginState(beforeCommit: (@MainActor () async throws -> Void)? = nil) async throws {
        guard var snapshot = store.project else { return }
        let checkpoint = try projectCheckpoint()
        let states = try await capturedPluginStates()
        if let beforeCommit { try await beforeCommit() }
        try Task.checkCancellation()
        try requireProjectCheckpoint(checkpoint)
        let previous = try JSONSerialization.data(withJSONObject: snapshot["audioPlugins"] ?? [], options: [.sortedKeys, .fragmentsAllowed])
        let next = try JSONSerialization.data(withJSONObject: states, options: [.sortedKeys, .fragmentsAllowed])
        snapshot["audioPlugins"] = states
        try store.update(snapshot, dirty: store.dirty || previous != next)
    }

    private func terminationProject() async throws -> [String: Any]? {
        guard let webView else { return nil }
        // Commit any in-flight controller/knob gesture first so a stale edit can't block quitting.
        // The script gives up on its edits after 3 s; a page that does not answer at all is hung, and
        // Quit reports it (Keep Working / Quit Anyway) instead of waiting for it forever.
        do {
            _ = try await awaitInterfaceReply(within: 8) { try await webView.callAsyncJavaScript(Self.flushEditsScript, arguments: [:], in: nil, contentWorld: .page) }
        } catch let error as InterfaceReplyError { throw error } catch {}
        do {
            guard let checkpoint = try await awaitInterfaceReply(within: 8, { try await webView.evaluateJavaScript(Self.projectSnapshotScript) }) as? [String: Any],
                  let ok = checkpoint["ok"] as? Bool else {
                throw TrackerError.message("The interface could not provide a valid quit checkpoint. Keep the project open and retry.")
            }
            guard ok else { throw TrackerError.message(checkpoint["reason"] as? String ?? "Finish the current operation before quitting.") }
            return checkpoint["project"] as? [String: Any]
        }
        catch {
            if let reason = (error as NSError).userInfo["WKJavaScriptExceptionMessage"] as? String {
                throw TrackerError.message(reason)
            }
            throw error
        }
    }

    func prepareForTermination() async throws {
        foregroundPersistence += 1
        defer { foregroundPersistence -= 1 }
        if let pending = recoverySaveTask { _ = try? await pending.value }
        try Task.checkCancellation()
        let latest = try await terminationProject()
        let initialUI = try latest.map { try JSONSerialization.data(withJSONObject: $0, options: .sortedKeys) }
        if let latest {
            let secured = try keyboardAcceptance?.prepare("project.autosave",payload:["project":latest])["project"] as? [String: Any] ?? latest
            try store.update(secured)
        }
        try await capturePluginState {
            let current = try await self.terminationProject()
            let currentUI = try current.map { try JSONSerialization.data(withJSONObject: $0, options: .sortedKeys) }
            guard currentUI == initialUI else {
                throw TrackerError.message("The project changed while preparing to quit. Retry after the current edit finishes.")
            }
        }
        try store.autosave()
        preparedTerminationSession = (projectSessionRevision, store.url)
    }

    /// The destination is chosen before the final renderer/native checkpoint.
    /// No suspension occurs between that checkpoint and the primary file write.
    /// This secures edits made during the quit decision and Save panel; it is not
    /// an external-input fence for the subsequent asynchronous audio shutdown.
    func saveTerminationProject(chooseDestination: (@MainActor (String) async throws -> URL?)? = nil) async throws -> Bool {
        foregroundPersistence += 1
        defer { foregroundPersistence -= 1 }
        let expected = preparedTerminationSession ?? (revision: projectSessionRevision, url: store.url)
        func requireSession() throws {
            guard projectSessionRevision == expected.revision, store.url == expected.url else {
                throw TrackerError.message("The session changed while quitting. Keep working and save the current session explicitly.")
            }
        }
        try requireSession()
        let policy = try keyboardAcceptance?.prepare("project.save", payload: [:])
        var destination = (policy?["path"] as? String).map { URL(fileURLWithPath: $0) } ?? store.url
        if destination == nil {
            let name = (store.project?["name"] as? String ?? "Untitled") + ".supertracker"
            if let chooseDestination { destination = try await chooseDestination(name) }
            else { destination = try await savePanel(name: name, type: UTType(filenameExtension: "supertracker") ?? .json) }
        }
        guard let destination else { return false }
        try requireSession()
        try await prepareForTermination()
        try requireSession()
        try Task.checkCancellation()
        let payload = try keyboardAcceptance?.prepare("project.save", payload: ["path": destination.path, "project": store.project ?? [:]])
        let snapshot = payload?["project"] as? [String: Any] ?? store.project
        _ = try store.save(to: destination, snapshot: snapshot)
        return true
    }

    func saveForTermination() async -> Bool {
        do {
            return try await saveTerminationProject()
        } catch {
            let alert = NSAlert(); alert.messageText = "The project could not be saved"; alert.informativeText = error.localizedDescription; alert.runModal()
            return false
        }
    }

    private func openPanel(directory: Bool, types: [UTType]) async throws -> URL? {
        let panel = NSOpenPanel(); panel.canChooseDirectories = directory; panel.canChooseFiles = !directory
        panel.allowsMultipleSelection = false; panel.canCreateDirectories = false
        if !types.isEmpty { panel.allowedContentTypes = types }
        return try await present(panel)
    }

    private func savePanel(name: String, type: UTType) async throws -> URL? {
        let panel = NSSavePanel(); panel.allowedContentTypes = [type]; panel.nameFieldStringValue = name; panel.canCreateDirectories = true
        return try await present(panel)
    }

    /// Shows an Open/Save panel: its URL, or nil when cancelled. macOS can fail to show the panel (its
    /// out-of-process panel service dropped the connection) and then never calls back; a panel that has not
    /// appeared after a few seconds fails instead of leaving the save or open waiting forever.
    private func present(_ panel: NSSavePanel) async throws -> URL? {
        enum Outcome { case chosen(URL?), failed }
        let outcome: Outcome = await withCheckedContinuation { continuation in
            var finished = false
            func finish(_ value: Outcome) { guard !finished else { return }; finished = true; continuation.resume(returning: value) }
            panel.begin { response in finish(.chosen(response == .OK ? panel.url : nil)) }
            func check(_ attempt: Int) {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                    guard !finished else { return }
                    if panel.isVisible { return }
                    if attempt < 4 { check(attempt + 1) } else { panel.close(); finish(.failed) }
                }
            }
            check(0)
        }
        switch outcome {
        case .chosen(let url): return url
        case .failed: throw TrackerError.message("macOS could not show the \(panel is NSOpenPanel ? "Open" : "Save") window. Try again; if it keeps failing, restart SuperTracker.")
        }
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = navigationAction.request.url else { decisionHandler(.cancel); return }
        if url.isFileURL || url.scheme == "about" { decisionHandler(.allow) }
        else { decisionHandler(.cancel) }
    }

    /// WebKit's content process can end on its own (memory pressure, a crash). Without a reload the window stays
    /// blank while audio keeps running. The reloaded page asks `ready` again and receives the session the store holds.
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        NSLog("SuperTracker: the interface process ended; reloading it")
        isReady = false
        initialProject = store.project ?? initialProject
        webView.reload()
    }

    func shutdown() { MaschineMK3Driver.shared.shutdownSynchronously(); autosaveTimer?.invalidate(); recoverySaveTask?.cancel(); mediaRelinking.cancel(); try? store.autosave(); audio.shutdown() }

    func shutdownAndWait() async {
        MaschineMK3Driver.shared.shutdownSynchronously()
        autosaveTimer?.invalidate()
        recoverySaveTask?.cancel()
        mediaRelinking.cancel()
        if pluginDiscoveryEnabled && keyboardAcceptance == nil { VST3Catalogue.shared.cancel(); VST2Catalogue.shared.cancel() }
        try? store.autosave()
        await audio.shutdownAndWait()
    }
}

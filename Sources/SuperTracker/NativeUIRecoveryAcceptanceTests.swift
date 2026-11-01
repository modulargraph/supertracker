// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit
import VST3Host

/// Dead-owner recovery fixtures use private storage and intentionally nonexistent
/// Audio Unit IDs, so this exercises recovery policy without entering vendor code.
enum NativeUIRecoveryAcceptanceTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let reportURL = output.appendingPathComponent("webkit-plugin-recovery-report.json")
        func write(_ passed: Bool, _ fields: [String: Any]) throws {
            var report = fields; report["passed"] = passed
            report["generatedAt"] = ISO8601DateFormatter().string(from: Date())
            try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]).write(to: reportURL, options: .atomic)
        }
        try write(false, ["status":"running"])
        do {
            let result = try await check(output: output)
            try write(true, result)
            return ["Real WebKit startup recovery preserves opaque plug-in state and retries only the explicitly selected identity"]
        } catch {
            try? write(false, ["status":"failed", "error":error.localizedDescription])
            throw error
        }
    }

    @MainActor private static func check(output: URL) async throws -> [String: Any] {
        let files = FileManager.default
        let scratch = files.temporaryDirectory.appendingPathComponent("SuperTracker-WebKit-Recovery-\(UUID().uuidString)", isDirectory:true)
        try files.createDirectory(at: scratch, withIntermediateDirectories:true)
        defer { try? files.removeItem(at: scratch) }
        let originalVisible = Set(NSApp.windows.filter(\.isVisible).map(ObjectIdentifier.init))
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("WEBKIT RECOVERY: " + message) }
        }
        let first = PluginRecoveryStore.Identity(format:.audioUnit, componentID:"1635083896:1:1", name:"Recovery fixture one")
        let second = PluginRecoveryStore.Identity(format:.audioUnit, componentID:"1635083896:2:1", name:"Recovery fixture two")
        let journalRoot = scratch.appendingPathComponent("PluginRecovery", isDirectory:true)
        let dead = try PluginRecoveryStore(directory:journalRoot, configurationID:"webkit-fixture", processID:777_001, processIsAlive:{ _ in false })
        try dead.startSession()
        let deadToken = try dead.beginLoad(first, instanceHint:"Track 1 effect 1")
        try dead.didLoad(deadToken)
        try dead.beginRelease(deadToken)
        _ = try dead.beginLoad(second, instanceHint:"Track 1 effect 2")
        let oldJournal = dead.directory.appendingPathComponent("session-\(dead.sessionID).json")
        let oldBytes = try Data(contentsOf: oldJournal)
        let pid = ProcessInfo.processInfo.processIdentifier
        let recovery = try PluginRecoveryStore(directory:journalRoot, configurationID:"webkit-fixture", processID:pid, processIsAlive:{ $0 == pid })
        let initialReport = try recovery.startSession()
        try require(initialReport.suspects.count == 2 && initialReport.suspects.allSatisfy(\.requiresReview), "Dead-owner fixture did not produce two blocked suspects.")
        let blobs = [Data([0,1,2,3,255,17,33]).base64EncodedString(), Data("opaque state that must survive quarantine".utf8).base64EncodedString()]
        let descriptors: [[String: Any]] = [first,second].enumerated().map { index, identity in
            ["track":0,"kind":"effect","slot":index,"component":identity.componentID,"format":"AU","name":identity.name,"state":blobs[index],"bypass":false]
        }
        var fixture = NativeSelfTest.testProject()
        fixture["name"] = "Isolated plug-in recovery"; fixture["masterVolume"] = 0
        fixture["audioPlugins"] = descriptors
        let store = try ProjectStore(directory:scratch.appendingPathComponent("ProjectRecovery",isDirectory:true))
        try store.update(fixture); try store.autosave()
        var attempts = 0
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots = []; vst3Configuration.cacheURL = scratch.appendingPathComponent("Private-vst3-catalogue.json")
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots = []; vst2Configuration.cacheURL = scratch.appendingPathComponent("Private-vst2-catalogue.json")
        let audio = try AudioHost(beforePluginInstantiation: { attempts += 1 }, recoveryStore: recovery, vst3Catalogue: VST3Catalogue(configuration:vst3Configuration), midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration:vst2Configuration))
        defer { audio.shutdown() }
        _ = try await audio.handle("midi.configure", payload:["enabled":false,"output":"none"])
        let library = LibraryService(cacheDirectory:scratch.appendingPathComponent("Library",isDirectory:true),defaultRoots:[],loadCacheAsynchronously:false)
        let bridge = Bridge(audio:audio,library:library,store:store,pluginDiscoveryEnabled:false)
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        config.userContentController.add(bridge,name:"supertracker")
        config.userContentController.addUserScript(WKUserScript(source:"window.__recoveryErrors=[];window.addEventListener('error',e=>__recoveryErrors.push(e.message));window.addEventListener('unhandledrejection',e=>__recoveryErrors.push(String(e.reason)));",injectionTime:.atDocumentStart,forMainFrameOnly:true))
        let web = WKWebView(frame:NSRect(x:0,y:0,width:1080,height:700),configuration:config)
        bridge.webView = web; web.navigationDelegate = bridge
        defer {
            web.stopLoading(); web.navigationDelegate = nil
            config.userContentController.removeScriptMessageHandler(forName:"supertracker")
            bridge.webView = nil; bridge.shutdown()
        }
        let bundled = Bundle.main.resourceURL?.appendingPathComponent("Web/index.html")
        let html = bundled.flatMap { files.fileExists(atPath:$0.path) ? $0 : nil } ?? URL(fileURLWithPath:files.currentDirectoryPath).appendingPathComponent("Resources/Web/index.html")
        try require(files.fileExists(atPath:html.path), "Actual Web UI is unavailable.")
        web.loadFileURL(html,allowingReadAccessTo:html.deletingLastPathComponent())
        func js(_ script: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(script,arguments:arguments,in:nil,contentWorld:.page) ?? NSNull()
        }
        func wait(_ expression: String, _ message: String) async throws {
            let deadline = Date().addingTimeInterval(20)
            while Date() < deadline {
                if (try? await js("return Boolean(\(expression));")) as? Bool == true { return }
                try await Task.sleep(nanoseconds:25_000_000)
            }
            let details = try? await js("return {modal:window.ST?.state.modal,pending:window.ST?Array.from(ST.state.pending.values()).map(x=>x.command):[],status:document.getElementById('plugin-recovery-status')?.textContent,errors:window.__recoveryErrors};")
            throw TrackerError.message("WEBKIT RECOVERY: \(message). \(String(describing:details))")
        }
        func stateBlobs(_ project: [String: Any]) -> [String: String] {
            Dictionary((project["audioPlugins"] as? [[String: Any]] ?? []).compactMap { item in
                guard let id = item["component"] as? String, let state = item["state"] as? String else { return nil }
                return (id,state)
            },uniquingKeysWith:{ first,_ in first })
        }
        let expected = [first.componentID:blobs[0],second.componentID:blobs[1]]
        func verifyOpaque(_ label: String) async throws {
            let ui = try await js("return window.__supertracker.getProject();") as? [String: Any] ?? [:]
            let native = try await audio.handle("engine.snapshot",payload:[:]) as? [String: Any] ?? [:]
            try require(stateBlobs(ui) == expected && stateBlobs(native) == expected, "Opaque settings changed during \(label).")
        }
        var pictures: [[String: Any]] = []
        func picture(_ name: String) async {
            do {
                let image = try await web.takeSnapshot(configuration:WKSnapshotConfiguration())
                guard let data = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data:data), let png = bitmap.representation(using:.png,properties:[:]) else { throw TrackerError.message("Cannot encode snapshot.") }
                try png.write(to:output.appendingPathComponent(name),options:.atomic)
                pictures.append(["file":name,"available":true])
            } catch { pictures.append(["file":name,"available":false,"error":error.localizedDescription]) }
        }

        try await wait("window.ST?.state.engineReady&&ST.state.modal==='plugin-recovery'&&document.querySelectorAll('[data-plugin-recovery-key]').length===2&&!ST.state.status.pluginsRestoring", "Startup did not show the interrupted-session review")
        try require(attempts == 0, "Startup attempted to instantiate a quarantined plug-in.")
        try require(try await js("return !document.querySelector('#toast-region .toast.error');") as? Bool == true, "Expected quarantine added an error toast over the recovery dialog.")
        try require(try await js("return document.getElementById('plugin-recovery-retry').disabled&&!Array.from(document.querySelectorAll('[data-plugin-recovery-key]')).some(x=>x.checked)&&document.querySelector('.plugin-recovery-intro').textContent.includes('not proven');") as? Bool == true, "Recovery must require an explicit selection and use uncertain attribution.")
        try await verifyOpaque("quarantined startup")
        await picture("plugin-recovery-startup.png")
        _ = try await js("document.getElementById('plugin-recovery-keep').click();return true;")
        try require(try await js("return !ST.state.modal;") as? Bool == true, "Keep disabled did not dismiss the review.")
        try require(!recovery.canAttempt(first) && !recovery.canAttempt(second), "Dismissing review implicitly allowed a plug-in to run.")
        let savedURL = scratch.appendingPathComponent("Preserved.supertracker")
        _ = try await js("return await ST.request('project.save',{path:path,project:window.__supertracker.getProject()});",["path":savedURL.path])
        try require(stateBlobs(try store.read(savedURL)) == expected, "Saving the quarantined project discarded opaque state.")

        // Exercise the actual mounted command palette to reopen a dismissed report.
        _ = try await js("document.getElementById('tracker')?.focus();document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'k',code:'KeyK',metaKey:true,bubbles:true,cancelable:true}));const input=document.getElementById('palette-search');input.value='Review interrupted plug-ins';input.dispatchEvent(new Event('input',{bubbles:true}));input.focus();input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',code:'Enter',bubbles:true,cancelable:true}));return true;")
        try await wait("ST.state.modal==='plugin-recovery'&&document.querySelectorAll('[data-plugin-recovery-key]').length===2", "Command-palette recovery review did not reopen")
        _ = try await js("const button=Array.from(document.querySelectorAll('[data-plugin-recovery-enable]')).find(x=>x.dataset.pluginRecoveryEnable===key);if(!button)throw Error('Missing Enable / Retry button');button.click();return true;",["key":first.key])
        try await wait("document.querySelectorAll('[data-plugin-recovery-key]').length===2&&document.querySelector('.plugin-recovery-errors')&&!Array.from(ST.state.pending.values()).some(x=>x.command==='plugins.recovery.retry')", "Selective explicit retry did not return its expected missing-component error")
        try require(!recovery.canAttempt(first) && !recovery.canAttempt(second), "Retry changed authorization for an unselected identity.")
        try require(attempts == 1, "Selective retry attempted an unexpected number of component loads: \(attempts).")
        let report = try await js("return await ST.request('plugins.recovery',{});") as? [String: Any] ?? [:]
        let suspects = report["suspects"] as? [[String: Any]] ?? []
        try require(suspects.filter { $0["requiresReview"] as? Bool == true }.count == 2, "Failed and unselected suspects did not remain quarantined.")
        try await verifyOpaque("selective missing-component retry")
        await picture("plugin-recovery-selected-retry.png")
        _ = try await js("document.getElementById('plugin-recovery-keep').click();return true;")
        try await bridge.prepareForTermination()
        try require(stateBlobs(try store.read(store.recoveryURL)) == expected, "Quit recovery lost the skipped or failed plug-in state.")
        try require(try Data(contentsOf:oldJournal) == oldBytes, "The prior interrupted journal was rewritten.")
        try require(try await js("return window.__recoveryErrors.length===0;") as? Bool == true, "The recovery UI emitted an uncaught JavaScript error.")
        let visible = NSApp.windows.filter { $0.isVisible && !originalVisible.contains(ObjectIdentifier($0)) }.count
        try require(visible == 0, "The private recovery test displayed a native window.")
        func currentJournal() throws -> [String: Any] {
            let url = recovery.directory.appendingPathComponent("session-\(recovery.sessionID).json")
            guard let journal = try JSONSerialization.jsonObject(with:Data(contentsOf:url)) as? [String: Any] else {
                throw TrackerError.message("WEBKIT RECOVERY: The current recovery journal is invalid.")
            }
            return journal
        }
        await audio.shutdownAndWait()
        let instanceShutdown = try currentJournal()
        try require(instanceShutdown["cleanShutdown"] as? Bool == false && instanceShutdown["phase"] as? String == "shuttingDown", "Instance shutdown marked the journal clean before module cleanup.")
        try audio.beginRecoveryModuleCleanup()
        let moduleShutdown = try currentJournal()
        try require(moduleShutdown["cleanShutdown"] as? Bool == false && moduleShutdown["phase"] as? String == "cleaningModules", "Module cleanup did not retain the interrupted-shutdown marker.")
        st_vst3_release_unused_modules()
        try audio.finishRecoveryShutdown()
        let journal = try currentJournal()
        try require(journal["cleanShutdown"] as? Bool == true, "Fixture host did not record clean shutdown after its caught load failure.")
        print("PASS WebKit recovery: startup review, opaque-state preservation, keep disabled, palette reopen, selective explicit retry, clean shutdown")
        return ["status":"passed","startupSuspects":2,"startupAttempts":0,"explicitRetryAttempts":attempts,
                "remainingQuarantined":2,"opaqueBlobsPreserved":true,"priorJournalUnchanged":true,
                "cleanShutdown":true,"visibleWindowsCreated":visible,"isolatedStorage":true,"vendorPluginsLoaded":0,"startupErrorToast":false,
                "retryOutcome":"The intentionally nonexistent AU returned a caught not-installed error; successful vendor restoration is outside this fixture test.",
                "keyboardMethod":"DOM KeyboardEvent through actual mounted command-palette and recovery handlers; no operating-system keyboard injection.",
                "snapshots":pictures]
    }
}

#endif

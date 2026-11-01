// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Fault injection at the Bridge's engine-snapshot boundary. No installed plug-ins,
/// panels, microphone, or user-profile files are used by these checks.
enum NativeBridgeSaveSafetyTests {
    @MainActor static func run(directory: URL, web: WKWebView) async throws -> [String] {
        let store = try ProjectStore(directory: directory.appendingPathComponent("Recovery"))
        let destination = directory.appendingPathComponent("Original.supertracker")
        let desired: [[String: Any]] = [["track": 0, "kind": "instrument", "slot": 0,
            "component": "missing.fixture.component", "name": "Unavailable fixture",
            "state": "opaque unavailable desired patch", "recoveryBlocked": true]]
        var project = NativeSelfTest.testProject()
        project["name"] = "Original saved project"; project["audioPlugins"] = desired
        _ = try store.save(to: destination, snapshot: project, collectSamples: false)
        let original = try Data(contentsOf: destination), recovery = try Data(contentsOf: store.recoveryURL)
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots = []; vst3Configuration.cacheURL = directory.appendingPathComponent("Private-vst3-catalogue.json")
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots = []; vst2Configuration.cacheURL = directory.appendingPathComponent("Private-vst2-catalogue.json")
        let audio = try AudioHost(beforePluginInstantiation: { throw TrackerError.message("This fixture never loads installed plug-ins.") }, vst3Catalogue: VST3Catalogue(configuration:vst3Configuration), midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration:vst2Configuration))
        _ = try await audio.handle("midi.configure", payload: ["enabled": false, "output": "none"])
        let library = LibraryService(cacheDirectory: directory.appendingPathComponent("Library"), defaultRoots: [], loadCacheAsynchronously: false)
        var response: Any = ["audioPlugins": desired]
        var providerError: Error?
        var captures = 0
        var suspendCapture = false
        var captureGate: CheckedContinuation<Void, Never>?
        var duringCapture: (@MainActor () async throws -> Void)?
        let bridge = Bridge(audio: audio, library: library, store: store, pluginDiscoveryEnabled:false, audioSnapshotProvider: {
            captures += 1
            if suspendCapture {
                await withCheckedContinuation { captureGate = $0 }
                suspendCapture = false; captureGate = nil
            }
            if let providerError { throw providerError }
            if let duringCapture { try await duringCapture() }
            return response
        })
        defer { bridge.shutdown() }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("BRIDGE SAVE SAFETY: " + message) }
        }
        func rejects(_ label: String, contains detail: String? = nil, _ action: () async throws -> Void) async throws {
            var failure: Error?
            do { try await action() } catch { failure = error }
            try require(failure != nil, label + " incorrectly succeeded")
            if let detail { try require(failure!.localizedDescription.contains(detail), label + " lost the useful error: " + failure!.localizedDescription) }
            try require(try Data(contentsOf: destination) == original, label + " replaced the original project")
            try require(try Data(contentsOf: store.recoveryURL) == recovery, label + " replaced recovery with incomplete state")
        }
        var latest = project; latest["name"] = "Latest unsaved project edits"
        response = ["audioPlugins": [["name": "Failing fixture synth", "state": "older fallback patch", "stateError": "Render is still busy"]]]
        try await rejects("Explicit Save", contains: "Failing fixture synth: Render is still busy") {
            _ = try await bridge.handle("project.save", payload: ["project": latest, "path": destination.path, "collectSamples": false])
        }
        try await rejects("Termination capture", contains: "Failing fixture synth") { try await bridge.prepareForTermination() }
        try await rejects("Recovery autosave", contains: "Failing fixture synth") { _ = try await bridge.handle("project.autosave", payload: [:]) }
        for malformed: Any in [NSNull(), [:], ["audioPlugins": NSNull()], ["audioPlugins": ["wrong entry"]], ["audioPlugins": [["name": "Malformed fixture", "state": 12]]]] {
            response = malformed
            try await rejects("Malformed explicit Save") { _ = try await bridge.handle("project.save", payload: ["path": destination.path, "collectSamples": false]) }
            try await rejects("Malformed termination capture") { try await bridge.prepareForTermination() }
        }
        providerError = TrackerError.message("Injected engine snapshot failure")
        try await rejects("Thrown capture failure", contains: "Injected engine snapshot failure") { try await bridge.prepareForTermination() }
        providerError = nil; response = ["audioPlugins": desired]
        func waitForCapture() async throws {
            for _ in 0..<200 {
                if captureGate != nil { return }
                try await Task.sleep(nanoseconds: 10_000_000)
            }
            throw TrackerError.message("Deferred snapshot fixture did not reach its capture boundary")
        }
        suspendCapture = true
        let pendingSave = Task { try await bridge.handle("project.save", payload: ["path": destination.path, "collectSamples": false]) }
        try await waitForCapture()
        var newer = latest; newer["name"] = "Edit made while capture was suspended"
        try store.update(newer)
        captureGate?.resume()
        try await rejects("Save after a concurrent edit", contains: "project changed") { _ = try await pendingSave.value }
        try require(store.project?["name"] as? String == newer["name"] as? String, "A stale save overwrote the newer in-memory edit")
        suspendCapture = true
        let pendingTermination = Task { try await bridge.prepareForTermination() }
        try await waitForCapture()
        var replacement = newer; replacement["name"] = "Different project opened during capture"
        let replacementURL = directory.appendingPathComponent("Different.supertracker")
        store.acceptLoaded(replacement, from: replacementURL)
        captureGate?.resume()
        try await rejects("Termination after project replacement", contains: "project changed") { try await pendingTermination.value }
        try require(store.project?["name"] as? String == replacement["name"] as? String && store.url == replacementURL, "Termination capture overwrote the replacement project")
        let saved = try await bridge.handle("project.save", payload: ["path": destination.path, "collectSamples": false]) as? [String: Any]
        try require(saved?["saved"] as? Bool == true, "Unavailable desired state could not be saved")
        let reopened = try store.read(destination)
        let states = reopened["audioPlugins"] as? [[String: Any]]
        try require(states?.first?["state"] as? String == desired.first?["state"] as? String && states?.first?["recoveryBlocked"] as? Bool == true, "Unavailable opaque desired state was changed")
        try await bridge.prepareForTermination()
        try require(!store.dirty, "Capturing unchanged desired state made a clean project dirty")
        try require(captures == 18, "Save or termination bypassed the shared capture boundary")

        // Recovery and explicit persistence share the real Bridge entry points.
        // A background capture may change opaque bytes even without a UI edit.
        var freshStates = desired; freshStates[0]["state"] = "freshly captured opaque state"
        response = ["audioPlugins":freshStates]; suspendCapture = true
        let background = Task { try await bridge.handle("project.autosave",payload:[:]) }
        try await waitForCapture()
        let capturesBeforeSave = captures, backgroundName = store.project?["name"] as? String
        var foregroundProject = store.project!
        foregroundProject["name"] = "Foreground save after recovery"
        let foreground = Task { try await bridge.handle("project.save",payload:["project":foregroundProject,"path":destination.path,"collectSamples":false]) }
        for _ in 0..<10 { await Task.yield() }
        var staleRecovery = foregroundProject; staleRecovery["name"] = "Stale deferred recovery"
        let deferred = try await bridge.handle("project.autosave",payload:["project":staleRecovery]) as? [String:Any]
        try require(deferred?["deferred"] as? Bool == true && captures == capturesBeforeSave && store.project?["name"] as? String == backgroundName,
                    "Foreground Save did not reserve persistence before waiting for active recovery")
        captureGate?.resume()
        _ = try await background.value
        let foregroundResult = try await foreground.value as? [String:Any]
        let foregroundFile = try store.read(destination)
        try require(foregroundResult?["saved"] as? Bool == true && captures == capturesBeforeSave+1 && foregroundFile["name"] as? String == "Foreground save after recovery" &&
                    (foregroundFile["audioPlugins"] as? [[String:Any]])?.first?["state"] as? String == "freshly captured opaque state",
                    "Recovery invalidated the following explicit Save or stale state was persisted")

        suspendCapture = true
        let saving = Task { try await bridge.handle("project.save",payload:["path":destination.path,"collectSamples":false]) }
        try await waitForCapture()
        let foregroundCheckpoint = try JSONSerialization.data(withJSONObject:store.project!,options:[.sortedKeys])
        let deferredDuringSave = try await bridge.handle("project.autosave",payload:["project":staleRecovery]) as? [String:Any]
        try require(deferredDuringSave?["deferred"] as? Bool == true &&
                    JSONSerialization.data(withJSONObject:store.project!,options:[.sortedKeys]) == foregroundCheckpoint,
                    "Recovery rewrote the foreground Save checkpoint")
        captureGate?.resume(); _ = try await saving.value
        let stableFile = try Data(contentsOf: destination), stableRecovery = try Data(contentsOf: store.recoveryURL)
        bridge.webView = web
        for (module, property, message) in [("STStudio", "isMovingEffect", "effect move"), ("STRecording", "isBusy", "audio recording"), ("STRecording", "isRecording", "audio recording"), ("ST","isChangingSession","save or project change"), ("STNKSControls","hasPendingEdits","plug-in control edit"), ("STPluginParameters","hasPendingEdits","plug-in control edit"), ("STInputCapture","hasPendingEdits","input recording edge")] {
            duringCapture = {
                _ = try await web.callAsyncJavaScript("window.__captureRaceModule=window[module];window[module]=Object.create(window[module]||null,{[property]:{value:true}});return true;", arguments:["module":module,"property":property],in:nil,contentWorld:.page)
            }
            var failure: Error?
            do { try await bridge.prepareForTermination() } catch { failure = error }
            _ = try await web.callAsyncJavaScript("window[module]=window.__captureRaceModule;delete window.__captureRaceModule;return true;", arguments:["module":module],in:nil,contentWorld:.page)
            try require(failure?.localizedDescription.contains(message) == true, "An operation starting during capture bypassed the second \(property) preflight")
            try require(try Data(contentsOf: destination) == stableFile && Data(contentsOf: store.recoveryURL) == stableRecovery, "An operation starting during capture replaced saved state")
        }
        duringCapture = {
            _ = try await web.evaluateJavaScript("window.__captureRaceName=ST.state.project.name;ST.state.project.name='New edit while capturing quit state';true;")
        }
        var editFailure: Error?
        do { try await bridge.prepareForTermination() } catch { editFailure = error }
        _ = try await web.evaluateJavaScript("ST.state.project.name=window.__captureRaceName;delete window.__captureRaceName;true;")
        try require(editFailure?.localizedDescription.contains("project changed") == true, "An unsynchronized UI edit during capture bypassed the final checkpoint")
        try require(try Data(contentsOf: destination) == stableFile && Data(contentsOf: store.recoveryURL) == stableRecovery, "A concurrent UI edit was replaced by stale quit recovery")
        duringCapture = nil; bridge.webView = nil
        await audio.shutdownAndWait()

        var requests = 0
        let delegate = AppDelegate(requestTermination: { requests += 1 })
        let main = NSWindow(contentRect: .zero, styleMask: [], backing: .buffered, defer: true)
        let other = NSWindow(contentRect: .zero, styleMask: [], backing: .buffered, defer: true)
        delegate.window = main
        try require(!delegate.windowShouldClose(main) && requests == 1, "Main-window close did not defer to application termination")
        try require(!delegate.windowShouldClose(main) && requests == 2, "A later close attempt did not retain the workspace")
        try require(delegate.windowShouldClose(other) && requests == 2, "An unrelated window triggered project termination")
        try require(!main.isVisible && !other.isVisible, "Close-routing fixture displayed a window")
        return ["Native Bridge rejects failed/malformed live captures before Save, recovery, or termination can replace files",
                "Deferred native captures abort rather than overwrite a newer edit or replacement project",
                "Background recovery and explicit Save preserve their complete checkpoints without competing opaque captures",
                "Quit rechecks mounted UI edits, effect movement, recording, tempo changes and pending plug-in controls after asynchronous capture",
                "Native Bridge preserves unavailable opaque desired state and keeps unchanged captures clean",
                "Main-window close requests termination while retaining the workspace; unrelated windows remain independent"]
    }

    @MainActor static func terminationPreflight(web: WKWebView, bridge: Bridge, store: ProjectStore) async throws -> String {
        let before = try JSONSerialization.data(withJSONObject: store.project!, options: .sortedKeys)
        try store.autosave()
        let recovery = try Data(contentsOf: store.recoveryURL)
        let cases = [("STStudio", "isMovingEffect", "effect move"), ("STRecording", "isBusy", "audio recording"), ("STRecording", "isRecording", "audio recording"), ("ST","isChangingSession","save or project change"), ("STNKSControls","hasPendingEdits","plug-in control edit"), ("STPluginParameters","hasPendingEdits","plug-in control edit"), ("STInputCapture","hasPendingEdits","input recording edge")]
        for (module, property, expected) in cases {
            _ = try await web.callAsyncJavaScript("window.__terminationModule=window[module];window[module]=Object.create(window[module]||null,{[property]:{value:true}});return true;", arguments: ["module":module,"property":property], in:nil, contentWorld:.page)
            var error: Error?
            do { try await bridge.prepareForTermination() } catch let failure { error = failure }
            _ = try await web.callAsyncJavaScript("window[module]=window.__terminationModule;delete window.__terminationModule;return true;", arguments:["module":module],in:nil,contentWorld:.page)
            guard let error else { throw TrackerError.message("Termination preflight ignored \(module).\(property)") }
            let reason = error.localizedDescription
            guard reason.contains(expected), try Data(contentsOf: store.recoveryURL) == recovery,
                  try JSONSerialization.data(withJSONObject: store.project!, options: .sortedKeys) == before else {
                throw TrackerError.message("Termination preflight did not preserve the project/recovery or explain \(property): \(reason)")
            }
        }
        return "Mounted WebKit prevents quit during effect movement, recording or pending plug-in controls before changing recovery"
    }
}

#endif

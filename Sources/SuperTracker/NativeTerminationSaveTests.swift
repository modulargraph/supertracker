// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Actual Bridge persistence and mounted MIDI/MK3 renderer handlers. The input
/// events and destination chooser are synthetic; no physical input or panel is used.
enum NativeTerminationSaveTests {
    @MainActor static func run(directory: URL, web: WKWebView) async throws -> [String] {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let store = try ProjectStore(directory: directory.appendingPathComponent("Recovery"))
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots = []; vst3Configuration.cacheURL = directory.appendingPathComponent("Private-vst3-catalogue.json")
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots = []; vst2Configuration.cacheURL = directory.appendingPathComponent("Private-vst2-catalogue.json")
        let audio = try AudioHost(beforePluginInstantiation: { throw TrackerError.message("Quit-save fixture cannot load plug-ins.") }, vst3Catalogue: VST3Catalogue(configuration:vst3Configuration), midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration:vst2Configuration))
        let library = LibraryService(cacheDirectory: directory.appendingPathComponent("Library"), defaultRoots: [], loadCacheAsynchronously: false)
        var duringCapture: (@MainActor () async throws -> Void)?
        let bridge = Bridge(audio: audio, library: library, store: store, pluginDiscoveryEnabled:false, audioSnapshotProvider: {
            if let duringCapture { try await duringCapture() }
            return ["audioPlugins": []] as [String: Any]
        })
        bridge.webView = web
        func js(_ source: String, _ args: [String: Any] = [:]) async throws -> Any { try await web.callAsyncJavaScript(source, arguments: args, in: nil, contentWorld: .page) ?? NSNull() }
        func require(_ value: Bool, _ message: String) throws { if !value { throw TrackerError.message("QUIT SAVE: " + message) } }
        func project() async throws -> [String: Any] {
            guard let project = try await js("return __supertracker.getProject();") as? [String: Any] else { throw TrackerError.message("Quit fixture could not read UI project") }
            return project
        }
        func note(_ project: [String: Any], row: Int) -> Int? {
            guard let patterns = project["patterns"] as? [[String: Any]], let rows = patterns.first?["steps"] as? [[Any]], let step = rows.first?[row] as? [String: Any] else { return nil }
            return step["note"] as? Int
        }
        func injectMIDI(_ row: Int, _ note: Int) async throws {
            _ = try await js("ST.state.row=row;ST.state.recording=true;window.nativeEvent({type:'midi.note',payload:{on:true,source:901,channel:1,note,velocity:99,track:0,instrument:0}});window.nativeEvent({type:'midi.note',payload:{on:false,source:901,channel:1,note,track:0,instrument:0}});ST.cancelPendingEngineSync();return true;", ["row": row, "note": note])
        }
        let backup = "window.__quitSaveMK3View={screen:STMK3.state.screen,lastView:STMK3.state.lastView};window.__quitSaveBackup={native:ST.state.native,project:ST.state.project,view:ST.state.view,recording:ST.state.recording,playing:ST.state.playing,track:ST.state.track,instrument:ST.state.instrument,row:ST.state.row,undo:ST.state.undo,redo:ST.state.redo,dirty:ST.state.dirty};ST.cancelPendingEngineSync();ST.state.native=false;ST.state.project=ST.normalize(project);ST.state.track=0;ST.state.instrument=0;ST.state.row=0;ST.state.recording=false;ST.state.playing=false;ST.state.undo=[];ST.state.redo=[];ST.state.dirty=false;STMK3.cancelGestures({projectChanged:true});ST.setView('pattern');return true;"
        _ = try await js(backup, ["project": NativeSelfTest.testProject()])
        func cleanup() async {
            _ = try? await js("ST.cancelPendingEngineSync();STMK3.cancelGestures({projectChanged:true});if(window.__quitSaveBackup){Object.assign(ST.state,__quitSaveBackup);delete window.__quitSaveBackup;}if(window.__quitSaveMK3View){Object.assign(STMK3.state,__quitSaveMK3View);delete window.__quitSaveMK3View;}ST.refresh();return true;")
            bridge.webView = nil; await bridge.shutdownAndWait()
        }
        do {
            let destination = directory.appendingPathComponent("Existing.supertracker")
            let initial = try await project()
            _ = try store.save(to: destination, snapshot: initial, collectSamples: false)
            try await bridge.prepareForTermination()
            try await injectMIDI(3, 73) // arrives while the quit decision is open
            try require(try await bridge.saveTerminationProject(), "Existing destination was not saved")
            try require(note(try store.read(destination), row: 3) == 73, "A MIDI edit after initial quit capture was omitted")

            store.newProject(); try store.update(try await project()); try await bridge.prepareForTermination()
            let chosen = directory.appendingPathComponent("Chosen after live edits.supertracker")
            var chooserCalls = 0, controllerPending = false
            let saved = try await bridge.saveTerminationProject { _ in
                chooserCalls += 1
                try await injectMIDI(5, 77)
                controllerPending = (try await js("ST.setView('mixer');STMK3.state.screen='mixer';STMK3.state.lastView='mixer';window.nativeEvent({type:'mk3.input',payload:{type:'encoder',index:0,delta:-0.1}});ST.cancelPendingEngineSync();return STMK3.hasPendingEdits;")) as? Bool == true
                return chosen
            }
            let chosenProject = try store.read(chosen)
            let chosenVolume = (chosenProject["tracks"] as? [[String: Any]])?.first?["volume"] as? Double
            try require(saved && chooserCalls == 1 && controllerPending, "Destination fixture did not preserve an actual pending controller burst")
            try require(note(chosenProject, row: 5) == 77 && chosenVolume != 0.7, "Final save omitted MIDI/controller changes made during destination choice")
            try require((try await js("return !STMK3.hasPendingEdits;")) as? Bool == true, "Final capture did not flush the pending controller burst")

            store.newProject(); try store.update(try await project()); try await bridge.prepareForTermination()
            let recoveryBeforeCancel = try Data(contentsOf: store.recoveryURL)
            let canceled = try await bridge.saveTerminationProject { _ in try await injectMIDI(7, 79); return nil }
            let afterCancel = try await project(), recoveryAfterCancel = try Data(contentsOf: store.recoveryURL)
            try require(!canceled && note(afterCancel, row: 7) == 79 && recoveryAfterCancel == recoveryBeforeCancel, "Cancel wrote an incomplete checkpoint or lost the retained UI edit")

            _ = try store.save(to: chosen, snapshot: try await project(), collectSamples: false)
            try await bridge.prepareForTermination()
            let savedBytes = try Data(contentsOf: chosen)
            duringCapture = { try await injectMIDI(9, 81) }
            var captureError: Error?
            do { _ = try await bridge.saveTerminationProject() } catch { captureError = error }
            duringCapture = nil
            let afterFailedCapture = try await project(), bytesAfterFailedCapture = try Data(contentsOf: chosen)
            try require(captureError?.localizedDescription.contains("project changed") == true && bytesAfterFailedCapture == savedBytes && note(afterFailedCapture, row: 9) == 81, "An edit during final capture was lost or written as a stale successful Save")

            store.newProject(); try store.update(try await project()); try await bridge.prepareForTermination()
            let replacement = directory.appendingPathComponent("Other session.supertracker")
            var replacementError: Error?
            do {
                _ = try await bridge.saveTerminationProject { _ in store.acceptLoaded(initial, from: replacement); return chosen }
            } catch { replacementError = error }
            let bytesAfterReplacement = try Data(contentsOf: chosen)
            try require(replacementError?.localizedDescription.contains("session changed") == true && bytesAfterReplacement == savedBytes && store.url == replacement, "A replacement session was saved over the old destination")
            await cleanup()
            return ["Quit Save recaptures MIDI edits after the initial decision checkpoint", "Quit Save chooses its destination before capturing late MIDI and pending MK3 knob edits", "Cancel and concurrent final-capture edits retain UI changes without replacing saved bytes", "Quit Save rejects a replacement session while destination choice is pending"]
        } catch { await cleanup(); throw error }
    }
}

#endif

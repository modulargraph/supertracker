// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AppKit
import WebKit

/// Real WebKit/JavaScript/native persistence coverage, with no window, panels, or user storage.
enum NativeUIBridgeSelfTest {
    @MainActor static func run(output: URL, kitOnly: Bool = false, workflowOnly: Bool = false) async throws -> [String] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let reportURL = output.appendingPathComponent("webkit-bridge-report.json")
        func report(_ status: String, error: String? = nil) throws {
            var value: [String: Any] = ["passed": false, "status": status,
                "generatedAt": ISO8601DateFormatter().string(from: Date())]
            if let error { value["error"] = error }
            try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys]).write(to: reportURL, options: .atomic)
        }
        try report("running")
        do { return try await runChecks(output: output, kitOnly: kitOnly, workflowOnly: workflowOnly) }
        catch { try? report("failed", error: error.localizedDescription); throw error }
    }

    @MainActor private static func runChecks(output: URL, kitOnly: Bool, workflowOnly: Bool) async throws -> [String] {
        let files = FileManager.default
        let originalVisibleWindows = Set(NSApp.windows.filter(\.isVisible).map(ObjectIdentifier.init))
        let scratch = files.temporaryDirectory.appendingPathComponent("SuperTracker-WebKit-\(UUID().uuidString)", isDirectory: true)
        try files.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? files.removeItem(at: scratch) }
        try files.createDirectory(at: output, withIntermediateDirectories: true)
        var checks: [String] = []
        func record(_ name: String) { checks.append(name); print("PASS WebKit: \(name)"); fflush(stdout) }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("WEBKIT SELF TEST: \(message)") }
        }

        let offline = scratch.appendingPathComponent("Offline", isDirectory: true)
        let replacements = scratch.appendingPathComponent("Replacement", isDirectory: true)
        try files.createDirectory(at: offline, withIntermediateDirectories: true)
        try files.createDirectory(at: replacements, withIntermediateDirectories: true)
        let missing = offline.appendingPathComponent("Bridge Fixture.wav")
        let replacement = replacements.appendingPathComponent(missing.lastPathComponent)
        let samples = (0..<4800).map { Float(sin(Double($0) * 2 * .pi * 220 / 48000) * 0.2) }
        try AudioSamples.write(DecodedSample(path: missing.path, sampleRate: 48000, left: samples, right: samples, originalChannels: 2), to: missing)
        var fixture = NativeSelfTest.testProject()
        fixture["name"] = "Isolated WebKit fixture"
        fixture["instruments"] = [["id": 0, "name": "Relink fixture", "type": "sample", "samplePath": missing.path,
            "volume": 0.61, "pan": -0.2, "start": 0.125, "end": 0.875, "loopStart": 0.2, "loopEnd": 0.7,
            "mode": "loop", "tune": -3, "rootNote": 57, "attack": 0.013, "release": 0.19] as [String: Any]]
        fixture["audioClips"] = [["id": "preserved-clip", "name": "Keep edits", "track": 0, "path": replacement.path,
            "startBeat": 2.0, "durationBeats": 0.1, "sourceOffset": 0.012, "gain": 0.44, "fadeIn": 0.01, "fadeOut": 0.02] as [String: Any]]
        fixture = try MediaRelinker.fingerprint(fixture)["project"] as! [String: Any]
        try files.moveItem(at: missing, to: replacement)
        if kitOnly || workflowOnly {
            var instruments = fixture["instruments"] as! [[String:Any]]
            instruments[0]["samplePath"] = replacement.path
            fixture["instruments"] = instruments
        }
        let originalBytes = try Data(contentsOf: replacement)
        let store = try ProjectStore(directory: scratch.appendingPathComponent("Recovery", isDirectory: true))
        try store.update(fixture); try store.autosave()
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots = []; vst3Configuration.cacheURL = scratch.appendingPathComponent("Private-vst3-catalogue.json")
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots = []; vst2Configuration.cacheURL = scratch.appendingPathComponent("Private-vst2-catalogue.json")
        let audio = try AudioHost(vst3Catalogue: VST3Catalogue(configuration:vst3Configuration), midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration:vst2Configuration))
        defer { audio.shutdown() }
        // This private host must never receive the user's live MIDI input during its test.
        _ = try await audio.handle("midi.configure", payload: ["enabled": false, "output": "none"])
        let library = LibraryService(cacheDirectory: scratch.appendingPathComponent("Library", isDirectory: true), defaultRoots: [], loadCacheAsynchronously: false)
        let bridge = Bridge(audio: audio, library: library, store: store, pluginDiscoveryEnabled:false)
        do {
            _ = try await bridge.handle("plugins.scan", payload:[:])
            throw TrackerError.message("WEBKIT SELF TEST: private fixture admitted plug-in discovery")
        } catch {
            try require(error.localizedDescription.contains("discovery is disabled"), "Private fixture must explicitly refuse manual plug-in discovery.")
        }
        record("Private bridge refuses manual discovery and uses empty-root private VST catalogues")
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.preferences.tabFocusesLinks = true
        configuration.userContentController.add(bridge, name: "supertracker")
        configuration.userContentController.addUserScript(WKUserScript(source: "window.__bridgeTestErrors=[];window.addEventListener('error',e=>window.__bridgeTestErrors.push(e.message));window.addEventListener('unhandledrejection',e=>window.__bridgeTestErrors.push(String(e.reason)));", injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let web = WKWebView(frame: CGRect(x: 0, y: 0, width: 1500, height: 940), configuration: configuration)
        bridge.webView = web; web.navigationDelegate = bridge
        defer {
            web.stopLoading()
            web.navigationDelegate = nil
            configuration.userContentController.removeScriptMessageHandler(forName: "supertracker")
            bridge.webView = nil
            bridge.shutdown()
        }
        let bundled = Bundle.main.resourceURL?.appendingPathComponent("Web/index.html")
        let development = URL(fileURLWithPath: files.currentDirectoryPath).appendingPathComponent("Resources/Web/index.html")
        let html = bundled.flatMap { files.fileExists(atPath: $0.path) ? $0 : nil } ?? development
        try require(files.fileExists(atPath: html.path), "Actual interface resources are missing.")
        web.loadFileURL(html, allowingReadAccessTo: html.deletingLastPathComponent())

        func js(_ script: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(script, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func waitFor(_ expression: String, _ description: String, timeout: TimeInterval = 30) async throws {
            let deadline = Date().addingTimeInterval(timeout)
            while Date() < deadline {
                if (try? await js("return Boolean(\(expression));")) as? Bool == true { return }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            let diagnostics = try? await js("return {errors:window.__bridgeTestErrors||[],toast:document.getElementById('toast-region')?.textContent,media:document.getElementById('media-relink-status')?.textContent,pending:window.ST?Array.from(ST.state.pending.values()).map(x=>x.command):[]};")
            throw TrackerError.message("WEBKIT SELF TEST: Timed out waiting for \(description). \(String(describing: diagnostics))")
        }
        func project() async throws -> [String: Any] {
            guard let value = try await js("return window.__supertracker.getProject();") as? [String: Any] else {
                throw TrackerError.message("WEBKIT SELF TEST: JavaScript did not return its current project.")
            }
            return value
        }
        func same(_ left: Any?, _ right: Any?) -> Bool {
            guard let left, let right else { return left == nil && right == nil }
            return (try? JSONSerialization.data(withJSONObject: left, options: [.sortedKeys, .fragmentsAllowed])) ==
                (try? JSONSerialization.data(withJSONObject: right, options: [.sortedKeys, .fragmentsAllowed]))
        }
        func instrumentPath(_ value: [String: Any]?) -> String? { (value?["instruments"] as? [[String: Any]])?.first?["samplePath"] as? String }
        try await waitFor("window.ST?.state.engineReady && ST.state.pending.size===0 && ST.state.project.name==='Isolated WebKit fixture'", "native ready handshake and initial project")
        try require(try await js("return ST.state.native && ST.state.dirty;") as? Bool == true, "Recovery must be native and marked unsaved in the actual interface.")
        if workflowOnly {
            for name in try await NativeUIWorkflowTests.run(web: web, scratch: scratch, output: output) { record(name) }
            try await waitFor("ST.state.pending.size===0", "settled workflow commands")
            let errors = try await js("return window.__bridgeTestErrors;") as? [String] ?? []
            try require(errors.isEmpty, "Unexpected JavaScript errors: \(errors)")
            let visibleWindows = NSApp.windows.filter { $0.isVisible && !originalVisibleWindows.contains(ObjectIdentifier($0)) }
            try require(visibleWindows.isEmpty, "The isolated workflow harness displayed a native window.")
            let report: [String: Any] = ["passed": true, "status": "passed", "suite": "workflow", "checks": checks,
                "interface": html.path, "visibleWindowsCreated": visibleWindows.count, "isolatedStorage": true,
                "generatedAt": ISO8601DateFormatter().string(from: Date())]
            try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]).write(to: output.appendingPathComponent("webkit-bridge-report.json"), options: .atomic)
            return checks.map { "WebKit: " + $0 }
        }
        let baseline = try await project()
        record("Actual interface boot restores isolated recovery and unsaved state")

        _ = try await js("""
        ST.setView('pattern');
        const before=JSON.stringify(ST.state.project),undo=ST.state.undo.length,original=ST.request,calls=[];
        ST.request=(command,...args)=>{calls.push(command);return original(command,...args);};
        try{
          const lineButton=document.getElementById('pattern-line-inputs'),recordButton=document.getElementById('pattern-record-sample');
          if(!lineButton||!recordButton)throw Error('Pattern input workflow buttons are missing');
          lineButton.click();
          const deadline=Date.now()+5000;
          while(ST.state.modal!=='line-inputs'&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,10));
          if(ST.state.modal!=='line-inputs')throw Error('Line inputs button did not open its dialog');
          for(let i=0;i<ST.state.project.tracks.length;i++){
            if(ST.state.project.tracks[i].kind==='kit')continue;
            const enabled=document.getElementById('line-input-enabled-'+i);
            if(!enabled||enabled.checked)throw Error('Fresh input lane must remain off');
            if(document.getElementById('line-input-right-'+i).value!=='mono')throw Error('Fresh input lane must default to mono');
          }
          if(!document.getElementById('line-input-status')||document.getElementById('line-input-apply').disabled)throw Error('Input dialog status or Apply control unavailable');
          document.getElementById('line-input-close').click();
          if(ST.state.modal||STLineInputs.isBusy)throw Error('Input cancellation did not release dialog');
          document.getElementById('pattern-record-sample').click();
          while((ST.state.modal!=='recording'||STRecording.isBusy||Array.from(ST.state.pending.values()).some(item=>item.command==='audio.devices'))&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,10));
          if(ST.state.modal!=='recording'||!document.getElementById('record-start')||!document.getElementById('record-meter-enable'))throw Error('Record sample button did not expose explicit recording controls');
          if(STRecording.isRecording)throw Error('Opening Record sample unexpectedly started capture');
          document.getElementById('record-close').click();
          if(ST.state.modal)throw Error('Recording dialog cancellation failed');
          ST.setView('recording');
          const inlineDeadline=Date.now()+5000;
          while((!document.getElementById('record-start')||document.getElementById('record-start').disabled||ST.state.pending.size)&&Date.now()<inlineDeadline)await new Promise(resolve=>setTimeout(resolve,10));
          if(ST.state.modal||!document.querySelector('#recording-inline-host #record-start'))throw Error('Recording controls must mount directly in the main tab');
          if(document.getElementById('record-destination')?.value!=='newTrack')throw Error('Recording tab must default to a new sample and lane');
          if(Array.from(document.getElementById('record-destination').options).some(option=>option.value==='new'))throw Error('Legacy reassign-current-track destination remains exposed');
          const source=document.getElementById('record-source');source.value='master';source.dispatchEvent(new Event('change',{bubbles:true}));
          const name=document.getElementById('record-name');name.value='Preserve my recording draft';name.dispatchEvent(new Event('input',{bubbles:true}));
          ST.refresh();await new Promise(resolve=>setTimeout(resolve,30));
          if(document.getElementById('record-source')?.value!=='master'||document.getElementById('record-name')?.value!=='Preserve my recording draft')throw Error('Refreshing Recording lost the source or name draft');
          ST.setView('pattern');
          if(calls.some(command=>['audio.inputs','audio.record','audio.configure'].includes(command)))throw Error('Opening or canceling input dialogs touched capture hardware');
          if(JSON.stringify(ST.state.project)!==before||ST.state.undo.length!==undo)throw Error('Input dialog inspection edited the project');
        }finally{ST.request=original;ST.closeModal();}
        return true;
        """)
        record("Mounted input controls and Recording tab preserve source selection, default to new sample/track, and cancel without capture or project edits")

        _ = try await js("ST.setView('recording');return true;")
        try await waitFor("document.getElementById('record-start') && !document.getElementById('record-start').disabled && ST.state.pending.size===0", "inline recording workspace")
        _ = try await js("const source=document.getElementById('record-source');source.value='input';source.dispatchEvent(new Event('change',{bubbles:true}));return true;")
        for size in [CGSize(width:1500,height:940), CGSize(width:1100,height:760)] {
            web.frame.size = size
            try await waitFor("innerWidth===\(Int(size.width)) && innerHeight===\(Int(size.height))", "recording viewport")
            let layout = try await js("""
            const ids=['record-name','record-source','record-destination','record-input-device','record-start','record-meter-enable','record-level-right'];
            const controls=ids.map(id=>{const e=document.getElementById(id),r=e?.getBoundingClientRect();return {id,visible:!!e&&r.width>0&&r.height>0&&r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight};});
            return {controls,passed:controls.every(x=>x.visible)};
            """) as? [String:Any] ?? [:]
            try require(layout["passed"] as? Bool == true, "Recording controls clipped at \(Int(size.width))×\(Int(size.height)): \(layout)")
            let snapshot = WKSnapshotConfiguration(); snapshot.rect = web.bounds
            let image = try await web.takeSnapshot(configuration:snapshot)
            if let data=image.tiffRepresentation, let bitmap=NSBitmapImageRep(data:data), let png=bitmap.representation(using:.png,properties:[:]) {
                try png.write(to:output.appendingPathComponent("recording-\(Int(size.width))x\(Int(size.height)).png"))
            }
        }
        web.frame.size = CGSize(width:1500,height:940)
        _ = try await js("ST.setView('pattern');return true;")
        record("Inline Recording workspace keeps primary controls visible at both window sizes")

        let recordedPath = try await js("""
        ST.setView('recording');
        const deadline=Date.now()+30000;
        while((!document.getElementById('record-start')||document.getElementById('record-start').disabled)&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,10));
        const before=JSON.stringify(ST.state.project.instruments),tracks=ST.state.project.tracks.length,undo=ST.state.undo.length;
        const source=document.getElementById('record-source');source.value=captureInput?'input':'master';source.dispatchEvent(new Event('change',{bubbles:true}));
        document.getElementById('record-name').value='Inline master capture';document.getElementById('record-start-playback').checked=false;
        document.getElementById('record-start').click();
        while((!STRecording.isRecording||STRecording.isBusy)&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,10));
        if(!STRecording.isRecording)throw Error('Inline capture did not start: '+document.getElementById('record-status')?.textContent);
        ST.setView('pattern');if(ST.state.view!=='recording')throw Error('Active recording allowed leaving its stop control');
        await new Promise(resolve=>setTimeout(resolve,captureInput?8000:300));
        document.getElementById('record-start').click();
        while((STRecording.isRecording||STRecording.isBusy||ST.state.project.tracks.length===tracks)&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,10));
        if(ST.state.project.tracks.length!==tracks+1||ST.state.undo.length!==undo+1)throw Error('Inline recording did not create one undoable sample lane');
        if(JSON.stringify(ST.state.project.instruments.slice(0,-1))!==before)throw Error('Inline recording modified an existing instrument');
        const path=ST.state.project.instruments.at(-1).samplePath;
        const gates=JSON.stringify({mk3:!!window.STMK3?.hasPendingEdits,kits:!!window.STKitGroups?.isBusy,edits:!!window.STInstrumentEdits?.isBusy,lines:!!window.STLineInputs?.isBusy,nks:!!window.STNKSControls?.hasPendingEdits,agent:!!window.STAgent?.hasPendingEdits,rec:!!STRecording.isBusy,active:document.activeElement?.id,activeType:document.activeElement?.type,view:ST.state.view});const undone=await ST.perform('undo');if(ST.state.project.tracks.length!==tracks)return 'UNDO-FAILED: '+undone+' '+gates+' · undo '+ST.state.undo.length+' · '+[...document.querySelectorAll('.toast')].map(t=>t.textContent).join(' | ');if(await ST.perform('redo')===false||ST.state.project.tracks.length!==tracks+1||ST.state.project.instruments.at(-1)?.samplePath!==path)return 'UNDO-FAILED: Redo did not restore the recorded lane';if(await ST.perform('undo')===false||ST.state.project.tracks.length!==tracks)return 'UNDO-FAILED: second Undo kept the recorded lane';ST.setView('pattern');return path;
        """, ["captureInput": ProcessInfo.processInfo.environment["SUPERTRACKER_TEST_MIC_CAPTURE"] == "1"]) as? String ?? ""
        try require(!recordedPath.hasPrefix("UNDO-FAILED"), "Undo kept the recorded sample lane: " + recordedPath)
        let recorded = try AudioSamples.decode(recordedPath)
        try require(!recorded.left.isEmpty, "Inline recording produced an empty capture file")
        record("Inline Record/Stop captures real audio into a new sample lane, protects navigation and preserves existing instruments")

        if kitOnly {
            for name in try await NativeUIKitsTests.run(web:web,scratch:scratch,output:output) { record(name) }
            for name in try await NativeUIBrowserDestinationsTests.run(web:web,scratch:scratch,output:output) { record(name) }
            try await waitFor("ST.state.pending.size===0", "settled kit commands")
            let errors = try await js("return window.__bridgeTestErrors;") as? [String] ?? []
            try require(errors.isEmpty, "Unexpected JavaScript errors: \(errors)")
            let visibleWindows = NSApp.windows.filter { $0.isVisible && !originalVisibleWindows.contains(ObjectIdentifier($0)) }
            try require(visibleWindows.isEmpty, "The isolated kit harness displayed a native window.")
            let report: [String: Any] = ["passed": true, "status": "passed", "suite": "kits", "checks": checks, "interface": html.path,
                "visibleWindowsCreated": visibleWindows.count, "isolatedStorage": true,
                "generatedAt": ISO8601DateFormatter().string(from: Date())]
            try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]).write(to: output.appendingPathComponent("webkit-bridge-report.json"), options: .atomic)
            return checks.map { "WebKit: " + $0 }
        }

        _ = try await js("await STMedia.open(); return true;")
        try await waitFor("document.querySelectorAll('[data-media-reference]').length===1", "missing-media inspection")
        let inspection = try await js("return await ST.request('project.media.inspect',{project:window.__supertracker.getProject()});") as? [String: Any]
        try require(inspection?["missingCount"] as? Int == 1 && inspection?["presentCount"] as? Int == 1, "Inspect must distinguish the missing sample and available clip: \(inspection ?? [:])")
        record("Actual media dialog inspects missing sample and available audio clip")

        _ = try await js("const field=document.getElementById('media-new-root');field.value=path;field.dispatchEvent(new Event('input',{bubbles:true}));document.getElementById('media-add-root').click();document.getElementById('media-search').click();return true;", ["path": replacements.path])
        try await waitFor("document.querySelector('[data-media-candidate]') && !document.querySelector('[data-media-candidate]').disabled", "real filesystem candidate search")
        try require(try await js("return document.querySelector('.media-candidate .verified')!==null && document.getElementById('media-relink-apply').disabled;") as? Bool == true, "A verified candidate must still require explicit selection.")
        _ = try await js("const radio=document.querySelector('[data-media-candidate]');radio.checked=true;radio.dispatchEvent(new Event('change',{bubbles:true}));document.getElementById('media-relink-apply').click();return true;")
        try await waitFor("document.querySelector('.media-relink-complete') && ST.state.undo.length===1", "undoable relink completion")
        let relinked = try await project()
        try require(instrumentPath(relinked) == replacement.path, "Apply must update the chosen sample path.")
        try await waitFor("ST.state.samples[0]?.path===ST.state.project.instruments[0].samplePath", "relinked PCM loading through native engine")
        let engineSnapshot = try await audio.handle("engine.snapshot", payload: [:]) as? [String: Any]
        try require(instrumentPath(engineSnapshot) == replacement.path, "Undoable media replacement did not synchronize the native engine.")
        try require(same(relinked["patterns"], baseline["patterns"]) && same(relinked["audioClips"], baseline["audioClips"]), "Relink changed notes, automation, or clip edits.")
        let relinkedInstrument = (relinked["instruments"] as! [[String: Any]])[0]
        let baselineInstrument = (baseline["instruments"] as! [[String: Any]])[0]
        for key in ["start", "end", "loopStart", "loopEnd", "mode", "tune", "rootNote", "volume", "pan", "attack", "release"] {
            try require(same(relinkedInstrument[key], baselineInstrument[key]), "Relink changed instrument \(key).")
        }
        _ = try await js("document.getElementById('media-relink-close').click();document.getElementById('tracker')?.focus();ST.perform('undo');return true;")
        try require(instrumentPath(try await project()) == missing.path, "Actual Undo must restore the original missing path.")
        _ = try await js("ST.perform('redo');return true;")
        try require(instrumentPath(try await project()) == replacement.path, "Actual Redo must restore the selected replacement.")
        record("Explicit verified relink preserves edits and supports actual Undo/Redo")

        let savedURL = scratch.appendingPathComponent("Saved Session.supertracker")
        _ = try await js("const saved=await ST.request('project.save',{project:window.__supertracker.getProject(),path:path});ST.acceptProject(saved.project);return saved.saved;", ["path": savedURL.path])
        _ = try await js("ST.mutate('saved note edit',p=>{p.patterns[0].steps[0][0].velocity=73;},{render:false});return true;")
        try require(try await js("return await ST.projectAction('save');") as? Bool == true, "Actual Save action did not complete.")
        let saved = try await project()
        try require(store.url == savedURL && !store.dirty, "Native save state did not settle.")
        let disk = try JSONSerialization.jsonObject(with: Data(contentsOf: savedURL)) as! [String: Any]
        let relative = instrumentPath(disk) ?? ""
        try require(relative.hasPrefix("Saved Session.media/"), "Saved project must collect media with a relative reference.")
        try require(try Data(contentsOf: replacement) == originalBytes, "Relink/save modified source sample bytes.")
        try require(try await js("return await ST.projectAction('open',path);", ["path": savedURL.path]) as? Bool == true, "Actual explicit-path Open action did not complete.")
        let reopened = try await project()
        try require(same(reopened, saved), "Save/reopen lost project content or sample settings.")
        try await waitFor("ST.state.pending.size===0", "reopen native commands")
        record("Real bridge Save, portable media collection, and Open preserve the complete project")

        for name in try await NativeMaschineChordPersistenceTests.run(web: web, scratch: scratch, output: output, restoreURL: savedURL) { record(name) }
        for name in try await NativeUIAcceptanceTests.run(web: web, scratch: scratch, output: output) { record(name) }
        for name in try await NativeUIRoutingTests.run(web:web,scratch:scratch,output:output) { record(name) }
        for name in try await NativeUISamplerTests.run(web:web,scratch:scratch,output:output) { record(name) }
        for name in try await NativeUINKSAutomationTests.run(web:web,scratch:scratch,output:output) { record(name) }
        for name in try await NativeUIDuplicateTests.run(web:web,scratch:scratch,output:output) { record(name) }
        for name in try await NativeUILibraryMetadataTests.run(web:web,scratch:scratch,output:output) { record(name) }
        for name in try await NativeUITakesTests.run(web:web,scratch:scratch,output:output) { record(name) }
        for name in try await NativeUIKitsTests.run(web:web,scratch:scratch,output:output) { record(name) }
        for name in try await NativeUIBrowserDestinationsTests.run(web:web,scratch:scratch,output:output) { record(name) }
        try await waitFor("ST.state.pending.size===0", "settled commands before termination preflight")
        for name in try await NativeBridgeSaveSafetyTests.run(directory: scratch.appendingPathComponent("Save safety"), web:web) { record(name) }
        for name in try await NativeTerminationSaveTests.run(directory: scratch.appendingPathComponent("Termination save"), web:web) { record(name) }
        for name in try await NativeBridgeProjectSyncTests.run(directory: output.appendingPathComponent("bridge-project-sync")) { record(name) }
        record(try await NativeBridgeSaveSafetyTests.terminationPreflight(web:web,bridge:bridge,store:store))

        // The debounce is deliberately still pending here. prepareForTermination must read
        // the actual exported JavaScript getter rather than relying on its last engine copy.
        let beforeQuit = store.project?["name"] as? String
        _ = try await js("ST.mutate('immediate quit edit',p=>{p.name='Unsynced quit edit';p.patterns[0].steps[0][0].velocity=47;},{render:false});return true;")
        try require(store.project?["name"] as? String == beforeQuit, "Quit test did not reach the pending-debounce condition.")
        try await bridge.prepareForTermination()
        let recovery = try store.read(store.recoveryURL)
        try require(recovery["name"] as? String == "Unsynced quit edit", "Termination missed the unsynced JavaScript edit.")
        let recoveryPatterns = recovery["patterns"] as? [[String: Any]]
        let steps = recoveryPatterns?.first?["steps"] as? [[Any]]
        try require((steps?.first?.first as? [String: Any])?["velocity"] as? Int == 47, "Termination lost the last unsynced tracker note edit.")
        try require(store.dirty, "Termination must retain unsaved status for the Save/Discard/Cancel decision.")
        record("Immediate quit flush captures pending real-WebKit edits into isolated recovery")
        let errors = try await js("return window.__bridgeTestErrors;") as? [String] ?? []
        try require(errors.isEmpty, "Unexpected JavaScript errors: \(errors)")
        // AppKit may allocate hidden text-input/service windows for a focused WebKit
        // field. What matters here is that the test never displays a window.
        let visibleWindows = NSApp.windows.filter { $0.isVisible && !originalVisibleWindows.contains(ObjectIdentifier($0)) }
        try require(visibleWindows.isEmpty, "The isolated harness displayed a native window.")
        let report: [String: Any] = ["passed": true, "status": "passed", "checks": checks, "interface": html.path,
            "visibleWindowsCreated": visibleWindows.count, "isolatedStorage": true, "sourceAudioUnchanged": true,
            "generatedAt": ISO8601DateFormatter().string(from: Date())]
        try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]).write(to: output.appendingPathComponent("webkit-bridge-report.json"), options: .atomic)
        return checks.map { "WebKit: " + $0 }
    }
}

#endif

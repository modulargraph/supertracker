// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Mounted routing/export coverage through the actual Bridge. File destinations
/// are supplied by the harness; no native panel, installed plug-in, or window opens.
@MainActor
enum NativeUIRoutingTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        var checks: [String] = [], evidence: [String: Any] = [:]
        let folder = output.appendingPathComponent("mounted-routing", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(source, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("WEBKIT ROUTING: " + message) }
        }
        func check(_ expression: String, _ message: String) async throws {
            try require(try await js("return Boolean(\(expression));") as? Bool == true, message)
        }
        func wait(_ expression: String, _ message: String, seconds: Double = 30) async throws {
            let deadline = Date().addingTimeInterval(seconds)
            while Date() < deadline {
                if try await js("return Boolean(\(expression));") as? Bool == true { return }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            let state = try? await js("return {modal:ST.state.modal,focus:document.activeElement?.outerHTML,pending:Array.from(ST.state.pending.values()).map(x=>x.command),toast:document.getElementById('toast-region')?.textContent,export:document.getElementById('export-progress')?.textContent,routing:ST.state.project.routing};")
            throw TrackerError.message("WEBKIT ROUTING: \(message). \(String(describing: state))")
        }
        func key(_ name: String, alt: Bool = false) async throws {
            _ = try await js("document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:name,altKey:alt,bubbles:true,cancelable:true}));return true;", ["name":name,"alt":alt])
        }
        func report(_ passed: Bool, _ error: String? = nil) throws {
            evidence["passed"] = passed; evidence["checks"] = checks
            evidence["generatedAt"] = ISO8601DateFormatter().string(from: Date())
            evidence["nativeFilePanelsTested"] = false; evidence["physicalKeyboardTested"] = false
            evidence["installedPluginsLoaded"] = false
            if let error { evidence["error"] = error }
            try JSONSerialization.data(withJSONObject:evidence,options:[.prettyPrinted,.sortedKeys]).write(to:folder.appendingPathComponent("report.json"),options:.atomic)
        }
        func cleanup() async throws {
            _ = try await js("""
                if(window.__routingNativeRequest){ST.request=window.__routingNativeRequest;delete window.__routingNativeRequest;}
                ST.closeModal();
                if(window.__routingOriginal){
                  ST.acceptProject(window.__routingOriginal);
                  await ST.request('engine.project',{project:window.__supertracker.getProject()});
                }
                for(const name of ['Original','Group','UndoCount','ExportResult'])delete window['__routing'+name];
                return true;
                """)
        }
        try report(false)
        let toneURL = scratch.appendingPathComponent("Mounted routing tone.wav")
        let tone = (0..<24_000).map { Float(sin(Double($0)*2*Double.pi*220/48_000)*0.25) }
        try AudioSamples.write(DecodedSample(path:toneURL.path,sampleRate:48_000,left:tone,right:tone,originalChannels:2),to:toneURL)
        var fixture = NativeSelfTest.testProject()
        fixture["name"] = "Mounted routing fixture"; fixture["masterVolume"] = 0.7
        fixture["tracks"] = [["id":0,"routingID":"mounted-tone","name":"Tone","instrument":0,"volume":0.8,"pan":0]]
        fixture["instruments"] = [["id":0,"name":"Tone sample","type":"sample","samplePath":toneURL.path,"rootNote":60,"volume":1,"pan":0,"mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0.001,"cutoff":20_000]]
        fixture["patterns"] = [["id":0,"name":"Tone","length":4,"steps":[[["note":60,"instrument":0,"velocity":127,"gate":8] as Any,NSNull(),NSNull(),NSNull()]],"automation":[]]]
        fixture["sequence"] = [0]; fixture["activePattern"] = 0; fixture["audioClips"] = []; fixture["audioPlugins"] = []
        fixture["bpm"] = 120; fixture["linesPerBeat"] = 4; fixture["loopSong"] = false; fixture["metronome"] = false
        fixture.removeValue(forKey:"routing")
        do {
            _ = try await js("window.__routingOriginal=window.__supertracker.getProject();await ST.request('transport.stop');ST.acceptProject(project);await ST.request('engine.project',{project:window.__supertracker.getProject()});ST.setView('mixer');return true;", ["project":fixture])
            try await wait("ST.state.pending.size===0", "Fixture synchronization did not settle")
            _ = try await js("await STRouting.createBus('group',{assignOwner:'track:mounted-tone'});return true;")
            try await check("ST.state.modal==='routing-enable'", "Legacy routing did not require explicit opt-in")
            _ = try await js("document.getElementById('routing-optin-apply').click();return true;")
            try await wait("ST.state.modal==='routing-bus'&&document.getElementById('routing-bus-name')", "Enable and continue did not open bus creation")
            _ = try await js("const name=document.getElementById('routing-bus-name');name.value='Drum group';document.getElementById('routing-bus-assign').click();name.focus();return true;")
            try await key("Enter")
            try await wait("!STRouting.isApplying&&!ST.state.modal&&ST.state.project.routing?.buses.length===1&&ST.state.pending.size===0", "Creating the group did not commit and synchronize")
            _ = try await js("window.__routingGroup='bus:'+ST.state.project.routing.buses[0].id;return true;")
            try await check("ST.state.project.routing.outputs['track:mounted-tone']===window.__routingGroup&&STStudio.selectedOwner()===window.__routingGroup&&ST.state.track===0", "Group creation lost its selected owner or track output")
            let nativeRouting = try await js("return (await ST.request('engine.snapshot')).routing;")
            evidence["createdRouting"] = nativeRouting
            checks.append("Mounted legacy opt-in and named group creation update the actual native graph")

            _ = try await js("STStudio.focusSelectedChannel();window.__routingUndoCount=ST.state.undo.length;return true;")
            try await key("ArrowRight")
            try await check("STStudio.selectedOwner()==='master'&&ST.state.track===0", "Right from group did not select Master without changing the note track")
            try await key("ArrowLeft")
            try await check("STStudio.selectedOwner()===window.__routingGroup&&ST.state.undo.length===window.__routingUndoCount", "Left did not return to the group without editing")
            _ = try await js("document.querySelector('[data-studio-owner=\"'+window.__routingGroup+'\"][data-studio-property=volume]').focus();return true;")
            try await key("a", alt:true)
            try await check("ST.state.view==='automation'&&ST.pattern().automation.some(x=>x.owner===window.__routingGroup&&x.target==='volume')&&ST.state.track===0", "Option-A did not create the selected group's volume lane")
            checks.append("Mounted mixer arrows preserve the note track and Option-A targets the selected bus")

            _ = try await js("STRouting.show('track:mounted-tone');await STRouting.showOutput('track:mounted-tone');return true;")
            try await wait("ST.state.modal==='routing-selector'", "Output chooser did not open")
            _ = try await js("await new Promise(resolve=>setTimeout(resolve,25));return true;")
            try await wait("document.activeElement?.id==='routing-search'", "Output chooser did not retain its initial search focus")
            try await key("Tab")
            try await check("document.activeElement?.id==='modal-close'", "Routing chooser Tab did not wrap past its arrow-only options")
            _ = try await js("document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Tab',shiftKey:true,bubbles:true,cancelable:true}));return true;")
            try await check("document.activeElement?.id==='routing-search'", "Routing chooser Shift-Tab did not return to search")
            _ = try await js("const input=document.getElementById('routing-search');input.value='Master';input.dispatchEvent(new Event('input',{bubbles:true}));input.focus();return true;")
            try await key("Enter")
            try await wait("!STRouting.isApplying&&ST.state.project.routing.outputs['track:mounted-tone']==='master'&&ST.state.pending.size===0", "Search and Return did not route the track to Master")
            try await wait("document.activeElement?.id==='routing-output-track_mounted-tone'", "Applied routing did not restore keyboard focus after native capabilities refreshed")
            _ = try await js("STStudio.focusSelectedChannel();ST.perform('undo');return true;")
            try await wait("ST.state.project.routing.outputs['track:mounted-tone']===window.__routingGroup&&ST.state.pending.size===0", "Undo did not restore the exact group destination")
            let savedURL = scratch.appendingPathComponent("Mounted routing.supertracker")
            _ = try await js("const saved=await ST.request('project.save',{path:path,project:window.__supertracker.getProject()});ST.acceptProject(saved.project);const opened=await ST.request('project.open',{path:path});ST.acceptProject(opened.project||opened);await ST.request('engine.project',{project:window.__supertracker.getProject()});return true;", ["path":savedURL.path])
            try await check("ST.state.project.routing.outputs['track:mounted-tone']===window.__routingGroup&&ST.state.project.routing.buses[0].name==='Drum group'&&ST.pattern().automation.some(x=>x.owner===window.__routingGroup)", "Save and reopen lost persistent owner routing or automation")
            checks.append("Mounted output search, Undo and native Save/Open preserve routing and owner automation")

            let exportFolder = folder.appendingPathComponent("stems",isDirectory:true)
            try FileManager.default.createDirectory(at:exportFolder,withIntermediateDirectories:true)
            _ = try await js("await STExport.show();const mode=document.getElementById('export-mode');mode.value='stems';mode.dispatchEvent(new Event('change',{bubbles:true}));return true;")
            try await check("document.querySelectorAll('[data-export-owner]').length===3", "Export did not receive track, group and Master from native capabilities")
            _ = try await js("for(const box of document.querySelectorAll('[data-export-owner]')){const selected=box.dataset.exportOwner===window.__routingGroup||box.dataset.exportOwner==='master';if(box.checked!==selected)box.click();}document.getElementById('export-tail').value='0';document.getElementById('export-search').focus();return true;")
            try await key("ArrowDown")
            try await check("document.activeElement?.dataset.exportOwner==='track:mounted-tone'", "Down from export search did not focus the first channel")
            try await key("End")
            try await check("document.activeElement?.dataset.exportOwner==='master'&&document.querySelectorAll('[data-export-owner]:checked').length===2", "Stem arrows changed the selection or failed to reach Master")
            let config = WKSnapshotConfiguration(); config.rect = web.bounds
            let snapshot = try await web.takeSnapshot(configuration:config)
            if let data = snapshot.tiffRepresentation, let bitmap = NSBitmapImageRep(data:data), let png = bitmap.representation(using:.png,properties:[:]) {
                try png.write(to:folder.appendingPathComponent("export-stems.png"))
            }
            _ = try await js("window.__routingNativeRequest=ST.request;window.__routingExportResult=null;ST.request=async function(command,payload){if(command==='dialog.export')return {path:destination,mode:'stems'};const result=await window.__routingNativeRequest(command,payload);if(command==='audio.export')window.__routingExportResult=result;return result;};document.getElementById('export-start').click();return true;", ["destination":exportFolder.path])
            try await wait("window.__routingExportResult&&!STExport.isBusy", "Mounted stem export did not finish through the real host",seconds:90)
            try await check("/2 stems exported/.test(document.getElementById('export-progress').textContent)", "The export dialog did not confirm the exact completed file count")
            let result = try await js("return window.__routingExportResult;") as? [String:Any] ?? [:]
            let paths = result["paths"] as? [String] ?? []
            try require(paths.count == 2, "The native render did not produce two selected stems")
            var rendered: [[String:Any]] = []
            for path in paths {
                let pcm = try AudioSamples.decode(path)
                let rms = sqrt(pcm.left.reduce(0.0) { $0+Double($1)*Double($1) }/Double(max(1,pcm.left.count)))
                try require(pcm.left.count == 24_000 && rms > 0.001, "Mounted export produced silent or misaligned stem PCM")
                rendered.append(["path":path,"frames":pcm.left.count,"rms":rms])
            }
            evidence["export"] = result; evidence["renderedFiles"] = rendered
            _ = try await js("ST.request=window.__routingNativeRequest;delete window.__routingNativeRequest;ST.closeModal();return true;")
            checks.append("Mounted selected-stem export renders audible aligned files through the actual native graph")

            // A damaged saved route must open as repairable intent, never continue
            // sounding the previous project or silently replace the bad connection.
            var unavailable = try await js("return window.__supertracker.getProject();") as! [String:Any]
            var badRouting = unavailable["routing"] as! [String:Any]
            var badOutputs = badRouting["outputs"] as! [String:Any]
            let busID = (badRouting["buses"] as! [[String:Any]])[0]["id"] as! String
            badOutputs["bus:"+busID] = "bus:"+busID; badRouting["outputs"] = badOutputs
            unavailable["routing"] = badRouting; unavailable["name"] = "Unavailable routed fixture"
            let unavailableURL = scratch.appendingPathComponent("Unavailable routing.supertracker")
            try JSONSerialization.data(withJSONObject:unavailable,options:[.sortedKeys]).write(to:unavailableURL,options:.atomic)
            _ = try await js("const result=await ST.request('project.open',{path:path});ST.acceptProject(result.project);STRouting.show('track:mounted-tone');return true;", ["path":unavailableURL.path])
            try await wait("ST.state.status.routingFailure&&document.getElementById('status-message').textContent.includes('Routing needs repair')", "Opened invalid routing was not exposed in the persistent status bar")
            try await wait("document.querySelector('[data-routing-error]')&&!document.querySelector('[data-routing-error]').hidden", "The mounted mixer did not explain unavailable saved routing")
            try await check("ST.state.project.routing.outputs[window.__routingGroup]===window.__routingGroup", "Opening the bad project silently rewrote its intended route")
            let blocked = try await js("try{await ST.request('transport.play',{mode:'song'});return false;}catch{return true;}") as? Bool ?? false
            try require(blocked, "Unavailable project played audio from the previous session")
            _ = try await js("await STRouting.showOutput(window.__routingGroup);const input=document.getElementById('routing-search');input.value='Master';input.dispatchEvent(new Event('input',{bubbles:true}));input.focus();return true;")
            try await key("Enter")
            try await wait("!STRouting.isApplying&&ST.state.project.routing.outputs[window.__routingGroup]==='master'&&!ST.state.status.routingFailure", "Repairing the saved output did not restore a valid native graph")
            try await check("!document.getElementById('status-message').textContent.includes('Routing needs repair')", "Successful routing repair left a stale failure notice")
            checks.append("Actual Bridge opens invalid saved routing intact, blocks old audio and accepts a keyboard repair")
            try await cleanup()
            try report(true)
        } catch {
            try? await cleanup()
            try? report(false,error.localizedDescription)
            throw error
        }
        return checks
    }
}

#endif

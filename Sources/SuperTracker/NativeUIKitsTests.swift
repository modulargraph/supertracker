// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Kits as track groups through the production UI and Bridge: New kit, Save kit into the
/// isolated kit library, Load kit while playing (one Undo step, incremental routing),
/// Replace kit keeping the pattern, group preview, collapse and the mixer group strip.
/// The caller supplies the isolated WebKit/Bridge/Library fixture. No vendor is loaded
/// and no device is selected.
@MainActor
enum NativeUIKitsTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        guard let support = ProcessInfo.processInfo.environment["SUPERTRACKER_SUPPORT_DIR"], support.hasPrefix("/") else {
            throw TrackerError.message("WEBKIT KITS: Set SUPERTRACKER_SUPPORT_DIR to the isolated harness directory before running kit acceptance.")
        }
        let files = FileManager.default
        let folder = output.appendingPathComponent("mounted-kits", isDirectory: true)
        let media = scratch.appendingPathComponent("Mounted kit assets", isDirectory: true)
        for directory in [folder, media] { try files.createDirectory(at: directory, withIntermediateDirectories: true) }
        var checks: [String] = []
        var evidence: [String: Any] = ["physicalKeyboardTested": false, "physicalMK3Tested": false, "nativeFilePanelsTested": false,
                                       "installedPluginsLoaded": false, "supportDirectory": support]
        func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            let wrapped = """
                try {
                \(source)
                } catch (__kitAcceptanceJSError) {
                  const message=String(__kitAcceptanceJSError?.message??__kitAcceptanceJSError);
                  const stack=__kitAcceptanceJSError?.stack?String(__kitAcceptanceJSError.stack):'';
                  throw new Error(message+(stack?'\\n'+stack:''));
                }
                """
            do {
                return try await web.callAsyncJavaScript(wrapped, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
            } catch {
                let native = error as NSError
                let details = native.userInfo.keys.filter { $0.hasPrefix("WKJavaScriptException") }.sorted().map { key in
                    "\(key): \(native.userInfo[key].map { String(describing: $0) } ?? "unavailable")"
                }
                throw TrackerError.message((["WEBKIT KITS JavaScript: \(native.localizedDescription) [\(native.domain) \(native.code)]"] + details).joined(separator: "\n"))
            }
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("WEBKIT KITS: " + message) }
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
            evidence["timeoutState"] = try? await js("return {phase:window.__kitAcceptancePhase,view:ST.state.view,modal:ST.state.modal,pending:Array.from(ST.state.pending.values()).map(x=>x.command),toast:document.getElementById('toast-region')?.textContent,status:ST.state.status,project:window.__supertracker.getProject()};")
            throw TrackerError.message("WEBKIT KITS: \(message). See timeoutState in mounted-kits/report.json.")
        }
        func settle() async throws {
            _ = try await js("await ST.waitForEngineUpdate();return true;")
            try await wait("ST.state.pending.size===0&&!STKitGroups.isBusy", "Kit edits did not settle")
        }
        func report(_ passed: Bool, error: String? = nil) throws {
            evidence["passed"] = passed; evidence["checks"] = checks
            evidence["generatedAt"] = ISO8601DateFormatter().string(from: Date())
            if let error { evidence["error"] = error }
            try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys])
                .write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func snapshot(_ name: String) async throws {
            let configuration = WKSnapshotConfiguration(); configuration.rect = web.bounds
            let image = try await web.takeSnapshot(configuration: configuration)
            if let data = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data), let png = bitmap.representation(using: .png, properties: [:]) {
                try png.write(to: folder.appendingPathComponent(name + ".png"))
            }
        }
        func cleanup() async throws {
            _ = try await js("""
                window.__kitAcceptancePhase='cleanup';
                ST.stopHeld(); await ST.request('transport.stop'); ST.closeModal();
                if(window.__kitAcceptanceOriginal){
                  ST.acceptProject(window.__kitAcceptanceOriginal);
                  if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Baseline graph restoration was superseded');
                  ST.setView('pattern');
                }
                for(const name of Object.keys(window).filter(x=>x.startsWith('__kitAcceptance')))delete window[name];
                return true;
                """)
        }
        func writeTone(_ name: String, frequency: Double, amplitude: Double, frames: Int = 24_000) throws -> URL {
            let url = media.appendingPathComponent(name + ".wav")
            let pcm = (0..<frames).map { index -> Float in
                let envelope = min(1, Double(index) / 64) * exp(-Double(index) / 12_000)
                return Float(sin(Double(index) * 2 * .pi * frequency / 48_000) * amplitude * envelope)
            }
            try AudioSamples.write(DecodedSample(path: url.path, sampleRate: 48_000, left: pcm, right: pcm, originalChannels: 2), to: url)
            return url
        }
        let kick = try writeTone("Kit kick", frequency: 110, amplitude: 0.24)
        let hat = try writeTone("Kit hat", frequency: 1760, amplitude: 0.12)
        var fixture = NativeSelfTest.testProject()
        fixture["name"] = "Mounted kit acceptance"
        fixture["bpm"] = 120; fixture["linesPerBeat"] = 4; fixture["masterVolume"] = 1
        fixture["tracks"] = [["id": 0, "routingID": "kit-fixture-standard", "name": "Standard tracker", "instrument": 0, "volume": 1, "pan": 0]]
        fixture["instruments"] = [["id": 0, "name": "Preserved synth", "type": "synth", "volume": 0.2, "waveform": "sine", "attack": 0, "decay": 0, "sustain": 1, "release": 0.01]]
        fixture["patterns"] = [["id": 0, "name": "Kit hits", "length": 8, "steps": [[["note": 48, "instrument": 0, "velocity": 100, "gate": 8.0]] + Array(repeating: NSNull(), count: 7)], "automation": []]]
        fixture["sequence"] = [0]; fixture["activePattern"] = 0; fixture["loopSong"] = true; fixture["metronome"] = false
        fixture["audioPlugins"] = []; fixture["audioClips"] = []; fixture["groups"] = []
        fixture["delay"] = ["mix": 0]; fixture["reverb"] = ["mix": 0]
        fixture["routing"] = ["version": 1, "mixModel": "standard", "buses": [], "outputs": [:], "sends": [], "sidechains": []]
        try report(false)
        do {
            // 1. New kit, a second pad, a sample per pad, the two pads in one choke group; Save kit.
            let saved = try await js("""
                window.__kitAcceptanceOriginal=window.__supertracker.getProject();
                window.__kitAcceptancePhase='fixture';
                await ST.request('transport.stop');
                ST.acceptProject(JSON.parse(JSON.stringify(project)));
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Initial fixture replacement was superseded');
                window.__kitAcceptancePhase='new kit';
                if(!STKitGroups.createKit('Acceptance Drums'))throw Error('New kit failed');
                for(let pad=1;pad<16;pad++)if(await ST.perform('addGroupTrack')===false)throw Error('Add track to group failed');
                const p=ST.state.project,group=p.groups[0],pads=STGroups.members(p,group.id);
                if(pads.length!==16||pads.some(i=>p.tracks[i].groupID!==group.id))throw Error('The kit does not hold sixteen pad tracks');
                for(const [i,path] of pads.map((_,i)=>[i,i%2?hat:kick])){
                  ST.state.track=pads[i];ST.state.instrument=p.tracks[pads[i]].instrument;
                  if(!await ST.loadSample(path,ST.state.instrument))throw Error('Pad sample did not load');
                }
                ST.mutate('choke hats',q=>{for(const i of STGroups.members(q,group.id).slice(0,2))q.tracks[i].chokeGroup=1;});
                window.__kitAcceptancePhase='save kit';
                ST.state.track=pads[0];
                if(!await STKitGroups.saveKit({name:'Acceptance Drums'}))throw Error('Save kit failed: '+document.getElementById('toast-region')?.textContent);
                const libraryPath=ST.state.project.groups[0].libraryPath;
                if(!libraryPath)throw Error('Save kit did not record the library package');
                return {libraryPath,groupID:group.id,pads};
                """, ["project": fixture, "kick": kick.path, "hat": hat.path]) as? [String: Any] ?? [:]
            try await settle()
            guard let libraryPath = saved["libraryPath"] as? String, let firstGroup = saved["groupID"] as? String else { throw TrackerError.message("WEBKIT KITS: Save kit returned no package") }
            try require(files.fileExists(atPath: URL(fileURLWithPath: libraryPath).appendingPathComponent("kit.json").path) || files.fileExists(atPath: libraryPath), "The saved kit package is missing")
            try require(libraryPath.hasPrefix(support), "Save kit wrote outside the isolated support directory: \(libraryPath)")
            evidence["libraryPath"] = libraryPath
            checks.append("New kit and Add track to group make sixteen pad tracks; Save kit writes the group into the isolated kit library")

            // 2. Load the saved kit while playing: one Undo step, the graph extends without a rebuild.
            let load = try await js("""
                window.__kitAcceptancePhase='load kit while playing';
                ST.state.track=0;ST.state.instrument=0;
                await ST.perform('play');
                const deadline=Date.now()+5000;
                while(!(ST.state.status.playing&&ST.state.status.running)&&Date.now()<deadline)await new Promise(r=>setTimeout(r,20));
                if(!ST.state.status.playing)throw Error('Transport did not start');
                const routing=await ST.request('routing.status'),undo=ST.state.undo.length,tracks=ST.state.project.tracks.length;
                const started=performance.now();
                if(!await STKitGroups.loadKit(path))throw Error('Load kit failed: '+document.getElementById('toast-region')?.textContent);
                const milliseconds=performance.now()-started;
                await ST.waitForEngineUpdate();
                const after=await ST.request('routing.status'),p=ST.state.project,group=p.groups.at(-1);
                const members=STGroups.members(p,group.id);
                return {milliseconds,undoAdded:ST.state.undo.length-undo,label:ST.state.undo.at(-1)?.label,tracksAdded:p.tracks.length-tracks,
                  groups:p.groups.length,members:members.length,chokes:members.map(i=>p.tracks[i].chokeGroup||0),
                  libraryPath:group.libraryPath||null,rebuildsBefore:routing.fullRebuilds,rebuildsAfter:after.fullRebuilds,
                  incrementalBefore:routing.incrementalChanges,incrementalAfter:after.incrementalChanges,rebuildReason:after.lastFullRebuildReason||null,
                  playing:ST.state.status.playing,running:ST.state.status.running,newGroup:group.id};
                """, ["path": libraryPath]) as? [String: Any] ?? [:]
            evidence["loadWhilePlaying"] = load
            try require((load["undoAdded"] as? NSNumber)?.intValue == 1 && load["label"] as? String == "load kit", "Load kit was not exactly one Undo step: \(load)")
            try require((load["tracksAdded"] as? NSNumber)?.intValue == 16 && (load["groups"] as? NSNumber)?.intValue == 2 && (load["members"] as? NSNumber)?.intValue == 16, "Load kit did not add one group of sixteen tracks: \(load)")
            try require((load["chokes"] as? [NSNumber])?.map(\.intValue) == [1, 1] + Array(repeating: 0, count: 14), "The loaded kit lost its choke group")
            try require(load["rebuildsBefore"] as? NSNumber == load["rebuildsAfter"] as? NSNumber
                        && ((load["incrementalAfter"] as? NSNumber)?.intValue ?? 0) > ((load["incrementalBefore"] as? NSNumber)?.intValue ?? 0),
                        "Load kit rebuilt the routing graph instead of extending it: \(load)")
            try require(load["playing"] as? Bool == true && load["running"] as? Bool == true, "Load kit stopped playback")
            try await wait("(ST.state.status.trackPeaks||[])[0]>.001", "The playing track went silent after Load kit")
            checks.append("Load kit while playing adds the group and its sixteen pad tracks as one Undo step (\(Int((load["milliseconds"] as? NSNumber)?.doubleValue ?? 0)) ms) with incremental routing and playback running")

            _ = try await js("""
                window.__kitAcceptancePhase='undo load';
                await ST.perform('undo');
                if(ST.state.project.groups.length!==1||ST.state.project.tracks.length!==17)throw Error('Undo did not remove the loaded kit');
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('Undo publication failed');
                const native=await ST.request('engine.snapshot');
                if(native.groups.length!==1||native.tracks.length!==17)throw Error('The native graph kept the undone kit');
                await ST.perform('redo');
                if(ST.state.project.groups.length!==2||ST.state.project.tracks.length!==33)throw Error('Redo did not restore the loaded kit');
                await ST.perform('stop');
                return true;
                """)
            try await settle()
            checks.append("Undo removes the loaded kit from the project and the native graph; Redo restores it")

            // 3. Replace kit keeps the pattern notes of the pads it replaces.
            let replaced = try await js("""
                window.__kitAcceptancePhase='replace kit';
                const group=ST.state.project.groups[1],pads=STGroups.members(ST.state.project,group.id);
                ST.mutate('pad notes',q=>{q.patterns[0].steps[pads[0]][0]={note:60,instrument:q.tracks[pads[0]].instrument,velocity:110};q.patterns[0].steps[pads[1]][2]={note:60,instrument:q.tracks[pads[1]].instrument,velocity:90};});
                ST.state.track=pads[0];
                const undo=ST.state.undo.length;
                if(!await STKitGroups.loadKit(path,{replace:true}))throw Error('Replace kit failed: '+document.getElementById('toast-region')?.textContent);
                const p=ST.state.project,now=STGroups.members(p,p.groups[1].id);
                return {undoAdded:ST.state.undo.length-undo,label:ST.state.undo.at(-1)?.label,groups:p.groups.length,members:now.length,
                  notes:[p.patterns[0].steps[now[0]][0]?.note??null,p.patterns[0].steps[now[1]][2]?.note??null]};
                """, ["path": libraryPath]) as? [String: Any] ?? [:]
            evidence["replace"] = replaced
            try require((replaced["undoAdded"] as? NSNumber)?.intValue == 1 && replaced["label"] as? String == "replace kit"
                        && (replaced["groups"] as? NSNumber)?.intValue == 2 && (replaced["notes"] as? [NSNumber])?.map(\.intValue) == [60, 60],
                        "Replace kit was not one Undo step or lost the pad notes: \(replaced)")
            try await settle()
            checks.append("Replace kit swaps the group's sounds in one Undo step and keeps the pad tracks' pattern notes")

            // 4. Group preview of the pattern, then the collapsed group, the expanded group and the mixer.
            let preview = try await js("""
                window.__kitAcceptancePhase='preview';
                const group=ST.state.project.groups[1];
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('Preview publication failed');
                return await ST.request('audio.kitPreview',{groupID:group.id,path,pattern:0});
                """, ["path": folder.appendingPathComponent("group-preview.wav").path]) as? [String: Any] ?? [:]
            let previewPCM = try AudioSamples.decode(folder.appendingPathComponent("group-preview.wav").path)
            try require(previewPCM.left.contains { abs($0) > 0.001 } && (preview["previewOwner"] as? String)?.hasPrefix("bus:") == true, "The group preview is silent or not the group bus")
            checks.append("Group preview renders the kit's bus from the pattern")
            _ = try await js("ST.setView('pattern');return true;")
            try await settle(); try await snapshot("kit-expanded")
            _ = try await js("const id=ST.state.project.groups[1].id;document.querySelector('[data-group-collapse=\"'+id+'\"]').click();return true;")
            try await wait("ST.state.project.groups[1].collapsed===true&&!!document.querySelector('.group-overview-cell')", "The group did not collapse")
            try await snapshot("kit-collapsed")
            _ = try await js("ST.setView('mixer');return true;")
            try await wait("!!document.querySelector('[data-group-mixer-expand]')", "The mixer shows no group strip")
            try await snapshot("kit-mixer")
            _ = try await js("ST.setView('pattern');return true;")
            checks.append("The group collapses to one overview column and the mixer shows its group strip")

            try await cleanup(); try report(true)
        } catch {
            evidence["failure"] = error.localizedDescription
            do { try await cleanup() } catch { evidence["cleanupError"] = error.localizedDescription }
            try? report(false, error: error.localizedDescription)
            throw error
        }
        return checks
    }
}

#endif

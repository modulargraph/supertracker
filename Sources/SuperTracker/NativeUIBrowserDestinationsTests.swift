// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Production Web resources and Bridge, private generated samples only.
/// DOM events are synthetic; this is not physical keyboard or listening evidence.
@MainActor enum NativeUIBrowserDestinationsTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        let files = FileManager.default
        let media = scratch.appendingPathComponent("Browser destination samples")
        let folder = output.appendingPathComponent("browser-destinations")
        for url in [media, folder] { try files.createDirectory(at: url, withIntermediateDirectories: true) }
        let paths = ["Destination_A.wav", "Destination_B.wav"].map { media.appendingPathComponent($0) }
        for (index, url) in paths.enumerated() {
            let count = 4800 + index * 2400
            let pcm = (0..<count).map { Float(sin(Double($0) * (index == 0 ? 0.04 : 0.07)) * 0.1) }
            try AudioSamples.write(DecodedSample(path: url.path, sampleRate: 48000, left: pcm, right: pcm, originalChannels: 2), to: url)
        }
        let originals = try paths.map { try Data(contentsOf: $0) }
        var checks: [String] = []
        let masterZeroProbe = ProcessInfo.processInfo.environment["SUPERTRACKER_TEST_ZERO_MASTER"] == "1"
        var evidence: [String: Any] = ["passed": false, "physicalKeyboardTested": false, "installedPluginsLoaded": false, "audioPlaybackTested": false, "masterZeroProbe": masterZeroProbe]
        func write() throws {
            evidence["checks"] = checks
            try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys]).write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            do { return try await web.callAsyncJavaScript(source, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull() }
            catch {
                let error = error as NSError
                throw TrackerError.message("BROWSER DESTINATIONS: \(error.localizedDescription) \(error.userInfo)")
            }
        }
        func require(_ condition: Bool, _ text: String) throws { if !condition { throw TrackerError.message("BROWSER DESTINATIONS: " + text) } }
        func check(_ expression: String, _ text: String) async throws { try require(try await js("return Boolean(\(expression));") as? Bool == true, text) }
        func wait(_ expression: String) async throws {
            let deadline = Date().addingTimeInterval(30)
            while Date() < deadline {
                if try await js("return Boolean(\(expression));") as? Bool == true { return }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            throw TrackerError.message("BROWSER DESTINATIONS: Timed out: " + expression)
        }
        func cleanup() async throws {
            _ = try await js("""
            if(window.__browserDestinationReceive)window.nativeReceive=window.__browserDestinationReceive;
            await STBrowser.stopPreview(); ST.stopHeld(); await ST.request('transport.stop'); ST.closeModal();
            await ST.request('library.removeRoot',{path:root});
            if(window.__browserDestinationOriginal){
              ST.acceptProject(JSON.parse(window.__browserDestinationOriginal));
              if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Baseline restoration was not acknowledged');
              ST.setView('pattern');
            }
            for(const key of Object.keys(window).filter(key=>key.startsWith('__browserDestination')))delete window[key];
            return true;
            """, ["root": media.path])
        }
        let priorFrame = web.frame
        defer { web.frame = priorFrame }
        var fixture = NativeSelfTest.testProject()
        fixture["name"] = "Browser destinations acceptance"
        fixture["audioPlugins"] = []; fixture["audioClips"] = []
        fixture["routing"] = ["version": 1, "mixModel": "standard", "buses": [], "outputs": [:], "sends": [], "sidechains": []]
        do {
            try write()
            _ = try await js("""
            ST.closeModal();window.__browserDestinationOriginal=JSON.stringify(window.__supertracker.getProject());
            await ST.request('transport.stop');ST.acceptProject(JSON.parse(JSON.stringify(project)));
            if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Fixture publication failed');
            const saved=await ST.request('project.save',{path:savePath,project:window.__supertracker.getProject(),collectSamples:false});
            if(!saved.saved)throw Error('Baseline project save failed');
            ST.acceptProject(saved.project);window.__browserDestinationBefore=JSON.stringify(ST.state.project.instruments);
            window.__browserDestinationUndo=ST.state.undo.length;
            await ST.request('library.scan',{roots:[root]});
            Object.assign(ST.state.library,{query:'Destination_',category:'',kind:'sample',filters:{},favorites:false,offset:0,selected:0});
            ST.setView('browser');STBrowser.controller.setTab('samples');
            const preview=document.getElementById('browser-audition');preview.checked=false;preview.dispatchEvent(new Event('change',{bubbles:true}));
            await STBrowser.search();return true;
            """, ["project": fixture, "savePath": scratch.appendingPathComponent("Browser baseline.stproj").path, "root": media.path])
            try await wait("!ST.state.library.scanning && ST.state.library.items.length===2")
            _ = try await js("await STBrowser.search();return true;")
            for index in 0..<2 {
                _ = try await js("""
                const index=ST.state.library.items.findIndex(item=>item.path===path);if(index<0)throw Error('Generated sample not found');
                ST.state.library.selected=index;ST.refresh();document.getElementById('browser-load').focus();document.getElementById('browser-load').click();return true;
                """, ["path": paths[index].path])
                try await wait("ST.state.project.instruments.length===\(index + 2) && !STInstrumentEdits.isBusy && !document.getElementById('browser-load').disabled")
            }
            try await check("ST.state.view==='browser'&&JSON.stringify(ST.state.project.instruments.slice(0,1))===window.__browserDestinationBefore&&ST.state.undo.length===window.__browserDestinationUndo+2", "Repeated Add changed an existing instrument, left Browser, or created extra Undo entries")
            let metadata = try await js("""
            const a=ST.state.project.instruments[1],b=ST.state.project.instruments[2];
            if(ST.state.track!==2||ST.state.project.tracks[1]?.instrument!==1||ST.state.project.tracks[2]?.instrument!==2)throw Error('Added samples need manual track assignment');
            if(a.id===b.id||a.samplePath===b.samplePath)throw Error('Two additions share an instrument ID or sample path');
            const infos=await Promise.all([a,b].map(i=>ST.request('sample.info',{instrument:i.id,path:i.samplePath})));
            if(infos[0].frames!==4800||infos[1].frames!==7200||infos.some(x=>x.sampleRate!==48000))throw Error('Native PCM metadata does not match generated sample data');
            return {instruments:[a,b].map(i=>({id:i.id,path:i.samplePath})),metadata:infos.map(x=>({path:x.path,frames:x.frames,sampleRate:x.sampleRate}))};
            """)
            evidence["loadedSamples"] = metadata
            checks.append("Two actual Add buttons create distinct native samples in Browser with one Undo each and preserve existing instruments")

            _ = try await js("window.__browserDestinationAdded=JSON.stringify(window.__supertracker.getProject());await ST.perform('undo');return true;")
            try await check("ST.state.project.instruments.length===2", "Undo did not remove exactly the latest sample instrument")
            _ = try await js("await ST.perform('redo');if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('Redo publication failed');return true;")
            try await check("JSON.stringify(window.__supertracker.getProject())===window.__browserDestinationAdded", "Redo did not restore the exact added samples")
            let failed = try await js("""
            const before=JSON.stringify(window.__supertracker.getProject()),undo=ST.state.undo.length,index=ST.state.library.selected,item=ST.state.library.items[index];
            ST.state.library.items[index]={...item,path:missing};
            try{await STBrowser.controller.load(true);}finally{ST.state.library.items[index]=item;ST.refresh();}
            return {unchanged:before===JSON.stringify(window.__supertracker.getProject()),undoUnchanged:undo===ST.state.undo.length,toast:document.getElementById('toast-region')?.textContent};
            """, ["missing": media.appendingPathComponent("missing.wav").path]) as? [String: Any] ?? [:]
            try require(failed["unchanged"] as? Bool == true && failed["undoUnchanged"] as? Bool == true, "A failed native sample decode left an empty instrument or Undo entry")
            evidence["failedLoad"] = failed
            checks.append("Undo/Redo preserves exact added samples and a missing-file load leaves project and history unchanged")

            var layouts: [[String: Any]] = []
            for kit in [false, true] {
                if kit {
                    // A kit pad is an ordinary member track: Replace selected instrument loads into that pad only.
                    _ = try await js("""
                    if(!STKitGroups.createKit('Browser pad bank'))throw Error('Kit creation failed');
                    const p=ST.state.project,group=p.groups.at(-1),pad=STGroups.members(p,group.id)[0];
                    if(ST.state.track!==pad)throw Error('New kit did not select its first pad');
                    ST.setView('browser');STBrowser.controller.setTab('samples');
                    window.__browserDestinationPad=pad;window.__browserDestinationGlobal=JSON.stringify(p.instruments.filter((_,i)=>i!==p.tracks[pad].instrument));
                    window.__browserDestinationPadUndo=ST.state.undo.length;return true;
                    """)
                    try await wait("!document.getElementById('browser-load-new')?.disabled")
                    _ = try await js("document.getElementById('browser-load-new').click();return true;")
                    try await wait("!STKitGroups.isBusy && !!ST.state.project.instruments[ST.state.project.tracks[window.__browserDestinationPad].instrument].samplePath && !document.getElementById('browser-load').disabled")
                    try await check("(()=>{const p=ST.state.project,pad=window.__browserDestinationPad;return ST.state.view==='browser'&&ST.state.track===pad&&p.tracks[pad].groupID===p.groups.at(-1).id&&JSON.stringify(p.instruments.filter((_,i)=>i!==p.tracks[pad].instrument))===window.__browserDestinationGlobal&&ST.state.undo.length===window.__browserDestinationPadUndo+1;})()", "Pad load overwrote another instrument, left the group, or changed view/selection/history")
                    let info = try await js("const p=ST.state.project,i=p.tracks[window.__browserDestinationPad].instrument;return await ST.request('sample.info',{instrument:i,path:p.instruments[i].samplePath});") as? [String: Any] ?? [:]
                    try require((info["frames"] as? NSNumber)?.intValue == 7200, "Selected pad has no matching native PCM")
                    checks.append("A new kit selects its first pad track; actual Replace loads only that pad's sampler with one Undo")
                }
                for size in [CGSize(width: 1080, height: 700), CGSize(width: 1500, height: 940)] {
                    web.frame.size = size
                    try await wait("innerWidth===\(Int(size.width)) && innerHeight===\(Int(size.height))")
                    let layout = try await js("""
                    const ids=['browser-destination-instrument','browser-load','browser-load-new'];
                    const controls=ids.map(id=>{const e=document.getElementById(id),r=e?.getBoundingClientRect();return {id,visible:!!e&&r.width>0&&r.height>0&&r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight&&e.contains(document.elementFromPoint(r.left+r.width/2,r.top+r.height/2))};});
                    return {kit,width:innerWidth,height:innerHeight,controls,passed:controls.every(x=>x.visible)};
                    """, ["kit": kit]) as? [String: Any] ?? [:]
                    try require(layout["passed"] as? Bool == true, "Persistent destination controls are clipped at \(Int(size.width))×\(Int(size.height)), kit=\(kit)")
                    layouts.append(layout); evidence["layouts"] = layouts; try write()
                    let configuration = WKSnapshotConfiguration(); configuration.rect = web.bounds
                    let image = try await web.takeSnapshot(configuration: configuration)
                    if let data = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data), let png = bitmap.representation(using: .png, properties: [:]) {
                        try png.write(to: folder.appendingPathComponent("browser-\(kit ? "kit" : "instruments")-\(Int(size.width))x\(Int(size.height)).png"))
                    }
                }
            }
            _ = try await js("""
            ST.closeModal();
            if(!await ST.projectAction('save'))throw Error('Private fixture save before New failed');
            if(!await ST.projectAction('new'))throw Error('Native New session failed');
            const p=ST.state.project;
            if(p.tracks.length!==1||p.patterns.some(pat=>pat.steps.length!==1||pat.steps[0].some(Boolean)))throw Error('New session did not contain exactly one empty lane');
            window.__browserDestinationEmpty=JSON.stringify(window.__supertracker.getProject());
            if(!STKitGroups.createKit('First lane kit'))throw Error('First kit creation failed');
            const kitProject=ST.state.project,pad=STGroups.members(kitProject,kitProject.groups[0].id)[0];
            if(kitProject.groups.length!==1||ST.state.track!==pad)throw Error('New kit did not select its pad track');
            if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('First kit publication failed');
            const native=await ST.request('engine.snapshot');
            if(native.groups?.length!==1||native.tracks[pad]?.groupID!==native.groups[0].id)throw Error('Native graph did not acknowledge the kit group');
            await ST.perform('undo');
            if(JSON.stringify(window.__supertracker.getProject())!==window.__browserDestinationEmpty)throw Error('Undo did not restore the exact empty session');
            await ST.perform('redo');
            if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('Kit Redo publication failed');
            ST.setView('pattern');ST.state.track=pad;ST.state.instrument=ST.state.project.tracks[pad].instrument;
            if(!await ST.loadSample(path,ST.state.instrument))throw Error('Generated kit sound did not load');
            window.__browserDestinationKitPad=pad;
            // Keep the ordinary master path active while silencing the generated pad.
            // The opt-in master-zero diagnostic reproduces the separately tracked ACK bug.
            ST.mutate('mute isolated playback',p=>{if(muteMaster)p.masterVolume=0;else p.instruments[p.tracks[pad].instrument].volume=0;});
            document.getElementById('pattern-enter-notes').focus();document.getElementById('pattern-enter-notes').click();return true;
            """, ["path": paths[0].path, "muteMaster": masterZeroProbe])
            try await wait("ST.state.recording && ST.state.keyboardPlay && document.activeElement.id==='tracker'")
            _ = try await js("""
            window.__browserDestinationInputReplies=[];window.__browserDestinationReceive=window.nativeReceive;
            window.nativeReceive=message=>{const m=typeof message==='string'?JSON.parse(message):message;const command=ST.state.pending.get(m.id)?.command;if(command==='note.on'||command==='note.off')window.__browserDestinationInputReplies.push({command,response:m});return window.__browserDestinationReceive(message);};
            const grid=document.getElementById('tracker');
            grid.dispatchEvent(new KeyboardEvent('keydown',{key:'z',code:'KeyZ',bubbles:true,cancelable:true}));
            grid.dispatchEvent(new KeyboardEvent('keyup',{key:'z',code:'KeyZ',bubbles:true,cancelable:true}));return true;
            """)
            try await wait("ST.pattern().steps[window.__browserDestinationKitPad]?.[0]?.note>=0")
            try await check("ST.pattern().steps.every((lane,i)=>i===window.__browserDestinationKitPad||!lane[0])", "Pattern keyboard entered the note on a track other than the kit pad")
            _ = try await js("document.getElementById('pattern-play-current').click();return true;")
            try await wait("ST.state.playing && ST.state.playbackMode==='pattern'")
            _ = try await js("await ST.perform('stop');await ST.setNoteRecording(false);return true;")
            try await wait("!ST.state.playing")
            checks.append("Native New then New kit: exact Undo/Redo, armed keyboard step entry on the pad track and muted Pattern playback")

            for size in [CGSize(width: 1080, height: 700), CGSize(width: 1500, height: 940)] {
                web.frame.size = size
                try await wait("innerWidth===\(Int(size.width)) && innerHeight===\(Int(size.height))")
                let layout = try await js("""
                const ids=['pattern-destination-lane','pattern-enter-notes','pattern-play-current'];
                const controls=ids.map(id=>{const e=document.getElementById(id),r=e?.getBoundingClientRect();return {id,visible:!!e&&r.width>0&&r.height>0&&r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight&&e.contains(document.elementFromPoint(r.left+r.width/2,r.top+r.height/2))};});
                const cpu=document.getElementById('cpu'),original=cpu.textContent,readings=[];
                for(const n of [0,9,10,99,100,999,1000,9999]){cpu.textContent='DSP '+n+'%';const r=cpu.getBoundingClientRect();readings.push({n,left:r.left,width:r.width});}
                cpu.textContent=original;
                return {width:innerWidth,height:innerHeight,controls,readings,passed:controls.every(x=>x.visible)&&readings.every(r=>r.left===readings[0].left&&r.width===readings[0].width)};
                """) as? [String: Any] ?? [:]
                evidence["patternLayout\(Int(size.width))"] = layout; try write()
                try require(layout["passed"] as? Bool == true, "Pattern destinations are clipped or changing DSP digits shift the status row")
                let configuration = WKSnapshotConfiguration(); configuration.rect = web.bounds
                let image = try await web.takeSnapshot(configuration: configuration)
                if let data = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data), let png = bitmap.representation(using: .png, properties: [:]) {
                    try png.write(to: folder.appendingPathComponent("pattern-destinations-\(Int(size.width))x\(Int(size.height)).png"))
                }
            }
            checks.append("Pattern lane and play/entry controls fit both sizes; DSP changes from zero to four digits keep a fixed position")
            NativeUINKSAutomationFixture.register()
            let pluginDestination = try await js("""
            await ST.perform('stop');await ST.setNoteRecording(false);
            if(!await ST.projectAction('saveAs',fixtureSavePath))throw Error('Private fixture save before plug-in destination test failed');
            if(!await ST.projectAction('new'))throw Error('Native New session failed before plug-in destination test');
            await ST.loadSample(firstPath,0);await ST.loadSampleAsNew(secondPath);
            const before=JSON.parse(JSON.stringify(ST.state.project)),oldCount=before.instruments.length,oldTracks=before.tracks.length;
            ST.setView('browser');
            if(!await STNKS.loadInstrument('plugin.load',{component},{name:'Destination fixture',stay:true}))throw Error('Fixture load failed');
            const p=ST.state.project,track=ST.state.track,instrument=ST.state.instrument;
            if(track!==oldTracks||instrument!==oldCount||p.tracks.length!==oldTracks+1)throw Error('Plug-in did not select its new lane and instrument');
            if(JSON.stringify(p.instruments.slice(0,oldCount))!==JSON.stringify(before.instruments))throw Error('Loading plug-in replaced a sample');
            if(JSON.stringify(p.tracks.slice(0,oldTracks))!==JSON.stringify(before.tracks))throw Error('Loading plug-in changed a sample lane');
            const native=await ST.request('engine.snapshot');
            const hosted=native.audioPlugins.find(x=>x.track===track&&x.kind==='instrument');
            if(!hosted||hosted.component!==component)throw Error('Native plug-in was attached to a different lane');
            const note=await ST.request('note.on',{track,instrument,note:60,velocity:1});
            await ST.request('note.off',{track,instrument,note:60});
            ST.state.track=0;ST.state.instrument=instrument;
            if(!await ST.perform('assignInstrument'))throw Error('Reassigning fixture to another lane failed: '+document.getElementById('toast-region')?.textContent+' · '+JSON.stringify(ST.state.project.audioPlugins.map(x=>({track:x.track,owner:x.owner,instanceID:x.instanceID,component:x.component}))));
            const assigned=await ST.request('engine.snapshot');
            const target=assigned.audioPlugins.find(x=>x.track===0&&x.kind==='instrument');
            const original=assigned.audioPlugins.find(x=>x.track===track&&x.kind==='instrument');
            if(!target||!original||target.instanceID===original.instanceID||target.owner===original.owner)throw Error('Reassignment reused the old owner or instance');
            return {track,instrument,component,originalOwner:original.owner,targetOwner:target.owner,noteAcknowledgement:note};
            """, ["component": NativeUINKSAutomationFixture.identifier, "firstPath": paths[0].path, "secondPath": paths[1].path, "fixtureSavePath": scratch.appendingPathComponent("before-plugin-destination.supertracker").path])
            evidence["pluginDestination"] = pluginDestination
            checks.append("Two generated samples survive native fixture plug-in loading on a new playable lane; assignment creates an independent owner and instance")
            let removedKit = try await js("""
            await ST.perform('stop');
            if(!STKitGroups.createKit('Removal isolation kit'))throw Error('Removal fixture kit creation failed');
            const kitID=ST.state.project.groups.at(-1).id;
            ST.state.track=ST.state.project.tracks.findIndex(t=>!t.groupID);ST.state.instrument=ST.state.project.tracks[ST.state.track].instrument;
            if(!await ST.loadSampleAsNew(secondPath))throw Error('After-kit sample creation failed');
            if(!await STNKS.loadInstrument('plugin.load',{component},{name:'After-kit fixture',stay:true}))throw Error('After-kit plug-in load failed');
            if(!await ST.captureNativeEdits())throw Error('Removal fixture capture failed');
            const before=window.__supertracker.getProject(),kitIndex=before.tracks.findIndex(t=>t.groupID===kitID);
            const samples=before.instruments.map((inst,index)=>({inst,index})).filter(x=>x.inst.type==='sample'&&x.inst.samplePath);
            const afterKit=before.audioPlugins.find(x=>Number.isInteger(x.track)&&x.track>kitIndex);
            if(!afterKit)throw Error('Fixture needs an ordinary plug-in after the kit');
            if(!STKitGroups.removeKit(kitID))throw Error('Remove kit failed: '+document.getElementById('toast-region')?.textContent);
            if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('Removal publication failed');
            const p=ST.state.project,native=await ST.request('engine.snapshot');
            if(p.groups.some(g=>g.id===kitID)||(native.groups||[]).some(g=>g.id===kitID)||native.tracks.some(t=>t.groupID===kitID))throw Error('Removed kit survived in UI or native graph');
            for(const {inst,index} of samples){
              if(before.tracks.some(t=>t.groupID===kitID&&t.instrument===index))continue;
              if(JSON.stringify(p.instruments[index])!==JSON.stringify(inst))throw Error('Kit removal changed a standalone sample instrument');
              if(native.instruments[index].samplePath!==inst.samplePath)throw Error('Native sample destination changed');
              const info=await ST.request('sample.info',{instrument:index,path:inst.samplePath});
              if(!info.frames||info.path!==inst.samplePath)throw Error('Surviving sample PCM is unavailable');
            }
            const originalTrack=before.tracks[afterKit.track],newIndex=p.tracks.findIndex(t=>t.routingID===originalTrack.routingID);
            const hosted=native.audioPlugins.find(x=>x.instanceID===afterKit.instanceID);
            if(newIndex!==afterKit.track-1||!hosted||hosted.track!==newIndex||hosted.owner!==afterKit.owner)throw Error('Surviving plug-in was retargeted during kit removal');
            await ST.perform('undo');
            if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Removal Undo publication failed');
            const restored=await ST.request('engine.snapshot');
            if(!ST.state.project.groups.some(g=>g.id===kitID)||!(restored.groups||[]).some(g=>g.id===kitID))throw Error('Undo did not restore the kit');
            if(restored.audioPlugins.find(x=>x.instanceID===afterKit.instanceID)?.track!==afterKit.track)throw Error('Undo did not restore the surviving plug-in lane');
            return {kitIndex,samples:samples.map(x=>({index:x.index,path:x.inst.samplePath})),survivingPlugin:afterKit.instanceID,remappedTrack:newIndex,undoRestored:true};
            """, ["component": NativeUINKSAutomationFixture.identifier, "secondPath": paths[1].path])
            evidence["kitRemoval"] = removedKit
            checks.append("Actual native kit removal (group and pad tracks) preserves standalone sample PCM and unrelated plug-ins across track reindexing and Undo")
            for (index, url) in paths.enumerated() { try require(try Data(contentsOf: url) == originals[index], "Source sample bytes changed") }
            checks.append("Persistent instrument actions fit both window sizes with and without a kit; source sample bytes are unchanged")
            try await cleanup()
            evidence["passed"] = true; try write(); return checks
        } catch {
            evidence["error"] = error.localizedDescription
            evidence["failureState"] = try? await js("return {view:ST.state.view,modal:ST.state.modal,focus:document.activeElement?.outerHTML,toast:document.getElementById('toast-region')?.textContent,pending:Array.from(ST.state.pending.values()).map(x=>x.command),errors:window.__bridgeTestErrors,inputReplies:window.__browserDestinationInputReplies,recording:ST.state.recording,keyboardPlay:ST.state.keyboardPlay,lastInputNote:ST.state.lastInputNote,status:ST.state.status,project:window.__supertracker.getProject()};")
            do { try await cleanup() } catch { evidence["cleanupError"] = error.localizedDescription }
            try? write(); throw error
        }
    }
}

#endif

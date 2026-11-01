// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import WebKit

/// Actual mounted UI → Bridge → original in-process AU acceptance. No installed
/// vendor code, physical keys, file panels, microphone or MIDI output is used.
@MainActor
enum NativeUIDuplicateTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        NativeUINKSAutomationFixture.register()
        let files = FileManager.default
        let folder = output.appendingPathComponent("mounted-track-duplicate", isDirectory: true)
        try files.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = []
        var evidence: [String: Any] = ["scope": "actual mounted UI and Bridge with original in-process AU",
            "installedPluginsLoaded": false, "physicalKeyboardTested": false,
            "hardwareInputTested": false, "midiSent": false, "audioPlaybackTested": false,
            "nativeFilePanelsTested": false]
        func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(source, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("WEBKIT DUPLICATE: " + message) }
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
            let diagnostic = try? await js("return {tracks:ST.state.project.tracks.length,undo:ST.state.undo.length,busy:window.STTrackDuplicate?.isBusy,applying:window.STTrackDuplicate?.isApplying,pending:Array.from(ST.state.pending.values()).map(x=>x.command),toast:document.getElementById('toast-region')?.textContent};")
            throw TrackerError.message("WEBKIT DUPLICATE: \(message). \(String(describing: diagnostic))")
        }
        func report(_ passed: Bool, _ error: String? = nil) throws {
            evidence["passed"] = passed; evidence["checks"] = checks
            evidence["generatedAt"] = ISO8601DateFormatter().string(from: Date())
            if let error { evidence["error"] = error }
            try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys])
                .write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func cleanup() async throws {
            _ = try await js("""
                // A timed-out native publication is canceled by the enclosing
                // harness's shutdown; do not wait on that same gate in cleanup.
                if(window.STTrackDuplicate?.isBusy)return false;
                ST.closeModal();
                if(window.__duplicateAcceptance?.original){
                  ST.acceptProject(window.__duplicateAcceptance.original);
                  if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Duplicate fixture cleanup was superseded');
                }
                delete window.__duplicateAcceptance;
                return true;
                """)
        }
        try report(false)
        try await NativeUIDuplicateFixture.register()
        let sampleURL = scratch.appendingPathComponent("Duplicate shared sample.wav")
        let wave = (0..<4_800).map { Float(sin(Double($0) * 2 * Double.pi * 220 / 48_000) * 0.125) }
        try AudioSamples.write(DecodedSample(path: sampleURL.path, sampleRate: 48_000,
            left: wave, right: wave, originalChannels: 2), to: sampleURL)
        let sourceBytes = try Data(contentsOf: sampleURL)
        let savedURL = scratch.appendingPathComponent("Duplicated original instrument.supertracker")
        let address = String(NativeUINKSAutomationFixture.mappedAddress)
        let fixture: [String: Any] = ["version": 1, "name": "Mounted duplicate fixture",
            "bpm": 120, "linesPerBeat": 4, "masterVolume": 0, "activePattern": 0,
            "loopSong": false, "metronome": false,
            "tracks": [
                ["id": 0, "routingID": "duplicate-source", "name": "Original patch", "instrument": 0,
                 "volume": 0.7, "pan": -0.2, "delaySend": 0, "reverbSend": 0, "mute": false, "solo": false],
                ["id": 1, "routingID": "duplicate-shared", "name": "Shared sample", "instrument": 1,
                 "volume": 0.5, "pan": 0.1, "delaySend": 0, "reverbSend": 0, "mute": true, "solo": false]],
            "instruments": [
                ["id": 0, "name": "Original registered AU", "type": "au", "volume": 1, "pan": 0],
                ["id": 1, "name": "Shared private sample", "type": "sample", "samplePath": sampleURL.path,
                 "rootNote": 60, "volume": 0.6, "mode": "oneshot", "start": 0.1, "end": 0.9]],
            "patterns": [
                ["id": 0, "name": "AU phrase", "length": 4,
                 "steps": [[["note": 60, "instrument": 0, "velocity": 101, "gate": 3] as Any, NSNull(), NSNull(), NSNull()],
                           [NSNull(), NSNull(), NSNull(), NSNull()]], "automation": []],
                ["id": 1, "name": "Alternate shared sample", "length": 4,
                 "steps": [[["note": 67, "instrument": 1, "velocity": 84, "gate": 0.5] as Any, NSNull(), NSNull(), NSNull()],
                           [NSNull(), NSNull(), NSNull(), NSNull()]], "automation": []]],
            "sequence": [0], "audioPlugins": [],
            "audioClips": [["id": "duplicate-source-clip", "track": 0, "name": "Preserved private clip",
                "path": sampleURL.path, "startBeat": 0.5, "durationBeats": 0.1, "sourceOffset": 0.01,
                "gain": 0.4, "fadeIn": 0.005, "fadeOut": 0.006, "muted": true]],
            "routing": ["version": 1, "mixModel": "standard",
                "buses": [["id": "duplicate-group", "name": "Shared group", "type": "group", "volume": 0.8, "pan": 0]],
                "outputs": ["track:duplicate-source": "bus:duplicate-group", "track:duplicate-shared": "master", "bus:duplicate-group": "master"],
                "sends": [], "sidechains": []],
            "delay": ["mix": 0], "reverb": ["mix": 0]]
        do {
            _ = try await js("""
                window.__duplicateAcceptance={original:window.__supertracker.getProject(),address,effectAddress};
                __duplicateAcceptance.stable=value=>value===null||typeof value!=='object'?JSON.stringify(value):Array.isArray(value)?'['+value.map(__duplicateAcceptance.stable).join(',')+']':'{'+Object.keys(value).sort().map(key=>JSON.stringify(key)+':'+__duplicateAcceptance.stable(value[key])).join(',')+'}';
                await ST.request('transport.stop');ST.state.playing=false;
                ST.acceptProject(project);
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Duplicate fixture setup was superseded');
                const loaded=await ST.request('plugin.load',{track:0,kind:'instrument',slot:0,component});
                const effect=await ST.request('plugin.load',{track:0,kind:'effect',slot:6,component:effectComponent});
                ST.mutate('Install original duplication fixture',p=>{p.audioPlugins=[loaded,effect];p.instruments[0].plugin={...loaded};},{render:false});
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('Original AU setup did not settle');
                await ST.request('plugin.parameter',{track:0,kind:'instrument',slot:0,instanceID:loaded.instanceID,address,value:.625,normalized:true});
                await ST.request('plugin.parameter',{track:0,kind:'effect',slot:6,instanceID:effect.instanceID,address:effectAddress,value:.375,normalized:true});
                if(!await ST.captureNativeEdits())throw Error('Original AU state capture failed');
                const baselinePlugin=ST.state.project.audioPlugins.find(x=>x.kind==='instrument');
                ST.mutate('Prepare original phrase',p=>{
                  p.patterns[0].steps[0][1]={note:-3,instrument:0,velocity:100,fx1:{type:'plugin',instanceID:baselinePlugin.instanceID,address,value:.375,name:'Original level lock'},fx2:{type:'plugin',instanceID:effect.instanceID,address:effectAddress,value:.25,name:'Original effect lock'}};
                  p.patterns[0].automation=[{id:'duplicate-source-lane',owner:'track:duplicate-source',track:0,target:'plugin:instrument:0:'+address,name:'Original level lane',interpolation:'step',points:[{row:0,value:.625},{row:2,value:.5}]}];
                },{render:false});
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false||!await ST.captureNativeEdits())throw Error('Original phrase preparation failed');
                ST.state.undo=[];ST.state.redo=[];ST.state.track=0;ST.state.instrument=0;ST.state.row=0;ST.setView('pattern');
                window.__duplicateAcceptance.before=window.__supertracker.getProject();
                window.__duplicateAcceptance.sourceID=ST.state.project.audioPlugins.find(x=>x.kind==='instrument').instanceID;
                window.__duplicateAcceptance.sourceState=ST.state.project.audioPlugins.find(x=>x.kind==='instrument').state;
                window.__duplicateAcceptance.effectID=ST.state.project.audioPlugins.find(x=>x.kind==='effect').instanceID;
                window.__duplicateAcceptance.effectState=ST.state.project.audioPlugins.find(x=>x.kind==='effect').state;
                return true;
                """, ["project": fixture, "component": NativeUINKSAutomationFixture.identifier, "address": address, "effectComponent": NativeUIDuplicateFixture.identifier, "effectAddress": String(NativeUIDuplicateFixture.address)])
            try await check("typeof __duplicateAcceptance.sourceState==='string'&&__duplicateAcceptance.sourceState.length>0&&ST.state.undo.length===0", "Fixture did not capture an opaque native state with an empty Undo history")
            let effectEncoding = try await js("return __duplicateAcceptance.effectState;") as? String ?? ""
            guard let effectData = Data(base64Encoded: effectEncoding),
                  let effectState = try PropertyListSerialization.propertyList(from: effectData, format: nil) as? [String: Any],
                  let typed = effectState["typed"] as? [String: Any] else {
                throw TrackerError.message("WEBKIT DUPLICATE: Original effect state is not a typed opaque property list.")
            }
            try require(effectState["opaque"] as? Data == Data([0, 1, 255, 0, 42]) && typed["enabled"] as? Bool == true && typed["index"] as? Int == 7,
                        "Original effect did not expose the expected opaque bytes and typed metadata")
            evidence["typedEffectState"] = ["opaqueBytes": [0, 1, 255, 0, 42], "enabled": true, "index": 7, "stateBytes": effectData.count]
            _ = try await js("document.getElementById('track-duplicate').focus();document.getElementById('track-duplicate').click();return true;")
            try await wait("!STTrackDuplicate.isBusy&&ST.state.project.tracks.length===3&&ST.state.undo.length===1", "The actual Duplicate Track button did not commit exactly one edit")
            try await wait("ST.state.pending.size===0", "Duplicate native publication did not settle")
            evidence["duplicate"] = try await js("""
                const c=__duplicateAcceptance,p=ST.state.project,copy=p.audioPlugins.find(x=>x.track===2&&x.kind==='instrument');
                if(!copy||copy.instanceID===c.sourceID||copy.state!==c.sourceState)throw Error('Copied opaque state or independent instance identity differs');
                const copyEffect=p.audioPlugins.find(x=>x.track===2&&x.kind==='effect'&&x.slot===6);
                if(!copyEffect||copyEffect.instanceID===c.effectID||copyEffect.state!==c.effectState)throw Error('Copied sparse effect identity or typed opaque state differs');
                c.copyEffectID=copyEffect.instanceID;
                const before=c.before;
                for(let i=0;i<2;i++)if(__duplicateAcceptance.stable(p.tracks[i])!==__duplicateAcceptance.stable(before.tracks[i])||__duplicateAcceptance.stable(p.instruments[i])!==__duplicateAcceptance.stable(before.instruments[i]))throw Error('A source track or shared instrument changed');
                for(let i=0;i<p.patterns.length;i++){
                  if(__duplicateAcceptance.stable(p.patterns[i].steps.slice(0,2))!==__duplicateAcceptance.stable(before.patterns[i].steps))throw Error('Original pattern columns changed');
                  if(__duplicateAcceptance.stable(p.patterns[i].automation.slice(0,before.patterns[i].automation.length))!==__duplicateAcceptance.stable(before.patterns[i].automation))throw Error('Original lanes changed');
                }
                if(p.instruments.length!==4||p.tracks[2].instrument!==2||p.patterns[1].steps[2][0].instrument!==3||p.tracks[1].instrument!==1)throw Error('Independent sample/instrument ownership differs');
                if(p.patterns[0].steps[2][1].fx1.instanceID!==copy.instanceID||p.patterns[0].steps[2][1].fx1.address!==c.address||p.patterns[0].steps[2][1].fx1.value!==.375)throw Error('Copied step effect target/value differs');
                if(p.patterns[0].steps[2][1].fx2.instanceID!==copyEffect.instanceID||p.patterns[0].steps[2][1].fx2.address!==c.effectAddress||p.patterns[0].steps[2][1].fx2.value!==.25)throw Error('Copied sparse-effect step lock differs');
                const lane=p.patterns[0].automation.find(x=>x.track===2);
                if(!lane||lane.id===before.patterns[0].automation[0].id||lane.owner!=='track:'+p.tracks[2].routingID||lane.target!=='plugin:instrument:0:'+c.address||__duplicateAcceptance.stable(lane.points)!==__duplicateAcceptance.stable(before.patterns[0].automation[0].points))throw Error('Copied lane target, owner or points differ');
                if(p.routing.outputs['track:'+p.tracks[2].routingID]!=='bus:duplicate-group'||__duplicateAcceptance.stable(p.routing.buses)!==__duplicateAcceptance.stable(before.routing.buses))throw Error('Shared routing changed');
                if(p.audioClips.length!==2||p.audioClips[0].id===p.audioClips[1].id||p.audioClips[1].track!==2||p.audioClips[1].path!==p.audioClips[0].path)throw Error('Clip copy did not preserve its private source');
                if(__duplicateAcceptance.stable(ST.state.undo[0].project)!==__duplicateAcceptance.stable(before))throw Error('The one undo snapshot contains the proposal instead of the original');
                if(ST.state.track!==2||ST.state.instrument!==2||document.activeElement?.id!=='tracker')throw Error('Focus or selection did not reach the duplicate');
                c.copyID=copy.instanceID;c.duplicated=window.__supertracker.getProject();
                return {sourceInstanceID:c.sourceID,duplicateInstanceID:c.copyID,sourceEffectID:c.effectID,duplicateEffectID:c.copyEffectID,sparseEffectSlot:6,opaqueStateByteExact:true,tracks:p.tracks.length,instruments:p.instruments.length,undoEntries:ST.state.undo.length,step:p.patterns[0].steps[2][1],lane,output:p.routing.outputs['track:'+p.tracks[2].routingID],focus:document.activeElement.id};
                """)
            checks.append("Mounted Duplicate Track creates independent AU instrument/sparse-FX/sample identities, exact typed opaque state, retargeted step FX and lane, preserved source/routing/clip, and one undo edit")

            _ = try await js("ST.perform('undo');if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Undo synchronization failed');return true;")
            try await check("__duplicateAcceptance.stable(window.__supertracker.getProject())===__duplicateAcceptance.stable(__duplicateAcceptance.before)", "Actual Undo did not restore the complete original project")
            try await check("await(async()=>{try{await ST.request('plugin.parameters',{track:2,kind:'instrument',slot:0,instanceID:__duplicateAcceptance.copyID,addresses:[]});return false;}catch{return true;}})()", "Undo left the copied native instance loaded")
            _ = try await js("ST.perform('redo');if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Redo synchronization failed');return true;")
            try await check("__duplicateAcceptance.stable(window.__supertracker.getProject())===__duplicateAcceptance.stable(__duplicateAcceptance.duplicated)", "Actual Redo changed duplicate IDs, state or copied content")
            checks.append("Actual Undo unloads the copied native AU; Redo restores the same independent IDs and opaque project state")

            evidence["independentParameters"] = try await js("""
                const c=__duplicateAcceptance;
                const value=async(track,instanceID,kind='instrument',slot=0,address=c.address)=>{
                  const params=await ST.request('plugin.parameters',{track,kind,slot,instanceID,addresses:[address]});
                  if(params.length!==1||String(params[0].address)!==address||!Number.isFinite(Number(params[0].normalized)))throw Error('Exact live parameter is missing');
                  return Number(params[0].normalized);
                };
                if(Math.abs(await value(0,c.sourceID)-.625)>1e-6||Math.abs(await value(2,c.copyID)-.625)>1e-6)throw Error('Restored original/copy parameter differs from captured patch');
                if(Math.abs(await value(0,c.effectID,'effect',6,c.effectAddress)-.375)>1e-6||Math.abs(await value(2,c.copyEffectID,'effect',6,c.effectAddress)-.375)>1e-6)throw Error('Sparse original/copied effect parameter differs');
                await ST.request('plugin.parameter',{track:2,kind:'instrument',slot:0,instanceID:c.copyID,address:c.address,value:.75,normalized:true});
                await ST.request('plugin.parameter',{track:2,kind:'effect',slot:6,instanceID:c.copyEffectID,address:c.effectAddress,value:.875,normalized:true});
                const original=await value(0,c.sourceID),duplicate=await value(2,c.copyID);
                const originalEffect=await value(0,c.effectID,'effect',6,c.effectAddress),duplicateEffect=await value(2,c.copyEffectID,'effect',6,c.effectAddress);
                if(Math.abs(original-.625)>1e-6||Math.abs(duplicate-.75)>1e-6)throw Error('Editing copied AU changed the original instance');
                if(Math.abs(originalEffect-.375)>1e-6||Math.abs(duplicateEffect-.875)>1e-6)throw Error('Editing copied effect changed its source instance');
                if(!await ST.captureNativeEdits())throw Error('Independent edits did not capture');
                if(ST.state.project.audioPlugins.find(x=>x.instanceID===c.sourceID).state!==c.sourceState)throw Error('Original opaque state changed after editing duplicate');
                if(ST.state.project.audioPlugins.find(x=>x.instanceID===c.effectID).state!==c.effectState)throw Error('Original effect opaque state changed');
                c.editedState=ST.state.project.audioPlugins.find(x=>x.instanceID===c.copyID).state;
                c.editedEffectState=ST.state.project.audioPlugins.find(x=>x.instanceID===c.copyEffectID).state;
                if(c.editedState===c.sourceState)throw Error('Copied native edit did not change its captured opaque state');
                if(c.editedEffectState===c.effectState)throw Error('Copied effect edit was not captured');
                return {original,duplicate,originalEffect,duplicateEffect,originalOpaqueStateUnchanged:true};
                """)
            checks.append("Fresh native parameter queries prove all four instrument/effect instances exist and duplicate edits leave original parameters and opaque states unchanged")

            _ = try await js("""
                const saved=await ST.request('project.save',{project:window.__supertracker.getProject(),path});
                if(!saved.saved)throw Error('Private duplicate project save failed');
                ST.acceptProject(saved.project);if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Saved project did not settle');
                __duplicateAcceptance.saved=window.__supertracker.getProject();
                // Clear the actual rack before Open, proving that reopen does not
                // merely read values from instances left alive by duplication.
                ST.acceptProject({name:'Empty duplicate acceptance project',masterVolume:0});
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Rack clearing failed');
                ST.state.dirty=false;
                return true;
                """, ["path": savedURL.path])
            try await check("await(async()=>{try{await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0,instanceID:__duplicateAcceptance.sourceID,addresses:[]});return false;}catch{return true;}})()", "Pre-open empty fixture did not remove the original live AU")
            try require(try await js("return await ST.projectAction('open',path);", ["path": savedURL.path]) as? Bool == true, "Actual project Open did not restore the saved duplicate")
            _ = try await js("if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Reopened duplicate synchronization failed');return true;")
            evidence["reopened"] = try await js("""
                const c=__duplicateAcceptance,p=ST.state.project;
                if(__duplicateAcceptance.stable(p)!==__duplicateAcceptance.stable(c.saved))throw Error('Save/Open changed duplicate content or portable paths');
                const original=p.audioPlugins.find(x=>x.instanceID===c.sourceID),duplicate=p.audioPlugins.find(x=>x.instanceID===c.copyID);
                if(original?.state!==c.sourceState||duplicate?.state!==c.editedState)throw Error('Save/Open lost opaque patch bytes');
                if(p.audioPlugins.find(x=>x.instanceID===c.effectID)?.state!==c.effectState||p.audioPlugins.find(x=>x.instanceID===c.copyEffectID)?.state!==c.editedEffectState)throw Error('Save/Open lost opaque effect bytes');
                const values=[];
                for(const [track,instanceID,kind,slot,address,expected] of [[0,c.sourceID,'instrument',0,c.address,.625],[2,c.copyID,'instrument',0,c.address,.75],[0,c.effectID,'effect',6,c.effectAddress,.375],[2,c.copyEffectID,'effect',6,c.effectAddress,.875]]){
                  const params=await ST.request('plugin.parameters',{track,kind,slot,instanceID,addresses:[address]});
                  const value=Number(params[0]?.normalized);if(params.length!==1||String(params[0].address)!==address||!Number.isFinite(value)||Math.abs(value-expected)>1e-6)throw Error('Fresh reopened AU state differs');values.push(value);
                }
                return {values,independentInstanceIDs:[c.sourceID,c.copyID,c.effectID,c.copyEffectID],opaqueStatesByteExact:true,patternsPreserved:true,sourceAudioPath:p.instruments[1].samplePath,duplicateAudioPath:p.instruments[3].samplePath};
                """)
            try require(try Data(contentsOf: sampleURL) == sourceBytes, "Save/duplication modified the original sample bytes")
            let disk = try JSONSerialization.jsonObject(with: Data(contentsOf: savedURL)) as? [String: Any]
            let diskInstruments = disk?["instruments"] as? [[String: Any]] ?? []
            try require(diskInstruments.count == 4 && (diskInstruments[1]["samplePath"] as? String)?.hasPrefix("Duplicated original instrument.media/") == true,
                        "Private Save did not collect the source sample into portable media")
            evidence["sourceAudioUnchanged"] = true; evidence["portableMediaCollected"] = true
            checks.append("Private native Save, complete rack teardown and actual Open restore copied content, portable media and independently edited AU states")
            try await cleanup()
            try report(true)
        } catch {
            try? report(false, error.localizedDescription)
            try? await cleanup()
            throw error
        }
        return checks
    }
}

#endif

// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import WebKit

/// Mounted Song controls and real Bridge persistence using original generated WAVs.
/// DOM key events exercise app handlers; physical keys and audio playback are not asserted.
@MainActor
enum NativeUITakesTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        let files = FileManager.default
        let folder = output.appendingPathComponent("mounted-takes", isDirectory: true)
        let working = scratch.appendingPathComponent("Mounted takes", isDirectory: true)
        try files.createDirectory(at: folder, withIntermediateDirectories: true)
        try files.createDirectory(at: working, withIntermediateDirectories: true)
        var checks: [String] = []
        var evidence: [String: Any] = ["scope": "mounted Song take controls, actual Bridge metadata, engine publication and portable persistence",
            "keyboardMethod": "DOM KeyboardEvent through the actual mounted app.js and clip-arranger.js handlers",
            "physicalKeyboardTested": false, "nativeFilePanelsTested": false, "audioPlaybackTested": false,
            "hardwareInputTested": false, "midiSent": false, "installedPluginsLoaded": false]
        func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(source, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("WEBKIT TAKES: " + message) }
        }
        func report(_ passed: Bool, _ error: String? = nil) throws {
            evidence["passed"] = passed; evidence["checks"] = checks
            evidence["generatedAt"] = ISO8601DateFormatter().string(from: Date())
            if let error { evidence["error"] = error }
            try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys])
                .write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func cleanup() async throws {
            try require(try await js("""
                const c=window.__takesAcceptance;if(!c)return true;
                ST.closeModal();ST.cancelPendingEngineSync?.();
                ST.acceptProject(c.original);
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Take fixture cleanup synchronization was superseded');
                if(c.stable(window.__supertracker.getProject())!==c.stable(c.original))throw Error('Take fixture cleanup changed the original project');
                ST.state.undo=c.undo;ST.state.redo=c.redo;ST.state.dirty=c.dirty;
                ST.state.keyboardPlay=c.keyboardPlay;ST.state.clipSelected=c.clipSelected;
                ST.state.track=c.track;ST.state.instrument=c.instrument;ST.setView(c.view);
                delete window.__takesAcceptance;return true;
                """) as? Bool == true, "Original project cleanup did not complete")
            evidence["originalProjectRestored"] = true
        }
        func wave(_ frequency: Double) -> Data {
            var result = Data()
            func text(_ text: String) { result.append(contentsOf: text.utf8) }
            func u16(_ value: UInt16) { result.append(UInt8(value & 255)); result.append(UInt8(value >> 8)) }
            func u32(_ value: UInt32) { for shift in stride(from: 0, to: 32, by: 8) { result.append(UInt8((value >> shift) & 255)) } }
            let frames: UInt32 = 48_000
            text("RIFF"); u32(36 + frames * 2); text("WAVEfmt "); u32(16); u16(1); u16(1)
            u32(48_000); u32(96_000); u16(2); u16(16); text("data"); u32(frames * 2)
            for frame in 0..<frames { u16(UInt16(bitPattern: Int16(sin(Double(frame) * 2 * .pi * frequency / 48_000) * 4_000))) }
            return result
        }
        try report(false)
        let originalURL = working.appendingPathComponent("Original performance.wav")
        let alternateURL = working.appendingPathComponent("Alternate performance.wav")
        let savedURL = working.appendingPathComponent("Comp.supertracker")
        let originalBytes = wave(220), alternateBytes = wave(330)
        try originalBytes.write(to: originalURL); try alternateBytes.write(to: alternateURL)
        let fixture: [String: Any] = ["version": 1, "name": "Mounted take fixture", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 0, "activePattern": 0, "loopSong": false, "metronome": false,
            "tracks": [["id": 0, "name": "Take region", "instrument": 0, "volume": 0, "pan": 0]],
            "instruments": [["id": 0, "name": "Silent fixture", "type": "sample", "volume": 0]],
            "patterns": [["id": 0, "name": "Empty", "length": 4, "steps": [[NSNull(), NSNull(), NSNull(), NSNull()]], "automation": []]],
            "sequence": [0], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0],
            "audioClips": [["id": "mounted-take", "track": 0, "name": "Original performance", "path": originalURL.path,
                "startBeat": 0, "durationBeats": 1, "sourceOffset": 0.1, "gain": 0.7, "fadeIn": 0.01, "fadeOut": 0.02, "muted": false]]]
        do {
            evidence["import"] = try await js("""
                const stable=value=>value===null||typeof value!=='object'?JSON.stringify(value):Array.isArray(value)?'['+value.map(stable).join(',')+']':'{'+Object.keys(value).sort().map(key=>JSON.stringify(key)+':'+stable(value[key])).join(',')+'}';
                const c=window.__takesAcceptance={original:window.__supertracker.getProject(),undo:ST.state.undo,redo:ST.state.redo,dirty:ST.state.dirty,
                  keyboardPlay:ST.state.keyboardPlay,clipSelected:ST.state.clipSelected,track:ST.state.track,instrument:ST.state.instrument,view:ST.state.view,stable,originalPath,alternatePath};
                c.near=(a,b)=>Math.abs(a-b)<1e-7;
                c.selected=()=>ST.state.project.audioClips.find(x=>x.id===ST.state.clipSelected);
                c.sync=async()=>{if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Take fixture publication was superseded');};
                c.key=key=>{const node=document.getElementById('clip-'+ST.state.clipSelected);if(!node)throw Error('Selected clip is not mounted');node.focus();const event=new KeyboardEvent('keydown',{key,bubbles:true,cancelable:true});node.dispatchEvent(event);if(!event.defaultPrevented)throw Error('Mounted clip did not handle '+key);};
                c.assertCanonical=clip=>{const take=clip.takeSet.takes.find(x=>x.id===clip.takeSet.activeID);if(!take||clip.path!==take.path||!c.near(clip.sourceOffset,take.sourceOffset))throw Error('Selected take and playback fields diverged');};
                await ST.request('transport.stop');ST.state.playing=false;ST.closeModal();ST.acceptProject(project);ST.state.keyboardPlay=false;
                ST.state.clipSelected='mounted-take';ST.setView('song');await c.sync();
                if(!document.getElementById('clips-take-import')||!window.STClipTakes)throw Error('Take controls are not mounted');
                await STClipTakes.addFile(alternatePath,STClipTakes.target(),'Alternate performance');await c.sync();
                const clip=c.selected();c.originalID=clip.takeSet.takes[0].id;c.alternateID=clip.takeSet.takes[1].id;
                if(ST.state.project.audioClips.length!==1||clip.takeSet.takes.length!==2||clip.path!==alternatePath||clip.sourceOffset!==0||ST.state.undo.length!==1)throw Error('Import did not add one alternate in one edit');
                if(clip.takeSet.takes[0].path!==originalPath||!c.near(clip.takeSet.takes[0].sourceOffset,.1)||!clip.takeSet.takes.every(t=>c.near(t.sourceDuration,1)))throw Error('Bridge metadata or original source alignment was lost');
                c.assertCanonical(clip);
                return {takeCount:clip.takeSet.takes.length,sourceDurations:clip.takeSet.takes.map(t=>t.sourceDuration),undoEntries:ST.state.undo.length,originalSourceOffset:clip.takeSet.takes[0].sourceOffset};
                """, ["project": fixture, "originalPath": originalURL.path, "alternatePath": alternateURL.path])
            checks.append("Mounted take import uses actual Bridge WAV metadata, retains the original offset and duration, and commits one ordinary edit")

            evidence["keyboardAndSplit"] = try await js("""
                const c=__takesAcceptance;
                c.key('PageUp');let clip=c.selected();c.assertCanonical(clip);
                if(clip.path!==c.originalPath||!c.near(clip.sourceOffset,.1))throw Error('Page Up did not select the original recording');
                c.key('PageDown');clip=c.selected();c.assertCanonical(clip);
                if(clip.path!==c.alternatePath||clip.sourceOffset!==0||document.getElementById('clips-active-take').value!==c.alternateID)throw Error('Page Down did not restore the alternate and inspector');
                const cursor=document.getElementById('clips-cursor');cursor.value='.5';cursor.dispatchEvent(new Event('change',{bubbles:true}));
                c.beforeSplit=window.__supertracker.getProject();c.key('s');await c.sync();
                const parts=ST.state.project.audioClips;if(parts.length!==2)throw Error('S did not split the selected region');
                const [left,right]=parts;c.rightID=right.id;c.leftBeforeManager=JSON.parse(JSON.stringify(left));
                if(left.id===right.id||right.id!==ST.state.clipSelected||!c.near(left.durationBeats,.5)||!c.near(right.startBeat,.5)||!c.near(right.durationBeats,.5))throw Error('Split boundaries or selection differ');
                for(const item of parts){if(item.takeSet.takes.length!==2)throw Error('Split discarded alternatives');c.assertCanonical(item);}
                for(let i=0;i<2;i++)if(left.takeSet.takes[i].id!==right.takeSet.takes[i].id||left.takeSet.takes[i].path!==right.takeSet.takes[i].path||!c.near(right.takeSet.takes[i].sourceOffset,left.takeSet.takes[i].sourceOffset+.25))throw Error('Split did not advance each source offset by elapsed seconds');
                if(!c.near(right.sourceOffset,.25)||left.fadeOut!==0||right.fadeIn!==0)throw Error('Split selected offset or boundary fades differ');
                return {leftID:left.id,rightID:right.id,leftOffsets:left.takeSet.takes.map(t=>t.sourceOffset),rightOffsets:right.takeSet.takes.map(t=>t.sourceOffset),physicalKeysTested:false};
                """)
            checks.append("Mounted Page Up and Page Down select canonical take paths; cursor field and S split both alternatives with independent offsets and correct boundaries")

            evidence["manager"] = try await js("""
                const c=__takesAcceptance;c.key('t');
                if(ST.state.modal!=='clip-takes'||!document.getElementById('takes-list'))throw Error('T did not open the take manager');
                const search=document.getElementById('takes-search');search.value='Original performance';search.dispatchEvent(new Event('input',{bubbles:true}));
                let list=document.getElementById('takes-list');if(list.options.length!==1||list.value!==c.originalID)throw Error('Take filter did not select the original');
                document.getElementById('takes-name').value='Comp preferred';document.getElementById('takes-rename').click();
                if(c.selected().takeSet.takes.find(t=>t.id===c.originalID).name!=='Comp preferred')throw Error('Manager rename did not persist');
                search.value='';search.dispatchEvent(new Event('input',{bubbles:true}));list=document.getElementById('takes-list');list.value=c.originalID;list.dispatchEvent(new Event('change',{bubbles:true}));
                document.getElementById('takes-offset').value='.4';document.getElementById('takes-align').click();
                const aligned=c.selected().takeSet.takes.find(t=>t.id===c.originalID);
                if(!c.near(aligned.sourceOffset,.4)||!c.near(c.selected().sourceOffset,.25))throw Error('Inactive alignment changed playback or did not persist');
                c.beforeUse=window.__supertracker.getProject();document.getElementById('takes-use').click();await c.sync();
                const selected=c.selected();c.assertCanonical(selected);
                if(ST.state.modal||selected.path!==c.originalPath||!c.near(selected.sourceOffset,.4)||selected.takeSet.activeID!==c.originalID)throw Error('Use selected take did not choose the aligned original');
                if(c.stable(ST.state.project.audioClips[0])!==c.stable(c.leftBeforeManager))throw Error('Manager edits leaked into the other split region');
                if(document.activeElement?.id!=='clip-'+c.rightID)throw Error('Manager did not return focus to the comp region');
                c.comp=window.__supertracker.getProject();return {name:aligned.name,selectedOffset:selected.sourceOffset,leftUnchanged:true,takeCount:selected.takeSet.takes.length};
                """)
            checks.append("T opens the mounted manager; filtering, naming, independent alignment and Use selected take preserve the neighboring comp region and return clip focus")

            evidence["undoRedo"] = try await js("""
                const c=__takesAcceptance;ST.perform('undo');await c.sync();
                if(c.stable(window.__supertracker.getProject())!==c.stable(c.beforeUse))throw Error('Actual Undo did not restore the previous take selection');
                ST.perform('redo');await c.sync();
                if(c.stable(window.__supertracker.getProject())!==c.stable(c.comp))throw Error('Actual Redo did not restore the complete comp');
                const native=await ST.request('engine.snapshot');
                if(c.stable(native.audioClips)!==c.stable(c.comp.audioClips))throw Error('Native engine publication differs from the selected comp');
                return {exactProjectUndoRedo:true,nativeSelectedPaths:native.audioClips.map(x=>x.path),nativeSelectedOffsets:native.audioClips.map(x=>x.sourceOffset),audioPlaybackTested:false};
                """)
            checks.append("Actual app Undo and Redo restore complete comp state and the real native engine snapshot receives the selected paths and offsets")

            _ = try await js("""
                const c=__takesAcceptance,saved=await ST.request('project.save',{project:window.__supertracker.getProject(),path});
                if(!saved.saved)throw Error('Explicit private comp save failed');
                ST.acceptProject(saved.project);await c.sync();c.saved=window.__supertracker.getProject();
                ST.acceptProject({name:'Empty takes acceptance',masterVolume:0});await c.sync();ST.state.dirty=false;return true;
                """, ["path": savedURL.path])
            try require(try await js("return await ST.projectAction('open',path);", ["path": savedURL.path]) as? Bool == true, "Actual Open did not load the private saved comp")
            evidence["reopened"] = try await js("""
                const c=__takesAcceptance;await c.sync();
                if(c.stable(window.__supertracker.getProject())!==c.stable(c.saved))throw Error('Actual Save/Open changed take data or collected paths');
                const clips=ST.state.project.audioClips;if(clips.length!==2||clips.some(x=>x.takeSet.takes.length!==2))throw Error('Open lost split alternatives');
                clips.forEach(c.assertCanonical);const native=await ST.request('engine.snapshot');
                if(c.stable(native.audioClips)!==c.stable(clips))throw Error('Reopened native clip state differs');
                return {clipCount:clips.length,takeCounts:clips.map(x=>x.takeSet.takes.length),selectedOffsets:clips.map(x=>x.sourceOffset),sourcePaths:clips.flatMap(x=>x.takeSet.takes.map(t=>t.path))};
                """)
            let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: savedURL)) as! [String: Any]
            let diskClips = raw["audioClips"] as! [[String: Any]]
            let storedPaths = diskClips.flatMap { (($0["takeSet"] as! [String: Any])["takes"] as! [[String: Any]]).map { $0["path"] as! String } }
            try require(Set(storedPaths).count == 2 && storedPaths.allSatisfy { $0.hasPrefix("Comp.media/") }, "Save did not collect both alternatives once using relative paths")
            for path in Set(storedPaths) {
                let collected = try Data(contentsOf: working.appendingPathComponent(path))
                try require(collected == (path.hasSuffix(originalURL.lastPathComponent) ? originalBytes : alternateBytes), "Collected take WAV bytes changed")
            }
            try require(try files.contentsOfDirectory(atPath: working.appendingPathComponent("Comp.media").path).count == 2, "Portable save duplicated shared split-region audio")
            try require(try Data(contentsOf: originalURL) == originalBytes && Data(contentsOf: alternateURL) == alternateBytes, "Take edits changed original WAV bytes")
            evidence["sourceAudioUnchanged"] = true; evidence["portableMediaFiles"] = 2
            checks.append("Explicit private native Save and actual Open retain both split take groups, deduplicate portable media and preserve original and collected WAV bytes")
            try await cleanup(); try report(true)
        } catch {
            let failure = error
            do { try await cleanup() }
            catch {
                let combined = "\(failure.localizedDescription) Cleanup also failed: \(error.localizedDescription)"
                try? report(false, combined); throw TrackerError.message(combined)
            }
            try? report(false, failure.localizedDescription); throw failure
        }
        return checks
    }
}

#endif

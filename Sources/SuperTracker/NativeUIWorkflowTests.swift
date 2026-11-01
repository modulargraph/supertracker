#if SELFTESTS
import AppKit
import WebKit

/// A continuous session built with mounted production controls. Generated audio,
/// synthetic DOM keys and path-directed Save/Open replace only external fixtures;
/// the project is never injected or modified by this acceptance helper.
@MainActor enum NativeUIWorkflowTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        let files = FileManager.default, folder = output.appendingPathComponent("mounted-workflow")
        let media = scratch.appendingPathComponent("Workflow samples")
        for directory in [folder, media] { try files.createDirectory(at: directory, withIntermediateDirectories: true) }
        let save = scratch.appendingPathComponent("Workflow session.stproj")
        let sources = [media.appendingPathComponent("Workflow_A.wav"), media.appendingPathComponent("Workflow_B.wav")]
        for (index, path) in sources.enumerated() {
            let pcm = (0..<48_000).map { n -> Float in
                let edge = min(1.0, min(Double(n) / 240, Double(47_999 - n) / 240))
                return Float(sin(Double(n) * 2 * .pi * (index == 0 ? 220 : 330) / 48_000) * 0.14 * edge)
            }
            try AudioSamples.write(DecodedSample(path: path.path, sampleRate: 48000, left: pcm, right: pcm, originalChannels: 2), to: path)
        }
        let originals = try sources.map { try Data(contentsOf: $0) }
        var phase = "boot", checks: [String] = []
        var evidence: [String: Any] = ["passed": false, "physicalKeyboardTested": false, "syntheticDOMKeyboard": true,
            "nativeFilePanelsTested": false, "projectFixtureInjected": false, "installedPluginsLoaded": false,
            "audioEvidence": "Native meters and decoded offline PCM; no listening or physical-device claim"]
        func js(_ body: String, _ args: [String: Any] = [:]) async throws -> Any {
            do { return try await web.callAsyncJavaScript("try {\n" + body + "\n} catch(e) { throw Error(String(e.message||e)+'\\n'+String(e.stack||'')); }", arguments: args, in: nil, contentWorld: .page) ?? NSNull() }
            catch { throw TrackerError.message("WORKFLOW [\(phase)]: \((error as NSError).userInfo)") }
        }
        func require(_ value: Bool, _ message: String) throws { if !value { throw TrackerError.message("WORKFLOW [\(phase)]: " + message) } }
        func check(_ expression: String, _ message: String) async throws { try require(try await js("return Boolean(\(expression));") as? Bool == true, message) }
        func wait(_ expression: String, seconds: Double = 30) async throws {
            let limit = Date().addingTimeInterval(seconds)
            while Date() < limit {
                if try await js("return Boolean(\(expression));") as? Bool == true { return }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            throw TrackerError.message("WORKFLOW [\(phase)] timed out: " + expression)
        }
        func click(_ selector: String) async throws {
            _ = try await js("const e=document.querySelector(selector);if(!e||e.disabled)throw Error('Unavailable control '+selector);e.scrollIntoView({block:'nearest'});if(e.closest('[hidden]')||!e.getBoundingClientRect().width||!e.getBoundingClientRect().height)throw Error('Hidden control '+selector);e.click();return true;", ["selector": selector])
        }
        func field(_ selector: String, _ value: String) async throws {
            _ = try await js("const e=document.querySelector(selector);if(!e||e.disabled)throw Error('Unavailable field '+selector);e.scrollIntoView({block:'nearest'});if(e.closest('[hidden]')||!e.getBoundingClientRect().width||!e.getBoundingClientRect().height)throw Error('Hidden field '+selector);e.focus();e.value=value;e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));e.blur();return true;", ["selector": selector, "value": value])
        }
        func key(_ value: String, code: String, meta: Bool = false, shift: Bool = false, down: Bool = true, up: Bool = true) async throws {
            _ = try await js("const target=document.activeElement||document.body,options={key:value,code,metaKey:meta,shiftKey:shift,bubbles:true,cancelable:true};if(down)target.dispatchEvent(new KeyboardEvent('keydown',options));if(down&&up&&!meta)await new Promise(resolve=>setTimeout(resolve,80));if(up)(document.activeElement||document.body).dispatchEvent(new KeyboardEvent('keyup',options));return true;", ["value": value, "code": code, "meta": meta, "shift": shift, "down": down, "up": up])
        }
        func settle() async throws {
            // Await the normal debounced publication; do not publish a replacement
            // project to make a broken UI action appear successful.
            try await Task.sleep(nanoseconds: 150_000_000)
            _ = try await js("await ST.waitForEngineUpdate();return true;")
            try await wait("ST.state.pending.size===0&&!STKitGroups.isBusy&&!STInstrumentEdits.isBusy")
        }
        func picture(_ name: String) async throws {
            let config = WKSnapshotConfiguration(); config.rect = web.bounds
            let image = try await web.takeSnapshot(configuration: config)
            if let data = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data), let png = bitmap.representation(using: .png, properties: [:]) { try png.write(to: folder.appendingPathComponent(name + ".png")) }
        }
        func report(_ passed: Bool, error: String? = nil) throws {
            evidence["passed"] = passed; evidence["phase"] = phase; evidence["checks"] = checks
            if let error { evidence["error"] = error }
            try JSONSerialization.data(withJSONObject: evidence, options: [.sortedKeys, .prettyPrinted]).write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func cell(_ track: Int, _ row: Int) async throws { try await click("td[data-cell='\(track):\(row)']") }
        func chooseSample(_ path: URL) async throws {
            try await wait("document.getElementById('browser-results-list')?.getAttribute('aria-busy')==='false'&&!document.getElementById('browser-load')?.disabled")
            let selector = try await js("const i=ST.state.library.items.findIndex(x=>x.path===path);if(i<0)throw Error('Generated sample missing');return '#browser-result-'+i;", ["path": path.path]) as? String ?? ""
            try await click(selector); try await click("#browser-load"); try await settle()
        }
        do {
            try report(false)
            phase = "New session from mounted keyboard command"
            try await key("n", code: "KeyN", meta: true)
            try await wait("ST.state.modal==='unsaved'||ST.state.project.tracks.length===1")
            if try await js("return !!document.getElementById('unsaved-discard');") as? Bool == true { try await click("#unsaved-discard") }
            try await wait("!ST.isChangingSession&&ST.state.project.tracks.length===1&&ST.state.project.patterns.length===1&&ST.state.project.patterns[0].steps[0].every(x=>!x)")
            try await settle(); try await picture("01-blank")
            checks.append("Cmd-N and Discard create one genuinely empty tracker lane through the production session flow")

            phase = "Browser import of two independent samples"
            _ = try await js("await ST.request('library.scan',{roots:[root]});return true;", ["root": media.path])
            try await click("#view-browser"); try await click("#browser-tab-samples")
            try await field("#browser-search", "Workflow_")
            _ = try await js("const e=document.getElementById('browser-audition');if(e.checked)e.click();return true;")
            try await wait("!ST.state.library.scanning&&ST.state.library.items.length===2")
            for path in sources { try await chooseSample(path) }
            try await check("ST.state.project.tracks.length===3&&ST.state.project.instruments.length===3&&ST.state.project.instruments[1].samplePath!==ST.state.project.instruments[2].samplePath", "Browser Add did not create independent playable destinations")
            evidence["sampleMetadata"] = try await js("return await Promise.all(ST.state.project.instruments.slice(1).map(i=>ST.request('sample.info',{instrument:i.id,path:i.samplePath}))); ")
            try await picture("02-browser-samples")
            checks.append("Actual browser search, result selection and two Add buttons create independent samples and native PCM metadata")

            phase = "Sampler control and first recorded pattern"
            try await click("#view-instrument"); try await click("#inst-page-tab-sample"); try await field("#param-value-volume", "0.35")
            try await settle(); try await check("Math.abs(ST.state.project.instruments[2].volume-.35)<1e-6", "Sampler volume field did not persist")
            try await picture("03-sampler-edit")
            try await click("#view-pattern"); try await field("#pattern-length", "16"); try await field("#edit-step", "0"); try await settle()
            try await click("#record"); try await wait("ST.state.recording&&ST.state.keyboardPlay")
            try await cell(1, 0); try await key("z", code: "KeyZ")
            try await wait("ST.pattern().steps[1][0]?.note>=0")
            try await cell(2, 4)
            if try await js("return document.getElementById('perf-toggle')?.getAttribute('aria-expanded')==='false';") as? Bool == true { try await click("#perf-toggle") }
            try await field("#chord", "major"); try await key("z", code: "KeyZ")
            try await wait("ST.pattern().steps[1][0]?.note>=0&&ST.pattern().steps[2][4]?.chord?.length===3")
            try await field("#chord", "single")
            try await cell(1, 8)
            // A quick tap and immediate navigation share one browser turn, so a
            // native acknowledgment cannot arrive before the target is changed.
            _ = try await js("const t=document.getElementById('tracker'),o={key:'x',code:'KeyX',bubbles:true,cancelable:true};t.dispatchEvent(new KeyboardEvent('keydown',o));(document.activeElement||document.body).dispatchEvent(new KeyboardEvent('keyup',o));document.querySelector('td[data-cell=\"2:9\"]').click();return true;")
            try await wait("ST.pattern().steps[1][8]?.note>=0")
            try await check("ST.state.track===2&&ST.state.row===9&&!ST.pattern().steps[2][9]", "Quick tap acknowledgment redirected the note or stole the new selection")
            checks.append("A zero-delay synthetic tap followed immediately by lane navigation records its original target and preserves the new cursor")
            try await click("#record")
            try await cell(1, 0); try await click("#step-fx1"); try await field("#step-fx-search", "Ratchet")
            try await click("[data-fx-key='ratchet']"); try await field("#step-fx-value", "2"); try await click("#step-fx-apply")
            try await wait("!ST.state.modal"); try await settle()
            try await click("#pattern-duplicate"); try await settle()
            try await check("ST.state.project.patterns.length===2&&ST.pattern().steps[1][0].fx1.type==='ratchet'&&ST.pattern().steps[2][4].chord.length===3", "Pattern duplication lost recorded notes, chord or chosen FX")
            try await picture("04-recorded-patterns")
            checks.append("Armed synthetic piano keys record notes/chord; actual FX picker and Duplicate retain them in two patterns")

            phase = "Create a kit group and load two pads through the browser"
            _ = try await js("if(!STKitGroups.createKit('Workflow Drums'))throw Error('Kit was not created');if(await ST.perform('addGroupTrack')===false)throw Error('Second pad was not added');return true;")
            try await settle()
            try await wait("ST.state.project.groups?.length===1&&STGroups.members(ST.state.project,ST.state.project.groups[0].id).length===2")
            let pads = (try await js("return STGroups.members(ST.state.project,ST.state.project.groups[0].id);") as? [NSNumber] ?? []).map(\.intValue)
            try require(pads.count == 2, "Expected two pad tracks in the kit group")
            for (index, pad) in pads.enumerated() {
                try await click("#view-pattern"); try await cell(pad, 0)
                try await click("#view-browser"); try await field("#browser-search", "Workflow_"); try await wait("ST.state.library.items.length===2")
                try await wait("document.getElementById('browser-results-list')?.getAttribute('aria-busy')==='false'&&!document.getElementById('browser-load-new')?.disabled")
                let selector = try await js("const i=ST.state.library.items.findIndex(x=>x.path===path);if(i<0)throw Error('Generated sample missing');return '#browser-result-'+i;", ["path": sources[index].path]) as? String ?? ""
                try await click(selector); try await click("#browser-load-new"); try await settle()
            }
            try await check("(()=>{const p=ST.state.project,m=STGroups.members(p,p.groups[0].id).map(i=>p.instruments[p.tracks[i].instrument]);return m.every(x=>!!x.samplePath)&&m[0].samplePath!==m[1].samplePath&&m.every(x=>x!==p.instruments[1]&&x!==p.instruments[2]);})()", "Browser did not load one sample into each pad track")
            try await click("#view-pattern"); try await field("#edit-step", "0")
            if try await js("return !ST.state.padKeys;") as? Bool == true { try await click("#pad-keys") }
            try await wait("ST.state.padKeys===true")
            if try await js("return !ST.state.recording;") as? Bool == true { try await click("#record") }
            try await wait("ST.state.recording&&ST.state.keyboardPlay")
            try await cell(pads[0], 0)
            for (keyName, code) in [("z", "KeyZ"), ("s", "KeyS")] { try await key(keyName, code: code); try await settle() }
            try await check("ST.pattern().steps[\(pads[0])][0]?.note>=0&&ST.pattern().steps[\(pads[1])][0]?.note>=0", "Pad keys did not record a hit on each pad track")
            if try await js("return !!ST.state.padKeys;") as? Bool == true { try await click("#pad-keys") }
            try await click("#record"); try await settle(); try await picture("05-kit-pad-hits")
            checks.append("New kit group plus Add track to group; browser Replace loads a sample per pad; Pad keys record a hit on each pad track")

            phase = "Per-pad effects and collapsed group views"
            try await cell(pads[0], 0)
            try await click("#step-fx1"); try await field("#step-fx-search", "Reverse")
            try await click("[data-fx-key='reverse']"); try await field("#step-fx-value", "1"); try await click("#step-fx-apply")
            try await wait("!ST.state.modal"); try await settle()
            try await check("ST.pattern().steps[\(pads[0])][0]?.fx1?.type==='reverse'&&!ST.pattern().steps[\(pads[1])][0]?.fx1", "Pad FX changed another pad's hit")
            _ = try await js("window.__musical=p=>JSON.stringify(p,(k,v)=>['viewState','collapsed','mixerCollapsed'].includes(k)?undefined:v);window.__workflowViews=window.__musical(window.__supertracker.getProject());return true;")
            let originalSize = web.frame.size
            let groupID = try await js("return ST.state.project.groups[0].id;") as? String ?? ""
            for width in [1500, 1100] {
                web.setFrameSize(NSSize(width: width, height: 900))
                for collapsed in [true, false] {
                    if try await js("return ST.state.project.groups[0].collapsed!==c;", ["c": collapsed]) as? Bool == true {
                        try await click("[data-group-collapse='\(groupID)']")
                    }
                    try await wait("ST.state.project.groups[0].collapsed===\(collapsed)")
                    try await Task.sleep(nanoseconds: 100_000_000)
                    try await picture("05-kit-\(collapsed ? "collapsed" : "expanded")-\(width)")
                    try await check("window.__musical(window.__supertracker.getProject())===window.__workflowViews", "Collapsing the group changed musical data")
                }
            }
            web.setFrameSize(originalSize)
            checks.append("Collapsed and expanded group views at 1500/1100 pixels preserve project data; Reverse targets only its own pad track")

            phase = "Keys toggle on a pad then live track addition"
            _ = try await js("window.__workflowBeforeKeys=JSON.stringify(window.__supertracker.getProject());window.__workflowTrack=ST.state.track;return true;")
            try await click("#keyboard-play"); try await click("#keyboard-play")
            try await check("window.__workflowBeforeKeys===JSON.stringify(window.__supertracker.getProject())&&ST.state.track===window.__workflowTrack", "Keys toggle changed the selected pad or project")
            if try await js("return !ST.state.keyboardPlay;") as? Bool == true { try await click("#keyboard-play") }
            try await key("z", code: "KeyZ", up: false)
            try await wait("(ST.state.status.trackPeaks?.[\(pads[0])]||0)>.001")
            try await key("z", code: "KeyZ", down: false)
            try await field("#transport-mode", "pattern"); try await cell(pads[1], 0); try await click("#play")
            try await wait("ST.state.status.playing&&ST.state.status.running&&(ST.state.status.trackPeaks||[]).some(x=>x>.001)")
            _ = try await js("window.__workflowBeforeLive=JSON.stringify(ST.pattern().steps);return true;")
            try await field("#pattern-record-placement", "live")
            try await click("#record"); try await wait("ST.state.recording")
            try await cell(pads[1], 0)
            try await key("x", code: "KeyX", up: false)
            try await Task.sleep(nanoseconds: 180_000_000)
            try await key("x", code: "KeyX", down: false)
            try await wait("JSON.stringify(ST.pattern().steps)!==window.__workflowBeforeLive")
            try await click("#record"); try await settle()
            checks.append("A pad track audibly responds after Keys toggle; a held synthetic key records a live note during advancing playback")
            evidence["beforeAdd"] = try await js("return JSON.parse(JSON.stringify(ST.state.status));")
            _ = try await js("window.__workflowTrackCount=ST.state.project.tracks.length;window.__workflowKit=JSON.stringify(ST.state.project.groups);return true;")
            try await cell(0, 0)
            try await click("#track-add"); try await settle()
            _ = try await js("window.__workflowPosition=ST.state.status.position;return true;")
            try await wait("ST.state.status.playing&&ST.state.status.running&&ST.state.status.position!==window.__workflowPosition&&(ST.state.status.trackPeaks||[]).some(x=>x>.001)")
            try await check("ST.state.project.tracks.length===window.__workflowTrackCount+1&&JSON.stringify(ST.state.project.groups)===window.__workflowKit", "Live Add track changed the kit group")
            evidence["afterAdd"] = try await js("return JSON.parse(JSON.stringify(ST.state.status));")
            try await picture("06-live-add-track"); try await click("#stop"); try await wait("!ST.state.playing&&!ST.state.status.playing")
            _ = try await js("window.__workflowAfterAdd=JSON.stringify(window.__supertracker.getProject());return true;")
            try await key("z", code: "KeyZ", meta: true); try await settle()
            try await check("ST.state.project.tracks.length===window.__workflowTrackCount", "Undo did not remove the just-added track")
            try await key("z", code: "KeyZ", meta: true, shift: true); try await settle()
            try await check("JSON.stringify(window.__supertracker.getProject())===window.__workflowAfterAdd", "Redo did not exactly restore live-added track")
            checks.append("Keys on/off preserves the selected pad; live Add track preserves advancing transport and the kit group; Undo/Redo exactly restores it")

            phase = "Native PCM renders and save/reopen"
            var renders: [[String: Any]] = []
            for pattern in 0..<2 {
                let destination = folder.appendingPathComponent("pattern-\(pattern).wav")
                _ = try await js("return await ST.request('audio.export',{mode:'pattern',pattern,path,tail:.2,encoding:'float32'});", ["pattern": pattern, "path": destination.path])
                let decoded = try AudioSamples.decode(destination.path)
                let energy = decoded.left.reduce(0.0) { $0 + Double($1 * $1) } / Double(max(1, decoded.left.count))
                try require(energy > 0.000001 && decoded.left.allSatisfy { $0.isFinite }, "Pattern \(pattern) rendered silent or invalid PCM")
                renders.append(["pattern": pattern, "frames": decoded.left.count, "meanSquare": energy, "path": destination.path])
            }
            evidence["renders"] = renders
            // Path-directed production Save/Open avoid interacting with a user's
            // file panels; serialization and rehydration still use the real Bridge.
            // Built-in synths migrate to wavetable samplers on reopen (ProjectMigration); they are left out of this comparison.
            _ = try await js("if(!await ST.projectAction('saveAs',path))throw Error('Save As failed');window.__workflowStable=value=>JSON.stringify(value,(_k,v)=>v&&typeof v==='object'&&!Array.isArray(v)?Object.fromEntries(Object.keys(v).sort().map(k=>[k,v[k]])):v);window.__workflowContent=p=>({patterns:p.patterns,tracks:p.tracks,groups:p.groups,instruments:p.instruments.filter(i=>i.type!=='synth'&&!String(i.samplePath||'').startsWith('builtin:')),routing:p.routing});window.__workflowSaved=window.__workflowStable(window.__workflowContent(window.__supertracker.getProject()));return true;", ["path": save.path])
            try await click("#save"); try await wait("!ST.state.dirty&&ST.state.pending.size===0")
            let fixture = folder.appendingPathComponent("saved-session")
            try files.createDirectory(at: fixture, withIntermediateDirectories: true)
            for item in [save, save.deletingPathExtension().appendingPathExtension("media")] {
                let target = fixture.appendingPathComponent(item.lastPathComponent)
                if files.fileExists(atPath: target.path) { try files.removeItem(at: target) }
                try files.copyItem(at: item, to: target)
            }
            evidence["savedProjectFixture"] = fixture.appendingPathComponent(save.lastPathComponent).path
            try await key("n", code: "KeyN", meta: true); try await wait("ST.state.project.tracks.length===1&&!ST.isChangingSession")
            _ = try await js("if(!await ST.projectAction('open',path))throw Error('Open failed');return true;", ["path": save.path])
            try await settle()
            try await check("ST.state.project.patterns.length===2&&ST.state.project.groups?.length===1&&STGroups.members(ST.state.project,ST.state.project.groups[0].id).every(i=>!!ST.state.project.instruments[ST.state.project.tracks[i].instrument].samplePath)&&ST.state.project.tracks.length===window.__workflowTrackCount+1", "Saved session lost patterns, kit PCM or added track on reopen")
            evidence["savedContent"] = try await js("return JSON.parse(window.__workflowSaved);")
            evidence["reopenedContent"] = try await js("return window.__workflowContent(window.__supertracker.getProject());")
            try await check("window.__workflowStable(window.__workflowContent(window.__supertracker.getProject()))===window.__workflowSaved", "Save/reopen changed recorded steps, FX, sampler settings, group membership or routing")
            evidence["reopenedSnapshot"] = try await js("return await ST.request('engine.snapshot');")
            try await picture("07-reopened-session")
            for (index, source) in sources.enumerated() { try require(try Data(contentsOf: source) == originals[index], "Workflow changed an original generated sample") }
            checks.append("Both built patterns produce finite native PCM; Save button and native reopen retain the complete session without changing source WAVs")
            try report(true)
            _ = try await js("await ST.request('transport.stop');await STBrowser.stopPreview();await ST.request('library.removeRoot',{path:root});return true;", ["root": media.path])
            return checks
        } catch {
            evidence["diagnostics"] = try? await js("const safe=async c=>{try{return await ST.request(c);}catch(e){return {error:String(e)}}};return {view:ST.state.view,modal:ST.state.modal,focus:document.activeElement?.outerHTML?.slice(0,1000),selection:{track:ST.state.track,row:ST.state.row,column:ST.state.column,keyboardPlay:ST.state.keyboardPlay,recording:ST.state.recording,group:STKitGroups.cursorGroup()?.id},toast:document.getElementById('toast-region')?.textContent,pending:Array.from(ST.state.pending.values()).map(x=>x.command),project:window.__supertracker.getProject(),status:ST.state.status,routing:await safe('routing.status'),snapshot:await safe('engine.snapshot')};")
            try? await picture("failure")
            try? report(false, error: error.localizedDescription)
            _ = try? await js("ST.stopHeld();await ST.request('transport.stop');await STBrowser.stopPreview();return true;")
            throw error
        }
    }
}
#endif

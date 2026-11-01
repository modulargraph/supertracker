// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Mounted sampler checks using a generated sample and the real Bridge. The
/// fixture's master gain is zero; no installed plug-in or native panel is opened.
@MainActor
enum NativeUISamplerTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        var checks: [String] = [], evidence: [String: Any] = [:]
        let folder = output.appendingPathComponent("mounted-sampler", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let originalFrame = web.frame
        defer { web.frame = originalFrame }
        func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(source, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func require(_ value: Bool, _ message: String) throws {
            if !value { throw TrackerError.message("WEBKIT SAMPLER: " + message) }
        }
        func check(_ expression: String, _ message: String) async throws {
            try require(try await js("return Boolean(\(expression));") as? Bool == true, message)
        }
        func wait(_ expression: String, _ message: String) async throws {
            let deadline = Date().addingTimeInterval(30)
            while Date() < deadline {
                if try await js("return Boolean(\(expression));") as? Bool == true { return }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            let diagnostics = try? await js("return {focus:document.activeElement?.outerHTML,view:ST.state.view,modal:ST.state.modal,status:document.getElementById('sampler-audition-status')?.textContent,pending:Array.from(ST.state.pending.values()).map(x=>x.command),requests:window.__samplerCalls,replies:window.__samplerReplies};")
            throw TrackerError.message("WEBKIT SAMPLER: \(message). \(String(describing: diagnostics))")
        }
        func focus(_ id: String) async throws {
            _ = try await js("const el=document.getElementById(id);if(!el)throw Error('Missing '+id);if(el.closest('[hidden]'))throw Error('Cannot focus hidden sampler control '+id);el.focus();el.dispatchEvent(new FocusEvent('focus'));return true;", ["id":id])
        }
        func key(_ name: String) async throws {
            _ = try await js("document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:name,bubbles:true,cancelable:true}));return true;", ["name":name])
        }
        func report(_ passed: Bool, _ error: String? = nil) throws {
            evidence["passed"] = passed; evidence["checks"] = checks
            evidence["generatedAt"] = ISO8601DateFormatter().string(from: Date())
            evidence["installedPluginsLoaded"] = false; evidence["nativeFilePanelsTested"] = false
            evidence["masterGain"] = 0; evidence["audibleOutputMeasured"] = false
            evidence["physicalKeyboardTested"] = false
            evidence["keyboardMethod"] = "DOM KeyboardEvent through mounted app.js and sampler.js. Inactive hidden WebKit focus listeners receive explicit FocusEvent. Native Tab traversal, number-input arrow defaults, physical shortcuts and acoustic output are not asserted. The test verifies number arrows are not intercepted, explicit marker edits commit, and actual Bridge preview requests succeed."
            if let error { evidence["error"] = error }
            try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys]).write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func cleanup() async throws {
            _ = try await js("""
                if(window.__samplerOriginalStudio)window.STStudio=window.__samplerOriginalStudio;
                window.__samplerReleaseGraph?.();
                STSampler.stopAudition();
                if(window.__samplerOriginalPostMessage)window.__samplerMessageHandler.postMessage=window.__samplerOriginalPostMessage;
                if(window.__samplerOriginalReceive)window.nativeReceive=window.__samplerOriginalReceive;
                await ST.request('transport.stop');ST.closeModal();
                if(window.__samplerOriginalProject){
                  ST.acceptProject(window.__samplerOriginalProject);
                  await ST.request('engine.project',{project:window.__supertracker.getProject()});
                  const old=window.__samplerOriginalUI;
                  ST.setView(old.view);
                  if(ST.state.recording!==old.recording)await ST.perform('record');
                  Object.assign(ST.state,{track:old.track,instrument:old.instrument,row:old.row,column:old.column,keyboardPlay:old.keyboardPlay,selection:old.selection});ST.refresh();
                }
                for(const name of ['OriginalStudio','ReleaseGraph','OriginalPostMessage','MessageHandler','OriginalReceive','OriginalProject','OriginalUI','Calls','Replies','Order','Before','NativeBefore','Undo','Pattern','Stopped','PreviewCount','GraphEntered'])delete window['__sampler'+name];
                return true;
                """)
        }
        let toneURL = scratch.appendingPathComponent("Mounted sampler slices.wav")
        let tone = (0..<96_000).map { Float(sin(Double($0) * 2 * Double.pi * 220 / 48_000) * 0.2) }
        try AudioSamples.write(DecodedSample(path: toneURL.path, sampleRate: 48_000, left: tone, right: tone, originalChannels: 2), to: toneURL)
        let points = (0..<128).map { Double($0) / 128 }
        var fixture = NativeSelfTest.testProject()
        fixture["name"] = "Muted mounted sampler fixture"; fixture["masterVolume"] = 0
        fixture["tracks"] = [["id":0,"name":"Unassigned sample audition","instrument":0,"volume":0.8,"pan":0]]
        fixture["instruments"] = [
            ["id":0,"name":"Assigned silent synth","type":"synth","volume":0] as [String:Any],
            ["id":1,"name":"128 slice fixture","type":"sample","samplePath":toneURL.path,"rootNote":127,"volume":1,"pan":0,"mode":"oneshot","start":0.0,"end":1.0,"slicePoints":points,"slices":points,"sliceCount":128,"attack":0,"decay":0,"sustain":1,"release":0.001,"cutoff":20_000] as [String:Any]
        ]
        fixture["patterns"] = [["id":0,"name":"Silent transport","length":64,"steps":[Array(repeating:NSNull(),count:64)],"automation":[]] as [String:Any]]
        fixture["sequence"] = [0]; fixture["activePattern"] = 0; fixture["audioClips"] = []; fixture["audioPlugins"] = []
        fixture["bpm"] = 120; fixture["linesPerBeat"] = 4; fixture["loopSong"] = true; fixture["metronome"] = false
        fixture.removeValue(forKey:"routing")
        try report(false)
        do {
            _ = try await js("""
                window.__samplerOriginalProject=JSON.parse(JSON.stringify(window.__supertracker.getProject()));
                window.__samplerOriginalUI={view:ST.state.view,track:ST.state.track,instrument:ST.state.instrument,row:ST.state.row,column:ST.state.column,recording:ST.state.recording,keyboardPlay:ST.state.keyboardPlay,selection:ST.state.selection};
                await ST.request('transport.stop');ST.acceptProject(project);
                await ST.request('engine.project',{project:window.__supertracker.getProject()});
                ST.state.instrument=1;ST.state.track=0;ST.state.row=13;ST.state.column=2;
                await ST.perform('record');ST.setView('instrument');document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'2',code:'Digit2',altKey:true,bubbles:true,cancelable:true}));
                return true;
                """, ["project":fixture])
            try await wait("ST.state.pending.size===0&&ST.state.samples[1]?.path===ST.state.project.instruments[1].samplePath", "Generated sample did not finish loading")
            try await wait("ST.state.recording===true&&(ST.state.status.recordArmSupported!==true||ST.state.status.noteRecordArmed===true)", "The real record-arm command did not reach the UI and native host")
            _ = try await js("""
                window.__samplerOriginalReceive=window.nativeReceive;
                window.__samplerOriginalStudio=window.STStudio;window.__samplerCalls=[];window.__samplerReplies=[];window.__samplerOrder=0;
                const handler=window.webkit.messageHandlers.supertracker;
                window.__samplerMessageHandler=handler;window.__samplerOriginalPostMessage=handler.postMessage;
                handler.postMessage=function(message){
                  const parsed=typeof message==='string'?JSON.parse(message):message;
                  const item={id:parsed.id,command:parsed.command,payload:JSON.parse(JSON.stringify(parsed.payload||{})),sentOrder:++window.__samplerOrder};window.__samplerCalls.push(item);
                  try{return window.__samplerOriginalPostMessage.call(handler,message);}
                  catch(error){item.error=error.message;item.finished=true;throw error;}
                };
                window.nativeReceive=function(message){
                  const parsed=typeof message==='string'?JSON.parse(message):message;
                  const pending=ST.state.pending.get(parsed.id);
                  const order=++window.__samplerOrder,item=window.__samplerCalls.find(call=>call.id===parsed.id);
                  if(item){item.result=parsed.result;item.ok=parsed.ok;item.finished=true;item.replyOrder=order;if(!parsed.ok)item.error=parsed.error;}
                  if(pending)window.__samplerReplies.push({id:parsed.id,command:pending.command,ok:parsed.ok,order});
                  return window.__samplerOriginalReceive(message);
                };
                return true;
                """)
            try await check("(await ST.request('engine.snapshot')).masterVolume===0&&ST.state.project.audioPlugins.length===0", "Fixture is not muted and plug-in-free")
            try await check("window.__samplerCalls.some(x=>x.command==='engine.snapshot'&&x.ok&&x.finished&&x.replyOrder>x.sentOrder)", "Actual native bridge send/reply observation is unavailable")
            try await focus("sampler-audition-0")
            try await key("End")
            try await check("document.activeElement.id==='sampler-audition-127'&&document.querySelectorAll('[data-sampler-audition][tabindex=\"0\"]').length===1&&document.getElementById('sampler-row-127').classList.contains('is-selected')", "End did not select the last of 128 slices with one audition tab stop")
            try await key("ArrowUp")
            try await check("document.activeElement.id==='sampler-audition-126'&&document.getElementById('sampler-marker-126').tabIndex===0", "Slice selection did not expose its own marker field")
            try await focus("sampler-marker-126")
            try await check("(()=>{const e=new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true,cancelable:true});document.activeElement.dispatchEvent(e);return !e.defaultPrevented;})()", "Numeric-field arrows were intercepted by slice navigation")
            checks.append("Mounted 128-slice roving navigation reaches the final slice and leaves number arrows to WebKit")

            _ = try await js("await ST.request('transport.play',{mode:'pattern',pattern:0,row:0});return true;")
            try await wait("ST.state.playing", "Muted native transport did not start")
            _ = try await js("window.__samplerBefore=JSON.stringify({project:ST.state.project,row:ST.state.row,column:ST.state.column,selection:ST.state.selection,recording:ST.state.recording,undo:ST.state.undo.length});window.__samplerStopped=window.__samplerReplies.filter(x=>x.command==='transport.stop').length;return true;")
            try await focus("sampler-audition-127")
            try await key("Enter")
            try await wait("window.__samplerCalls.some(x=>x.command==='step.preview'&&x.payload.step.note===127&&x.finished)", "Return did not complete a real native slice preview")
            try await check("(()=>{const p=window.__samplerCalls.findLast(x=>x.command==='step.preview'&&x.payload.step.note>=0);return p.payload.track===0&&p.payload.step.instrument===1&&p.payload.step.note===127&&p.payload.step.fx1.type==='slice'&&p.payload.step.fx1.value===128&&p.payload.duration===2&&p.result?.auditioning===true&&JSON.stringify({project:ST.state.project,row:ST.state.row,column:ST.state.column,selection:ST.state.selection,recording:ST.state.recording,undo:ST.state.undo.length})===window.__samplerBefore;})()", "Preview pitch/instrument/slice or non-mutating record-arm behavior is incorrect")
            try await key("Escape")
            try await wait("window.__samplerCalls.some(x=>x.command==='step.preview'&&x.payload.step.note===-1&&x.finished)", "Escape did not cancel the owned native preview")
            try await check("ST.state.playing&&window.__samplerReplies.filter(x=>x.command==='transport.stop').length===window.__samplerStopped", "Escape stopped transport instead of only its slice preview")
            checks.append("Actual Bridge Return previews slice 128 at root 127 on the selected instrument without recording; Escape preserves native transport")

            _ = try await js("window.__samplerPreviewCount=window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note>=0).length;return true;")
            try await focus("sampler-audition-0")
            try await key(" ")
            try await wait("window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note>=0&&x.finished).length>window.__samplerPreviewCount", "Space did not preview the first slice")
            try await key("Escape")
            try await check("window.__samplerCalls.findLast(x=>x.command==='step.preview'&&x.payload.step.note>=0).payload.step.fx1.value===1&&ST.state.playing", "Space selected the wrong slice or toggled transport")
            checks.append("Mounted Space auditions the first slice without triggering transport shortcuts")

            try await focus("sampler-audition-1")
            try await focus("sampler-marker-1")
            _ = try await js("window.__samplerUndo=ST.state.undo.length;window.__samplerPattern=JSON.stringify(ST.state.project.patterns);window.__samplerNativeBefore=window.__samplerCalls.length;document.activeElement.value='1';return true;")
            try await key("Enter")
            try await wait("window.__samplerCalls.slice(window.__samplerNativeBefore).some(x=>x.command==='step.preview'&&x.payload.step.note>=0&&x.finished)", "Marker Return did not commit and hear the edit")
            let markerCommit = try await js("""
                const calls=window.__samplerCalls.slice(window.__samplerNativeBefore),preview=calls.find(x=>x.command==='step.preview'&&x.payload.step.note>=0);
                const sync=calls.findLast(x=>x.command==='engine.project'&&x.payload.awaitPlugins===true&&x.ok===true&&x.finished&&x.replyOrder<preview?.sentOrder&&x.payload.project.instruments[1].slicePoints[1]===.01);
                const native=await ST.request('engine.snapshot');
                return {synchronized:!!sync,replyBeforePreview:!!sync&&sync.replyOrder<preview.sentOrder,previewSucceeded:preview?.ok===true&&preview.result?.auditioning===true,correctSlice:preview?.payload.step.fx1.value===2,nativeMarker:native.instruments[1].slicePoints[1],uiMarker:ST.state.project.instruments[1].slicePoints[1],oneUndo:ST.state.undo.length===window.__samplerUndo+1,focus:document.activeElement.id,patternsUnchanged:JSON.stringify(ST.state.project.patterns)===window.__samplerPattern,recording:ST.state.recording,row:ST.state.row,syncID:sync?.id??null,syncSent:sync?.sentOrder??null,syncReplied:sync?.replyOrder??null,previewID:preview?.id??null,previewSent:preview?.sentOrder??null};
                """) as? [String:Any] ?? [:]
            evidence["markerCommit"] = markerCommit
            try require(["synchronized","replyBeforePreview","previewSucceeded","correctSlice","oneUndo","patternsUnchanged","recording"].allSatisfy { markerCommit[$0] as? Bool == true } && markerCommit["focus"] as? String == "sampler-marker-1" && markerCommit["row"] as? Int == 13 && (markerCommit["uiMarker"] as? NSNumber)?.doubleValue == 0.01 && (markerCommit["nativeMarker"] as? NSNumber)?.doubleValue == 0.01, "Marker commit lost focus, duplicated undo, changed notes, or previewed before native synchronization: \(markerCommit)")
            try await key("Escape")
            checks.append("Mounted marker Return commits one undo step, strictly synchronizes its map, then previews while preserving field focus")

            for navigation in [false, true] {
                _ = try await js("""
                    ST.setView('instrument');document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'2',code:'Digit2',altKey:true,bubbles:true,cancelable:true}));document.getElementById('sampler-audition-2').focus();
                    document.activeElement.dispatchEvent(new FocusEvent('focus'));
                    window.__samplerPreviewCount=window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note>=0).length;
                    const gate=new Promise(resolve=>{window.__samplerReleaseGraph=resolve;});window.__samplerGraphEntered=false;
                    window.STStudio=Object.create(window.__samplerOriginalStudio);
                    Object.defineProperty(window.STStudio,'isMovingEffect',{value:true,configurable:true});
                    window.STStudio.waitForEffectMove=()=>{window.__samplerGraphEntered=true;return gate;};
                    document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true}));
                    const deadline=performance.now()+30000;
                    while(!window.__samplerGraphEntered&&performance.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));
                    if(!window.__samplerGraphEntered)throw Error('Sampler helper did not enter the actual graph gate');
                    if(navigation)ST.setView('pattern');else ST.openModal('Sampler cancellation fixture','<p>Native graph wait cancellation</p>');
                    window.STStudio=window.__samplerOriginalStudio;window.__samplerReleaseGraph();
                    await new Promise(resolve=>setTimeout(resolve,50));return true;
                    """, ["navigation":navigation])
                try await check("window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note>=0).length===window.__samplerPreviewCount&&ST.state.playing", "A modal or view change allowed a delayed preview to start")
                _ = try await js("ST.closeModal();return true;")
            }
            checks.append("Mounted modal and navigation hooks invalidate graph-wait auditions before any native preview request")

            _ = try await js("ST.setView('instrument');document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'2',code:'Digit2',altKey:true,bubbles:true,cancelable:true}));return true;")
            try await focus("sampler-audition-3")
            _ = try await js("window.__samplerPreviewCount=window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note>=0).length;return true;")
            try await key("Enter")
            try await wait("window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note>=0&&x.finished).length>window.__samplerPreviewCount", "Focus-loss fixture preview did not begin")
            _ = try await js("window.__samplerStopped=window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note<0).length;const panel=document.getElementById('sampler-slice-map');document.getElementById('instrument-rename').focus();panel.dispatchEvent(new FocusEvent('focusout',{bubbles:true}));await Promise.resolve();return true;")
            try await wait("window.__samplerCalls.filter(x=>x.command==='step.preview'&&x.payload.step.note<0&&x.finished).length>window.__samplerStopped", "Leaving Slice Map did not cancel its native preview")
            try await check("ST.state.playing", "Leaving Slice Map stopped song playback")
            checks.append("Mounted focus-loss cancellation stops only the owned native preview")

            var layouts: [[String:Any]] = []
            for size in [CGSize(width:1080,height:700),CGSize(width:1500,height:940)] {
                web.setFrameSize(size)
                try await focus("sampler-audition-0")
                try await key("End")
                let layout = try await js("const el=document.activeElement,r=el.getBoundingClientRect(),list=el.closest('.sampler-marker-list').getBoundingClientRect();return {width:innerWidth,height:innerHeight,focus:el.id,left:r.left,right:r.right,top:r.top,bottom:r.bottom,listTop:list.top,listBottom:list.bottom,visible:r.left>=0&&r.right<=innerWidth&&r.top>=Math.max(0,list.top)&&r.bottom<=Math.min(innerHeight,list.bottom)};") as? [String:Any] ?? [:]
                try require(layout["visible"] as? Bool == true, "Last-slice focus was clipped at \(Int(size.width))×\(Int(size.height))")
                layouts.append(layout)
                let config = WKSnapshotConfiguration(); config.rect = web.bounds
                let image = try await web.takeSnapshot(configuration:config)
                if let tiff = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data:tiff), let png = bitmap.representation(using:.png,properties:[:]) {
                    try png.write(to:folder.appendingPathComponent("slices-\(Int(size.width))x\(Int(size.height)).png"))
                }
            }
            evidence["layouts"] = layouts
            checks.append("Selected slice focus stays visible within the scrolled 128-marker list at both target sizes")
            evidence["previewCalls"] = try await js("return window.__samplerCalls.filter(x=>x.command==='step.preview');")
            evidence["nativeTransportStopRepliesDuringChecks"] = try await js("return window.__samplerReplies.filter(x=>x.command==='transport.stop').length;")
            try await check("!window.__samplerCalls.some(x=>x.error)", "A native sampler bridge request failed")
            try await cleanup()
            try report(true)
            return checks
        } catch {
            evidence["failureDOM"] = try? await js("return {focus:document.activeElement?.outerHTML,view:ST.state.view,modal:ST.state.modal,status:document.getElementById('sampler-audition-status')?.textContent};")
            try? await cleanup()
            try? report(false,error.localizedDescription)
            throw error
        }
    }
}

#endif

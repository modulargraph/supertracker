// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Mounted-interface checks. DOM keyboard events exercise the real handlers without
/// taking keyboard focus from the user's application; native transport remains real.
@MainActor
final class NativeUIAcceptanceTests {
    private let web: WKWebView
    private let output: URL
    private var evidence: [String: Any] = [:]
    private var snapshots: [[String: Any]] = []
    private init(web: WKWebView, output: URL) { self.web = web; self.output = output }

    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        let test = NativeUIAcceptanceTests(web: web, output: output)
        let report = output.appendingPathComponent("webkit-interface-report.json")
        func write(_ passed: Bool, _ error: String? = nil) throws {
            var result = test.evidence
            result["passed"] = passed; result["snapshots"] = test.snapshots
            result["generatedAt"] = ISO8601DateFormatter().string(from: Date())
            result["keyboardMethod"] = "DOM KeyboardEvent dispatched through the mounted application's real handlers. Inactive hidden WebKit suppresses focus events, so FocusEvent explicitly covers facet roving-tabindex and range draft listeners. Operating-system shortcuts, native Tab traversal, select-menu navigation and Space activation defaults are not simulated. Select values plus change events and checkbox click() separately verify the mounted change handlers; these are programmatic activation, not physical keyboard acceptance."
            result["installedPluginsLoaded"] = false
            if let error { result["error"] = error }
            try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]).write(to: report, options: .atomic)
        }
        try write(false)
        do {
            try await test.browser(root: scratch.appendingPathComponent("Browser fixtures"))
            try await test.browserDrafts()
            try await test.keyboardLayouts()
            try await test.workspaceNavigation()
            try await test.modulationKeyboard()
            try await test.patternWorkflows()
            try await test.automationTransport()
            try write(true)
            return ["Mounted browser facets combine native source/type/tempo/key/length filters and clear correctly",
                    "Browser range drafts survive native search replies and disappearing filters retain useful focus",
                    "DOM keyboard navigation and viewport layout work at 1080×700 and 1500×940",
                    "Mounted workspace, mixer, sidebar, browser and clip keys stay within their intended controls",
                    "LFO timing and exact beat edits preserve values and focus; checkbox Space is not intercepted",
                    "Real native transport moves automation playhead without replacing focused editors",
                    "Pattern Keys toggle, native note recording, live cell repaint, drag and Undo work in real WebKit"]
        } catch { try? write(false, error.localizedDescription); throw error }
    }

    private func js(_ body: String, _ args: [String: Any] = [:]) async throws -> Any {
        try await web.callAsyncJavaScript(body, arguments: args, in: nil, contentWorld: .page) ?? NSNull()
    }
    private func require(_ value: Bool, _ message: String) throws {
        if !value { throw TrackerError.message("WEBKIT ACCEPTANCE: " + message) }
    }
    private func check(_ expression: String, _ message: String) async throws {
        if try await js("return Boolean(\(expression));") as? Bool != true {
            evidence["failureDOM"] = try? await js("return {focus:document.activeElement?.outerHTML,keys:Array.from(document.querySelectorAll('[data-browser-facet=keys]')).map(x=>({id:x.id,tabIndex:x.tabIndex,open:x.closest('details')?.open})),view:ST.state.view,selection:ST.state.library.selected};")
            throw TrackerError.message("WEBKIT ACCEPTANCE: " + message)
        }
    }
    private func wait(_ expression: String, _ message: String, seconds: Double = 15) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if try await js("return Boolean(\(expression));") as? Bool == true { return }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        let state = try? await js("return {view:ST.state.view,total:ST.state.library.total,filters:ST.state.library.filters,items:ST.state.library.items.map(x=>({name:x.name,duration:x.duration,key:x.key,sampleType:x.sampleType})),status:ST.state.status,focus:document.activeElement?.id,toast:document.getElementById('toast-region')?.textContent};")
        throw TrackerError.message("WEBKIT ACCEPTANCE: \(message). \(String(describing: state))")
    }
    private func key(_ value: String, code: String = "", meta: Bool = false, control: Bool = false, alt: Bool = false, shift: Bool = false) async throws {
        _ = try await js("(document.activeElement||document.body).dispatchEvent(new KeyboardEvent('keydown',{key:key,code:code,bubbles:true,cancelable:true,metaKey:meta,ctrlKey:control,altKey:alt,shiftKey:shift}));return true;", ["key":value,"code":code,"meta":meta,"control":control,"alt":alt,"shift":shift])
    }
    // Enter the real mounted page using its keyboard shortcut; hidden controls
    // deliberately cannot be focused directly after the instrument-page redesign.
    private func instrumentPage(_ page: String) async throws {
        let index = try await js("return STInstrumentPages.list().findIndex(x=>x[0]===page)+1;", ["page":page]) as? Int ?? 0
        try require(index > 0, "Missing instrument page \(page)")
        try await key(String(index), code:"Digit\(index)", alt:true)
        try await check("STInstrumentPages.current()==='\(page)'&&!document.getElementById('inst-page-\(page)').hidden&&document.activeElement.closest('[data-inst-page]')?.dataset.instPage==='\(page)'", "Instrument page shortcut did not enter \(page)")
    }
    private func facet(_ field: String, _ value: String) async throws {
        _ = try await js("const button=Array.from(document.querySelectorAll('[data-browser-facet]')).find(x=>x.dataset.browserFacet===field&&x.dataset.browserValue===value);if(!button)throw Error('Missing facet '+field+':'+value);button.closest('details').open=true;button.click();return true;", ["field":field,"value":value])
        _ = try await js("await STBrowser.search();return true;")
    }
    private func range(_ field: String, _ bound: String, _ value: String) async throws {
        _ = try await js("const input=document.getElementById('browser-range-'+field+'-'+bound);input.closest('details').open=true;input.value=value;input.dispatchEvent(new Event('change',{bubbles:true}));return true;", ["field":field,"bound":bound,"value":value])
        _ = try await js("await STBrowser.search();return true;")
    }
    private func clearFilters() async throws {
        _ = try await js("document.getElementById('browser-clear-all')?.click();await STBrowser.search();return true;")
        try await wait("ST.state.library.total===4", "Clearing filters did not restore all four fixture samples")
    }

    private func patternWorkflows() async throws {
        _ = try await js("window.__patternBackup={project:window.__supertracker.getProject(),keyboardPlay:ST.state.keyboardPlay,undo:ST.state.undo,redo:ST.state.redo};return true;")
        let restore = "await ST.setNoteRecording(false);ST.stopHeld();ST.state.project=window.__patternBackup.project;ST.state.keyboardPlay=window.__patternBackup.keyboardPlay;ST.state.undo=window.__patternBackup.undo;ST.state.redo=window.__patternBackup.redo;ST.state.track=0;ST.state.instrument=0;ST.state.row=0;ST.state.selection=null;await ST.syncEngine(true,true);ST.refresh();return true;"
        do {
            _ = try await js("""
            const p=ST.demoProject();p.tracks=p.tracks.slice(0,2);p.tracks.forEach((t,i)=>{t.instrument=0;t.volume=0;t.mute=false;t.solo=false;});
            p.instruments=[{...ST.instrumentDefaults(0),type:'synth',waveform:'sine',name:'Workflow sine'}];
            p.audioPlugins=[];p.audioClips=[];p.patterns=p.patterns.slice(0,1);p.patterns[0].length=16;p.patterns[0].steps=[Array(16).fill(null),Array(16).fill(null)];p.patterns[0].automation=[];p.activePattern=0;p.sequence=[p.patterns[0].id];
            ST.state.project=ST.normalize(p);Object.assign(ST.state,{track:0,instrument:0,row:0,column:0,selection:null,keyboardPlay:false,recording:false,undo:[],redo:[],editStep:1,chord:'single',chordSet:'',scale:'chromatic'});
            await ST.syncEngine(true,true);ST.setView('pattern');ST.refresh();
            window.__keysButton=document.getElementById('keyboard-play');document.getElementById('tempo').focus();window.__keysButton.click();return true;
            """)
            try await check("ST.state.keyboardPlay&&document.activeElement.id==='tracker'&&document.getElementById('keyboard-play')===window.__keysButton", "First Keys click must leave text input and retain its button")
            _ = try await js("for(let i=0;i<6;i++){const before=ST.state.keyboardPlay;window.__keysButton.click();if(ST.state.keyboardPlay===before)throw Error('Keys click '+i+' was ignored');}await ST.setNoteRecording(true);document.getElementById('tracker').focus();return true;")
            try await key("z",code:"KeyZ")
            try await wait("ST.pattern().steps[0][0]?.note>=0", "Native acknowledged keyboard note was not recorded")
            _ = try await js("document.dispatchEvent(new KeyboardEvent('keyup',{key:'z',code:'KeyZ',bubbles:true}));return true;")
            try await check("document.querySelector('#tracker td[data-cell=\"0:0\"]').textContent.includes('C–')", "Recorded note data is not visible in the actual grid")
            _ = try await js("await ST.setNoteRecording(false);ST.state.row=2;ST.refresh();window.__liveGrid=document.getElementById('tracker');window.__liveRecord=ST.recordLiveStep(67,90,0,0,ST.pattern().id,2);return true;")
            try await check("ST.pattern().steps[0][2]?.note===67&&document.querySelector('#tracker td[data-cell=\"0:2\"]').textContent.includes('G–')&&document.getElementById('tracker')===window.__liveGrid", "Live recording must repaint its cell without replacing the grid")
            _ = try await js("ST.finishLiveRecord(window.__liveRecord);ST.state.row=0;ST.state.selection=null;ST.refresh();return true;")
            // Hit-test real layout, dispatch through the mounted pointer listeners, then wait for the deferred edit.
            _ = try await js("""
            const source=document.querySelector('#tracker td[data-cell="0:0"]'),target=document.querySelector('#tracker td[data-cell="1:4"]');
            target.scrollIntoView({block:'nearest',inline:'nearest'});const a=source.getBoundingClientRect(),b=target.getBoundingClientRect();
            if(document.elementFromPoint(b.x+b.width/2,b.y+b.height/2)?.closest('td')!==target)throw Error('Drag target is not hittable');
            source.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,cancelable:true,button:0,clientX:a.x+a.width/2,clientY:a.y+a.height/2}));
            window.dispatchEvent(new PointerEvent('pointermove',{clientX:b.x+b.width/2,clientY:b.y+b.height/2}));
            window.dispatchEvent(new PointerEvent('pointerup',{button:0}));return true;
            """)
            try await wait("ST.pattern().steps[0][0]===null&&ST.pattern().steps[1][4]?.note>=0", "Dragging a single occupied cell did not move its note")
            _ = try await js("document.getElementById('tracker').focus();ST.perform('undo');return true;")
            try await wait("ST.pattern().steps[0][0]?.note>=0&&ST.pattern().steps[1][4]===null", "Undo did not restore both cells after moving a note")
            try await snapshot("pattern-workflows.png")
            evidence["patternWorkflows"] = ["keysFirstClick":true,"repeatedKeysClicks":6,"nativeRecordedNoteVisible":true,"livePaintRetainsGrid":true,"singleNoteDragAcrossTracks":true,"moveUndo":true,"outputMuted":true]
        } catch { _ = try? await js(restore); throw error }
        _ = try await js(restore)
    }

    private func browser(root: URL) async throws {
        let fixtures: [(String, Double)] = [
            ("Loopcloud/Neon Pack/Loops/Bass_120bpm_Cmin_loop.wav", 1.25),
            ("Loopcloud/Neon Pack/Loops/Bass_128bpm_Gmin_loop.wav", 1.5),
            ("Native Access Content/Anima Ascent Library/Samples/One Shots/Kick_punch.wav", 0.1),
            ("User Texture Pack/Textures/Soft_air.wav", 0.3)]
        for (relative, duration) in fixtures {
            let url = root.appendingPathComponent(relative)
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            let silence = [Float](repeating: 0, count: Int(duration * 48000))
            try AudioSamples.write(DecodedSample(path:url.path,sampleRate:48000,left:silence,right:silence,originalChannels:2),to:url)
        }
        for folder in ["Groups", "Sounds"] {
            try FileManager.default.createDirectory(at: root.appendingPathComponent("Native Access Content/Anima Ascent Library/" + folder), withIntermediateDirectories: true)
        }
        _ = try await js("await ST.request('library.scan',{roots:[path]});ST.setView('browser');const toggle=document.getElementById('browser-audition');toggle.checked=false;toggle.dispatchEvent(new Event('change',{bubbles:true}));return true;", ["path":root.path])
        try await wait("ST.state.library.total===4&&!ST.state.library.scanning", "Fixture library did not finish scanning")
        try await wait("ST.state.library.items.length===4&&ST.state.library.items.every(x=>Number.isFinite(x.duration))", "Native duration metadata did not reach mounted browser")
        try await wait("ST.state.library.items.some(x=>x.name==='Kick_punch'&&x.collection==='Maschine expansions')", "Verified expansion metadata did not reach mounted browser")
        evidence["browserFixtures"] = try await js("return ST.state.library.items.map(x=>({name:x.name,collection:x.collection,sampleType:x.sampleType,bpm:x.bpm??null,key:x.key??null,duration:x.duration}));")

        try await facet("categories", "Bass")
        try await wait("ST.state.library.total===2", "Category filter did not select two bass samples")
        try await facet("categories", "Kick")
        try await wait("ST.state.library.total===3", "Multiple categories must combine with OR")
        try await facet("collections", "Loopcloud")
        try await wait("ST.state.library.total===2", "Source and category filters must combine with AND")
        try await clearFilters()
        try await facet("collections", "Maschine expansions")
        try await facet("sampleTypes", "oneshot")
        try await wait("ST.state.library.total===1&&ST.state.library.items[0].name==='Kick_punch'", "Maschine expansion and one-shot filters did not select the fixture kick")
        try await clearFilters()
        try await facet("collections", "Loopcloud")
        try await facet("sampleTypes", "oneshot")
        try await wait("ST.state.library.total===0&&document.querySelector('.browser-empty h3')?.textContent==='No matching sounds'", "Incompatible filters did not show a useful empty state")
        _ = try await js("document.querySelector('[data-browser-remove-filter=sampleTypes]').click();await STBrowser.search();return true;")
        try await wait("ST.state.library.total===2", "Removing an individual chip did not restore matches")
        try await facet("sampleTypes", "loop")
        try await range("bpm", "min", "119")
        try await range("bpm", "max", "122")
        try await wait("ST.state.library.total===1", "Tempo bounds did not select the 120 BPM loop")
        try await facet("keys", "C minor")
        try await range("duration", "min", "1.1")
        try await range("duration", "max", "1.4")
        try await wait("ST.state.library.total===1&&ST.state.library.items[0].name.includes('120bpm')&&document.querySelectorAll('[data-browser-remove-filter]').length===5", "Combined source/type/key/tempo/duration controls returned the wrong sample")
        evidence["combinedFilter"] = try await js("return {filters:ST.state.library.filters,names:ST.state.library.items.map(x=>x.name),chips:Array.from(document.querySelectorAll('[data-browser-remove-filter]')).map(x=>x.textContent)};")
        try await clearFilters()
        _ = try await js("const input=document.getElementById('browser-search');input.value='Soft_air';input.dispatchEvent(new Event('input',{bubbles:true}));return true;")
        try await wait("ST.state.library.total===1&&ST.state.library.items[0].name==='Soft_air'", "Text search did not combine with the native index")
        try await clearFilters()
        _ = try await js("const input=document.getElementById('browser-unknown-bpm');input.closest('details').open=true;input.checked=true;input.dispatchEvent(new Event('change',{bubbles:true}));await STBrowser.search();return true;")
        try await wait("ST.state.library.total===2&&ST.state.library.items.every(x=>!x.bpm)", "Unknown-tempo filter returned known tempos")
        try await clearFilters()
    }

    private func browserDrafts() async throws {
        _ = try await js("""
        ST.setView('browser');await STBrowser.search();
        const input=document.getElementById('browser-range-duration-min');input.closest('details').open=true;
        input.focus();input.dispatchEvent(new FocusEvent('focus'));window.__browserDraftNode=input;
        input.value='1.2';input.dispatchEvent(new Event('input',{bubbles:true}));
        await STBrowser.search();return true;
        """)
        try await check("document.getElementById('browser-range-duration-min')===window.__browserDraftNode&&document.activeElement===window.__browserDraftNode&&document.activeElement.value==='1.2'&&!Number.isFinite(ST.state.library.filters.duration.min)", "Native library response replaced or prematurely committed the focused numeric draft")
        // Explicitly move DOM focus after Tab; synthetic Tab has no native default.
        try await key("Tab")
        _ = try await js("const input=window.__browserDraftNode;input.dispatchEvent(new Event('change',{bubbles:true}));document.getElementById('browser-search').focus();input.dispatchEvent(new FocusEvent('blur'));await STBrowser.search();return true;")
        try await wait("ST.state.library.total===2&&ST.state.library.filters.duration.min===1.2&&document.activeElement.id==='browser-search'&&document.getElementById('browser-range-duration-min').value==='1.2'&&document.getElementById('browser-range-duration-min')!==window.__browserDraftNode", "Committed range or explicit focus destination was lost after deferred browser paint")
        try await clearFilters()
        try await facet("keys", "C minor")
        _ = try await js("const chip=document.getElementById('browser-filter-chip-0');if(!chip)throw Error('Missing final filter chip');chip.focus();chip.click();await STBrowser.search();return true;")
        try await wait("ST.state.library.total===4&&!document.getElementById('browser-clear-all')&&document.activeElement.id==='browser-results-list'", "Removing the final filter stranded browser focus")
        evidence["browserDrafts"] = ["nativeSearchReplyPreservedDraftNode":true,"commitRetainedValue":true,"explicitDepartureRetainedFocus":true,"lastChipFocusFallback":true,"nativeTabTraversalTested":false]
    }

    private func keyboardLayouts() async throws {
        var sizes: [[String: Any]] = []
        for (width, height) in [(1080,700),(1500,940)] {
            web.setFrameSize(NSSize(width: width, height: height))
            try await wait("innerWidth===\(width)&&innerHeight===\(height)", "Hidden WebKit viewport did not resize")
            try await key("3", code:"Digit3", meta:true)
            try await wait("ST.state.view==='browser'&&ST.state.library.total===4", "Command-3 did not open the populated browser")
            try await key("f", code:"KeyF", meta:true)
            try await check("document.activeElement?.id==='browser-search'", "Command-F did not focus browser search")
            try await key("ArrowDown", code:"ArrowDown")
            try await check("document.activeElement?.id==='browser-results-list'", "Down from search did not focus results")
            try await key("End", code:"End")
            try await check("ST.state.library.selected===3&&document.getElementById('browser-results-list').getAttribute('aria-activedescendant')==='browser-result-3'", "End did not select the last result accessibly")
            try await key("ArrowUp", code:"ArrowUp")
            try await check("ST.state.library.selected===2", "Up did not move the sample selection")
            try await key("Home", code:"Home")
            try await check("ST.state.library.selected===0", "Home did not return to the first sample")
            _ = try await js("document.getElementById('browser-group-keys').focus();return true;")
            try await key("ArrowRight", code:"ArrowRight")
            try await check("document.activeElement?.id==='browser-facet-search-keys'", "Right did not open the key facet and enter its search")
            try await key("ArrowDown", code:"ArrowDown")
            try await check("document.activeElement?.dataset.browserFacet==='keys'", "Down from facet search did not enter its options")
            try await key("End", code:"End")
            try await check("document.activeElement===Array.from(document.querySelectorAll('[data-browser-facet=keys]')).at(-1)", "Facet keyboard focus did not reach its last key")
            _ = try await js("document.activeElement.dispatchEvent(new FocusEvent('focus'));return true;")
            try await check("document.activeElement.tabIndex===0&&Array.from(document.querySelectorAll('[data-browser-facet=keys]')).filter(x=>x.tabIndex===0).length===1", "Facet focus listener did not maintain one keyboard tab stop")
            try await key("ArrowLeft", code:"ArrowLeft")
            try await check("document.activeElement?.id==='browser-group-keys'&&!document.activeElement.parentElement.open", "Left did not close the facet and restore summary focus")
            _ = try await js("document.getElementById('browser-search').focus();return true;")
            try await key("k", code:"KeyK", meta:true)
            try await wait("ST.state.modal==='palette'&&document.activeElement?.id==='palette-search'", "Browser command palette did not take dialog focus")
            try await key("Escape", code:"Escape")
            try await check("!ST.state.modal&&document.activeElement?.id==='browser-search'", "Closing the browser dialog did not restore its invoker focus")
            let browserLayout = try await layout([".browser-layout", ".browser-results", "#browser-search", "#browser-results-list", ".browser-detail"])
            try await snapshot("browser-\(width)x\(height).png")
            try await key("1", code:"Digit1", meta:true)
            try await key("Home", code:"Home")
            try await key("ArrowDown", code:"ArrowDown")
            try await check("ST.state.view==='pattern'&&ST.state.row===1", "Tracker arrow navigation failed")
            try await key("Tab", code:"Tab", control:true)
            try await check("document.activeElement?.id==='step-note'", "Control-Tab did not escape the tracker grid to its editor")
            try await key("k", code:"KeyK", meta:true)
            try await wait("document.getElementById('palette-search')!==null", "Command palette did not open")
            _ = try await js("const input=document.getElementById('palette-search');input.value='Go to Automation';input.dispatchEvent(new Event('input',{bubbles:true}));input.focus();return true;")
            try await key("Enter", code:"Enter")
            try await check("ST.state.view==='automation'&&!ST.state.modal", "Keyboard palette selection did not enter automation")
            try await key("f", code:"KeyF", meta:true)
            try await check("document.activeElement?.id==='automation-search'", "Automation search shortcut failed")
            try await key("ArrowDown", code:"ArrowDown")
            try await check("document.activeElement?.dataset.chooseTarget==='volume'", "Already-automated volume did not lead the keyboard target chooser")
            _ = try await js("document.getElementById('automation-envelope').focus();return true;")
            try await key("Home", code:"Home")
            _ = try await js("window.__pointBefore=ST.pattern().automation[0].points[0].value;return true;")
            try await key("ArrowUp", code:"ArrowUp")
            try await check("ST.pattern().automation[0].points[0].value>window.__pointBefore", "Automation arrow editing failed")
            try await key("z", code:"KeyZ", meta:true)
            try await check("Math.abs(ST.pattern().automation[0].points[0].value-window.__pointBefore)<0.000001", "Keyboard Undo did not restore the edited automation point")
            let automationLayout = try await layout([".studio-automation-layout", "#automation-envelope", "#automation-search", "#point-value-0"])
            try await snapshot("automation-\(width)x\(height).png")
            try await check("(()=>{const input=document.getElementById('point-value-0'),main=document.querySelector('main').getBoundingClientRect(),r=input.getBoundingClientRect();if(r.top>=main.top&&r.bottom<=main.bottom)return true;for(let p=input.parentElement;p&&p!==document.body;p=p.parentElement){if(['auto','scroll'].includes(getComputedStyle(p).overflowY)&&p.scrollHeight>p.clientHeight+1)return true;}return false;})()", "Offscreen automation point editors have no user-scrollable ancestor")
            _ = try await js("document.getElementById('point-value-0').focus();return true;")
            try await wait("(()=>{const r=document.getElementById('point-value-0').getBoundingClientRect(),m=document.querySelector('main').getBoundingClientRect();return r.top>=m.top&&r.bottom<=m.bottom;})()", "Keyboard focus did not reveal the precise automation point editor")
            _ = try await js("const input=document.getElementById('point-value-0');window.__precisePointValue=Number(input.value);input.value=window.__precisePointValue+0.01;input.dispatchEvent(new Event('change',{bubbles:true}));return true;")
            try await check("(()=>{const r=document.getElementById('point-value-0').getBoundingClientRect(),m=document.querySelector('main').getBoundingClientRect();return document.activeElement.id==='point-value-0'&&r.top>=m.top&&r.bottom<=m.bottom;})()", "Editing a revealed automation point lost focus or reset its viewport scroll")
            _ = try await js("const input=document.getElementById('point-value-0');input.value=window.__precisePointValue;input.dispatchEvent(new Event('change',{bubbles:true}));return true;")
            let focusedPoint = try await js("const r=document.getElementById('point-value-0').getBoundingClientRect();return {top:r.top,bottom:r.bottom,focus:document.activeElement.id};")
            if width == 1080 { try await snapshot("automation-point-focus-1080x700.png") }
            sizes.append(["width":width,"height":height,"browser":browserLayout,"automation":automationLayout,"focusedPoint":focusedPoint])
        }
        evidence["viewportChecks"] = sizes
    }

    private func workspaceNavigation() async throws {
        web.setFrameSize(NSSize(width:1080,height:700))
        try await wait("innerWidth===1080&&innerHeight===700", "Keyboard acceptance viewport did not resize")
        _ = try await js("""
        window.__navBackup={project:window.__supertracker.getProject(),keyboardPlay:ST.state.keyboardPlay,undo:ST.state.undo,redo:ST.state.redo};
        const p=window.__supertracker.getProject();p.masterVolume=0;
        p.tracks=Array.from({length:8},(_,i)=>({...p.tracks[0],id:i,routingID:'navigation-track-'+i,name:'Navigation track '+(i+1),instrument:0}));
        if(new Set(p.tracks.map(track=>track.routingID)).size!==p.tracks.length)throw Error('Navigation fixture requires unique track routing identities.');
        p.instruments.push({...p.instruments[0],id:1,name:'Second keyboard fixture'});
        p.patterns.forEach(pattern=>{while(pattern.steps.length<8)pattern.steps.push(Array(pattern.length).fill(null));});
        p.audioClips.push({...p.audioClips[0],id:'navigation-clip',name:'Keyboard clip',track:1,startBeat:8});
        ST.state.project=ST.normalize(p);ST.state.track=0;ST.state.instrument=0;ST.state.sequence=0;ST.state.keyboardPlay=false;ST.state.undo=[];ST.state.redo=[];
        await ST.syncEngine(true,true);return true;
        """)
        do {
            var entry: [[String: Any]] = []
            for (number, view, focus) in [(1,"pattern","tracker"),(2,"instrument","instrument-rename"),(3,"browser","browser-search"),(4,"mixer","mix-track-0"),(5,"automation","automation-search"),(6,"song","sequence-list")] {
                try await key(String(number),code:"Digit\(number)",meta:true)
                if view == "browser" { try await key("f",code:"KeyF",meta:true) }
                try await check("ST.state.view==='\(view)'&&document.activeElement?.id==='\(focus)'", "Workspace entry did not focus \(view)")
                entry.append(["view":view,"focus":focus])
            }
            _ = try await js("ST.setView('instrument');document.getElementById('view-instrument').focus();window.__navInstrument=JSON.stringify(ST.state.project.instruments);return true;")
            try await key("ArrowRight")
            try await check("document.activeElement.id==='view-browser'&&ST.state.view==='instrument'", "View arrows changed workspaces before activation")
            _ = try await js("document.getElementById('instrument-type-sample').focus();return true;")
            try await key("ArrowRight")
            try await check("document.activeElement.id==='instrument-type-au'&&JSON.stringify(ST.state.project.instruments)===window.__navInstrument", "Instrument type arrows changed the instrument")
            _ = try await js("const keys=Array.from(document.querySelectorAll('[data-piano]')).sort((a,b)=>Number(a.dataset.piano)-Number(b.dataset.piano));window.__navFirstNote=Number(keys[0].dataset.piano);keys[0].focus();return true;")
            try await key("ArrowRight")
            try await check("Number(document.activeElement.dataset.piano)===window.__navFirstNote+1&&JSON.stringify(ST.state.project.instruments)===window.__navInstrument", "Piano arrows did not navigate chromatically without changing the instrument")
            _ = try await js("document.getElementById('sidebar-instrument-0').focus();return true;")
            try await key("ArrowDown")
            try await check("ST.state.instrument===1&&document.activeElement.id==='sidebar-instrument-1'", "Instrument sidebar arrows did not preserve their focus")
            try await key("ArrowRight")
            try await check("document.activeElement.id==='instrument-rename'", "Sidebar Right did not enter the instrument workspace")

            _ = try await js("ST.setView('song');window.__navSequence=JSON.stringify(ST.state.project.sequence);document.getElementById('sidebar-pattern-0').focus();return true;")
            try await key("ArrowDown")
            try await check("ST.state.project.activePattern===1&&ST.state.sequence===0&&document.activeElement.id==='sidebar-pattern-1'&&JSON.stringify(ST.state.project.sequence)===window.__navSequence", "Pattern sidebar arrows edited song sequence")
            try await key("ArrowRight")
            try await check("document.activeElement.id==='sequence-list'", "Pattern sidebar Right did not enter sequence")
            try await key("ArrowDown")
            try await check("ST.state.sequence===1", "Sequence arrows did not select the next position")
            _ = try await js("ST.state.clipSelected=ST.state.project.audioClips[0].id;ST.refresh();window.__navClips=JSON.stringify(ST.state.project.audioClips);document.getElementById('clips-preview').focus();return true;")
            for key in ["ArrowRight","ArrowUp","Delete","s"] { try await self.key(key) }
            try await key("d",code:"KeyD",meta:true)
            try await check("JSON.stringify(ST.state.project.audioClips)===window.__navClips&&JSON.stringify(ST.state.project.sequence)===window.__navSequence&&document.activeElement.id==='clips-preview'", "Clip action-button keys changed the arrangement or stole focus")
            _ = try await js("document.getElementById('clip-navigation-clip').focus();return true;")
            try await key("ArrowRight")
            try await check("ST.state.project.audioClips.find(x=>x.id==='navigation-clip').startBeat===8.25&&ST.state.project.audioClips[0].startBeat===2&&ST.state.clipSelected==='navigation-clip'", "Clip keyboard edit did not target the focused clip")
            try await key("z",code:"KeyZ",meta:true)
            try await check("JSON.stringify(ST.state.project.audioClips)===window.__navClips", "Undo did not restore the focused clip nudge")
            _ = try await js("document.getElementById('clips-timeline').focus();return true;")
            try await key("End")
            try await check("ST.state.clipSelected==='navigation-clip'&&document.activeElement.id==='clip-navigation-clip'", "Timeline End did not select and focus its last clip")

            _ = try await js("ST.state.track=0;ST.state.instrument=0;ST.setView('mixer');document.getElementById('mix-pan-0').focus();window.__navMix=JSON.stringify(ST.state.project.tracks);return true;")
            try await key("ArrowRight")
            try await wait("ST.state.track===1&&document.activeElement.id==='mix-pan-1'&&JSON.stringify(ST.state.project.tracks)===window.__navMix", "Mixer Right failed to preserve the focused control role")
            try await check("(()=>{const e=new KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true,cancelable:true});document.activeElement.dispatchEvent(e);return !e.defaultPrevented;})()", "Mixer vertical range keys must retain their native default action")
            for index in 2...7 {
                try await key("ArrowRight")
                try await wait("document.activeElement.id==='mix-pan-\(index)'", "Mixer focus did not finish moving to track \(index + 1)")
            }
            try await check("ST.state.track===7&&document.activeElement.id==='mix-pan-7'", "Mixer arrows did not reach the final track")
            try await wait("(()=>{const r=document.activeElement.getBoundingClientRect(),c=document.activeElement.closest('[data-mixer-channel]').getBoundingClientRect(),p=document.getElementById('mixer-channels').getBoundingClientRect(),m=document.querySelector('.studio-master').getBoundingClientRect();return c.left>=p.left&&c.right<=m.left+1&&r.right<=m.left&&r.top>=0&&r.bottom<=innerHeight;})()", "Final mixer channel is clipped or hidden behind the sticky master")
            try await snapshot("keyboard-mixer-final-track-1080x700.png")
            try await key("ArrowRight")
            try await wait("document.activeElement.id==='mix-master'", "Mixer Right did not reach master")
            try await key("ArrowLeft")
            try await wait("document.activeElement.id==='mix-pan-7'", "Master Left did not restore the prior track control role")
            _ = try await js("document.querySelector('[data-studio-action=add-plugin]').click();return true;")
            try await wait("document.getElementById('studio-plugin-search')!==null", "Effect chooser did not open")
            try await key("Escape")
            try await wait("!document.getElementById('studio-plugin-search')&&document.activeElement.id==='mix-slots-7'", "Escape did not dismiss only the local effect chooser and return focus")

            _ = try await js("ST.setView('browser');await STBrowser.search();ST.state.library.selected=0;document.getElementById('browser-audition').focus();return true;")
            try await key("ArrowDown")
            try await check("ST.state.library.selected===0&&document.activeElement.id==='browser-audition'", "Browser checkbox arrow moved the result selection")
            _ = try await js("document.getElementById('browser-kind-all').focus();return true;")
            try await key("ArrowRight")
            try await wait("ST.state.library.kind==='sample'&&document.activeElement.id==='browser-kind-sample'&&ST.state.library.total===4", "Browser kind arrow did not activate the adjacent filter and preserve focus")
            try await key("Home")
            try await wait("ST.state.library.kind==='all'&&document.activeElement.id==='browser-kind-all'", "Browser kind Home did not restore all sounds")
            try await key("f",code:"KeyF",meta:true)
            try await key("ArrowDown")
            try await check("document.activeElement.id==='browser-results-list'", "Browser search Down did not enter results")

            _ = try await js("ST.state.track=0;ST.state.instrument=0;ST.setView('instrument');return true;")
            try await instrumentPage("slices")
            _ = try await js("document.getElementById('sampler-transient-sensitivity').focus();return true;")
            try await wait("(()=>{const r=document.activeElement.getBoundingClientRect(),m=document.querySelector('main').getBoundingClientRect();return r.width>0&&r.left>=m.left&&r.right<=m.right&&r.top>=m.top&&r.bottom<=m.bottom;})()", "Sampler slice controls cannot be revealed by keyboard focus at1080×700")
            let sampler = try await layout([".sampler-slices", "#sampler-transient-sensitivity", "#sampler-detect-transients"])
            try await snapshot("keyboard-sampler-slices-1080x700.png")
            let recording = try await recordingLayouts()
            let settings = try await settingsLayout()
            evidence["workspaceNavigation"] = ["entries":entry,"mixerRolePreserved":true,"mixerLastTrackVisible":true,"rangeDefaultPreserved":true,"sidebarConfined":true,"clipControlsConfined":true,"focusedClipEditing":true,"browserControlsConfined":true,"localEscape":true,"sampler":sampler,"recording":recording,"recordingDevicesReadOnly":true,"settings":settings,"settingsDevicesReadOnly":true]
        } catch {
            _ = try? await js("if(window.__recordGeometryHandler&&window.__recordGeometryPost)window.__recordGeometryHandler.postMessage=window.__recordGeometryPost;ST.state.project=window.__navBackup.project;ST.state.keyboardPlay=window.__navBackup.keyboardPlay;ST.state.undo=window.__navBackup.undo;ST.state.redo=window.__navBackup.redo;ST.state.track=0;ST.state.instrument=0;ST.state.sequence=0;await ST.syncEngine(true,true);return true;")
            throw error
        }
        _ = try await js("ST.state.project=window.__navBackup.project;ST.state.keyboardPlay=window.__navBackup.keyboardPlay;ST.state.undo=window.__navBackup.undo;ST.state.redo=window.__navBackup.redo;ST.state.track=0;ST.state.instrument=0;ST.state.sequence=0;ST.state.project.activePattern=0;await ST.syncEngine(true,true);return true;")
    }

    /// Flush setup edits and wait for pending source hydration/advisory freeze reads
    /// before the dialog-only observation window. Status events do not reset this.
    private func settleLayoutRequests() async throws {
        _ = try await js("""
        await ST.waitForEngineUpdate();
        const handler=window.webkit.messageHandlers.supertracker,original=handler.postMessage,receive=window.nativeReceive;
        const autosaves=new Set();let last=performance.now(),browserSaveObserved=false,autosaveCompleted=false,autosaveError=null;
        handler.postMessage=function(message){
            last=performance.now();if(message.command==='library.state.set')browserSaveObserved=true;
            if(message.command==='project.autosave')autosaves.add(message.id);
            return original.call(handler,message);
        };
        window.nativeReceive=function(message){
            const reply=typeof message==='string'?JSON.parse(message):message;
            if(autosaves.has(reply.id)){
                last=performance.now();autosaves.delete(reply.id);
                if(!reply.ok)autosaveError=reply.error||'Autosave failed';
                else if(reply.result?.deferred!==true)autosaveCompleted=true;
            }
            return receive.call(window,message);
        };
        try{
            // Drain browser persistence and the private fixture's pending autosave
            // before the strict read-only guard. Autosave has no public flush API:
            // a no-op edit replaces its timer without changing project data, and
            // its temporary history/dirty bookkeeping is restored immediately.
            STBrowser.saveState(0);
            const before=JSON.stringify(ST.state.project),history={undo:ST.state.undo,redo:ST.state.redo,dirty:ST.state.dirty};
            ST.state.undo=history.undo.slice();ST.state.redo=history.redo.slice();
            try{if(!ST.mutate('Settle private acceptance autosave',()=>{},{render:false}))throw Error('Could not schedule layout fixture autosave');}
            finally{ST.state.undo=history.undo;ST.state.redo=history.redo;ST.state.dirty=history.dirty;}
            if(JSON.stringify(ST.state.project)!==before)throw Error('Layout settling changed the fixture project');
            const deadline=performance.now()+10000;
            while(!browserSaveObserved||!autosaveCompleted||ST.state.pending.size||performance.now()-last<550){
                if(autosaveError)throw Error('Layout setup autosave failed: '+autosaveError);
                if(performance.now()>deadline)throw Error('Layout setup did not settle: '+JSON.stringify({browserSaveObserved,autosaveCompleted,pending:Array.from(ST.state.pending.values()).map(x=>x.command)}));
                await new Promise(resolve=>setTimeout(resolve,25));
            }
        }finally{handler.postMessage=original;window.nativeReceive=receive;}
        return true;
        """)
    }

    /// Observe the real boundary, including app.js lexical request() calls. Only
    /// exact read-only payloads reach native while a dialog is under inspection.
    private func installLayoutReadGuard(_ name: String, devices: [String]) async throws {
        _ = try await js("""
        const handler=window.webkit.messageHandlers.supertracker;
        window[name+'Commands']=[];window[name+'Handler']=handler;window[name+'Post']=handler.postMessage;
        window[name+'Project']=JSON.stringify(ST.state.project);
        handler.postMessage=function(message){
            const payload=message.payload||{},keys=Object.keys(payload).sort(),command=message.command;
            const metadata=command==='sample.info'&&JSON.stringify(keys)===JSON.stringify(['instrument','path'])&&
                Number.isInteger(payload.instrument)&&ST.state.project.instruments[payload.instrument]?.type==='sample'&&
                typeof payload.path==='string'&&payload.path.length>0&&ST.state.project.instruments[payload.instrument].samplePath===payload.path;
            const allowed=(devices.includes(command)&&keys.length===0)||(command==='freeze.status'&&keys.length===0)||metadata;
            window[name+'Commands'].push({command,payload:JSON.parse(JSON.stringify(payload)),allowed});
            if(!allowed){window.nativeReceive({id:message.id,ok:false,error:'Read-only layout acceptance blocked '+command});return;}
            return window[name+'Post'].call(handler,message);
        };
        return true;
        """, ["name":name,"devices":devices])
    }

    private func recordingLayouts() async throws -> Any {
        let originalSize = web.frame.size
        try await settleLayoutRequests()
        try await installLayoutReadGuard("__recordGeometry", devices:["audio.devices"])
        _ = try await js("""
        window.__recordGeometryRead=()=>{
            const modal=document.querySelector('.recording-modal'),body=modal.querySelector('.modal-body'),head=modal.querySelector('.modal-head'),foot=modal.querySelector('.modal-foot');
            const rect=e=>{const r=e.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,height:r.height};};
            const inside=(r,b)=>r.width>0&&r.height>0&&r.left>=b.left-1&&r.right<=b.right+1&&r.top>=b.top-1&&r.bottom<=b.bottom+1;
            const m=rect(modal),b=rect(body),h=rect(head),f=rect(foot),viewport={left:0,right:innerWidth,top:0,bottom:innerHeight};
            const bodyClient={left:b.left+body.clientLeft,top:b.top+body.clientTop,right:b.left+body.clientLeft+body.clientWidth,bottom:b.top+body.clientTop+body.clientHeight};
            const controls=Object.fromEntries(['record-name','record-root-note','record-monitor','record-start','record-close','record-status','record-duration'].map(id=>{
                const e=document.getElementById(id),r=rect(e),area=e.closest('.modal-foot')?f:bodyClient;
                const x=(r.left+r.right)/2,y=(r.top+r.bottom)/2,dx=Math.min(2,r.width/4),dy=Math.min(2,r.height/4);
                const hits=[[x,y],[r.left+dx,y],[r.right-dx,y],[x,r.top+dy],[x,r.bottom-dy]].map(([x,y])=>{
                    const hit=document.elementFromPoint(x,y);return {x,y,id:hit?.id||null,tag:hit?.tagName||null,hitsControl:!!hit&&(hit===e||e.contains(hit))};
                });
                return [id,{rect:r,contained:inside(r,area)&&inside(r,m)&&inside(r,viewport),hittable:hits.every(hit=>hit.hitsControl),hits,focused:document.activeElement===e}];
            }));
            return {width:innerWidth,height:innerHeight,modal:m,head:h,body:b,bodyClient,footer:f,controls,
                modalContained:inside(m,viewport),footerContained:inside(f,m)&&inside(f,viewport),footerClearOfBody:f.top>=b.bottom-1,
                bodyScrollTop:body.scrollTop,bodyClientHeight:body.clientHeight,bodyScrollHeight:body.scrollHeight,
                bodyScrollRange:Math.max(0,body.scrollHeight-body.clientHeight),modalScrollTop:modal.scrollTop,
                inputValues:{name:document.getElementById('record-name').value,root:document.getElementById('record-root-note').value,monitor:document.getElementById('record-monitor').checked}};
        };
        window.__recordGeometryVisible=(ids)=>{const g=window.__recordGeometryRead();return g.modalContained&&g.footerContained&&g.footerClearOfBody&&['record-status','record-duration'].every(id=>document.getElementById(id).closest('.modal-foot'))&&[...ids,'record-status','record-duration'].every(id=>g.controls[id].contained&&g.controls[id].hittable);};
        return true;
        """)
        var sizes: [[String: Any]] = []
        do {
            for (width,height) in [(1080,700),(1500,940)] {
                web.setFrameSize(NSSize(width:width,height:height))
                try await wait("innerWidth===\(width)&&innerHeight===\(height)", "Recording viewport did not resize")
                _ = try await js("ST.showRecording();return true;")
                try await wait("ST.state.modal==='recording'&&document.getElementById('record-root-note')&&!document.getElementById('record-start').disabled&&document.activeElement.id==='record-name'", "Recording dialog did not finish its read-only device query and initial focus")
                // Capture before asserting so the old clipped-footer CSS leaves useful failure evidence.
                try await snapshot("keyboard-recording-dialog-\(width)x\(height).png")
                try require(snapshots.last?["available"] as? Bool == true, "Recording initial-layout snapshot was unavailable")
                let initial = try await js("window.__recordGeometryInitial=window.__recordGeometryRead();return window.__recordGeometryInitial;")
                evidence["recordingLayoutInProgress"] = ["initial":initial]
                try await check("window.__recordGeometryVisible(['record-name','record-root-note','record-start','record-close'])&&window.__recordGeometryInitial.bodyScrollTop<=1", "Initial recording name/root or footer is clipped or occluded at \(width)×\(height)")
                if height == 700 {
                    try await check("window.__recordGeometryRead().bodyScrollRange>1", "The compact recorder fixture no longer exercises body overflow")
                }
                _ = try await js("const body=document.querySelector('.recording-modal .modal-body');body.scrollTop=body.scrollHeight;return true;")
                try await wait("(()=>{const g=window.__recordGeometryRead(),i=window.__recordGeometryInitial;return g.bodyScrollTop>=g.bodyScrollRange-1&&g.modalScrollTop===0&&Math.abs(g.footer.top-i.footer.top)<=1&&Math.abs(g.footer.bottom-i.footer.bottom)<=1&&window.__recordGeometryVisible(['record-start','record-close']);})()", "Scrolling the recording body moved, clipped, or occluded its footer at \(width)×\(height)")
                let scrolled = try await js("return window.__recordGeometryRead();")
                _ = try await js("document.getElementById('record-name').focus();document.querySelector('.recording-modal .modal-body').scrollTop=0;return true;")
                try await wait("window.__recordGeometryRead().bodyScrollTop<=1", "Recording body did not return to the top before the focus-reveal check")
                // Programmatic focus uses WebKit's actual scroll behavior. No scrollIntoView,
                // activation, or synthetic Tab default is used or claimed here.
                _ = try await js("document.getElementById('record-monitor').focus();return true;")
                try await wait("(()=>{const g=window.__recordGeometryRead(),i=window.__recordGeometryInitial;return g.controls['record-monitor'].focused&&window.__recordGeometryVisible(['record-monitor','record-start','record-close'])&&Math.abs(g.footer.top-i.footer.top)<=1&&Math.abs(g.footer.bottom-i.footer.bottom)<=1&&(g.bodyScrollRange<=1||g.bodyScrollTop>0);})()", "Focusing the lower recording option did not reveal it while keeping the footer visible at \(width)×\(height)")
                let focused = try await js("return window.__recordGeometryRead();")
                try await snapshot("keyboard-recording-lower-focus-\(width)x\(height).png")
                try require(snapshots.last?["available"] as? Bool == true, "Recording lower-focus snapshot was unavailable")
                _ = try await js("document.getElementById('record-name').focus();return true;")
                try await wait("window.__recordGeometryVisible(['record-name','record-root-note','record-start','record-close'])", "Returning focus to the recording name did not reveal the upper fields at \(width)×\(height)")
                try await check("JSON.stringify(window.__recordGeometryRead().inputValues)===JSON.stringify(window.__recordGeometryInitial.inputValues)&&!STRecording.isBusy&&!STRecording.isRecording", "Recording layout/focus navigation changed capture settings or activated recording")
                sizes.append(["width":width,"height":height,"initial":initial,"bodyScrolled":scrolled,"lowerOptionFocused":focused,"footerHitTestsPassed":true])
                evidence["recordingLayouts"] = sizes
                _ = try await js("document.getElementById('record-close').click();return true;")
                try await wait("!ST.state.modal", "Recording geometry check did not close its dialog")
            }
            evidence["recordingLayoutCommands"] = try await js("return window.__recordGeometryCommands;")
            try await check("window.__recordGeometryCommands.filter(x=>x.command==='audio.devices').length===2&&window.__recordGeometryCommands.every(x=>x.allowed)&&JSON.stringify(ST.state.project)===window.__recordGeometryProject", "Recording layout checks issued an invalid read payload, a write, or changed the project")
            _ = try await js("window.__recordGeometryHandler.postMessage=window.__recordGeometryPost;return true;")
        } catch {
            evidence["recordingLayoutCommands"] = try? await js("return window.__recordGeometryCommands;")
            _ = try? await js("window.__recordGeometryHandler.postMessage=window.__recordGeometryPost;return true;")
            web.setFrameSize(originalSize)
            throw error
        }
        web.setFrameSize(originalSize)
        evidence.removeValue(forKey:"recordingLayoutInProgress")
        return ["sizes":sizes,"nativeCommands":evidence["recordingLayoutCommands"] ?? [],"focusMethod":"Programmatic DOM focus and native WebKit scroll; no physical Tab, checkbox activation, microphone access, or device changes."]
    }

    private func settingsLayout() async throws -> Any {
        let originalSize = web.frame.size
        web.setFrameSize(NSSize(width:1080,height:700))
        try await wait("innerWidth===1080&&innerHeight===700", "Studio settings viewport did not resize")
        try await settleLayoutRequests()
        try await installLayoutReadGuard("__settingsGeometry", devices:["audio.devices","midi.devices","midi.clock","agent.status"])
        _ = try await js("""
        window.__settingsGeometryRead=()=>{
            const modal=document.querySelector('.studio-settings-modal'),body=modal.querySelector('.modal-body'),foot=modal.querySelector('.modal-foot');
            const rect=e=>{const r=e.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,height:r.height};};
            const inside=(r,b)=>r.width>0&&r.height>0&&r.left>=b.left-1&&r.right<=b.right+1&&r.top>=b.top-1&&r.bottom<=b.bottom+1;
            const m=rect(modal),b=rect(body),f=rect(foot),viewport={left:0,right:innerWidth,top:0,bottom:innerHeight};
            const bodyClient={left:b.left+body.clientLeft,top:b.top+body.clientTop,right:b.left+body.clientLeft+body.clientWidth,bottom:b.top+body.clientTop+body.clientHeight};
            const controls=Object.fromEntries(['audio-output','midi-enabled','settings-apply','settings-cancel'].map(id=>{
                const e=document.getElementById(id),r=rect(e),area=e.closest('.modal-foot')?f:bodyClient;
                const x=(r.left+r.right)/2,y=(r.top+r.bottom)/2,dx=Math.min(2,r.width/4),dy=Math.min(2,r.height/4);
                const hits=[[x,y],[r.left+dx,y],[r.right-dx,y],[x,r.top+dy],[x,r.bottom-dy]].map(([x,y])=>{
                    const hit=document.elementFromPoint(x,y);return {x,y,id:hit?.id||null,tag:hit?.tagName||null,hitsControl:!!hit&&(hit===e||e.contains(hit))};
                });
                return [id,{rect:r,contained:inside(r,area)&&inside(r,m)&&inside(r,viewport),hittable:hits.every(hit=>hit.hitsControl),hits,focused:document.activeElement===e}];
            }));
            return {width:innerWidth,height:innerHeight,modal:m,body:b,bodyClient,footer:f,controls,
                modalContained:inside(m,viewport),footerContained:inside(f,m)&&inside(f,viewport),footerClearOfBody:f.top>=b.bottom-1,
                bodyScrollTop:body.scrollTop,bodyScrollRange:Math.max(0,body.scrollHeight-body.clientHeight),modalScrollTop:modal.scrollTop,
                inputValues:Array.from(modal.querySelectorAll('input,select')).map(e=>({id:e.id,value:e.value,checked:e.checked??null}))};
        };
        window.__settingsGeometryVisible=ids=>{const g=window.__settingsGeometryRead();return g.modalContained&&g.footerContained&&g.footerClearOfBody&&ids.every(id=>g.controls[id].contained&&g.controls[id].hittable);};
        ST.perform('settings');return true;
        """)
        do {
            try await wait("ST.state.modal==='settings'&&document.querySelector('.studio-settings-modal')&&document.getElementById('settings-apply')&&document.getElementById('settings-cancel')&&document.activeElement.id==='audio-output'", "Studio settings did not open after its read-only device queries")
            try await snapshot("keyboard-settings-dialog-1080x700.png")
            try require(snapshots.last?["available"] as? Bool == true, "Studio settings initial-layout snapshot was unavailable")
            let initial = try await js("window.__settingsGeometryInitial=window.__settingsGeometryRead();return window.__settingsGeometryInitial;")
            evidence["settingsLayoutInProgress"] = ["initial":initial]
            try await check("window.__settingsGeometryVisible(['audio-output','settings-apply','settings-cancel'])&&window.__settingsGeometryInitial.bodyScrollTop<=1", "Initial Studio settings fields or footer are clipped or occluded at 1080×700")
            // Focus only: neither checkbox activation nor native Tab defaults are simulated.
            _ = try await js("document.getElementById('midi-enabled').focus();return true;")
            try await wait("(()=>{const g=window.__settingsGeometryRead(),i=window.__settingsGeometryInitial;return g.controls['midi-enabled'].focused&&window.__settingsGeometryVisible(['midi-enabled','settings-apply','settings-cancel'])&&g.modalScrollTop===0&&Math.abs(g.footer.top-i.footer.top)<=1&&Math.abs(g.footer.bottom-i.footer.bottom)<=1&&(i.controls['midi-enabled'].contained||g.bodyScrollRange<=1||g.bodyScrollTop>0);})()", "Lower MIDI focus did not reveal the settings control while keeping the footer visible")
            let focused = try await js("return window.__settingsGeometryRead();")
            try await snapshot("keyboard-settings-midi-focus-1080x700.png")
            try require(snapshots.last?["available"] as? Bool == true, "Studio settings MIDI-focus snapshot was unavailable")
            try await check("JSON.stringify(window.__settingsGeometryRead().inputValues)===JSON.stringify(window.__settingsGeometryInitial.inputValues)", "Studio settings focus changed a configuration control")
            _ = try await js("document.getElementById('settings-cancel').click();return true;")
            try await wait("!ST.state.modal", "Cancel did not close Studio settings")
            evidence["settingsLayoutCommands"] = try await js("return window.__settingsGeometryCommands;")
            try await check("JSON.stringify(window.__settingsGeometryCommands.filter(x=>['agent.status','audio.devices','midi.devices'].includes(x.command)).map(x=>x.command).sort())===JSON.stringify(['agent.status','audio.devices','midi.devices'])&&window.__settingsGeometryCommands.every(x=>x.allowed)&&JSON.stringify(ST.state.project)===window.__settingsGeometryProject", "Studio settings open/focus/Cancel wrote configuration, issued an invalid read payload, or changed the project")
            _ = try await js("window.__settingsGeometryHandler.postMessage=window.__settingsGeometryPost;return true;")
            let result: [String: Any] = ["initial":initial,"lowerOptionFocused":focused,"footerHitTestsPassed":true,"nativeCommands":evidence["settingsLayoutCommands"] ?? [],"focusMethod":"Programmatic DOM focus and native WebKit scroll; Cancel only, without configuration writes or checkbox activation."]
            evidence["settingsLayout"] = result
            evidence.removeValue(forKey:"settingsLayoutInProgress")
            web.setFrameSize(originalSize)
            return result
        } catch {
            evidence["settingsLayoutCommands"] = try? await js("return window.__settingsGeometryCommands;")
            _ = try? await js("window.__settingsGeometryHandler.postMessage=window.__settingsGeometryPost;return true;")
            web.setFrameSize(originalSize)
            throw error
        }
    }

    private func modulationKeyboard() async throws {
        _ = try await js("window.__modBackup={project:window.__supertracker.getProject(),keyboardPlay:ST.state.keyboardPlay,undo:ST.state.undo,redo:ST.state.redo};ST.state.keyboardPlay=false;return true;")
        let restore = "ST.state.project=window.__modBackup.project;ST.state.keyboardPlay=window.__modBackup.keyboardPlay;ST.state.undo=window.__modBackup.undo;ST.state.redo=window.__modBackup.redo;ST.state.track=0;ST.state.instrument=0;await ST.syncEngine(true,true);return true;"
        do {
            var sizes: [[String: Any]] = []
            for (width, height) in [(1080,700),(1500,940)] {
                web.setFrameSize(NSSize(width:width,height:height))
                try await wait("innerWidth===\(width)&&innerHeight===\(height)", "Modulation viewport did not resize")
                _ = try await js("""
                const p=window.__supertracker.getProject();p.masterVolume=0;
                Object.assign(p.instruments[0],{lfoRate:3.25,lfoSync:false,lfoBeats:1.5,lfoDepth:.4,lfo2Rate:7.5,lfo2Sync:false,lfo2Beats:4,lfo2Depth:.2});
                ST.state.project=ST.normalize(p);ST.state.track=0;ST.state.instrument=0;ST.state.undo=[];ST.state.redo=[];
                ST.setView('instrument');return true;
                """)
                try await instrumentPage("modulation")
                _ = try await js("document.getElementById('mod-0-lfoTiming').focus();return true;")
                try await check("(()=>{const e=new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true,cancelable:true});document.activeElement.dispatchEvent(e);return !e.defaultPrevented&&document.activeElement.id==='mod-0-lfoTiming'&&!ST.state.project.instruments[0].lfoSync;})()", "LFO select navigation was intercepted by a workspace shortcut")
                // An untrusted KeyboardEvent cannot select a native option. Drive the
                // actual change listener separately, without claiming an OS key test.
                _ = try await js("const select=document.getElementById('mod-0-lfoTiming');select.value='sync';select.dispatchEvent(new Event('change',{bubbles:true}));return true;")
                try await check("ST.state.project.instruments[0].lfoSync&&ST.state.project.instruments[0].lfoRate===3.25&&ST.state.project.instruments[0].lfoBeats===1.5&&document.activeElement.id==='mod-0-lfoTiming'&&document.getElementById('mod-0-lfoDivision').value==='dotted-4'", "Switching LFO timing lost its stored Hz/beat values or selector focus")
                try await wait("(()=>{const r=document.activeElement.getBoundingClientRect(),m=document.querySelector('main').getBoundingClientRect();return r.top>=m.top&&r.bottom<=m.bottom;})()", "LFO timing focus became hidden after changing mode")
                _ = try await js("const select=document.getElementById('mod-0-lfoDivision');select.focus();select.value='triplet-4';select.dispatchEvent(new Event('change',{bubbles:true}));return true;")
                try await check("ST.state.project.instruments[0].lfoBeats===2/3&&document.activeElement.id==='mod-0-lfoDivision'&&document.getElementById('mod-number-0-lfoBeats').valueAsNumber===2/3&&document.getElementById('mod-number-0-lfoBeats').validity.valid", "Triplet selection rounded the stored value, invalidated the input, or lost focus")
                _ = try await js("document.getElementById('mod-number-0-lfoBeats').focus();return true;")
                try await key("ArrowUp")
                try await check("ST.state.project.instruments[0].lfoBeats===2/3+1/64&&document.activeElement.id==='mod-number-0-lfoBeats'&&document.getElementById('mod-0-lfoDivision').value==='custom'", "Beat Up did not apply an exact 1/64 increment with stable focus")
                try await key("ArrowUp",shift:true)
                try await check("ST.state.project.instruments[0].lfoBeats===2/3+1/64+1&&document.activeElement.id==='mod-number-0-lfoBeats'", "Shift-Up did not apply an exact one-beat increment")
                // Cmd-Z belongs to the text editor while a numeric field is focused.
                // Focus a button to exercise the application's project Undo shortcut.
                _ = try await js("document.getElementById('instrument-rename').focus();return true;")
                try await key("z",code:"KeyZ",meta:true)
                try await check("ST.state.project.instruments[0].lfoBeats===2/3+1/64", "Project Undo did not restore the first exact beat increment")
                try await key("z",code:"KeyZ",meta:true)
                try await check("ST.state.project.instruments[0].lfoBeats===2/3", "Project Undo did not restore the exact triplet")
                _ = try await js("const select=document.getElementById('mod-0-lfo2Timing');select.focus();select.value='sync';select.dispatchEvent(new Event('change',{bubbles:true}));const division=document.getElementById('mod-0-lfo2Division');division.focus();division.value='dotted-8';division.dispatchEvent(new Event('change',{bubbles:true}));document.getElementById('mod-number-0-lfo2Beats').focus();return true;")
                try await check("ST.state.project.instruments[0].lfo2Sync&&ST.state.project.instruments[0].lfo2Beats===.75&&ST.state.project.instruments[0].lfo2Rate===7.5&&ST.state.project.instruments[0].lfoSync&&ST.state.project.instruments[0].lfoBeats===2/3&&document.activeElement.id==='mod-number-0-lfo2Beats'", "LFO 2 timing changed LFO 1 or lost independent Hz state")
                try await wait("(()=>{const r=document.activeElement.getBoundingClientRect(),m=document.querySelector('main').getBoundingClientRect();return r.left>=m.left&&r.right<=m.right&&r.top>=m.top&&r.bottom<=m.bottom;})()", "Synced LFO 2 cycle cannot be revealed by focus")
                let geometry = try await layout([".modulation-console", "#mod-0-lfoTiming", "#mod-0-lfoDivision", "#mod-number-0-lfoBeats", "#mod-0-lfo2Timing", "#mod-0-lfo2Division", "#mod-number-0-lfo2Beats"])
                try await snapshot("keyboard-lfo-sync-\(width)x\(height).png")
                try await key("a",code:"KeyA",alt:true)
                try await check("ST.state.view==='automation'&&ST.pattern().automation.some(x=>x.target==='lfo2Beats'&&x.track===0&&x.points[0].value===.75)", "Option-A did not create a beat-cycle lane with the exact current value")
                _ = try await js("ST.setView('instrument');const select=document.getElementById('mod-0-lfoTiming');select.focus();select.value='hz';select.dispatchEvent(new Event('change',{bubbles:true}));await ST.syncEngine(true,true);window.__modNative=await ST.request('engine.snapshot');return true;")
                try await check("!ST.state.project.instruments[0].lfoSync&&ST.state.project.instruments[0].lfoRate===3.25&&ST.state.project.instruments[0].lfoBeats===2/3&&document.activeElement.id==='mod-0-lfoTiming'&&document.getElementById('mod-number-0-lfoRate').valueAsNumber===3.25&&window.__modNative.instruments[0].lfo2Sync===true&&window.__modNative.instruments[0].lfo2Beats===.75", "Returning to Hz lost the separate beat state or native synchronization")
                sizes.append(["width":width,"height":height,"geometry":geometry,"tripletExact":true,"independentSecondLFO":true,"focusPreserved":true,"beatAutomation":true,"nativeProjectSynchronized":true])
            }
            _ = try await js("ST.setView('song');document.getElementById('loop-song').focus();window.__modLoop=ST.state.project.loopSong;window.__modPlaying=ST.state.playing;return true;")
            try await check("(()=>{const event=new KeyboardEvent('keydown',{key:' ',code:'Space',bubbles:true,cancelable:true});document.activeElement.dispatchEvent(event);return !event.defaultPrevented&&document.activeElement.id==='loop-song'&&ST.state.project.loopSong===window.__modLoop&&ST.state.playing===window.__modPlaying;})()", "Space on a checkbox was intercepted or changed the transport")
            _ = try await js("document.getElementById('loop-song').click();return true;")
            try await check("ST.state.project.loopSong===!window.__modLoop&&document.getElementById('loop-song').checked===!window.__modLoop&&document.activeElement.id==='loop-song'", "Programmatic checkbox activation did not reach the mounted change handler with stable focus")
            evidence["modulationKeyboard"] = ["viewports":sizes,"selectArrowDefaultNotPrevented":true,"checkboxSpaceDefaultNotPrevented":true,"checkboxProgrammaticActivation":true,"nativeKeyboardDefaultsTested":false]
        } catch { _ = try? await js(restore); throw error }
        _ = try await js(restore)
    }

    private func layout(_ selectors: [String]) async throws -> Any {
        let result = try await js("return {width:innerWidth,height:innerHeight,documentWidth:document.documentElement.scrollWidth,rects:selectors.map(selector=>{const e=document.querySelector(selector),r=e?.getBoundingClientRect();return {selector,visible:!!r&&r.width>0&&r.height>0,left:r?.left,right:r?.right,top:r?.top,bottom:r?.bottom};})};", ["selectors":selectors]) as! [String: Any]
        let width = result["width"] as! Double
        try require((result["documentWidth"] as! Double) <= width + 1, "The document overflows its supported viewport horizontally.")
        for rect in result["rects"] as! [[String: Any]] {
            try require(rect["visible"] as? Bool == true && (rect["left"] as! Double) >= -1 && (rect["right"] as! Double) <= width+1, "Control outside supported viewport: \(rect)")
        }
        return result
    }

    private func snapshot(_ name: String) async throws {
        do {
            let config = WKSnapshotConfiguration(); config.afterScreenUpdates = true
            let image = try await web.takeSnapshot(configuration: config)
            guard let tiff = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data:tiff), let png = bitmap.representation(using:.png,properties:[:]) else { throw TrackerError.message("Snapshot image conversion failed.") }
            try png.write(to: output.appendingPathComponent(name), options:.atomic)
            snapshots.append(["file":name,"available":true,"width":bitmap.pixelsWide,"height":bitmap.pixelsHigh])
        } catch { snapshots.append(["file":name,"available":false,"reason":error.localizedDescription]) }
    }

    private func automationTransport() async throws {
        _ = try await js("ST.mutate('silent acceptance transport',p=>{p.masterVolume=0;p.activePattern=0;p.loopSong=false;},{render:false});await ST.syncEngineProject(()=>window.__supertracker.getProject());ST.setView('automation');const input=document.getElementById('point-value-0');input.focus();window.__acceptanceEditor=input;window.__acceptanceEnvelope=document.getElementById('automation-envelope');window.__acceptancePoints=JSON.stringify(ST.pattern().automation[0].points);await ST.request('transport.play',{mode:'song',fromStart:true});return true;")
        try await wait("ST.state.status.playing&&ST.state.status.pattern===0&&ST.state.status.row>=2&&!document.getElementById('automation-playhead').hidden", "Native transport did not move the viewed automation playhead")
        let first = try await js("return {status:ST.state.status,left:document.getElementById('automation-playhead').style.left,label:document.getElementById('automation-position').textContent};")
        _ = try await js("window.__acceptancePosition=ST.state.status.row+(ST.state.status.fraction??ST.state.status.rowFraction??0);return true;")
        try await wait("ST.state.status.pattern===0&&(ST.state.status.row+(ST.state.status.fraction??ST.state.status.rowFraction??0))>window.__acceptancePosition+0.4", "Automation playhead did not advance on later native ticks")
        try await check("document.activeElement===window.__acceptanceEditor&&document.getElementById('automation-envelope')===window.__acceptanceEnvelope&&JSON.stringify(ST.pattern().automation[0].points)===window.__acceptancePoints", "Native playhead tick replaced editor focus, envelope node, or automation points")
        try await check("Math.abs(parseFloat(document.getElementById('automation-playhead').style.left)-(96+(ST.state.status.position??(ST.state.status.row+(ST.state.status.fraction??ST.state.status.rowFraction??0)))/ST.pattern().length*780)/9)<0.01", "Playhead position disagrees with native fractional row")
        let next = try await js("return {status:ST.state.status,left:document.getElementById('automation-playhead').style.left,label:document.getElementById('automation-position').textContent};")
        try await wait("ST.state.status.playing&&ST.state.status.pattern===1&&document.getElementById('automation-transport').dataset.mode==='other-pattern'&&document.getElementById('automation-playhead').hidden", "Following song pattern displayed a misleading playhead on the viewed pattern")
        let other = try await js("return document.getElementById('automation-position').textContent;")
        _ = try await js("await ST.request('transport.stop');return true;")
        try await wait("!ST.state.status.playing&&document.getElementById('automation-transport').dataset.mode==='stopped'&&document.getElementById('automation-playhead').hidden", "Stopped native transport did not hide the automation playhead")
        try await check("document.activeElement===window.__acceptanceEditor&&document.getElementById('automation-envelope')===window.__acceptanceEnvelope", "Other-pattern or stopped status replaced the focused editor")
        evidence["nativeAutomationTransport"] = ["first":first,"later":next,"otherPatternLabel":other,"focusPreserved":true,"outputMuted":true]
    }
}

#endif

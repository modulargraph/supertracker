import AppKit
import WebKit
import Foundation
import CryptoKit

/// Explicit installed-vendor diagnostic, excluded from the application target.
/// This covers one installed preset through actual WebKit and native persistence.
/// It is not physical-keyboard, real-time, hardware, or broad NKS compatibility QA.
@MainActor enum NativeNKSUIAcceptance {
    static let selectedClassID = "5653544E6924486D6173736976652078"
    static let selectedPreset = "/Library/Application Support/Native Instruments/Massive X/Presets/Contender.nksf"
    static let selectedBundle = "/Library/Audio/Plug-Ins/VST3/Massive X.vst3"

    static func run(output: URL, webRoot: URL, runID: String) async throws {
        let files = FileManager.default
        let folder = output.appendingPathComponent("nks-ui", isDirectory: true)
        try files.createDirectory(at: folder, withIntermediateDirectories: true)
        let scratch = folder.appendingPathComponent("IsolatedSupport", isDirectory: true)
        try files.createDirectory(at: scratch, withIntermediateDirectories: true)
        let presetURL = URL(fileURLWithPath: selectedPreset), bundleURL = URL(fileURLWithPath: selectedBundle)
        let originalWindows = Set(NSApp.windows.filter(\.isVisible).map(ObjectIdentifier.init))
        let trace = NativeNKSUIBridgeTrace()
        var checks: [String] = [], failures: [String] = [], phase = "preflight", measurements: [String: Any] = [:]
        var report: [String: Any] = ["passed": false, "checksPassed": false, "processExitPassed": false,
            "scope": "one-installed-vst3-nks-mapped-ui", "runID": runID, "presetPath": selectedPreset,
            "bundlePath": selectedBundle, "classID": selectedClassID,
            "physicalKeyboardTested": false, "realtimePlaybackTested": false, "hardwareInputTested": false,
            "audibleOutputUsed": false, "broadNKSCompatibilityClaimed": false]
        func write(_ status: String = "running") throws {
            report["status"] = status; report["phase"] = phase; report["checks"] = checks
            report["failures"] = failures; report["measurements"] = measurements
            report["checksPassed"] = status == "checksComplete" && failures.isEmpty
            report["bridgeTrace"] = trace.records; report["bridgeTraceDropped"] = trace.dropped
            try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
                .write(to: folder.appendingPathComponent("report.json"), options: .atomic)
        }
        func begin(_ name: String) throws { phase = name; trace.phase = name; print("NKS UI: \(name)"); fflush(stdout); try write() }
        func require(_ condition: Bool, _ message: String) throws { if !condition { throw TrackerError.message("NKS UI: " + message) } }
        func same(_ first: Any, _ second: Any) -> Bool {
            (try? JSONSerialization.data(withJSONObject: first, options: [.sortedKeys, .fragmentsAllowed])) ==
                (try? JSONSerialization.data(withJSONObject: second, options: [.sortedKeys, .fragmentsAllowed]))
        }
        let diagnosticScript = "return {errors:window.__nksDiagnosticErrors||[],view:window.ST?.state.view,modal:window.ST?.state.modal,pending:window.ST?Array.from(ST.state.pending.values()).map(x=>x.command):[],toast:document.getElementById('toast-region')?.textContent||'',nks:document.querySelector('[data-nks-status]')?.textContent||'',loading:window.STNKS?.loading,inputTrace:window.__nksInputTrace||null,bindingTrace:window.__nksBindingTrace||[],nksEditGeneration:window.STNKSControls?.editGeneration,nksPendingEdits:window.STNKSControls?.hasPendingEdits,plugins:window.ST?.state.project?.audioPlugins?.map(x=>({component:x.component,instanceID:x.instanceID,kind:x.kind,track:x.track}))||[]};"
        func diagnostic(_ web: WKWebView) async -> Any {
            (try? await web.callAsyncJavaScript(diagnosticScript, arguments: [:], in: nil, contentWorld: .page)) ?? ["unavailable": true]
        }
        try write()
        var activeBridge: Bridge?, activeAudio: AudioHost?, activeWeb: WKWebView?, activeConfiguration: WKWebViewConfiguration?, activeCatalogue: VST3Catalogue?
        do {
            // Validate construction and disk round-trip before any vendor scan
            // or AudioHost is created; fixture errors must not enter vendor code.
            try begin("validate diagnostic project before vendor entry")
            var rows = [Any](repeating: NSNull(), count: 16)
            rows[0] = ["note": 60, "instrument": 0, "velocity": 105, "gate": 7.8]
            rows[8] = ["note": 67, "instrument": 0, "velocity": 105, "gate": 7.5]
            let fixture: [String: Any] = ["version": 1, "name": "Installed NKS mapped workflow",
                "bpm": 120.0, "linesPerBeat": 4, "masterVolume": 0.65, "activePattern": 0, "loopSong": false,
                "tracks": [["id": 0, "routingID": "installed-nks-ui-track", "name": "NKS instrument", "instrument": 0,
                            "volume": 0.7, "pan": 0, "delaySend": 0, "reverbSend": 0]],
                "instruments": [["id": 0, "type": "au", "name": "Preset destination", "volume": 0.7, "pan": 0]],
                "patterns": [["id": 0, "name": "NKS acceptance", "length": 16, "steps": [rows], "automation": []]],
                "sequence": [0], "audioPlugins": [], "audioClips": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
            let store = try ProjectStore(directory: scratch.appendingPathComponent("ProjectStore"))
            try store.update(fixture); try store.autosave()
            try require(same(try store.read(store.recoveryURL), fixture), "Fixture changed during its initial native disk round-trip.")
            measurements["fixtureValidatedBeforeVendorEntry"] = true
            let recovery = try PluginRecoveryStore(directory: scratch.appendingPathComponent("PluginRecovery"))
            try recovery.startSession()
            measurements["privateRecoverySessionStarted"] = true
            let sourceBytes = try Data(contentsOf: presetURL), preset = try NKSHostPreset(url: presetURL)
            try require(preset.identity.vst3ClassID == selectedClassID, "The selected preset no longer declares the expected exact VST3 class.")
            try require(files.fileExists(atPath: bundleURL.path), "The selected installed VST3 bundle is missing.")
            try require(files.fileExists(atPath: webRoot.appendingPathComponent("index.html").path), "Frozen interface resources are missing.")
            measurements["presetSHA256Before"] = SHA256.hash(data: sourceBytes).map { String(format: "%02x", $0) }.joined()
            measurements["presetBytes"] = sourceBytes.count
            var catalogueConfiguration = VST3Catalogue.Configuration()
            catalogueConfiguration.roots = []
            catalogueConfiguration.cacheURL = scratch.appendingPathComponent("vst3-catalogue.json")
            catalogueConfiguration.executableURL = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
            catalogueConfiguration.timeout = 25; catalogueConfiguration.maximumBundles = 1
            let catalogue = VST3Catalogue(configuration: catalogueConfiguration); activeCatalogue = catalogue
            try begin("scan selected factory only")
            let descriptors = try await catalogue.scanBundle(bundleURL)
            guard let descriptor = descriptors.first(where: { $0.classID == selectedClassID && $0.isInstrument }) else {
                throw TrackerError.message("NKS UI: The selected factory does not expose the preset's exact instrument class.")
            }
            measurements["installedDescriptor"] = descriptor.dictionary
            checks.append("Selected installed preset PLID matches the only scanned VST3 instrument factory")
            var vst2Configuration = VST2Catalogue.Configuration()
            vst2Configuration.roots = []; vst2Configuration.cacheURL = scratch.appendingPathComponent("Private-vst2-catalogue.json")
            var instantiations = 0
            let audio = try AudioHost(beforePluginInstantiation: { instantiations += 1 },
                recoveryStore: recovery,
                vst3Catalogue: catalogue, midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration:vst2Configuration))
            activeAudio = audio
            let midi = try await audio.handle("midi.devices", payload: [:]) as? [String: Any] ?? [:]
            try require(midi["allInputs"] as? Bool == false && (midi["inputIDs"] as? [Int])?.isEmpty == true,
                        "The private host subscribed to external MIDI.")
            measurements["midiInputsDisabled"] = true
            let library = LibraryService(cacheDirectory: scratch.appendingPathComponent("Library"), defaultRoots: [], loadCacheAsynchronously: false)
            let bridge = Bridge(audio: audio, library: library, store: store, pluginDiscoveryEnabled:false); activeBridge = bridge
            let configuration = WKWebViewConfiguration(); activeConfiguration = configuration
            configuration.websiteDataStore = .nonPersistent(); configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
            configuration.preferences.tabFocusesLinks = true
            configuration.userContentController.add(NativeNKSUIMessageObserver(bridge: bridge, trace: trace), name: "supertracker")
            configuration.userContentController.addUserScript(WKUserScript(source: "window.__nksDiagnosticErrors=[];window.addEventListener('error',e=>__nksDiagnosticErrors.push(e.message));window.addEventListener('unhandledrejection',e=>__nksDiagnosticErrors.push(String(e.reason)));", injectionTime: .atDocumentStart, forMainFrameOnly: true))
            let web = NativeNKSUIObservedWebView(frame: CGRect(x: 0, y: 0, width: 1300, height: 850), configuration: configuration); web.trace = trace; activeWeb = web
            bridge.webView = web; web.navigationDelegate = bridge
            web.loadFileURL(webRoot.appendingPathComponent("index.html"), allowingReadAccessTo: webRoot)
            func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
                try await web.callAsyncJavaScript(source, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
            }
            func wait(_ expression: String, _ description: String, timeout: Double = 30) async throws {
                let deadline = Date().addingTimeInterval(timeout)
                while Date() < deadline {
                    if (try? await js("return Boolean(\(expression));")) as? Bool == true { return }
                    try await Task.sleep(nanoseconds: 25_000_000)
                }
                let context = await diagnostic(web)
                measurements["lastUIContext"] = context
                throw TrackerError.message("NKS UI: Timed out waiting for \(description). \(String(describing: context))")
            }
            func check(_ expression: String, _ message: String) async throws { try require(try await js("return Boolean(\(expression));") as? Bool == true, message) }
            // Start ordinary UI session actions without awaiting their promise
            // inside a single WebKit call. Their real unsaved dialog must remain
            // observable and serviceable by the fixture's bounded poll loop.
            func sessionAction(_ action: String, path: String? = nil, discardBaseline: [String: Any]) async throws {
                let token = UUID().uuidString, deadline = Date().addingTimeInterval(40)
                var promptCount = 0
                try begin("fresh instance Open: " + action)
                _ = try await js("""
                    if(window.__nksSessionAction?.pending)throw Error('Another fixture session action is still pending');
                    const entry={token,action,pending:true,result:null,error:null};window.__nksSessionAction=entry;
                    Promise.resolve().then(()=>ST.projectAction(action,path||undefined)).then(
                      result=>{entry.result=result;entry.pending=false;},
                      error=>{entry.error=String(error?.message||error);entry.pending=false;});
                    return true;
                    """, ["token": token, "action": action, "path": path ?? ""])
                while Date() < deadline {
                    let context = try await js("""
                        const entry=window.__nksSessionAction;
                        if(!entry||entry.token!==token)throw Error('The fixture session action was replaced');
                        const modal=ST.state.modal;
                        const current=modal==='unsaved'?window.__supertracker.getProject():null;
                        return {...entry,modal,discardEnabled:!!document.getElementById('unsaved-discard')&&!document.getElementById('unsaved-discard').disabled,
                          project:current?{name:current.name,patterns:current.patterns,sequence:current.sequence,audioClips:current.audioClips}:null};
                        """, ["token": token]) as? [String: Any] ?? [:]
                    if context["pending"] as? Bool == false {
                        measurements["sessionAction-" + action] = ["result": context["result"] ?? NSNull(), "unsavedPromptsHandled": promptCount]
                        try require(context["result"] as? Bool == true && (context["error"] is NSNull || context["error"] == nil),
                                    "Ordinary \(action) failed: \(context). UI: \(await diagnostic(web))")
                        try write(); return
                    }
                    if context["modal"] as? String == "unsaved" {
                        let current = context["project"] as? [String: Any] ?? [:]
                        try require(promptCount == 0 && context["discardEnabled"] as? Bool == true &&
                            current["name"] as? String == discardBaseline["name"] as? String &&
                            same(current["patterns"] ?? [], discardBaseline["patterns"] ?? []) &&
                            same(current["sequence"] ?? [], discardBaseline["sequence"] ?? []) &&
                            same(current["audioClips"] ?? [], discardBaseline["audioClips"] ?? []),
                            "Refusing to discard a changed fixture project or unexpected repeated prompt during \(action).")
                        promptCount += 1
                        measurements["sessionPrompt-" + action] = ["kind": "unsaved", "choice": "Discard",
                            "matchesVerifiedBaseline": true, "projectName": current["name"] ?? NSNull()]
                        try write()
                        _ = try await js("""
                            if(window.__nksSessionAction?.token!==token||!window.__nksSessionAction.pending||ST.state.modal!=='unsaved')throw Error('Session prompt ownership changed');
                            const button=document.getElementById('unsaved-discard');if(!button||button.disabled)throw Error('Discard is unavailable');
                            button.click();return true;
                            """, ["token": token])
                    }
                    try await Task.sleep(nanoseconds: 25_000_000)
                }
                let context = await diagnostic(web); measurements["sessionActionTimeout-" + action] = context
                throw TrackerError.message("NKS UI: Ordinary \(action) exceeded its 40-second deadline. UI context: \(context)")
            }
            func change(_ id: String, _ value: String) async throws {
                _ = try await js("const input=document.getElementById(id);if(!input)throw Error('Missing '+id);input.value=value;input.dispatchEvent(new Event('change',{bubbles:true}));return true;", ["id": id, "value": value])
            }
            func render(_ name: String) async throws {
                try begin("offline render " + name)
                let path = folder.appendingPathComponent(name + ".wav")
                _ = try await js("if(!await ST.captureNativeEdits())throw Error('Native capture failed');return await ST.request('audio.export',{path:path,mode:'song',tail:.25});", ["path": path.path])
                let pcm = try AudioSamples.decode(path.path)
                try require(!pcm.left.isEmpty && pcm.left.count == pcm.right.count && pcm.left.allSatisfy(\.isFinite) && pcm.right.allSatisfy(\.isFinite), "Offline render is empty or non-finite.")
                let energy = sqrt((pcm.left + pcm.right).reduce(0.0) { $0 + Double($1) * Double($1) } / Double(pcm.left.count + pcm.right.count))
                try require(abs(pcm.duration - 2.25) < 0.003 && energy > 0.00001, "Reopened installed preset produced the wrong duration or silence.")
                measurements[name] = ["frames": pcm.left.count, "sampleRate": pcm.sampleRate, "duration": pcm.duration, "rms": energy]
                // Vendor voices/modulation can be nondeterministic. No waveform
                // equality, variation, or exact row-audio timing claim is made.
                try write()
            }
            try begin("real interface boot and NKS load")
            try await wait("window.ST?.state.engineReady&&ST.state.pending.size===0&&ST.state.project.name==='Installed NKS mapped workflow'", "private bridge boot")
            let loaded = try await js("return await STNKS.load(path);", ["path": selectedPreset]) as? Bool == true
            if !loaded {
                let context = await diagnostic(web); measurements["nksLoadFailureUI"] = context
                try write()
                throw TrackerError.message("NKS UI: The actual NKS loader did not accept the selected installed preset. UI context: \(context)")
            }
            try await wait("ST.state.view==='instrument'&&document.querySelector('[data-nks-root]')&&ST.state.pending.size===0", "real mapped controls")
            let selected = try await js("""
                const plugin=ST.state.project.audioPlugins.find(x=>x.track===0&&x.kind==='instrument');
                const params=await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0,instanceID:plugin.instanceID});
                const native=new Map(params.map(x=>[String(x.address),x]));
                for(let page=0;page<(plugin.nks?.pages||[]).length;page++)for(const slot of plugin.nks.pages[page].parameters||[]){
                  const parameter=native.get(String(slot.parameterID));
                  if(slot.mapped===true&&parameter?.writable===true&&parameter?.automatable===true){
                    window.__nksSelected={page,slot:slot.slot,address:String(slot.parameterID),name:slot.name,pageName:plugin.nks.pages[page].name,nativeName:parameter.name,instanceID:plugin.instanceID};
                    document.getElementById('nks-0-page').value=String(page);document.getElementById('nks-0-page').dispatchEvent(new Event('change',{bubbles:true}));
                    return {...window.__nksSelected,component:plugin.component,bundlePath:plugin.bundlePath,nks:plugin.nks};
                  }
                }throw Error('No usable native-mapped NKS parameter');
                """) as? [String: Any] ?? [:]
            try require(selected["component"] as? String == descriptor.id && selected["bundlePath"] as? String == selectedBundle,
                        "NKS loader substituted a different plug-in identity or installation.")
            guard let address = selected["address"] as? String, UInt32(address) != nil,
                  let name = selected["name"] as? String, !name.isEmpty,
                  let instanceID = selected["instanceID"] as? String,
                  let slot = selected["slot"] as? Int else { throw TrackerError.message("NKS UI: Invalid resolved mapping.") }
            trace.watchedAddress = address
            let baseID = "nks-0-\(slot)", laneTarget = "plugin:instrument:0:\(address)"
            // Read readiness and the exact target in the same JavaScript turn
            // as its action. A previous ready DOM node can be replaced by a
            // disabled loading node before a later WebKit call is dispatched.
            let bindingPrelude = """
                const selected=window.__nksSelected,number=document.getElementById('\(baseID)-number'),range=document.getElementById('\(baseID)-range');
                const plugins=ST.state.project.audioPlugins.filter(x=>x.instanceID===selected.instanceID),plugin=plugins[0];
                const node=input=>input?{id:input.id,connected:input.isConnected,disabled:input.disabled,value:input.value,address:input.dataset.nksAddress,automate:input.dataset.automate||null,bound:input.closest('[data-nks-root]')?.dataset.nksBound||null}:null;
                const binding={number:node(number),range:node(range),page:document.getElementById('nks-0-page')?.value,
                  plugin:plugin?{instanceID:plugin.instanceID,component:plugin.component,kind:plugin.kind,track:plugin.track,slot:plugin.slot??0}:null,
                  pendingCommands:Array.from(ST.state.pending.values()).map(x=>x.command),time:performance.now(),wallTimeMS:Date.now()};
                window.__nksBindingTrace||=[];window.__nksBindingTrace.push(binding);if(window.__nksBindingTrace.length>32)window.__nksBindingTrace.shift();
                if(plugins.length!==1||plugin.component!==expectedComponent||plugin.kind!=='instrument'||plugin.track!==0||(plugin.slot??0)!==0)throw Error('Mapped control plug-in identity or route changed: '+JSON.stringify(binding));
                const mapping=plugin.nks?.pages?.[selected.page]?.parameters?.find(x=>x.slot===selected.slot);
                if(mapping?.mapped!==true||String(mapping.parameterID)!==selected.address||mapping.name!==selected.name)throw Error('Mapped control metadata changed: '+JSON.stringify(binding));
                if(!number?.isConnected||!range?.isConnected)throw Error('Mapped control is missing from the instrument view: '+JSON.stringify(binding));
                if(!number.disabled&&(number.dataset.nksAddress!==selected.address||number.closest('[data-nks-root]')?.dataset.nksBound!=='true'))throw Error('Enabled numeric control has the wrong exact ParamID: '+JSON.stringify(binding));
                if(!range.disabled&&(range.dataset.nksAddress!==selected.address||range.dataset.automate!==expectedTarget||range.closest('[data-nks-root]')?.dataset.nksBound!=='true'))throw Error('Enabled mapped control has the wrong exact automation target: '+JSON.stringify(binding));
                if(number.disabled||range.disabled)return {ready:false,binding};
                """
            func mappedAction(_ description: String, source: String, arguments: [String: Any] = [:]) async throws -> [String: Any] {
                let deadline = Date().addingTimeInterval(30)
                var arguments = arguments
                arguments["expectedComponent"] = descriptor.id; arguments["expectedTarget"] = laneTarget
                while Date() < deadline {
                    let result = try await js(bindingPrelude + "\n" + source, arguments) as? [String: Any] ?? [:]
                    measurements["binding-" + description] = result["binding"] ?? NSNull()
                    if result["ready"] as? Bool == true { return result }
                    // Only an explicitly disabled, identity-verified control
                    // reaches this wait; no gesture or Save has been dispatched.
                    try await Task.sleep(nanoseconds: 25_000_000)
                }
                throw TrackerError.message("NKS UI: Timed out while mapped controls were disabled for \(description). Last binding: \(String(describing: measurements["binding-" + description]))")
            }
            func remountSelectedPage() async throws {
                try await wait("document.getElementById('nks-0-page')", "mapped page selector")
                _ = try await js("const select=document.getElementById('nks-0-page');if(Number(select.value)!==__nksSelected.page){select.value=String(__nksSelected.page);select.dispatchEvent(new Event('change',{bubbles:true}));}return true;")
                _ = try await mappedAction("selected page remount", source: "return {ready:true,binding};")
            }
            measurements["selectedMapping"] = selected
            let metadata = selected["nks"] as? [String: Any] ?? [:]
            let parameters = try await audio.handle("plugin.parameters", payload: ["track": 0, "kind": "instrument", "slot": 0, "instanceID": instanceID]) as? [[String: Any]] ?? []
            let parameterIDs = Set(parameters.compactMap { ($0["address"] as? String).flatMap(UInt32.init) })
            try require(same(metadata["pages"] ?? [], preset.validatedMappings(vst3ClassID: descriptor.classID, parameterIDs: parameterIDs)),
                        "Mounted NKS pages differ from the selected file's mapping validated against actual native parameter IDs.")
            measurements["selectedNativeParameter"] = parameters.first { $0["address"] as? String == address }
            _ = try await mappedAction("initial exact native ParamID", source: "return {ready:true,binding};")
            checks.append("Actual NKS loader and mounted page bind the selected mapped label to an exact writable native ParamID")
            let savedURL = folder.appendingPathComponent("Installed NKS workflow.supertracker")
            _ = try await js("const saved=await ST.request('project.save',{path:path,project:window.__supertracker.getProject()});ST.acceptProject(saved.project);ST.setView('instrument');return true;", ["path": savedURL.path])
            try await remountSelectedPage()
            try begin("input-only mapped edit and immediate Save")
            let immediateSave = try await mappedAction("input-only edit and immediate Save", source: """
                const input=document.getElementById(id);
                const observe=()=>({id:input?.id,connected:input?.isConnected,disabled:input?.disabled,value:input?.value,
                  address:input?.dataset.nksAddress,bound:input?.closest('[data-nks-root]')?.dataset.nksBound,
                  currentNode:document.getElementById(id)===input,focused:document.activeElement===input,
                  editGeneration:STNKSControls.editGeneration,pendingEdits:STNKSControls.hasPendingEdits,
                  dirty:ST.state.dirty,pendingCommands:Array.from(ST.state.pending.values()).map(x=>x.command),
                  instanceID:ST.state.project.audioPlugins.find(x=>x.track===0&&x.kind==='instrument')?.instanceID,
                  time:performance.now(),wallTimeMS:Date.now()});
                window.__nksInputTrace={before:observe()};
                input.focus();window.__nksInputTrace.afterFocus=observe();input.value='.47';
                input.dispatchEvent(new Event('input',{bubbles:true}));window.__nksInputTrace.afterInput=observe();
                const saved=await ST.projectAction('save');window.__nksInputTrace.afterSave=observe();
                return {ready:true,binding,saved,inputTrace:window.__nksInputTrace};
                """, arguments: ["id": baseID + "-number"])
            measurements["immediateSave"] = immediateSave
            // This is the same post-Save assertion/read as before. Diagnostics
            // are passive native counters; no notes, settling waits or retries.
            let afterSave = try await js("return await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0,instanceID:__nksSelected.instanceID,addresses:[__nksSelected.address],diagnostics:true});") as? [[String: Any]] ?? []
            measurements["immediateSaveNativeParameters"] = afterSave
            if let savedBytes = try? Data(contentsOf: savedURL) {
                measurements["immediateSaveFileSHA256"] = NativeNKSUIBridgeTrace.digest(savedBytes)
                if let document = try? JSONSerialization.jsonObject(with: savedBytes) as? [String: Any] {
                    measurements["immediateSaveDiskPlugins"] = trace.summarize(document["audioPlugins"] ?? [])
                }
            }
            try write()
            try require(immediateSave["saved"] as? Bool == true, "Immediate Save did not drain the pending mapped-control draft.")
            try require(afterSave.first?["normalized"] as? Double != nil && abs((afterSave.first?["normalized"] as? Double ?? -1) - 0.47) < 0.003,
                        "Immediate Save lost the normalized mapped edit. Expected 0.47; returned \(String(describing: afterSave.first?["normalized"])); input trace and passive bridge log are recorded.")
            checks.append("Input-only mapped draft settles through actual Save without a preliminary note or export")
            try begin("mapped parameter to automation lane")
            try await remountSelectedPage()
            _ = try await mappedAction("mapped Option A", source: "const input=document.getElementById(id);input.focus();input.value='.73';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'å',code:'KeyA',altKey:true,bubbles:true,cancelable:true}));return {ready:true,binding};", arguments: ["id": baseID + "-range"])
            try await wait("ST.state.view==='automation'&&ST.pattern().automation.length===1", "mapped Option+A automation")
            try await check("ST.pattern().automation[0].target==='\(laneTarget)'&&ST.pattern().automation[0].name===__nksSelected.name&&ST.pattern().automation[0].points[0].value===.73", "Mapped automation lost its label, exact target, or pending value.")
            try await change("automation-interpolation", "step")
            _ = try await js("document.getElementById('automation-add-point').click();return true;")
            try await change("point-row-1", "8"); try await change("point-value-1", ".29")
            _ = try await js("const search=document.getElementById('automation-search');search.value=__nksSelected.name;search.dispatchEvent(new Event('input',{bubbles:true}));return true;")
            try await wait("Array.from(document.querySelectorAll('[data-choose-target]')).some(x=>x.dataset.chooseTarget==='\(laneTarget)')", "mapped automation target search")
            try await check("document.querySelector('.studio-list-caption').textContent.includes('Already automated')", "Existing mapped automation was not ranked first.")
            checks.append("Actual mapped Option+A creates the exact native lane and its label remains searchable among automated targets")
            try begin("mapped parameter to step effect")
            _ = try await js("ST.state.row=4;ST.state.column=3;ST.setView('instrument');return true;")
            try await remountSelectedPage()
            try await wait("document.getElementById('\(baseID)-step')&&!document.getElementById('\(baseID)-step').disabled", "mapped Use on step action")
            _ = try await js("document.getElementById(id).click();return true;", ["id": baseID + "-step"])
            try await wait("ST.state.modal==='stepFX'&&document.getElementById('step-fx-value')&&!document.getElementById('step-fx-value').disabled", "mapped step picker")
            try await check("document.getElementById('step-fx-choice').textContent.includes(__nksSelected.name)", "Step picker did not retain the mapped label.")
            _ = try await js("const input=document.getElementById('step-fx-value');input.value='.61';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true}));return true;")
            try await wait("!ST.state.modal&&ST.pattern().steps[0][4]?.fx1?.type==='plugin'", "step lock commit")
            try await check("ST.pattern().steps[0][4].note===-3&&ST.pattern().steps[0][4].fx1.address===__nksSelected.address&&ST.pattern().steps[0][4].fx1.instanceID===__nksSelected.instanceID&&ST.pattern().steps[0][4].fx1.value===.61", "Mapped step lock lost its effect-only row, exact address, value or stable plug-in instance.")
            try require(try await js("return await ST.projectAction('save');") as? Bool == true, "Mapped lane and step lock Save failed.")
            let saved = try await js("return window.__supertracker.getProject();") as? [String: Any] ?? [:]
            let disk = try store.read(savedURL)
            try require(same(saved["patterns"] ?? [], disk["patterns"] ?? []), "Disk persistence changed the mapped lane or step lock.")
            measurements["savedPatterns"] = disk["patterns"]
            checks.append("Mapped Use on step commits an effect-only row with exact ParamID/instance identity and survives native disk serialization")
            try begin("fresh instance Open and native round trip")
            let beforeFreshOpen = instantiations
            try await sessionAction("new", discardBaseline: saved)
            try await wait("ST.state.pending.size===0&&ST.state.project.audioPlugins.length===0", "old instance removal")
            let emptySession = try await js("return window.__supertracker.getProject();") as? [String: Any] ?? [:]
            try require((emptySession["audioPlugins"] as? [[String: Any]])?.isEmpty == true, "New did not create an empty private session.")
            try await sessionAction("open", path: savedURL.path, discardBaseline: emptySession)
            try await wait("ST.state.pending.size===0&&ST.state.project.audioPlugins.some(x=>x.instanceID==='\(instanceID)')", "fresh saved plug-in restoration")
            try require(instantiations > beforeFreshOpen, "Save/Open reused the prior live instance instead of testing restoration.")
            let reopened = try await js("if(!await ST.captureNativeEdits())throw Error('Reopened native capture failed');return await ST.request('engine.snapshot');") as? [String: Any] ?? [:]
            try require(same(disk["patterns"] ?? [], reopened["patterns"] ?? []), "Native reopened project lost mapped lane/step automation.")
            let reopenedPlugin = (reopened["audioPlugins"] as? [[String: Any]])?.first ?? [:]
            try require(reopenedPlugin["component"] as? String == descriptor.id && reopenedPlugin["instanceID"] as? String == instanceID,
                        "Fresh Open lost the exact saved plug-in identity.")
            try require(same(reopenedPlugin["nks"] ?? [:], metadata), "Fresh Open changed the preset's validated mapped pages.")
            try await check("Math.abs((await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0,instanceID:__nksSelected.instanceID,addresses:[__nksSelected.address]}))[0].normalized-.73)<.003", "Fresh Open lost the settled mapped-control value before rendering.")
            checks.append("New then Open creates a fresh installed instance, restoring native mapped value, NKS metadata, lane and step-lock identity")
            try await render("mapped-lane-and-step-reopened")
            try await check("window.__nksDiagnosticErrors.length===0", "The actual interface raised a JavaScript error or unhandled rejection.")
            try require(try Data(contentsOf: presetURL) == sourceBytes, "The diagnostic changed the installed source preset.")
            measurements["sourcePresetUnchanged"] = true; measurements["instantiations"] = instantiations
            measurements["audioScope"] = "Finite non-silent offline smoke only; deterministic lane/lock sample timing is covered by original fixtures, not inferred from vendor PCM."
            checks.append("Reopened one-preset offline graph renders finite non-silent audio and leaves the installed preset untouched")
            try begin("ordinary teardown")
            web.stopLoading(); web.navigationDelegate = nil
            configuration.userContentController.removeScriptMessageHandler(forName: "supertracker"); bridge.webView = nil
            await bridge.shutdownAndWait(); activeBridge = nil; activeAudio = nil
            catalogue.cancel(); activeCatalogue = nil
            try require(Set(NSApp.windows.filter(\.isVisible).map(ObjectIdentifier.init)) == originalWindows, "The diagnostic opened a visible window.")
            checks.append("Private host and bridge shut down normally without visible windows or hardware input")
            phase = "checks complete; parent must verify normal process exit"; try write("checksComplete")
        } catch {
            if let web = activeWeb { measurements["failureUIContext"] = await diagnostic(web) }
            failures.append(error.localizedDescription); try? write("failed")
            activeWeb?.stopLoading(); activeWeb?.navigationDelegate = nil
            activeConfiguration?.userContentController.removeScriptMessageHandler(forName: "supertracker")
            activeBridge?.webView = nil
            if let bridge = activeBridge { await bridge.shutdownAndWait() }
            else if let audio = activeAudio { await audio.shutdownAndWait() }
            activeCatalogue?.cancel()
            throw error
        }
    }
}


/// Test-only native observation. Original messages, Bridge tasks, JavaScript
/// promises and delivery callbacks remain unchanged. Opaque states are hashed;
/// bounded memory is written only with the fixture's ordinary phase/report data.
@MainActor private final class NativeNKSUIBridgeTrace {
    var phase = "preflight", watchedAddress: String?
    private(set) var records: [[String: Any]] = []
    private(set) var dropped = 0
    private var commands: [String: String] = [:]
    private let started = ProcessInfo.processInfo.systemUptime
    static func digest(_ bytes: Data) -> String { SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined() }
    private func relevant(_ command: String) -> Bool {
        command.hasPrefix("plugin.") || command.hasPrefix("plugins.") || command.hasPrefix("project.") || ["ready", "engine.project", "engine.snapshot"].contains(command)
    }
    func summarize(_ value: Any, depth: Int = 0) -> Any {
        if depth > 7 { return "[depth limit]" }
        if let fields = value as? [String: Any] {
            var result: [String: Any] = [:]
            for (key, child) in fields {
                if key == "state", let opaque = child as? String {
                    result["stateSummary"] = ["characters": opaque.count, "sha256": Self.digest(Data(opaque.utf8))]
                } else if ["nks", "patterns", "instruments", "tracks", "sequence", "audioClips", "pages", "mapping"].contains(key) {
                    if let array = child as? [Any] { result[key + "Count"] = array.count }
                } else { result[key] = summarize(child, depth: depth + 1) }
            }
            return result
        }
        if let array = value as? [Any] {
            let parameters = array.compactMap { $0 as? [String: Any] }
            if let watchedAddress, !parameters.isEmpty, parameters.allSatisfy({ $0["address"] != nil }) {
                return parameters.filter { String(describing: $0["address"]!) == watchedAddress }.map { summarize($0, depth: depth + 1) }
            }
            return array.prefix(24).map { summarize($0, depth: depth + 1) }
        }
        if let string = value as? String, string.count > 1024 { return ["characters": string.count, "sha256": Self.digest(Data(string.utf8))] }
        return value
    }
    private func append(_ direction: String, _ command: String, _ data: [String: Any]) {
        guard records.count < 2048 else { dropped += 1; return }
        records.append(["order": records.count, "elapsedMS": (ProcessInfo.processInfo.systemUptime - started) * 1000, "wallTimeMS": Date().timeIntervalSince1970 * 1000,
                        "phase": phase, "direction": direction, "command": command, "data": summarize(data)])
    }
    func request(_ body: Any) {
        guard let body = body as? [String: Any], let command = body["command"] as? String, relevant(command) else { return }
        if let id = body["id"] { commands[String(describing: id)] = command }
        append("request", command, body)
    }
    func delivery(_ source: String) {
        for function in ["nativeReceive", "nativeEvent"] {
            let prefix = "window.\(function) && window.\(function)("
            guard source.hasPrefix(prefix), source.hasSuffix(");"),
                  let bytes = String(source.dropFirst(prefix.count).dropLast(2)).data(using: .utf8),
                  let body = try? JSONSerialization.jsonObject(with: bytes) as? [String: Any] else { continue }
            if function == "nativeReceive", let id = body["id"], let command = commands.removeValue(forKey: String(describing: id)) {
                append("delivery", command, body)
            } else if let type = body["type"] as? String, relevant(type) || type == "audioPlugins.changed" || type == "error" {
                append("event", type, body)
            }
            return
        }
    }
}

@MainActor private final class NativeNKSUIMessageObserver: NSObject, WKScriptMessageHandler {
    let bridge: Bridge, trace: NativeNKSUIBridgeTrace
    init(bridge: Bridge, trace: NativeNKSUIBridgeTrace) { self.bridge = bridge; self.trace = trace }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        if message.frameInfo.isMainFrame { trace.request(message.body) }
        bridge.userContentController(userContentController, didReceive: message)
    }
}

@MainActor private final class NativeNKSUIObservedWebView: WKWebView {
    var trace: NativeNKSUIBridgeTrace?
    override func evaluateJavaScript(_ javaScriptString: String, completionHandler: (@MainActor @Sendable (Any?, Error?) -> Void)? = nil) {
        trace?.delivery(javaScriptString)
        super.evaluateJavaScript(javaScriptString, completionHandler: completionHandler)
    }
}

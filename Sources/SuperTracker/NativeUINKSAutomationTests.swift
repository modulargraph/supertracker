// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit
import AVFoundation
import AudioToolbox

/// Exercises mapped-control UI → saved automation → native audio using an original
/// in-process AU. Its NKS metadata is deliberately synthetic: this is not an NKS
/// parser, installed-vendor compatibility, physical-keyboard, or real-time test.
enum NativeUINKSAutomationFixture {
    static let mappedAddress: AUParameterAddress = 0x10203040
    static let wideAddress: AUParameterAddress = 0xfedcba9876543210
    private static let subtype: OSType = 0x6e6b7569 // nkui
    static let wideSubtype: OSType = 0x6e6b7764 // nkwd
    private static let manufacturer: OSType = 0x53745473 // StTs
    static let identifier = "\(kAudioUnitType_MusicDevice):\(subtype):\(manufacturer)"
    static let wideIdentifier = "\(kAudioUnitType_MusicDevice):\(wideSubtype):\(manufacturer)"
    private static let registration: Void = {
        for (subtype, name) in [(subtype, "NKS mapped automation"), (wideSubtype, "wide parameter automation")] {
            AUAudioUnit.registerSubclass(NativeUINKSAutomationInstrument.self,
                as: AudioComponentDescription(componentType: kAudioUnitType_MusicDevice,
                    componentSubType: subtype, componentManufacturer: manufacturer,
                    componentFlags: 0, componentFlagsMask: 0),
                name: "SuperTracker Test: \(name) fixture", version: 1)
        }
    }()
    @MainActor static func register() { _ = registration }
}

@MainActor
enum NativeUINKSAutomationTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        NativeUINKSAutomationFixture.register()
        let folder = output.appendingPathComponent("mounted-nks-automation", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let identifier = NativeUINKSAutomationFixture.identifier
        let address = NativeUINKSAutomationFixture.mappedAddress
        let parameterID = String(address), laneTarget = "plugin:instrument:0:\(address)"
        var checks: [String] = []
        var evidence: [String: Any] = ["scope": "original AU with explicitly synthetic NKS pages",
            "installedPluginsLoaded": false, "nksParserTested": false, "physicalKeyboardTested": false,
            "realtimePlaybackTested": false, "component": identifier, "parameterAddress": parameterID]
        func js(_ source: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(source, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("WEBKIT NKS AUTOMATION: " + message) }
        }
        func sameJSON(_ left: Any, _ right: Any) throws -> Bool {
            try JSONSerialization.data(withJSONObject: left, options: [.sortedKeys]) ==
                JSONSerialization.data(withJSONObject: right, options: [.sortedKeys])
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
            let state = try? await js("""
                const describe=plugin=>({track:plugin.track,owner:plugin.owner,kind:plugin.kind,slot:plugin.slot,component:plugin.component,instanceID:plugin.instanceID,hasNKS:!!plugin.nks,pages:plugin.nks?.pages?.length??0});
                const input=document.getElementById('nks-0-0-range');
                return {view:ST.state.view,focus:document.activeElement?.outerHTML,pending:Array.from(ST.state.pending.values()).map(x=>x.command),toast:document.getElementById('toast-region')?.textContent,nks:document.querySelector('[data-nks-status]')?.textContent,lanes:ST.pattern().automation,
                  selectedTrack:ST.state.track,selectedInstrument:ST.state.instrument,assignedInstrument:ST.state.project.tracks[ST.state.track]?.instrument,instrumentType:ST.state.project.instruments[ST.state.instrument]?.type,
                  nksModuleLoaded:!!window.STNKSControls,nksPanelPresent:!!document.querySelector('[data-nks-root]'),mappedInputPresent:!!input,mappedInputDisabled:input?.disabled,
                  plugins:(ST.state.project.audioPlugins||[]).map(describe)};
                """)
            throw TrackerError.message("WEBKIT NKS AUTOMATION: \(message). \(String(describing: state))")
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
                ST.closeModal();
                if(window.__nksAutomationOriginal){
                  ST.acceptProject(window.__nksAutomationOriginal);
                  if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Fixture cleanup was superseded');
                }
                delete window.__nksAutomationOriginal;delete window.__nksAutomationSaved;
                delete window.__nksAutomationBeforeStep;delete window.__nksAutomationStepUndo;
                return true;
                """)
        }
        func render(_ name: String) async throws -> DecodedSample {
            let file = folder.appendingPathComponent(name + ".wav")
            _ = try await js("if(!await ST.captureNativeEdits())throw Error('Native edit capture failed');return await ST.request('audio.export',{path:path,mode:'song',tail:0});", ["path": file.path])
            let pcm = try AudioSamples.decode(file.path)
            try require(pcm.sampleRate == 48_000 && pcm.left.count == 24_000 && pcm.right.count == 24_000,
                        "The \(name) export has the wrong sample rate or duration.")
            try require(pcm.left.allSatisfy(\.isFinite) && pcm.right.allSatisfy(\.isFinite), "The \(name) export contains non-finite samples.")
            return pcm
        }
        func change(_ id: String, _ value: String) async throws {
            _ = try await js("const input=document.getElementById(id);if(!input)throw Error('Missing '+id);input.value=value;input.dispatchEvent(new Event('change',{bubbles:true}));return true;", ["id": id, "value": value])
        }

        var rows = [Any](repeating: NSNull(), count: 4)
        rows[0] = ["note": 60, "instrument": 0, "velocity": 127, "gate": 3]
        let fixture: [String: Any] = ["version": 1, "name": "Mounted NKS automation fixture",
            "bpm": 120, "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0,
            "loopSong": false, "metronome": false,
            "tracks": [["id": 0, "routingID": "mounted-nks-original", "name": "Mapped fixture",
                        "instrument": 0, "volume": 1, "pan": 0, "delaySend": 0, "reverbSend": 0]],
            "instruments": [["id": 0, "name": "Original mapped instrument", "type": "au", "volume": 1, "pan": 0]],
            "patterns": [["id": 0, "name": "Mapped automation", "length": 4, "steps": [rows], "automation": []]],
            "sequence": [0], "audioClips": [], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        // Slot zero deliberately maps a large, non-index AU address. The second
        // named slot is unassigned, so an accidental position-based mapping fails.
        let metadata: [String: Any] = ["name": "Original fixture mappings", "identity": "synthetic-original-au",
            "pages": [["name": "Fixture tone", "parameters": [
                ["slot": 0, "name": "Fixture brightness", "section": "Original tone", "parameterID": address,
                 "mapped": true, "mappingStatus": "mapped"] as [String: Any],
                ["slot": 1, "name": "Unassigned fixture slot", "mapped": false, "mappingStatus": "unassigned"]]],
                ["name": "Performance fixture", "parameters": [
                    ["slot": 0, "name": "Fixture color alias", "section": "Second-page alias", "parameterID": address,
                     "mapped": true, "mappingStatus": "mapped"] as [String: Any]]]]]
        try report(false)
        do {
            _ = try await js("window.__nksAutomationOriginal=window.__supertracker.getProject();await ST.request('transport.stop');ST.acceptProject(project);if(await ST.syncEngineProject(()=>window.__supertracker.getProject(),{replace:true})===false)throw Error('Fixture setup was superseded');return true;", ["project": fixture])
            evidence["fixtureSetup"] = try await js("""
                const loaded=await ST.request('plugin.load',{track:0,kind:'instrument',slot:0,component:component,nks:metadata});
                ST.mutate('Install original fixture mapping',p=>{p.audioPlugins=[loaded];p.instruments[0].plugin={...loaded};},{render:false});
                if(await ST.syncEngineProject(()=>window.__supertracker.getProject())===false)throw Error('Fixture mapping synchronization was superseded');
                if(!await ST.captureNativeEdits())throw Error('Fixture synchronization failed');ST.state.track=0;ST.state.instrument=0;ST.state.row=0;ST.setView('instrument');
                const describe=plugin=>({track:plugin.track,kind:plugin.kind,slot:plugin.slot,component:plugin.component,instanceID:plugin.instanceID,hasNKS:!!plugin.nks,pages:plugin.nks?.pages?.length??0});
                return {loaded:describe(loaded),synchronized:(ST.state.project.audioPlugins||[]).map(describe)};
                """, ["component": identifier, "metadata": metadata])
            try await wait("document.getElementById('nks-0-0-range')&&!document.getElementById('nks-0-0-range').disabled&&ST.state.pending.size===0", "Mapped controls did not resolve through the actual native parameter list")
            try await check("document.getElementById('nks-0-0-range').dataset.nksAddress==='\(parameterID)'&&document.getElementById('nks-0-0-range').dataset.automate==='\(laneTarget)'&&document.getElementById('nks-0-1-range').disabled", "Mapped and unassigned slots did not preserve their exact identities")
            evidence["initialParameters"] = try await js("return await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0});")
            checks.append("Mounted NKS controls resolve the original AU's exact non-index parameter address and disable unassigned slots")

            let savedURL = scratch.appendingPathComponent("Mounted NKS automation.supertracker")
            _ = try await js("const saved=await ST.request('project.save',{path:path,project:window.__supertracker.getProject()});ST.acceptProject(saved.project);ST.setView('instrument');return true;", ["path": savedURL.path])
            try await wait("document.getElementById('nks-0-0-number')&&!document.getElementById('nks-0-0-number').disabled", "Saved mapped controls did not remount")
            // Only input is dispatched: saving must drain the pending control
            // draft even though no browser change/blur default was simulated.
            let saved = try await js("const input=document.getElementById('nks-0-0-number');input.focus();input.value='0.625';input.dispatchEvent(new Event('input',{bubbles:true}));return await ST.projectAction('save');") as? Bool ?? false
            try require(saved, "Save did not accept an outstanding mapped-control draft.")
            try await check("Math.abs((await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0})).find(x=>String(x.address)==='\(parameterID)').normalized-.625)<.000001", "Immediate Save missed the mapped-control draft")
            try require(try await js("return await ST.projectAction('open',path);", ["path": savedURL.path]) as? Bool == true, "The saved mapped instrument did not reopen")
            try await check("Math.abs((await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0})).find(x=>String(x.address)==='\(parameterID)').normalized-.625)<.000001", "Reopened AU state lost the edited mapped value")
            let reference = try await render("mapped-value-reference")
            let referenceLeft = reference.left[0], referenceRight = reference.right[0]
            try require(referenceLeft > 0.001 && referenceRight > 0.001, "The mapped instrument reference is silent")
            var referenceError: Float = 0
            for frame in 0..<24_000 {
                referenceError = max(referenceError, abs(reference.left[frame] - (frame < 18_000 ? referenceLeft : 0)))
                referenceError = max(referenceError, abs(reference.right[frame] - (frame < 18_000 ? referenceRight : 0)))
            }
            try require(referenceError <= 0.000001, "The original AU reference is not constant through its exact MIDI gate")
            checks.append("Immediate native Save drains an input-only NKS draft and Open restores the audible mapped AU state")

            _ = try await js("ST.state.row=0;ST.setView('instrument');return true;")
            try await wait("document.getElementById('nks-0-0-range')&&!document.getElementById('nks-0-0-range').disabled", "Mapped control did not return after export")
            _ = try await js("const input=document.getElementById('nks-0-0-range');input.focus();input.value='0.75';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'å',code:'KeyA',altKey:true,bubbles:true,cancelable:true}));return true;")
            try await wait("ST.state.view==='automation'&&ST.pattern().automation.length===1", "Mac Option+A shape did not create a mapped lane")
            try await check("ST.pattern().automation[0].target==='\(laneTarget)'&&ST.pattern().automation[0].name==='Fixture brightness'&&ST.pattern().automation[0].track===0&&Math.abs(ST.pattern().automation[0].points[0].value-.75)<.000001", "Option+A lost the mapped parameter identity, name, channel, or pending value")
            try await check("Math.abs((await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0})).find(x=>String(x.address)==='\(parameterID)').normalized-.75)<.000001", "Option+A opened the lane before settling the pending mapped value")
            for query in ["Fixture brightness", "Fixture tone", "Fixture color alias", "Performance fixture", "Original AU level"] {
                _ = try await js("const input=document.getElementById('automation-search');input.value=query;input.dispatchEvent(new Event('input',{bubbles:true}));return true;", ["query": query])
                try await wait("document.querySelectorAll('[data-choose-target]').length===1", "Automation search did not resolve \(query) to one target")
                try await check("document.querySelector('[data-choose-target]').dataset.chooseTarget==='\(laneTarget)'&&document.querySelector('[data-choose-target]').dataset.targetName==='Fixture brightness'&&document.querySelector('.studio-list-caption').textContent.includes('Already automated')", "Mapped name, page, alias, or original host-name search resolved a different automation identity")
            }
            _ = try await js("const input=document.getElementById('automation-search');input.value='';input.dispatchEvent(new Event('input',{bubbles:true}));return true;")
            evidence["automationSearches"] = ["Fixture brightness", "Fixture tone", "Fixture color alias", "Performance fixture", "Original AU level"]
            try await change("automation-interpolation", "step")
            try await change("point-value-0", "0")
            _ = try await js("document.getElementById('automation-add-point').click();return true;")
            try await change("point-row-1", "1")
            try await change("point-value-1", "1")
            _ = try await js("document.getElementById('automation-add-point').click();return true;")
            try await change("point-row-2", "2")
            try await change("point-value-2", "0.5")
            try await check("JSON.stringify(ST.pattern().automation[0].points)===JSON.stringify([{row:0,value:0},{row:1,value:1},{row:2,value:.5}])", "Mounted breakpoint controls did not create the intended envelope")
            try await change("automation-name", "Mapped brightness motion")
            let savedAutomation = try await js("return ST.pattern().automation;")
            try require(try await js("return await ST.projectAction('save');") as? Bool == true, "Automation Save failed")
            try require(try await js("return await ST.projectAction('open',path);", ["path": savedURL.path]) as? Bool == true, "Automation Open failed")
            try require(try sameJSON(savedAutomation, try await js("return ST.pattern().automation;")), "Save/Open changed the mapped lane")
            try await check("ST.state.project.audioPlugins[0].nks.pages[0].parameters[0].parameterID===\(address)", "Save/Open changed the original NKS mapping")
            evidence["savedLane"] = try await js("return ST.pattern().automation[0];")
            checks.append("Mac Option+A settles an input-only mapped draft; mapped names, pages, aliases and host names find the same existing lane")
            checks.append("Mounted breakpoint editing and real Save/Open preserve the mapped lane's exact target and envelope")

            let automated = try await render("mapped-envelope-reopened")
            var maximumDifference: Float = 0
            var firstSignal = -1, lastSignal = -1
            for frame in 0..<24_000 {
                let multiplier: Float = frame < 6_000 ? 0 : frame < 12_000 ? 1 / 0.625 : frame < 18_000 ? 0.5 / 0.625 : 0
                maximumDifference = max(maximumDifference, abs(automated.left[frame] - referenceLeft * multiplier))
                maximumDifference = max(maximumDifference, abs(automated.right[frame] - referenceRight * multiplier))
                if abs(automated.left[frame]) > 0.000001 { if firstSignal < 0 { firstSignal = frame }; lastSignal = frame }
            }
            evidence["audio"] = ["sampleRate": 48_000, "frames": 24_000, "referenceGain": 0.625,
                "referenceLeft": referenceLeft, "referenceRight": referenceRight, "referenceMaximumError": referenceError,
                "expectedPointFrames": [0, 6_000, 12_000], "expectedNoteOffFrame": 18_000,
                "firstSignalFrame": firstSignal, "lastSignalFrame": lastSignal, "maximumExpectedPCMDifference": maximumDifference]
            try require(maximumDifference <= 0.000001 && firstSignal == 6_000 && lastSignal == 17_999,
                        "Mapped envelope did not reach AU audio at exact row boundaries: maximum error \(maximumDifference), signal \(firstSignal)...\(lastSignal)")
            checks.append("Reopened mapped automation drives actual AU PCM at frames 6000 and 12000, with exact frame-18000 note-off")

            // A mapped knob can create an independent pattern lock. Merely opening
            // its picker or changing a draft must never edit the cell or live AU.
            let originalSize = web.frame.size
            web.setFrameSize(NSSize(width: 1080, height: 700))
            try await wait("innerWidth===1080&&innerHeight===700", "Compact viewport did not resize")
            _ = try await js("ST.state.track=0;ST.state.instrument=0;ST.state.row=1;ST.state.column=3;ST.setView('instrument');return true;")
            try await wait("document.getElementById('nks-0-0-step')&&!document.getElementById('nks-0-0-step').disabled", "Mapped Use on step control is unavailable")
            let beforePicker = try await js("return await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0});")
            _ = try await js("window.__nksAutomationBeforeStep=JSON.stringify(ST.pattern().steps);window.__nksAutomationStepUndo=ST.state.undo.length;const button=document.getElementById('nks-0-0-step');button.scrollIntoView({block:'center'});button.focus();button.click();return true;")
            try await wait("ST.state.modal==='stepFX'&&document.getElementById('step-fx-value')&&!document.getElementById('step-fx-value').disabled&&document.getElementById('step-fx-choice').textContent.includes('Fixture brightness')", "Mapped Use on step did not select its exact native parameter")
            try await wait("document.activeElement?.id==='step-fx-search'", "Step effect picker did not retain initial search focus")
            _ = try await js("const value=document.getElementById('step-fx-value');value.value='.125';value.dispatchEvent(new Event('input',{bubbles:true}));document.getElementById('step-fx-cancel').click();return true;")
            try await wait("!ST.state.modal&&document.activeElement?.id==='nks-0-0-step'", "Cancelling step effect selection did not return focus to its mapped control")
            try await check("JSON.stringify(ST.pattern().steps)===window.__nksAutomationBeforeStep&&ST.state.undo.length===window.__nksAutomationStepUndo", "Cancelled step effect selection changed pattern data or Undo history")
            let afterPicker = try await js("return await ST.request('plugin.parameters',{track:0,kind:'instrument',slot:0});")
            try require(try JSONSerialization.data(withJSONObject: beforePicker, options: [.sortedKeys]) == JSONSerialization.data(withJSONObject: afterPicker, options: [.sortedKeys]), "Step effect picker changed the live AU parameter")
            checks.append("Mapped Use on step selects the exact native control; cancelling preserves pattern, Undo, live parameter and keyboard focus")

            _ = try await js("document.getElementById('nks-0-0-step').click();return true;")
            try await wait("ST.state.modal==='stepFX'&&document.getElementById('step-fx-value')&&!document.getElementById('step-fx-value').disabled", "Mapped step effect picker did not reopen")
            _ = try await js("const input=document.getElementById('step-fx-search');input.value='Fixture color alias';input.dispatchEvent(new Event('input',{bubbles:true}));return true;")
            try await wait("document.querySelectorAll('[data-fx-result]').length===1", "Step effect search did not resolve the second-page mapped alias")
            _ = try await js("document.getElementById('step-fx-search').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true}));return true;")
            try await check("document.activeElement?.id==='step-fx-value'", "Return on a step effect result did not focus its exact value")
            try await check("(()=>{const r=document.activeElement.getBoundingClientRect();return r.width>0&&r.height>0&&r.top>=0&&r.left>=0&&r.bottom<=innerHeight&&r.right<=innerWidth;})()", "Focused step effect value is clipped in the compact viewport")
            let snapshotConfiguration = WKSnapshotConfiguration(); snapshotConfiguration.rect = web.bounds
            let snapshot = try await web.takeSnapshot(configuration: snapshotConfiguration)
            if let data = snapshot.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data), let png = bitmap.representation(using: .png, properties: [:]) {
                try png.write(to: folder.appendingPathComponent("mapped-step-picker-1080x700.png"))
            }
            _ = try await js("const input=document.getElementById('step-fx-value');input.value='.25';input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true}));return true;")
            try await wait("!ST.state.modal&&ST.pattern().steps[0][1]?.fx1?.type==='plugin'", "Return did not apply the mapped step effect")
            try await check("ST.pattern().steps[0][1].note===-3&&ST.pattern().steps[0][1].fx1.address==='\(parameterID)'&&ST.pattern().steps[0][1].fx1.instanceID===ST.state.project.audioPlugins[0].instanceID&&ST.pattern().steps[0][1].fx1.value===.25&&ST.state.undo.length===window.__nksAutomationStepUndo+1", "Mapped step effect created a note, lost identity/value, or used more than one Undo entry")
            let savedStepEffect = try await js("return ST.pattern().steps[0][1];")
            try require(try await js("return await ST.projectAction('save');") as? Bool == true, "Mapped step effect Save failed")
            try require(try await js("return await ST.projectAction('open',path);", ["path": savedURL.path]) as? Bool == true, "Mapped step effect Open failed")
            try require(try sameJSON(savedStepEffect, try await js("return ST.pattern().steps[0][1];")), "Save/Open changed the effect-only row or exact mapped parameter identity")
            evidence["savedStepEffect"] = try await js("return ST.pattern().steps[0][1];")
            let locked = try await render("mapped-step-lock-reopened")
            var lockDifference: Float = 0, nextRowDifference: Float = 0
            for frame in 0..<24_000 {
                let multiplier: Float = frame < 6_000 ? 0 : frame < 12_000 ? 0.25 / 0.625 : frame < 18_000 ? 0.5 / 0.625 : 0
                lockDifference = max(lockDifference, abs(locked.left[frame] - referenceLeft * multiplier))
                lockDifference = max(lockDifference, abs(locked.right[frame] - referenceRight * multiplier))
                if frame >= 12_000 {
                    nextRowDifference = max(nextRowDifference, abs(locked.left[frame] - automated.left[frame]))
                    nextRowDifference = max(nextRowDifference, abs(locked.right[frame] - automated.right[frame]))
                }
            }
            evidence["stepLockAudio"] = ["frames": 24_000, "lockFrame": 6_000, "laneResumptionFrame": 12_000,
                "maximumExpectedPCMDifference": lockDifference, "maximumNextRowLaneDifference": nextRowDifference,
                "viewport": ["width": 1080, "height": 700], "focusedValueVisible": true]
            try require(lockDifference <= 0.000001 && nextRowDifference <= 0.000001,
                        "Effect-only row did not override its lane for exactly one row: lock error \(lockDifference), resumption error \(nextRowDifference)")
            checks.append("Mapped step effect keyboard choice saves an effect-only row and actual PCM overrides its lane for one row before resuming")
            web.setFrameSize(originalSize)
            try await cleanup()
            try report(true)
        } catch {
            try? await cleanup()
            try? report(false, error.localizedDescription)
            throw error
        }
        return checks
    }
}

/// Constant MIDI instrument, with one audible parameter at a deliberately sparse
/// address. All storage is allocated before rendering; no installed code is used.
private final class NativeUINKSAutomationInstrument: AUAudioUnit {
    private let gainAddress: AUParameterAddress
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree!
    private var left = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var right = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var capacity = 16_384
    private var heldNote = -1
    private var gain: AUValue = 0.25
    private var savedGain: AUValue = 0.25

    override init(componentDescription: AudioComponentDescription, options: AudioComponentInstantiationOptions = []) throws {
        gainAddress = componentDescription.componentSubType == NativeUINKSAutomationFixture.wideSubtype ? NativeUINKSAutomationFixture.wideAddress : NativeUINKSAutomationFixture.mappedAddress
        left.initialize(repeating: 0, count: capacity); right.initialize(repeating: 0, count: capacity)
        try super.init(componentDescription: componentDescription, options: options)
        let bus = try AUAudioUnitBus(format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!)
        bus.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [bus])
        let level = AUParameterTree.createParameter(withIdentifier: "fixtureLevel", name: "Original AU level",
            address: gainAddress, min: 0, max: 1, unit: .linearGain, unitName: nil,
            flags: [.flag_IsReadable, .flag_IsWritable], valueStrings: nil, dependentParameters: nil)
        parameters = AUParameterTree.createTree(withChildren: [level])
        parameters.implementorValueObserver = { [weak self] _, value in self?.savedGain = value; self?.gain = value }
        parameters.implementorValueProvider = { [weak self] _ in self?.gain ?? 0.25 }
        level.value = 0.25; maximumFramesToRender = 16_384
    }
    deinit {
        left.deinitialize(count: capacity); left.deallocate()
        right.deinitialize(count: capacity); right.deallocate()
    }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [0, 2] }
    override var parameterTree: AUParameterTree? { get { parameters } set { parameters = newValue } }
    override var fullState: [String: Any]? {
        get { ["originalFixture": "NKS UI automation", "gain": Double(savedGain)] }
        set { if let value = newValue?["gain"] as? NSNumber { parameters.parameter(withAddress: gainAddress)?.value = AUValue(value.doubleValue) } }
    }
    override var fullStateForDocument: [String: Any]? { get { fullState } set { fullState = newValue } }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 && format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }
    override func allocateRenderResources() throws {
        let requested = max(16_384, Int(maximumFramesToRender))
        if requested > capacity {
            left.deinitialize(count: capacity); left.deallocate(); right.deinitialize(count: capacity); right.deallocate()
            capacity = requested; left = .allocate(capacity: requested); right = .allocate(capacity: requested)
            left.initialize(repeating: 0, count: requested); right.initialize(repeating: 0, count: requested)
        }
        reset(); try super.allocateRenderResources()
    }
    override func reset() { heldNote = -1; gain = savedGain; super.reset() }
    override var internalRenderBlock: AUInternalRenderBlock {
        { [unowned self] flags, timestamp, frames, bus, outputData, events, _ in
            guard bus == 0 else { return kAudioUnitErr_InvalidElement }
            let count = Int(frames)
            guard count <= capacity else { return kAudioUnitErr_TooManyFramesToProcess }
            let buffers = UnsafeMutableAudioBufferListPointer(outputData)
            guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
            let bytes = UInt32(count * MemoryLayout<Float>.size)
            if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(left); buffers[0].mDataByteSize = bytes }
            if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(right); buffers[1].mDataByteSize = bytes }
            guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes,
                  let outLeft = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let outRight = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            flags.pointee.remove(.unitRenderAction_OutputIsSilence)
            let sampleTime = AUEventSampleTime(timestamp.pointee.mSampleTime)
            var next = events
            for frame in 0..<count {
                while let event = next, event.pointee.head.eventSampleTime <= sampleTime + AUEventSampleTime(frame) {
                    switch event.pointee.head.eventType {
                    case .MIDI:
                        let midi = event.pointee.MIDI
                        if midi.length >= 3 {
                            let kind = midi.data.0 & 0xf0, note = Int(midi.data.1)
                            if kind == 0x90 && midi.data.2 > 0 { heldNote = note }
                            else if (kind == 0x80 || (kind == 0x90 && midi.data.2 == 0)) && heldNote == note { heldNote = -1 }
                            else if kind == 0xb0 && (midi.data.1 == 120 || midi.data.1 == 123) { heldNote = -1 }
                        }
                    case .parameter, .parameterRamp:
                        if event.pointee.parameter.parameterAddress == gainAddress { gain = max(0, min(1, event.pointee.parameter.value)) }
                    default: break
                    }
                    next = event.pointee.head.next.map { UnsafePointer($0) }
                }
                let sample: Float = heldNote >= 0 ? 0.25 * gain : 0
                outLeft[frame] = sample; outRight[frame] = sample
            }
            buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
            return noErr
        }
    }
}

#endif

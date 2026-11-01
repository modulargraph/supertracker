import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// The native audio host: the tracker engine, the AVAudioEngine graph with its plug-in
/// rack, MIDI and audio I/O. Commands arrive through `handle` (AudioFreezeTrack.swift) and
/// `handleCore` (AudioHost+Commands.swift); the extensions in AudioHost+*.swift hold the rest.
final class AudioHost {
    var onEvent: ((String, [String: Any]) -> Void)?
    let graph:AVAudioEngine
    var engine: OpaquePointer
    var inputCapture:AudioInputCapture!
    var inputArmGeneration:UInt64 = 0
    var midiCaptureOwners:[String:(owner:String,onsetID:UInt64?)] = [:]
    var rack: AudioPluginRack!
    /// Diagnostics: publications applied as pattern data only (see applyPatternDataOnly).
    var patternDataPublications: UInt64 = 0
    let midi: MIDIHost
    let recoveryStore: PluginRecoveryStore?

    // Audio callbacks alone serialize the C multibus cache and event consumer.
    // Control/UI publication must never acquire this lock.
    let renderLock = NSLock()

    /// Canonical project for user Track Freeze (AudioFreezeTrack.swift); `project` is its runtime projection.
    let freezeTrackRuntime = AudioFreezeTrackRuntime()
    let renderControls = AudioRenderControl()
    let renderHeartbeat = RenderHeartbeat()
    let renderClockDiagnostics:AudioRenderClockDiagnostics? = ProcessInfo.processInfo.environment["SUPERTRACKER_RENDER_CLOCK_DIAGNOSTICS"] == "1" ? AudioRenderClockDiagnostics() : nil
    var lastRenderHeartbeat:RenderHeartbeat.Snapshot?
    let sampleRate = 48000.0

    // Serialized with renderLock; reset only after graph quiescence.
    var midiTimebase = AudioMIDITimebase(ticksPerFrame:Double(AVAudioTime.hostTime(forSeconds:1.0 / 48000.0)))
    var nativeSourceUsesHostClock = true
    var sources: [AVAudioSourceNode] = []
    var nativeFormat = AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!

    /// Track outputs 0..<meterTaps carry a post-insert meter tap.
    var meterTaps = 0
    var project: [String: Any] = [:] { didSet { projectLists = nil } }
    /// The project's instrument and track lists, converted once per project change (live MIDI reads them per message).
    var projectLists: (instruments:[[String:Any]],tracks:[[String:Any]])?
    var liveInstruments: [[String:Any]] {
        if let projectLists { return projectLists.instruments }
        let lists=(instruments:ProjectLayout.instruments(project),tracks:project["tracks"] as? [[String:Any]] ?? [])
        projectLists=lists; return lists.instruments
    }
    var liveTracks: [[String:Any]] { _ = liveInstruments; return projectLists?.tracks ?? [] }
    var exportOriginalProject:[String:Any]?
    var unavailableProject:[String:Any]?
    var routingFailure:String?
    var samples: [Int: DecodedSample] = [:]
    var builtinSamples: [String: DecodedSample] = [:]

    /// Samples decoded for instruments the next project publication adds (see sample.load).
    var stagedSamples: [String: DecodedSample] = [:]
    var sampleTasks: [Int: Task<Void, Never>] = [:]
    var pendingSamplePaths: [Int: String] = [:]
    var zoneSignatures: [Int: String] = [:]
    var zoneTasks: [Int: Task<Void, Never>] = [:]
    var clipAssetSlots:[String:Int] = [:]
    var clipTasks:[String:Task<Void,Never>] = [:]
    var clipMetadata:[String:[String:Any]] = [:]
    var sampleRequestGeneration:[Int:UInt64] = [:]
    var projectGeneration:UInt64 = 0
    var projectIdentity:UInt64 = 0
    var playRequestGeneration:UInt64 = 0
    var playPreparations = 0
    var previewGeneration:UInt64 = 0
    var previewPreparations = 0
    var graphMutationsInFlight = 0
    var directPluginLoads:[UUID:Task<Any,Error>] = [:]
    var restoreTask: Task<Void, Never>?
    var pluginRestoreRevision:UInt64 = 0
    var pendingPluginRestore:UInt64?
    var appliedPluginSignature = ""
    var stateCaptureOwner:UUID?
    var stateCaptureWaiters:[(id:UUID,continuation:CheckedContinuation<Void,Error>)] = []
    var midiRoutes: [MIDIRoute] = Array(repeating: MIDIRoute(), count: AudioRenderControl.routeCount)
    /// Lead for notes to External MIDI instruments heard back through their track's line input (refreshMIDIClock).
    var returnedNoteLeadSeconds = 0.0
    var midiTrack = 0
    var midiInstrument = 0
    var midiInputChannel = 0
    var midiSustain = MIDISustainState(destinationCount: Int(st_channel_count(Int32(ST_TRACK_SANITY_LIMIT))))

    /// Sustain source for notes the app plays itself (note.on/note.off: pads, piano, computer keyboard).
    /// Only a `midi.expression` sustain pedal holds it down; hardware MIDI inputs keep their own sources.
    static let appNoteSource: Int32 = -2

    /// Last live expression value sent per destination, so repeated identical values cost nothing.
    var lastExpression: [String: Int] = [:]
    var heldNotes = Set<Int>()
    var midiNoteDestinations:[String:(track:Int,instrument:Int)] = [:]
    var inputMixer: AVAudioMixerNode?
    var inputMonitorSelector: AudioLineInput?
    var lineInputs:[Int:AudioLineInput] = [:]
    var lineInputSignature = ""

    /// Times line-input routing stopped the graph to rewire it (self-test evidence).
    var lineInputGraphRebuilds = 0
    var lineInputError:String?

    /// Enabled routes kept as saved but not live (interface absent), with their status text.
    var lineInputPaused:[Int:String] = [:]

    /// Live routes reached through an aggregate: engine channels and the aggregate's name.
    var lineInputMapped:[Int:(channels:[Int],via:String)] = [:]
    var lineInputReporter = LineInputProblemReporter()
    var lineInputDeviceObserver: AudioDeviceListObserver?
    /// Re-applies the saved interface when it appears after launch (AudioHost+DevicePreferences).
    var savedDeviceObserver: AudioDeviceListObserver?
    var lineInputRecheckUntil: Date?
    var lineInputAutoStart = false
    /// A MIDI route published while plug-ins were restoring: audio starts once they have loaded.
    var midiAutoStart = false
    var inputTapInstalled = false

    // Source-render-owned pairing; retained recordingHistory pins these pointers.
    var masterObservedRecording:Int64 = 0
    var recording: AudioRecording?
    var recordingStopTask:Task<[String:Any],Error>?
    var recordingStopping = false
    var audioConfigurationsInFlight = 0
    var recordingEpoch:UInt64 = 0

    // Session-local note capture intent; never saved as project content. Audio
    // recording contributes independently to the transport's recording flag.
    var noteRecordArmed = false
    let meters = AudioMeters(trackCapacity:Int(ST_TRACK_SANITY_LIMIT))
    let masterRecordingEndpoint:UnsafeMutablePointer<Int64> = {
        let pointer = UnsafeMutablePointer<Int64>.allocate(capacity:1); pointer.initialize(to:0); return pointer
    }()
    let inputRecordingEndpoint:UnsafeMutablePointer<Int64> = {
        let pointer = UnsafeMutablePointer<Int64>.allocate(capacity:1); pointer.initialize(to:0); return pointer
    }()
    var recordingHistory:[AudioRecording] = []

    /// Browser/sampler audition: one persistent source node with crossfading
    /// voices and a decoded-preview cache (AudioPreview.swift).
    var previewPlayer: AudioPreviewPlayer?
    var exporting = false
    var exportCancelled = false
    var selectedInputChannels:[Int] = [0,1]
    var selectedInputDevice:UInt32?
    var selectedOutputDevice:UInt32?
    /// Where the Settings audio interface choice is kept (nil: not remembered, as in tests).
    var devicePreferencesURL:URL?
    var midiClockPreferencesURL:URL?
    let midiClock = MIDIClockOutput()
    var restoringDevicePreferences=false
    lazy var deviceSelection=AudioDeviceSelection(engine:graph)
    var timer: Timer?
    /// Sample editor playheads (sampler.playheads): the watched instrument (-1 all) and the display-rate poll.
    var playheadTimer: Timer?
    var playheadWatch = -2
    var playheadsShown = false
    var latencyPollTicks = 0
    var renderCacheNeedsReset = true
    var renderObserverToken: Int?
    var masterRenderUtilization = AudioRenderUtilization()
    var recordingConfigurationObserver:NSObjectProtocol?
    var realtimeAudioStarted = false
    var deviceRecoveryPending = false
    let routingAutomationReset:UnsafeMutablePointer<Int32> = {
        let pointer = UnsafeMutablePointer<Int32>.allocate(capacity:1); pointer.initialize(to:0); return pointer
    }()
    /// Render quanta since launch (audio thread increments). The stall watchdog
    /// restarts a graph that reports running while its device delivers no cycles.
    let renderQuanta:UnsafeMutablePointer<Int64> = {
        let pointer = UnsafeMutablePointer<Int64>.allocate(capacity:1); pointer.initialize(to:0); return pointer
    }()
    var stallWatch = AudioStallWatch()
    var closed = false
    var engineDestroyed = false
    var shutdownTask:Task<Void,Never>?
    var tempo = 120.0
    var songModeRequested = true
    let hostTicksPerFrame = Double(AVAudioTime.hostTime(forSeconds:1.0 / 48000.0))
    let secondsPerHostTick:Double = {
        var info = mach_timebase_info_data_t(); mach_timebase_info(&info)
        return Double(info.numer)/Double(info.denom)/1e9
    }()
    typealias MIDIRoute = AudioMIDIRoute
    @MainActor init(beforePluginInstantiation: (() async throws -> Void)? = nil, recoveryStore: PluginRecoveryStore? = nil, beforeEffectOrderResume:(() throws -> Void)? = nil, audioGraph:AVAudioEngine? = nil, vst3Catalogue:VST3Catalogue = .shared, midiInputIDs:[Int32]? = nil, midiInputSourceProvider:(()->[UInt32])? = nil, midiOutputDestinationProvider:(()->[UInt32])? = nil, vst2Catalogue:VST2Catalogue = .shared) throws {
        self.graph = audioGraph ?? AVAudioEngine()
        self.recoveryStore = recoveryStore
        SampleReferences.install()
        // The model layer mirrors the engine's channel numbering in pure Swift; they must agree.
        let layoutAgrees = ProjectLayout.trackSanityLimit == Int(ST_TRACK_SANITY_LIMIT) && ProjectLayout.minimumReturnBus == Int(ST_MIN_RETURN_BUS)
            && ProjectLayout.instrumentSanityLimit == Int(ST_INSTRUMENT_SANITY_LIMIT)
            && AudioRenderControl.routeCount == ProjectLayout.instrumentSanityLimit
        precondition(layoutAgrees && [1,15,16,17,64,200,1024].allSatisfy { ProjectLayout.returnBus(trackCount:$0) == Int(st_return_bus(Int32($0)))
            && ProjectLayout.channelCount(trackCount:$0) == Int(st_channel_count(Int32($0))) })
        guard let pointer = st_engine_create(sampleRate) else { throw AudioSampleError.invalid("Unable to allocate the audio engine.") }
        engine = pointer
        do { midi = try MIDIHost(inputIDs:midiInputIDs,inputSourceProvider:midiInputSourceProvider,outputDestinationProvider:midiOutputDestinationProvider) } catch { st_engine_destroy(pointer); throw error }
        inputCapture = AudioInputCapture(engine:pointer)
        rack = try AudioPluginRack(engine: graph, trackCount:ProjectLayout.channelCapacity, sampleRate: sampleRate, recoveryStore: recoveryStore,vst3Catalogue:vst3Catalogue,vst2Catalogue:vst2Catalogue)
        rack.onEngineStopped = { [weak self] in self?.renderCacheNeedsReset = true }
        rack.onInstrumentsMoved = { [weak self] moves in self?.forgetMovedInstrumentVoices(moves) }
        rack.beforeEngineStart = { [weak self] in
            guard let self else { throw AudioSampleError.invalid("The audio host is closed.") }
            try self.ensureProcessingOutputFormat()
            try self.resetRenderCacheIfNeeded()
        }
        rack.beforeInstantiation = beforePluginInstantiation
        rack.beforeEffectOrderResume = beforeEffectOrderResume
        rack.onRecoveryError = { [weak self] message in self?.emit("plugin.error", ["error":message]) }
        rack.onLatencyPlanChanged = { [weak self] in
            guard let self else { return }
            _ = OSAtomicCompareAndSwap32Barrier(0,1,self.routingAutomationReset)
        }
        rack.onHostAutomationMapChanged = { [weak self] in
            guard let self, !self.engineDestroyed else { return }
            st_engine_refresh_automation(self.engine)
        }
        guard let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) else { throw AudioSampleError.invalid("Cannot create stereo audio format.") }
        nativeFormat = format
        for channel in 0..<rack.realizedStrips { sources.append(makeNativeSource(channel:Int32(channel))) }
        let returnSource = makeNativeSource(channel:-1)
        rack.installNativeSources(sources,returns:[returnSource])
        sources.append(returnSource)
        // Attached once and never reconnected: previews never pause the graph.
        let previewPlayer = AudioPreviewPlayer(format: format)
        previewPlayer.kitPreviewResolver = { KitLibrary.previewURL(for: URL(fileURLWithPath: $0))?.path }
        self.previewPlayer = previewPlayer
        graph.attach(previewPlayer.node); graph.connect(previewPlayer.node, to: graph.mainMixerNode, format: format)
        renderObserverToken = graph.mainMixerNode.auAudioUnit.token(byAddingRenderObserver: { [weak self] flags,time,frames,_ in
            guard let self else { return }
            if flags.contains(.unitRenderAction_PreRender) {
                self.masterRenderUtilization.begin(at:mach_absolute_time(),frames:frames)
                let endpoint = OSAtomicAdd64Barrier(0,self.masterRecordingEndpoint)
                self.masterObservedRecording = endpoint
                if endpoint != 0, let pointer = UnsafeRawPointer(bitPattern:UInt(UInt64(bitPattern:endpoint))) {
                    Unmanaged<AudioRecording>.fromOpaque(pointer).takeUnretainedValue().observeRender(time.pointee,frames:frames)
                }
                self.renderHeartbeat.beganRender()
                self.rack.beginRenderRead()
                self.prepareRender(time,frames:frames)
            }
            // AUAudioUnit's observer contract delivers post-render even for render
            // errors. Keep ownership until every downstream branch has completed.
            if flags.contains(.unitRenderAction_PostRender) {
                if let percent = self.masterRenderUtilization.finish(at:mach_absolute_time(),frames:frames,
                    sampleRate:self.sampleRate,secondsPerTick:self.secondsPerHostTick) {
                    self.renderControls.storeCPU(percent)
                }
                if flags.contains(.unitRenderAction_PostRenderError) {
                    let endpoint = self.masterObservedRecording
                    if endpoint != 0, let pointer = UnsafeRawPointer(bitPattern:UInt(UInt64(bitPattern:endpoint))) {
                        Unmanaged<AudioRecording>.fromOpaque(pointer).takeUnretainedValue().observeRender(time.pointee,frames:frames,failed:true)
                    }
                }
                self.masterObservedRecording = 0
                self.rack.endRenderRead()
                self.renderHeartbeat.completedRender(at:mach_absolute_time(),succeeded:!flags.contains(.unitRenderAction_PostRenderError))
            }
        })
        while meterTaps < min(Int(ST_MIN_RETURN_BUS),rack.realizedStrips) {
            let track = meterTaps
            rack.outputNode(track:track)?.installTap(onBus:0,bufferSize:512,format:nil) { [weak self] buffer,time in
                self?.meters.measure(buffer,bus:track)
                self?.freezeTrackRuntime.tap.receive(buffer,at:time,track:track)
            }
            meterTaps += 1
        }
        graph.mainMixerNode.installTap(onBus:0,bufferSize:512,format:nil) { [weak self] buffer,time in
            guard let self else { return }
            self.meters.measureMaster(buffer)
            let value = OSAtomicAdd64Barrier(0,self.masterRecordingEndpoint)
            if value != 0, let pointer = UnsafeRawPointer(bitPattern:UInt(UInt64(bitPattern:value))) {
                Unmanaged<AudioRecording>.fromOpaque(pointer).takeUnretainedValue().receive(buffer,at:time)
            }
        }
        recordingConfigurationObserver = NotificationCenter.default.addObserver(forName:.AVAudioEngineConfigurationChange,
            object:graph,queue:nil) { [weak self] _ in
            guard let self else { return }
            // Device/sample-clock changes invalidate an active source interval.
            // Recording failure is immediate; graph recovery is serialized on main.
            DispatchQueue.main.async { [weak self] in
                guard let self, !self.closed, !self.exporting, !self.graph.isInManualRenderingMode else { return }
                self.deviceRecoveryPending = self.realtimeAudioStarted
            }
            for endpoint in [self.masterRecordingEndpoint,self.inputRecordingEndpoint] {
                let value = OSAtomicAdd64Barrier(0,endpoint)
                if value != 0, let pointer = UnsafeRawPointer(bitPattern:UInt(UInt64(bitPattern:value))) {
                    let capture = Unmanaged<AudioRecording>.fromOpaque(pointer).takeUnretainedValue()
                    capture.fail(AudioRecordingError.invalid("The audio device or source clock changed during recording."))
                }
            }
        }
        midi.onPacket = { [weak self] status, data1, data2, source in
            // Clock, tick and active sensing never reach the main thread (see receiveMIDIDirect).
            if status == 0xF8 || status == 0xF9 || status == 0xFE { return }
            DispatchQueue.main.async { self?.receiveMIDI(status, data1, data2, source: source) }
        }
        midi.onEvent = { [weak self] data in
            DispatchQueue.main.async { self?.refreshMIDIRoutes(); self?.refreshMIDIClock(); self?.emit("midi.status", data) }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            guard let self, !self.closed else { return }
            Task { @MainActor [weak self] in self?.restartStalledDevice();self?.recoverDeviceIfNeeded();self?.recheckPausedLineInputs();self?.startLiveInputsWhenIdle() }
            self.inputCapture.poll()
            st_engine_collect(self.engine)
            self.rack.collectRetiredEndpoints()
            if !self.exporting, var failure = self.rack.renderFailure() {
                failure["error"] = failure["message"]
                self.emit("plugin.error",failure)
            }
            if !self.exporting, self.graphMutationsInFlight == 0, self.rack.applyPendingLatencyRestarts() {
                self.latencyPollTicks = 0; self.emit("audio.latency",self.rack.latencyStatus())
            }
            self.latencyPollTicks += 1
            if self.latencyPollTicks >= 10 && !self.exporting {
                self.latencyPollTicks = 0
                if self.rack.refreshLatencyCompensation() { self.emit("audio.latency",self.rack.latencyStatus()) }
            }
            self.emit("status", self.status())
        }
    }

    func status() -> [String: Any] {
        if engineDestroyed { return ["closed":true,"running":false,"endpointLifetime":rack.endpointLifetimeStatus()] }
        var state = st_engine_status(engine)
        // A Play/Stop/count-in queued for the audio thread is reported as requested, not as the stale rendered
        // transport: a "playing" status sent after Stop made the UI's next Play press send another Stop, and
        // while the device is not rendering (configuration change, routing failure) it never caught up.
        let playing = st_status_transport_playing(&state) != 0, countIn = Int(st_status_count_in(&state))
        if state.transportPending != 0 { state.queuedPattern = -1; state.queuedSequence = -1 }
        let levels = meters.snapshot(tracks:Int(state.trackCount))
        let mixAutomation = rack.automationStatus()
        let droppedMixEvents = mixAutomation["droppedEvents"] as? Int ?? 0
        let busPeaks = rack.hasRouting && routingFailure == nil ? rack.routingMeterSnapshot() : [:]
        let trackPeaks:[Float]
        if rack.hasRouting {
            let tracks = project["tracks"] as? [[String:Any]] ?? []
            trackPeaks = tracks.prefix(Int(state.trackCount)).map { track in
                busPeaks["track:"+(track["routingID"] as? String ?? "")]?.max() ?? 0
            }
        } else { trackPeaks = Array(levels.tracks.prefix(Int(state.trackCount))) }
        return ["playing":playing,"songMode":state.songMode != 0,"pattern":Int(state.pattern),"row":Int(state.row),"sequence":Int(state.sequence),
            "position":state.position,"seconds":state.seconds,"beat":state.beat,"bpm":state.bpm,"bar":state.bar,"barStartBeat":state.barStartBeat,"numerator":state.numerator,"denominator":state.denominator,"timingRevision":String(state.timingRevision),"transportSeekRevision":String(state.transportSeekRevision),"peakL":levels.left,"peakR":levels.right,
            "inputPeakLeft":levels.inputLeft,"inputPeakRight":levels.inputRight,"inputMetering":inputTapInstalled,
            "trackPeaks":trackPeaks,"busPeaks":busPeaks,"routing":routingStatus(),"routingFailure":routingFailure as Any? ?? NSNull(),
            "cpu":renderControls.loadCPU(),"sampleRate":sampleRate,"recordArmSupported":true,"inputAcknowledgements":AudioInputCapture.capability,"noteRecordArmed":noteRecordArmed,"transportRecording":state.recording != 0,"queuedPattern":Int(state.queuedPattern),"queuedSequence":Int(state.queuedSequence),"countIn":countIn,"transportPending":state.transportPending != 0,"recording":recording != nil,"recordingStopping":recordingStopping,"recordingDuration":recording.map { Double($0.frames)/$0.sampleRate } ?? 0,"exporting":exporting,
            "running":graph.isRunning,"droppedCommands":state.droppedCommands,"droppedEvents":Int(state.droppedEvents)+droppedMixEvents,
            "engineDroppedEvents":state.droppedEvents,"mixAutomation":mixAutomation,"latencyCompensation":rack.latencyStatus(),"pluginsRestoring":pendingPluginRestore != nil,"playPreparing":playPreparations > 0,"previewPreparing":previewPreparations > 0,"graphMutationsPending":graphMutationsInFlight,"endpointLifetime":rack.endpointLifetimeStatus()]
    }
    @MainActor func shutdown() {
        guard !closed else { return }
        closed = true; previewGeneration &+= 1; playRequestGeneration &+= 1
        lineInputDeviceObserver = nil
        recording?.fail(AudioRecordingError.invalid("The audio host closed before the recording source tail completed."))
        noteRecordArmed = false; st_engine_set_recording(engine,0)
        rack.notifyStateCaptureMutation()
        let captureWaiters = stateCaptureWaiters; stateCaptureWaiters.removeAll()
        for waiter in captureWaiters { waiter.continuation.resume(throwing:CancellationError()) }
        inputCapture.cancelAll(reason:"The audio host closed.")
        cancelMIDICaptures(reason:"The audio host closed.")
        rack.beginRecoveryShutdown()
        timer?.invalidate(); stopPlayheads(); restoreTask?.cancel(); previewPlayer?.stop(fade: false); graph.stop(); panicExternal()
        for task in directPluginLoads.values { task.cancel() }
        for task in sampleTasks.values { task.cancel() }
        for task in zoneTasks.values { task.cancel() }
        for task in clipTasks.values { task.cancel() }
        if rack.closeAllPluginsAfterStop() { completeShutdown(); return }
        // Stop denies new graph work. If a callback is still finishing, keep the
        // host and observer alive for its post-render notification without blocking
        // either the UI thread or the callback. Never destroy a borrowed endpoint.
        shutdownTask = Task { @MainActor [self] in
            while !rack.closeAllPluginsAfterStop() {
                try? await Task.sleep(nanoseconds:1_000_000)
            }
            completeShutdown()
            shutdownTask = nil
        }
    }
    @MainActor func shutdownAndWait() async {
        shutdown()
        await shutdownTask?.value
        _ = await recordingStopTask?.result
        // Pending vendor instantiation must return and release its uncommitted
        // candidate before the caller can unload cached plug-in modules.
        await restoreTask?.value
        for task in Array(directPluginLoads.values) { _ = try? await task.value }
        rack.collectRetiredEndpoints()
        _ = rack.recoveryReport()
    }
    @MainActor func beginRecoveryModuleCleanup() throws {
        guard engineDestroyed else { throw AudioSampleError.invalid("Stop the audio host before releasing plug-in modules.") }
        try rack.beginRecoveryModuleCleanup()
    }
    @MainActor func finishRecoveryShutdown() throws { try rack.finishRecoveryShutdown() }
    @MainActor func completeShutdown() {
        if let observer = recordingConfigurationObserver { NotificationCenter.default.removeObserver(observer) }
        recordingConfigurationObserver = nil
        if let renderObserverToken { graph.mainMixerNode.auAudioUnit.removeRenderObserver(renderObserverToken) }
        renderObserverToken = nil
        publishMasterRecording(nil); publishInputRecording(nil)
        if inputTapInstalled { graph.inputNode.removeTap(onBus:0); inputTapInstalled = false }
        if let recording { recording.queue.sync { recording.close() } }
        graph.mainMixerNode.removeTap(onBus:0)
        for track in 0..<meterTaps { rack.outputNode(track:track)?.removeTap(onBus:0) }
        recording = nil; midi.shutdown(); AudioProject.forgetPatternSync(engine); st_engine_destroy(engine); engineDestroyed = true
    }

    deinit {
        masterRecordingEndpoint.deinitialize(count:1); masterRecordingEndpoint.deallocate()
        inputRecordingEndpoint.deinitialize(count:1); inputRecordingEndpoint.deallocate()
        routingAutomationReset.deinitialize(count:1); routingAutomationReset.deallocate()
        renderQuanta.deinitialize(count:1); renderQuanta.deallocate()
    }

    /// Optional 128-entry note map from the interface's chord/scale performance mode.
    /// Each played key becomes the listed notes; releases use the notes recorded at note-on.
    var midiPerformanceMap: [[Int]]?
    var midiExpandedNotes: [String: [Int]] = [:]

    /// Tracks in the applied project; channels from the return bus on are not tracks.
    var projectTrackCount:Int { max(1,(project["tracks"] as? [[String:Any]])?.count ?? 1) }
    var returnBus:Int { Int(st_return_bus(Int32(projectTrackCount))) }

    /// Ordinary instruments of the current project (at least one slot for MIDI input).
    var ordinaryInstruments:Int { max(1,min(ProjectLayout.instrumentSanityLimit,(project["instruments"] as? [Any])?.count ?? 0)) }

    /// Instrument indices a request may name: the project's instruments plus ones the UI is adding.
    var instrumentCapacity:Int { ProjectLayout.instrumentSanityLimit }

    /// Clamp a track index below the return bus (0...15 for small projects, as before).
    func lastTrack(_ value:Int)->Int { max(0,min(returnBus-1,value)) }
}

/// Cross-thread state is synchronized explicitly (the render lock, atomics and the
/// main-actor control API), which the compiler cannot verify for this class.
extension AudioHost: @unchecked Sendable {}

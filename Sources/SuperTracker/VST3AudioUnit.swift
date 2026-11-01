import AppKit
import AVFoundation
import AudioToolbox
import CoreAudioKit
import Darwin
import VST3Host
#if canImport(AURenderShim)
import AURenderShim
#endif

/// A graph adapter only: VST3Host owns the component, controller and audio processing.
/// The original rack can consequently share its routing, timestamped automation,
/// sends, compensation and offline rendering between Audio Units and VST3 plug-ins.
final class VST3AudioUnit: STAURenderUnit {
    enum HostError: LocalizedError {
        case message(String)
        var errorDescription: String? { switch self { case .message(let text): return text } }
    }
    private static let manufacturer: OSType = 0x53705472
    private static let instrumentDescription = AudioComponentDescription(componentType: kAudioUnitType_MusicDevice,
        componentSubType: 0x73747669, componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0)
    private static let effectDescription = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x73747665, componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(VST3AudioUnit.self, as: instrumentDescription,
            name: "SuperTracker: VST3 Instrument Adapter", version: 1)
        AUAudioUnit.registerSubclass(VST3AudioUnit.self, as: effectDescription,
            name: "SuperTracker: VST3 Effect Adapter", version: 1)
    }()

    /// Construction and all state/editor operations belong to the main thread. The
    /// returned node is not attached to the graph, so loading a patch remains transactional.
    static func makeNode(descriptor: VST3PluginDescriptor, sampleRate: Double) throws -> AVAudioUnit {
        vst3Trace("makeNode register")
        _ = registration
        vst3Trace("makeNode construct wrapper")
        let node: AVAudioUnit = descriptor.isInstrument
            ? AVAudioUnitMIDIInstrument(audioComponentDescription: instrumentDescription)
            : AVAudioUnitEffect(audioComponentDescription: effectDescription)
        guard let adapter = node.auAudioUnit as? VST3AudioUnit else {
            throw HostError.message("The VST3 audio adapter could not be initialized.")
        }
        vst3Trace("makeNode configure host")
        try adapter.configure(descriptor: descriptor, sampleRate: sampleRate)
        vst3Trace("makeNode complete")
        return node
    }

    /// Only call on an unattached candidate, before its first preset/state load.
    /// Some releases finish default-patch initialization asynchronously; applying
    /// a user preset sooner lets that worker overwrite the requested sound.
    @MainActor func prepareForInitialState() async throws {
        guard let descriptor, let profile = VST3Compatibility.startupProfile(
            classID: descriptor.classID, version: descriptor.version) else { return }
        try Task.checkCancellation()
        try prime(profile.initialFrames)
        try await Task.sleep(nanoseconds: profile.initialWaitNanoseconds)
        try Task.checkCancellation()
        try prime(profile.beforeRestoreFrames)
    }

    /// Completes the measured, release-specific startup recipe before the rack
    /// commits the candidate. Awaiting leaves the current instrument playable and
    /// allows project replacement/cancellation to invalidate this load normally.
    @MainActor func finishInitialStateRestore(componentState: Bool) async throws {
        guard let descriptor, let profile = VST3Compatibility.startupProfile(
            classID: descriptor.classID, version: descriptor.version) else { return }
        try Task.checkCancellation()
        if componentState { try await Task.sleep(nanoseconds: profile.componentStateWaitNanoseconds) }
        try Task.checkCancellation()
        try prime(profile.restoredFrames)
        synchronizeController()
    }

    private func prime(_ frames: UInt32) throws {
        guard let handle = state.handle else { throw HostError.message("The VST3 loading request was closed.") }
        var error = [CChar](repeating: 0, count: 2048)
        guard st_vst3_prime(handle, frames, &error, error.count) != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 plug-in could not finish initializing."))
        }
    }

    /// The rack calls this only after graph rendering and endpoint readers are
    /// quiescent. Destruction happens here on the control thread even if AVFoundation
    /// temporarily retains the lightweight Audio Unit wrapper after graph removal.
    func closePlugin() {
        controllerTimer?.invalidate(); controllerTimer = nil
        cancelReadyStateCapture()
        state.musicalContext = nil; state.transportContext = nil
        state.close()
        parameters = nil; parameterSteps.removeAll(); parameterList.removeAll(); publishedParameterGeneration = nil
    }

    /// The rack schedules sequencer (timeline) notes on this MIDI cable; live input
    /// keeps cable 0, so only live notes reach the plug-in flagged kIsLive.
    static let sequencedCable: UInt8 = 1

    static func isAdapterDescription(_ description: AudioComponentDescription) -> Bool {
        description.componentManufacturer == manufacturer &&
            [instrumentDescription.componentSubType, effectDescription.componentSubType].contains(description.componentSubType)
    }

    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree?
    private var parameterSteps:[AUParameterAddress:Int] = [:]
    /// Flat copy of the tree's parameters: `allParameters` rebuilds an array per call.
    private var parameterList:[AUParameter] = []
    /// Host parameter-cache generation last published to the AU tree; nil forces a sweep.
    private var publishedParameterGeneration:UInt64?
    private var lastParameterSweep:TimeInterval = 0
    private let state = VST3RenderState()
    private var descriptor: VST3PluginDescriptor?
    private var nativeInputMetadata: [AudioRouting.InputBus] = []
    private(set) var mainInputBus: Int = -1
    private var controllerTimer: Timer?
    private var publishingParameterValues = false
    private(set) var latencyRestartPending = false
    private var readyStateCaptureToken: UInt64?
    private(set) var lastStateError: String?
    private let instrument: Bool

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        instrument = componentDescription.componentType == kAudioUnitType_MusicDevice
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let output = try AUAudioUnitBus(format: format)
        output.supportedChannelCounts = [2]
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        if instrument {
            inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [])
        } else {
            let input = try AUAudioUnitBus(format: format)
            input.supportedChannelCounts = [2]
            inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        }
        maximumFramesToRender = VST3RenderState.maximumFrames
        setRenderFunction(Self.render, context: Unmanaged.passUnretained(state).toOpaque())
    }

    deinit { controllerTimer?.invalidate() }

    private func configure(descriptor: VST3PluginDescriptor, sampleRate: Double) throws {
        guard Thread.isMainThread, !renderResourcesAllocated, state.handle == nil,
              descriptor.isInstrument == instrument else {
            throw HostError.message("The VST3 slot cannot be configured in its current state.")
        }
        guard sampleRate.isFinite, sampleRate >= 8_000, sampleRate <= 768_000,
              let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) else {
            throw HostError.message("The VST3 audio format is invalid.")
        }
        var error = [CChar](repeating: 0, count: 2048)
        vst3Trace("create C++ host")
        let handle = st_vst3_create(descriptor.bundlePath, descriptor.classID, sampleRate,
            VST3RenderState.maximumFrames, &error, error.count)
        vst3Trace("created C++ host")
        guard let handle else { throw HostError.message(Self.errorText(error, fallback: "The VST3 plug-in could not be opened.")) }
        guard (st_vst3_is_instrument(handle) != 0) == instrument else {
            st_vst3_destroy(handle)
            throw HostError.message("This VST3 plug-in has the wrong type for the selected slot.")
        }
        self.descriptor = descriptor
        state.handle = handle
        state.sampleRate = sampleRate
        state.isInstrument = instrument
        // Diagnostic children opt in before this candidate can render. Production
        // has no watcher and no extra parameter-delivery accounting by default.
        if let requested = getenv("SUPERTRACKER_VST3_WATCH_PARAMETER"),
           let parameterID = UInt32(String(cString: requested)) {
            _ = st_vst3_watch_parameter(handle, parameterID)
        }
        try outputs[0].setFormat(format)
        let count = Int(st_vst3_input_bus_count(handle))
        guard (0...32).contains(count) else { throw HostError.message("The VST3 input bus count is unsupported.") }
        var busses: [AUAudioUnitBus] = [], metadata: [AudioRouting.InputBus] = []
        for index in 0..<count {
            var info = STVST3InputBus()
            guard st_vst3_input_bus(handle, Int32(index), &info) != 0, info.index == index,
                  (0...64).contains(info.channels) else { throw HostError.message("The VST3 input layout is invalid.") }
            let name = Self.cString(info.name)
            let title = name.isEmpty ? "Input \(index + 1)" : name
            // A zero-channel inactive native bus still occupies its real ordinal.
            // Its placeholder AU format is never activated or rendered.
            guard let inputFormat = AVAudioFormat(standardFormatWithSampleRate: sampleRate,
                channels: AVAudioChannelCount(max(1, info.channels))) else {
                throw HostError.message("The VST3 input format is unsupported.")
            }
            let bus = try AUAudioUnitBus(format: inputFormat)
            bus.name = title; bus.supportedChannelCounts = [NSNumber(value: max(1, info.channels))]
            bus.isEnabled = info.is_active != 0
            busses.append(bus)
            metadata.append(AudioRouting.InputBus(index: index, name: title,
                channels: Int(info.channels), isAuxiliary: info.is_auxiliary != 0))
        }
        mainInputBus = Int(st_vst3_main_input_bus(handle))
        guard mainInputBus >= -1, mainInputBus < metadata.count else {
            throw HostError.message("The VST3 main input index is invalid.")
        }
        if mainInputBus >= 0, metadata[mainInputBus].channels != 2 {
            throw HostError.message("The VST3 main input must support stereo audio.")
        }
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: busses)
        nativeInputMetadata = metadata
        state.configureInputs(metadata, enabled: busses.map(\.isEnabled), main: mainInputBus)
        vst3Trace("build parameter tree")
        rebuildParameterTree()
        vst3Trace("parameter tree ready")
        let timer = Timer(timeInterval: 1.0 / 30.0, repeats: true) { [weak self] _ in self?.synchronizeController() }
        controllerTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    /// The graph owner must stop and drain all render readers before calling.
    /// Applies a requested kLatencyChanged restart; read `latency` afterwards.
    func restartForLatencyChange() throws {
        guard Thread.isMainThread, let handle = state.handle else { throw HostError.message("The VST3 plug-in is closed.") }
        latencyRestartPending = false
        var error = [CChar](repeating: 0, count: 2048)
        guard st_vst3_restart_component(handle, &error, error.count) != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 plug-in could not apply its latency change."))
        }
    }

    /// Stable native bus ordinals and names, including unsupported layouts so a
    /// saved sidechain can be reported as unavailable without guessing a new bus.
    func inputBusMetadata() -> [AudioRouting.InputBus] { nativeInputMetadata }

    /// The graph owner must stop and drain all render readers before calling.
    /// Bus activation is transactional and never changes the vendor's layout.
    func configureAuxiliaryInput(bus: Int, enabled: Bool) throws {
        guard Thread.isMainThread, let handle = state.handle,
              nativeInputMetadata.indices.contains(bus), nativeInputMetadata[bus].isAuxiliary,
              (1...2).contains(nativeInputMetadata[bus].channels) else {
            throw HostError.message("This VST3 auxiliary input does not support mono or stereo routing.")
        }
        if state.inputEnabled(bus) == enabled { return }
        var error = [CChar](repeating: 0, count: 2048)
        let wasProcessing = st_vst3_is_processing(handle) != 0
        guard st_vst3_set_processing(handle, 0, &error, error.count) != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 input could not stop for routing."))
        }
        let configured = st_vst3_configure_input_bus(handle, Int32(bus), Int32(nativeInputMetadata[bus].channels),
            enabled ? 1 : 0, &error, error.count) != 0
        let configurationError = Self.errorText(error, fallback: "The VST3 plug-in rejected this auxiliary input.")
        if configured { state.setInputEnabled(bus, enabled); inputs[bus].isEnabled = enabled }
        if wasProcessing, st_vst3_set_processing(handle, 1, &error, error.count) == 0 {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 plug-in could not resume after routing."))
        }
        guard configured else { throw HostError.message(configurationError) }
    }

    private func rebuildParameterTree() {
        publishedParameterGeneration = nil
        guard let handle = state.handle else { parameters = nil; parameterSteps.removeAll(); parameterList.removeAll(); return }
        var nodes: [AUParameterNode] = []
        var steps:[AUParameterAddress:Int] = [:]
        let count = min(65_536, max(0, Int(st_vst3_parameter_count(handle))))
        nodes.reserveCapacity(count)
        var seen = Set<UInt32>()
        for index in 0..<count {
            var info = STVST3Parameter()
            guard st_vst3_parameter(handle, Int32(index), &info) != 0, seen.insert(info.id).inserted else { continue }
            steps[AUParameterAddress(info.id)] = max(0,Int(info.steps))
            let name = Self.cString(info.title)
            var flags: AudioUnitParameterOptions = [.flag_IsReadable]
            if info.flags & 2 == 0 { flags.insert(.flag_IsWritable) }
            if info.flags & 1 == 0 { flags.insert(.flag_NonRealTime) }
            // Audio Unit identifiers form KVC paths; periods are reserved separators.
            // A dotted identifier raises AUInvalidNodeInfoException and can strand an async load.
            let parameter = AUParameterTree.createParameter(withIdentifier: "vst3_\(info.id)",
                name: name.isEmpty ? "Parameter \(info.id)" : name, address: AUParameterAddress(info.id),
                min: 0, max: 1, unit: .generic, unitName: Self.cString(info.units),
                flags: flags, valueStrings: nil, dependentParameters: nil)
            // AUParameter.value has its own cache and does not invoke the provider
            // for an ordinary Swift getter. Seed it before installing the observer.
            parameter.value = AUValue(max(0, min(1, info.value)))
            nodes.append(parameter)
        }
        let tree = AUParameterTree.createTree(withChildren: nodes)
        tree.implementorValueProvider = { [weak state] parameter in
            guard let handle = state?.handle else { return 0 }
            return AUValue(st_vst3_parameter_value(handle, UInt32(truncatingIfNeeded: parameter.address)))
        }
        tree.implementorValueObserver = { [weak self] parameter, value in
            // Timeline automation travels through AU's realtime event list. Only
            // main-thread control edits use the VST3 controller's SPSC input queue.
            guard Thread.isMainThread, let self, !self.publishingParameterValues,
                  let handle = self.state.handle, value.isFinite else { return }
            let grid = AudioPluginParameterMetadata.Grid(minimum:0,maximum:1,stepCount:steps[parameter.address] ?? 0)
            let normalized = grid?.normalized(Double(value)) ?? Double(max(0,min(1,value)))
            _ = st_vst3_set_parameter(handle, UInt32(truncatingIfNeeded: parameter.address), normalized)
        }
        tree.implementorStringFromValueCallback = { [weak state] parameter, value in
            guard let handle = state?.handle else { return "—" }
            let normalized = value.map { Double($0.pointee) } ?? st_vst3_parameter_value(handle, UInt32(truncatingIfNeeded: parameter.address))
            var text = [CChar](repeating: 0, count: 512)
            if st_vst3_parameter_text(handle, UInt32(truncatingIfNeeded: parameter.address), normalized, &text, text.count) != 0 {
                return String(cString: text)
            }
            return String(format: "%.3f", normalized)
        }
        parameterSteps = steps
        parameterList = nodes.compactMap { $0 as? AUParameter }
        parameters = tree
    }

    /// VST3 always uses normalized 0...1 AU values, even for discrete parameters.
    /// Keep its interval count separately so existing saved automation is unchanged.
    func parameterStepCount(_ address:AUParameterAddress) -> Int { parameterSteps[address] ?? 0 }

    /// Controller/UI changes are delivered to the audio processor by the C++ host's
    /// bounded queue. Processor changes return to the controller here, never on audio.
    func synchronizeParameters() { publishedParameterGeneration = nil; synchronizeController() }

    private func synchronizeController() {
        guard let handle = state.handle else { return }
        st_vst3_sync_controller(handle)
        let changes = st_vst3_take_restart_flags(handle)
        // Reload parameter metadata when a vendor changes titles or value layouts.
        // Component reload/I/O changes require an explicit rack reload; attempting a
        // graph mutation from this timer would race the owning host's transaction.
        if changes & (1 << 4) != 0 { rebuildParameterTree() }
        // kLatencyChanged needs a deactivate/reactivate before the new latency is valid.
        // That must not race rendering, so the owning rack performs it in a quiescent
        // transaction (AudioPluginRack.applyPendingLatencyRestarts) and refreshes PDC.
        if changes & UInt32(ST_VST3_RESTART_LATENCY_CHANGED) != 0 { latencyRestartPending = true }
        state.restartFlags |= changes
        // This 30 Hz main-thread tick used to republish every parameter (hundreds per
        // hardware editor). Only sweep when the host cache changed, plus a slow
        // periodic sweep that still repairs any AU value that missed the cache.
        let generation = st_vst3_parameter_generation(handle), now = ProcessInfo.processInfo.systemUptime
        if generation == publishedParameterGeneration && now - lastParameterSweep < 1 { return }
        publishedParameterGeneration = generation; lastParameterSweep = now
        publishingParameterValues = true
        defer { publishingParameterValues = false }
        for parameter in parameterList {
            let value = AUValue(st_vst3_parameter_value(handle, UInt32(truncatingIfNeeded: parameter.address)))
            if parameter.value != value { parameter.value = value }
        }
    }

    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var parameterTree: AUParameterTree? { get { parameters } set { parameters = newValue } }
    override var channelCapabilities: [NSNumber]? { instrument ? [0, 2] : [2, 2] }
    override var canProcessInPlace: Bool { false }
    override var shouldBypassEffect: Bool {
        get { state.bypassed }
        set { state.bypassed = newValue; super.shouldBypassEffect = newValue }
    }

    /// The rack sets host context before attachment. Render retains only this DSP
    /// owner, never an unowned Audio Unit reference that could outlive a retired slot.
    func captureRenderContext() {
        state.musicalContext = musicalContextBlock
        state.transportContext = transportStateBlock
    }
    override var latency: TimeInterval {
        guard let handle = state.handle else { return 0 }
        return shouldBypassEffect ? 0 : Double(st_vst3_latency(handle)) / max(1, state.sampleRate)
    }
    override var tailTime: TimeInterval {
        guard let handle = state.handle else { return 0 }
        let frames = st_vst3_tail(handle)
        return frames == UInt32.max ? 60 : Double(frames) / max(1, state.sampleRate)
    }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = VST3RenderState.maximumFrames }
    }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        // Re-applying the bus's own format is no change. A stopped engine reapplies every connection's format,
        // and the adapter is already processing (it starts right after loading), so accept it.
        if format == bus.format { return true }
        guard !renderResourcesAllocated, format.sampleRate >= 8_000, format.sampleRate <= 768_000,
              format.commonFormat == .pcmFormatFloat32, !format.isInterleaved else { return false }
        if bus === outputs[0] { return format.channelCount == 2 }
        guard let index = nativeInputMetadata.indices.first(where: { inputs[$0] === bus }) else { return false }
        return format.channelCount == max(1, nativeInputMetadata[index].channels)
    }

    override func allocateRenderResources() throws {
        guard let handle = state.handle,
              mainInputBus < 0 || inputs[mainInputBus].format == outputs[0].format,
              nativeInputMetadata.indices.allSatisfy({ !state.inputEnabled($0) ||
                inputs[$0].format.sampleRate == outputs[0].format.sampleRate }) else {
            throw HostError.message("The VST3 plug-in is not configured for this audio format.")
        }
        var error = [CChar](repeating: 0, count: 2048)
        state.sampleRate = outputs[0].format.sampleRate
        guard st_vst3_prepare(handle, state.sampleRate, VST3RenderState.maximumFrames, &error, error.count) != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 plug-in could not start processing."))
        }
        try super.allocateRenderResources()
    }

    override func deallocateRenderResources() {
        if let handle = state.handle {
            var error = [CChar](repeating: 0, count: 512)
            _ = st_vst3_set_processing(handle, 0, &error, error.count)
        }
        super.deallocateRenderResources()
    }
    override func reset() {
        if let handle = state.handle {
            var error = [CChar](repeating: 0, count: 512)
            _ = st_vst3_reset(handle, &error, error.count)
        }
        super.reset()
    }

    /// The render callback (see STAURenderUnit); `context` is this unit's render state.
    private static let render: STAURenderFunction = { context, flags, timestamp, frames, bus, output, events, pull in
        guard bus == 0 else { return kAudioUnitErr_InvalidElement }
        guard let context else { return kAudioUnitErr_Uninitialized }
        guard timestamp.pointee.mSampleTime.isFinite, abs(timestamp.pointee.mSampleTime) < 1e15 else { return kAudio_ParamError }
        return Unmanaged<VST3RenderState>.fromOpaque(context)._withUnsafeGuaranteedRef { dsp in
            var transport = VST3TransportContext.make(timestamp: timestamp.pointee,
                musical: dsp.musicalContext, transport: dsp.transportContext)
            return dsp.render(flags: flags, timestamp: timestamp, frames: frames, output: output,
                events: events, pull: pull, transport: &transport, bypass: dsp.bypassed)
        }
    }

    override var fullState: [String: Any]? {
        get { try? documentState() }
        set {
            do { if let newValue { try restoreDocumentState(newValue) }; lastStateError = nil }
            catch { lastStateError = error.localizedDescription }
        }
    }
    override var fullStateForDocument: [String: Any]? { get { fullState } set { fullState = newValue } }

    var pendingStateEdits: UInt32 {
        state.handle.map(st_vst3_pending_parameter_edits) ?? 0
    }
    var requiredStateFlushFrames: UInt32 {
        state.handle.map(st_vst3_required_state_flush_frames) ?? 0
    }
    var stateCaptureSnapshot: VST3StatePreparation.CaptureSnapshot {
        guard let handle = state.handle else {
            return .init(editGeneration: 0, dirty: false, pendingEdits: 0)
        }
        var value = STVST3StateCaptureStatus()
        guard st_vst3_state_capture_status(handle, &value) != 0 else {
            return .init(editGeneration: 0, dirty: true, pendingEdits: pendingStateEdits)
        }
        return .init(editGeneration: value.edit_generation, dirty: value.dirty != 0,
                     pendingEdits: value.pending_edits)
    }

    /// Begin/authorize/capture require the rack's quiescence guard. The one-second
    /// actor suspension happens outside this adapter, with no render lease held.
    func beginStateCapture(allowStoppedAudioAdvance: Bool) throws -> UInt64 {
        guard let handle = state.handle else { throw HostError.message("The VST3 plug-in was closed while saving.") }
        var token: UInt64 = 0, error = [CChar](repeating: 0, count: 2048)
        guard st_vst3_begin_state_capture(handle, allowStoppedAudioAdvance ? 1 : 0,
            &token, &error, error.count) != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 state could not be prepared."))
        }
        readyStateCaptureToken = nil
        return token
    }
    func cancelStateCapture(token: UInt64) {
        if readyStateCaptureToken == token { readyStateCaptureToken = nil }
        if let handle = state.handle { st_vst3_cancel_state_capture(handle, token) }
    }
    func acceptStateCapturePreparation(_ preparation: VST3StatePreparation.CapturePreparation) throws {
        guard state.handle != nil, stateCaptureSnapshot.editGeneration == preparation.editGeneration else {
            cancelStateCapture(token: preparation.token)
            throw HostError.message("The VST3 controls changed while their state was being prepared. Retry saving.")
        }
        readyStateCaptureToken = preparation.token == 0 ? nil : preparation.token
    }
    private func cancelReadyStateCapture() {
        if let token = readyStateCaptureToken { cancelStateCapture(token: token) }
    }

    /// The rack has already paused/drained rendering. Preserve whether the graph
    /// was originally stopped; the native compatibility policy may still refuse
    /// capture when an edit has not completed ordinary audio processing.
    var basePatchCapabilities:[String:Any] {
        let flags=state.handle.map { st_vst3_baseline_capabilities($0) } ?? 0
        return ["version":1,"supported":flags & 1 != 0,"captureReady":flags & 2 != 0,"manualEdits":flags & 4 != 0]
    }

    /// The caller owns the rack's paused/drained state-access lease. The editor
    /// promotes cached live values without sending a patch or touching hardware.
    func captureBasePatch() throws {
        guard let handle=state.handle else { throw HostError.message("No VST3 plug-in is loaded.") }
        var error=[CChar](repeating:0,count:2048)
        guard st_vst3_capture_baseline(handle,&error,error.count) != 0 else {
            throw HostError.message(Self.errorText(error,fallback:"The plug-in could not update its base patch."))
        }
    }

    func documentState(allowStoppedAudioAdvance: Bool = false) throws -> [String: Any] {
        guard let handle = state.handle, let descriptor else { throw HostError.message("No VST3 plug-in is loaded.") }
        vst3Trace("documentState start")
        var bytes: UnsafeMutableRawPointer?, count = 0
        var error = [CChar](repeating: 0, count: 2048)
        let preparedToken = readyStateCaptureToken
        readyStateCaptureToken = nil
        defer { if let preparedToken { st_vst3_cancel_state_capture(handle, preparedToken) } }
        if let preparedToken {
            guard st_vst3_authorize_state_capture(handle, preparedToken, &error, error.count) != 0 else {
                throw HostError.message(Self.errorText(error, fallback: "The VST3 state preparation was invalidated. Retry saving."))
            }
        }
        guard st_vst3_prepare_state_capture(handle, allowStoppedAudioAdvance ? 1 : 0, &error, error.count) != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 edit has not finished processing. Retry saving."))
        }
        guard st_vst3_copy_state(handle, &bytes, &count, &error, error.count) != 0, let bytes else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 state could not be saved."))
        }
        vst3Trace("documentState copied")
        defer { st_vst3_free(bytes) }
        guard count >= 0, count <= 256 * 1024 * 1024 else { throw HostError.message("The VST3 state exceeds the supported limit.") }
        return ["format": "SuperTracker.VST3", "version": 1, "classID": descriptor.classID,
                "bundlePath": descriptor.bundlePath, "data": Data(bytes: bytes, count: count)]
    }

    /// Capture while the graph keeps rendering (VST3 getState is a UI-thread call
    /// that plug-ins must serve concurrently with process()). Returns nil, without
    /// consuming a prepared capture, while UI edits still wait for an audio block:
    /// only the render thread may deliver them, so the caller retries.
    func documentStateConcurrent() throws -> [String: Any]? {
        guard let handle = state.handle, let descriptor else { throw HostError.message("No VST3 plug-in is loaded.") }
        if st_vst3_pending_parameter_edits(handle) > 0 { return nil }
        var bytes: UnsafeMutableRawPointer?, count = 0
        var error = [CChar](repeating: 0, count: 2048)
        let preparedToken = readyStateCaptureToken
        if let preparedToken {
            guard st_vst3_authorize_state_capture(handle, preparedToken, &error, error.count) != 0 else {
                readyStateCaptureToken = nil; st_vst3_cancel_state_capture(handle, preparedToken)
                throw HostError.message(Self.errorText(error, fallback: "The VST3 state preparation was invalidated. Retry saving."))
            }
        }
        // UI edits are enqueued on this (main) thread and only drain on the render
        // thread, so after the check above the ring cannot gain an entry here.
        let result = st_vst3_copy_state_concurrent(handle, &bytes, &count, &error, error.count)
        if result < 0 && preparedToken == nil { return nil }
        readyStateCaptureToken = nil
        if let preparedToken { st_vst3_cancel_state_capture(handle, preparedToken) }
        guard result != 0, let bytes else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 state could not be saved."))
        }
        defer { st_vst3_free(bytes) }
        guard count >= 0, count <= 256 * 1024 * 1024 else { throw HostError.message("The VST3 state exceeds the supported limit.") }
        return ["format": "SuperTracker.VST3", "version": 1, "classID": descriptor.classID,
                "bundlePath": descriptor.bundlePath, "data": Data(bytes: bytes, count: count)]
    }

    func restoreDocumentState(_ value: [String: Any]) throws {
        cancelReadyStateCapture()
        guard let handle = state.handle, let descriptor,
              value["format"] as? String == "SuperTracker.VST3", value["version"] as? Int == 1,
              (value["classID"] as? String)?.uppercased() == descriptor.classID,
              let data = value["data"] as? Data, !data.isEmpty, data.count <= 256 * 1024 * 1024 else {
            throw HostError.message("The saved VST3 state does not match this plug-in.")
        }
        var error = [CChar](repeating: 0, count: 2048)
        let result = data.withUnsafeBytes { st_vst3_restore_state(handle, $0.baseAddress, data.count, &error, error.count) }
        guard result != 0 else { throw HostError.message(Self.errorText(error, fallback: "The VST3 plug-in rejected its saved state.")) }
        lastStateError = nil
        rebuildParameterTree()
    }

    /// Applies the unmodified PCHK component stream only after the caller validates
    /// the NKS class ID. Native controller mappings then bind actual VST3 ParamIDs.
    func restoreComponentState(_ data: Data, classID: String) throws {
        cancelReadyStateCapture()
        guard let handle = state.handle, let descriptor, classID.uppercased() == descriptor.classID,
              !data.isEmpty, data.count <= 256 * 1024 * 1024 else {
            throw HostError.message("The NKS state does not identify this VST3 plug-in.")
        }
        var error = [CChar](repeating: 0, count: 2048)
        let result = data.withUnsafeBytes { st_vst3_restore_component_state(handle, $0.baseAddress, data.count, &error, error.count) }
        guard result != 0 else { throw HostError.message(Self.errorText(error, fallback: "The VST3 plug-in rejected this NKS preset.")) }
        rebuildParameterTree()
    }

    /// Explicit migration of an identified legacy bank. The rack validates the
    /// saved STV2 document and quiesces rendering before entering this method.
    /// A true return records that only the unchanged raw fallback was accepted.
    @discardableResult
    func restoreLegacyVST2Bank(_ data: Data, uniqueID: UInt32, program: Int32) throws -> Bool {
        cancelReadyStateCapture()
        let predecessor = String(format: "565354%08X", uniqueID)
        guard let handle = state.handle, let descriptor, uniqueID != 0, program >= 0,
              descriptor.classID.hasPrefix(predecessor), !data.isEmpty, data.count <= 32 * 1024 * 1024 else {
            throw HostError.message("The legacy VST2 bank does not identify this VST3 replacement.")
        }
        var error = [CChar](repeating: 0, count: 2048), usedRawFallback: Int32 = 0
        let result = data.withUnsafeBytes {
            st_vst3_restore_vst2_bank(handle, uniqueID, program, $0.baseAddress, data.count,
                                     &usedRawFallback, &error, error.count)
        }
        guard result != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST3 replacement rejected this legacy bank."))
        }
        lastStateError = nil
        rebuildParameterTree()
        return usedRawFallback != 0
    }

    var classID: String? { descriptor?.classID }
    var parameterIDs: Set<UInt32> { Set(parameters?.allParameters.map { UInt32(truncatingIfNeeded: $0.address) } ?? []) }
    /// Passive atomics only: this neither flushes controls nor serializes state.
    func parameterDiagnostics() -> [String: Any]? {
        guard let handle = state.handle else { return nil }
        var value = STVST3ParameterDiagnostics()
        guard st_vst3_parameter_diagnostics(handle, &value) != 0 else { return nil }
        return ["parameterID": value.id, "pendingUIEdits": value.pending_ui_edits,
            "uiQueued": value.ui_queued, "uiDequeued": value.ui_dequeued,
            "automationReceived": value.automation_received, "midiReceived": value.midi_received,
            "offeredPoints": value.offered_points, "readPoints": value.read_points,
            "processes": value.processes, "processingStarts": value.processing_starts,
            "processingStops": value.processing_stops, "resets": value.resets,
            "stateCopies": value.state_copies, "stateRestores": value.state_restores,
            "offeredProcess": value.offered_process, "readProcess": value.read_process,
            "lastUIValue": value.last_ui_value, "lastDequeuedValue": value.last_dequeued_value,
            "lastAutomationValue": value.last_automation_value, "lastMIDIValue": value.last_midi_value,
            "lastOfferedValue": value.last_offered_value, "lastReadValue": value.last_read_value,
            "lastOfferedOffset": value.last_offered_offset, "lastReadOffset": value.last_read_offset,
            "lastOfferedFrames": value.last_offered_frames, "lastReadFrames": value.last_read_frames,
            "lastProcessResult": value.last_process_result]
    }
    var status: [String: Any] {
        ["format": "VST3", "droppedEvents": state.handle.map(st_vst3_dropped_events) ?? 0,
         "adapterDroppedEvents": state.droppedEvents, "restartFlags": state.restartFlags,
         "stateFlushFrames": requiredStateFlushFrames, "pendingStateEdits": pendingStateEdits,
         "factoryVersion": state.handle.map { String(cString: st_vst3_factory_version($0)) } ?? "",
         "loadedBundleVersion": state.handle.map { String(cString: st_vst3_bundle_version($0)) } ?? "",
         "needsReload": state.restartFlags & ((1 << 0) | (1 << 1)) != 0]
    }

    override func requestViewController(completionHandler: @escaping (NSViewController?) -> Void) {
        let create = { [weak self] in
            guard let self else { completionHandler(nil); return }
            let controller = VST3EditorController(state: self.state)
            if controller.prepare() { completionHandler(controller) } else { completionHandler(nil) }
        }
        if Thread.isMainThread { create() } else { DispatchQueue.main.async(execute: create) }
    }

    private static func cString<T>(_ tuple: T) -> String {
        withUnsafeBytes(of: tuple) { bytes in String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self) }
    }
    private static func errorText(_ chars: [CChar], fallback: String) -> String {
        let text = String(cString: chars)
        return text.isEmpty ? fallback : text
    }
}

/// All audio buffers and event arrays are allocated before rendering. AU's base
/// scheduler owns the absolute event queue and gives this adapter only the current
/// block. The C++ host receives those events at their original sample offsets.
private final class VST3RenderState {
    static let maximumFrames: UInt32 = 16_384
    private static let eventCapacity = 16_384
    var handle: OpaquePointer?
    var sampleRate: Double = 48_000
    var isInstrument = false
    var restartFlags: UInt32 = 0
    var musicalContext: AUHostMusicalContextBlock?
    var transportContext: AUHostTransportStateBlock?
    private let bypassFlag = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let lostEvents = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    var droppedEvents: UInt64 { UInt64(bitPattern: OSAtomicAdd64Barrier(0, lostEvents)) }
    var bypassed: Bool {
        get { OSAtomicAdd64Barrier(0, bypassFlag) != 0 }
        set {
            let desired: Int64 = newValue ? 1 : 0
            var old = OSAtomicAdd64Barrier(0, bypassFlag)
            while !OSAtomicCompareAndSwap64Barrier(old, desired, bypassFlag) { old = OSAtomicAdd64Barrier(0, bypassFlag) }
        }
    }
    private let left = UnsafeMutablePointer<Float>.allocate(capacity: Int(maximumFrames))
    private let right = UnsafeMutablePointer<Float>.allocate(capacity: Int(maximumFrames))
    // Owned by `inputStorage`; the render loop reads only the trivial slot copies
    // (`inputSlots`, rebuilt during stopped graph edits), so it never retains a storage object.
    private var inputStorage: [VST3InputStorage] = []
    private let inputSlots = UnsafeMutablePointer<VST3InputSlot>.allocate(capacity: 32)
    private var inputSlotCount = 0
    private var mainInput = -1
    private let inputDescriptors = UnsafeMutablePointer<STVST3InputBuffer>.allocate(capacity: 32)
    private let midi = UnsafeMutablePointer<STVST3MIDI>.allocate(capacity: eventCapacity)
    private let changes = UnsafeMutablePointer<STVST3ParameterChange>.allocate(capacity: eventCapacity)

    init() {
        bypassFlag.initialize(to: 0); lostEvents.initialize(to: 0)
        for pointer in [left, right] { pointer.initialize(repeating: 0, count: Int(Self.maximumFrames)) }
        midi.initialize(repeating: STVST3MIDI(), count: Self.eventCapacity)
        changes.initialize(repeating: STVST3ParameterChange(), count: Self.eventCapacity)
        inputDescriptors.initialize(repeating: STVST3InputBuffer(), count: 32)
    }
    func close() {
        guard let existing = handle else { return }
        handle = nil
        st_vst3_destroy(existing)
    }
    deinit {
        close()
        bypassFlag.deinitialize(count: 1); bypassFlag.deallocate()
        lostEvents.deinitialize(count: 1); lostEvents.deallocate()
        for pointer in [left, right] { pointer.deinitialize(count: Int(Self.maximumFrames)); pointer.deallocate() }
        midi.deinitialize(count: Self.eventCapacity); midi.deallocate()
        changes.deinitialize(count: Self.eventCapacity); changes.deallocate()
        inputDescriptors.deinitialize(count: 32); inputDescriptors.deallocate()
        inputSlots.deinitialize(count: inputSlotCount); inputSlots.deallocate()
    }

    func configureInputs(_ metadata: [AudioRouting.InputBus], enabled: [Bool], main: Int) {
        inputStorage = metadata.prefix(32).map { VST3InputStorage(channels: $0.channels, enabled: enabled[$0.index]) }
        publishInputSlots()
        mainInput = main
    }
    func inputEnabled(_ bus: Int) -> Bool { inputStorage.indices.contains(bus) && inputStorage[bus].enabled }
    func setInputEnabled(_ bus: Int, _ enabled: Bool) { inputStorage[bus].enabled = enabled; publishInputSlots() }
    private func publishInputSlots() {
        if inputSlotCount > 0 { inputSlots.deinitialize(count: inputSlotCount) }
        for (index, storage) in inputStorage.enumerated() {
            (inputSlots + index).initialize(to: VST3InputSlot(channels: storage.channels, enabled: storage.enabled,
                samples: storage.samples, pointers: storage.pointers, list: storage.list))
        }
        inputSlotCount = inputStorage.count
    }

    func render(flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, timestamp: UnsafePointer<AudioTimeStamp>,
                frames: UInt32, output: UnsafeMutablePointer<AudioBufferList>, events: UnsafePointer<AURenderEvent>?,
                pull: UnsafeRawPointer?, transport: UnsafePointer<STVST3Transport>, bypass: Bool) -> AUAudioUnitStatus {
        guard let handle else { return kAudioUnitErr_Uninitialized }
        guard frames <= Self.maximumFrames else { return kAudioUnitErr_TooManyFramesToProcess }
        guard timestamp.pointee.mSampleTime.isFinite, abs(timestamp.pointee.mSampleTime) < 1e15 else { return kAudio_ParamError }
        let buffers = UnsafeMutableAudioBufferListPointer(output), bytes = frames * UInt32(MemoryLayout<Float>.size)
        guard buffers.count == 2, buffers[0].mNumberChannels == 1, buffers[1].mNumberChannels == 1 else { return kAudioUnitErr_FormatNotSupported }
        if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(left); buffers[0].mDataByteSize = bytes }
        if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(right); buffers[1].mDataByteSize = bytes }
        guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes,
              let outLeft = buffers[0].mData?.assumingMemoryBound(to: Float.self),
              let outRight = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
        var sourceLeft: UnsafePointer<Float>?, sourceRight: UnsafePointer<Float>?
        var inputCount = 0
        for index in 0..<inputSlotCount {
            let source = inputSlots[index]
            guard source.enabled else { continue }
            guard (1...2).contains(source.channels) else { return kAudioUnitErr_FormatNotSupported }
            source.list.unsafeMutablePointer.pointee.mNumberBuffers = UInt32(source.channels)
            for channel in 0..<source.channels {
                source.list[channel] = AudioBuffer(mNumberChannels: 1, mDataByteSize: bytes,
                    mData: source.samples.advanced(by: channel * Int(Self.maximumFrames)))
            }
            var inputFlags = flags.pointee
            inputFlags.remove(.unitRenderAction_OutputIsSilence)
            let result = st_au_pull(pull, &inputFlags, timestamp, frames, Int(index), source.list.unsafeMutablePointer)
            if result != noErr {
                // An instrument's optional main audio input may be unconnected.
                guard isInstrument && index == mainInput && result == kAudioUnitErr_NoConnection else { return result }
                inputFlags.insert(.unitRenderAction_OutputIsSilence)
            }
            guard source.list.count == source.channels else { return kAudio_ParamError }
            for channel in 0..<source.channels {
                let buffer = source.list[channel]
                guard buffer.mNumberChannels == 1, buffer.mDataByteSize >= bytes,
                      let pointer = buffer.mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
                if inputFlags.contains(.unitRenderAction_OutputIsSilence) {
                    let zero = source.samples.advanced(by: channel * Int(Self.maximumFrames))
                    memset(zero, 0, Int(bytes)); source.pointers[channel] = UnsafePointer(zero)
                } else { source.pointers[channel] = UnsafePointer(pointer) }
            }
            inputDescriptors[inputCount] = STVST3InputBuffer(bus_index: Int32(index),
                channel_count: Int32(source.channels), channels: UnsafePointer(source.pointers))
            inputCount += 1
            if index == mainInput {
                sourceLeft = source.pointers[0]; sourceRight = source.pointers[source.channels == 1 ? 0 : 1]
            }
        }
        var next = events, midiCount = 0, parameterCount = 0, inspected = 0
        let blockStart = Int64(timestamp.pointee.mSampleTime)
        while let event = next, inspected < Self.eventCapacity * 2 {
            inspected += 1
            let sample = event.pointee.head.eventSampleTime
            let relative = sample == AUEventSampleTimeImmediate ? 0 : max(0, sample - blockStart)
            if relative < Int64(frames) {
                switch event.pointee.head.eventType {
                case .MIDI:
                    let message = event.pointee.MIDI
                    if message.length >= 1, message.length <= 3, midiCount < Self.eventCapacity {
                        midi[midiCount] = STVST3MIDI(sample_offset: Int32(relative), status: message.data.0,
                            data1: message.length > 1 ? message.data.1 : 0, data2: message.length > 2 ? message.data.2 : 0, note_id: -1,
                            flags: message.cable == VST3AudioUnit.sequencedCable ? UInt8(ST_VST3_MIDI_SEQUENCED) : 0)
                        midiCount += 1
                    } else { OSAtomicIncrement64Barrier(lostEvents) }
                case .parameter, .parameterRamp:
                    let parameter = event.pointee.parameter
                    if parameter.parameterAddress <= UInt32.max, parameter.value.isFinite, parameterCount < Self.eventCapacity {
                        changes[parameterCount] = STVST3ParameterChange(sample_offset: Int32(relative),
                            id: UInt32(parameter.parameterAddress), value: Double(max(0, min(1, parameter.value))))
                        parameterCount += 1
                    } else { OSAtomicIncrement64Barrier(lostEvents) }
                default: break
                }
            } else { OSAtomicIncrement64Barrier(lostEvents) }
            next = event.pointee.head.next.map { UnsafePointer($0) }
        }
        if next != nil { OSAtomicIncrement64Barrier(lostEvents) }
        // Continue processing while bypassed so note-offs and parameter changes are
        // consumed. Returning dry audio also preserves rack latency compensation.
        let result = st_vst3_process_indexed(handle, inputDescriptors, UInt32(inputCount), outLeft, outRight, frames,
            midi, UInt32(midiCount), changes, UInt32(parameterCount), transport)
        guard result != 0 else { memset(outLeft, 0, Int(bytes)); memset(outRight, 0, Int(bytes)); return kAudioUnitErr_CannotDoInCurrentContext }
        if bypass {
            if let sourceLeft, let sourceRight {
                memcpy(outLeft, sourceLeft, Int(bytes)); memcpy(outRight, sourceRight, Int(bytes))
            } else { memset(outLeft, 0, Int(bytes)); memset(outRight, 0, Int(bytes)) }
        }
        buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
        flags.pointee.remove(.unitRenderAction_OutputIsSilence)
        return noErr
    }
}

/// Trivial render-thread copy of one input's storage pointers (no ARC).
private struct VST3InputSlot {
    let channels: Int
    let enabled: Bool
    let samples: UnsafeMutablePointer<Float>
    let pointers: UnsafeMutablePointer<UnsafePointer<Float>?>
    let list: UnsafeMutableAudioBufferListPointer
}

/// Storage survives every render and changes only during a stopped graph edit.
/// Channel pointers may be replaced by upstream pull blocks for the current call.
private final class VST3InputStorage {
    let channels: Int
    var enabled: Bool
    let samples = UnsafeMutablePointer<Float>.allocate(capacity: Int(VST3RenderState.maximumFrames) * 2)
    let pointers = UnsafeMutablePointer<UnsafePointer<Float>?>.allocate(capacity: 2)
    let list = AudioBufferList.allocate(maximumBuffers: 2)
    init(channels: Int, enabled: Bool) {
        self.channels = channels; self.enabled = enabled
        samples.initialize(repeating: 0, count: Int(VST3RenderState.maximumFrames) * 2)
        pointers.initialize(repeating: nil, count: 2)
    }
    deinit {
        samples.deinitialize(count: Int(VST3RenderState.maximumFrames) * 2); samples.deallocate()
        pointers.deinitialize(count: 2); pointers.deallocate()
        free(list.unsafeMutablePointer)
    }
}

private final class VST3EditorController: NSViewController {
    private let state: VST3RenderState
    private var attached = false, prepared = false
    init(state: VST3RenderState) { self.state = state; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
    override func loadView() { view = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 640)) }
    /// Creates the plug-in view and sizes this one to it. It attaches in viewDidAppear,
    /// once this view is inside the editor window (see st_vst3_create_editor).
    func prepare() -> Bool {
        guard let handle = state.handle else { return false }
        var width: Int32 = 900, height: Int32 = 640
        var error = [CChar](repeating: 0, count: 2048)
        guard st_vst3_create_editor(handle, &width, &height, &error, error.count) != 0 else { return false }
        prepared = true
        view.setFrameSize(NSSize(width: max(1, Int(width)), height: max(1, Int(height))))
        return true
    }
    func attach() -> Bool {
        guard !attached, view.window != nil, let handle = state.handle else { return attached }
        var width: Int32 = 900, height: Int32 = 640
        var error = [CChar](repeating: 0, count: 2048)
        let parent = Unmanaged.passUnretained(view).toOpaque()
        attached = st_vst3_attach_editor(handle, parent, &width, &height, &error, error.count) != 0
        if attached { view.setFrameSize(NSSize(width: max(1, Int(width)), height: max(1, Int(height)))) }
        return attached
    }
    override func viewDidAppear() { super.viewDidAppear(); _ = attach() }
    override func viewDidDisappear() {
        if attached || prepared, let handle = state.handle { st_vst3_detach_editor(handle); attached = false; prepared = false }
        super.viewDidDisappear()
    }
    deinit { if attached || prepared, let handle = state.handle { st_vst3_detach_editor(handle) } }
}

private func vst3Trace(_ message: String) {
    if getenv("SUPERTRACKER_VST3_TRACE") != nil { fputs("VST3 adapter: \(message)\n", stderr) }
}

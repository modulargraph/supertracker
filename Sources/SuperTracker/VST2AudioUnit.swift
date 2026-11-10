import AppKit
import AVFoundation
import AudioToolbox
import Darwin
import VST2Host
import VST3Host
#if canImport(AURenderShim)
import AURenderShim
#endif

/// Graph adapter for one VST2 effect. VST2Host owns the plug-in; the rack treats the
/// node as an ordinary Audio Unit for routing, automation, sends and offline render.
final class VST2AudioUnit: STAURenderUnit {
    enum HostError: LocalizedError {
        case message(String)
        var errorDescription: String? { switch self { case .message(let text): return text } }
    }
    private static let manufacturer: OSType = 0x53705472
    private static let instrumentDescription = AudioComponentDescription(componentType: kAudioUnitType_MusicDevice,
        componentSubType: 0x73743269 /* st2i */, componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0)
    private static let effectDescription = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x73743265 /* st2e */, componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(VST2AudioUnit.self, as: instrumentDescription, name: "SuperTracker: VST2 Instrument Adapter", version: 1)
        AUAudioUnit.registerSubclass(VST2AudioUnit.self, as: effectDescription, name: "SuperTracker: VST2 Effect Adapter", version: 1)
    }()

    static func isAdapterDescription(_ description: AudioComponentDescription) -> Bool {
        description.componentManufacturer == manufacturer &&
            [instrumentDescription.componentSubType, effectDescription.componentSubType].contains(description.componentSubType)
    }

    /// Main thread. The node is unattached so loading stays transactional.
    static func makeNode(descriptor: VST2PluginDescriptor, sampleRate: Double) throws -> AVAudioUnit {
        _ = registration
        let node: AVAudioUnit = descriptor.isInstrument
            ? AVAudioUnitMIDIInstrument(audioComponentDescription: instrumentDescription)
            : AVAudioUnitEffect(audioComponentDescription: effectDescription)
        guard let adapter = node.auAudioUnit as? VST2AudioUnit else { throw HostError.message("The VST2 audio adapter could not be initialized.") }
        try adapter.configure(descriptor: descriptor, sampleRate: sampleRate)
        return node
    }

    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree?
    private let state = VST2RenderState()
    private(set) var descriptor: VST2PluginDescriptor?
    private var timer: Timer?
    private var publishing = false
    private let instrument: Bool
    private(set) var lastStateError: String?

    override init(componentDescription: AudioComponentDescription, options: AudioComponentInstantiationOptions = []) throws {
        instrument = componentDescription.componentType == kAudioUnitType_MusicDevice
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let output = try AUAudioUnitBus(format: format)
        output.supportedChannelCounts = [2]
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        if instrument { inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: []) }
        else {
            let input = try AUAudioUnitBus(format: format)
            input.supportedChannelCounts = [2]
            inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        }
        maximumFramesToRender = VST2RenderState.maximumFrames
        setRenderFunction(Self.render, context: Unmanaged.passUnretained(state).toOpaque())
    }

    deinit { timer?.invalidate() }

    private func configure(descriptor: VST2PluginDescriptor, sampleRate: Double) throws {
        guard Thread.isMainThread, state.handle == nil, descriptor.isInstrument == instrument else {
            throw HostError.message("The VST2 slot cannot be configured in its current state.")
        }
        var error = [CChar](repeating: 0, count: 2048)
        guard let handle = st_vst2_create(descriptor.bundlePath, descriptor.shellID, sampleRate, VST2RenderState.maximumFrames, &error, error.count) else {
            throw HostError.message(Self.errorText(error, fallback: "The VST2 plug-in could not be opened."))
        }
        state.handle = handle; state.sampleRate = sampleRate; state.isInstrument = instrument
        self.descriptor = descriptor
        if let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) {
            try outputs[0].setFormat(format)
            if !instrument { try inputs[0].setFormat(format) }
        }
        rebuildParameterTree()
        let timer = Timer(timeInterval: 1.0 / 30.0, repeats: true) { [weak self] _ in self?.synchronizeParameters() }
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    /// Called by the rack once rendering is quiesced; destruction stays on the main thread.
    func closePlugin() {
        timer?.invalidate(); timer = nil
        state.musicalContext = nil; state.transportContext = nil
        state.close()
        parameters = nil
    }

    func captureRenderContext() {
        state.musicalContext = musicalContextBlock
        state.transportContext = transportStateBlock
    }

    private func rebuildParameterTree() {
        guard let handle = state.handle else { parameters = nil; return }
        let count = min(65_536, max(0, Int(st_vst2_parameter_count(handle))))
        var nodes: [AUParameterNode] = []
        nodes.reserveCapacity(count)
        var text = [CChar](repeating: 0, count: 512)
        for index in 0..<count {
            _ = st_vst2_parameter_name(handle, Int32(index), &text, text.count)
            let name = String(cString: text).trimmingCharacters(in: .whitespacesAndNewlines)
            _ = st_vst2_parameter_label(handle, Int32(index), &text, text.count)
            let unit = String(cString: text).trimmingCharacters(in: .whitespacesAndNewlines)
            let parameter = AUParameterTree.createParameter(withIdentifier: "vst2_\(index)",
                name: name.isEmpty ? "Parameter \(index + 1)" : name, address: AUParameterAddress(index),
                min: 0, max: 1, unit: .generic, unitName: unit.isEmpty ? nil : unit,
                flags: [.flag_IsReadable, .flag_IsWritable], valueStrings: nil, dependentParameters: nil)
            parameter.value = AUValue(st_vst2_parameter_value(handle, Int32(index)))
            nodes.append(parameter)
        }
        let tree = AUParameterTree.createTree(withChildren: nodes)
        tree.implementorValueProvider = { [weak state] parameter in
            guard let handle = state?.handle else { return 0 }
            return AUValue(st_vst2_parameter_value(handle, Int32(truncatingIfNeeded: parameter.address)))
        }
        tree.implementorValueObserver = { [weak self] parameter, value in
            guard Thread.isMainThread, let self, !self.publishing, let handle = self.state.handle, value.isFinite else { return }
            _ = st_vst2_set_parameter(handle, Int32(truncatingIfNeeded: parameter.address), Float(max(0, min(1, value))))
        }
        tree.implementorStringFromValueCallback = { [weak state] parameter, value in
            guard let handle = state?.handle else { return "—" }
            let index = Int32(truncatingIfNeeded: parameter.address)
            // VST2 only formats its live value; for other values show a normalized number.
            if value == nil || abs(Double(value!.pointee) - Double(st_vst2_parameter_value(handle, index))) < 1e-6 {
                var text = [CChar](repeating: 0, count: 256), label = [CChar](repeating: 0, count: 64)
                if st_vst2_parameter_display(handle, index, &text, text.count) != 0 {
                    _ = st_vst2_parameter_label(handle, index, &label, label.count)
                    let display = String(cString: text).trimmingCharacters(in: .whitespaces)
                    let unit = String(cString: label).trimmingCharacters(in: .whitespaces)
                    if !display.isEmpty { return unit.isEmpty ? display : display + " " + unit }
                }
            }
            return String(format: "%.3f", value.map { Double($0.pointee) } ?? Double(st_vst2_parameter_value(handle, index)))
        }
        parameters = tree
    }

    /// Publishes values the plug-in changed itself (its own editor, program changes).
    func synchronizeParameters() {
        guard let handle = state.handle else { return }
        let flags = st_vst2_take_host_flags(handle)
        if flags & 2 != 0 { st_vst2_refresh_parameters(handle) }
        var changed = [Int32](repeating: 0, count: 256)
        let count = Int(st_vst2_take_changed_parameters(handle, &changed, Int32(changed.count)))
        guard count > 0 || flags & 2 != 0, let tree = parameters else { return }
        publishing = true
        defer { publishing = false }
        let all = flags & 2 != 0 || count > changed.count
        for parameter in all ? tree.allParameters : changed.prefix(count).compactMap({ tree.parameter(withAddress: AUParameterAddress($0)) }) {
            let value = AUValue(st_vst2_parameter_value(handle, Int32(truncatingIfNeeded: parameter.address)))
            if parameter.value != value { parameter.setValue(value, originator: nil) }
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
    override var latency: TimeInterval {
        guard let handle = state.handle, !shouldBypassEffect else { return 0 }
        return Double(st_vst2_latency(handle)) / max(1, state.sampleRate)
    }
    override var tailTime: TimeInterval {
        guard let handle = state.handle else { return 0 }
        return Double(st_vst2_tail(handle)) / max(1, state.sampleRate)
    }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = VST2RenderState.maximumFrames }
    }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        // Re-applying the bus's own format (a stopped engine does so for every connection) is no change.
        format == bus.format || !renderResourcesAllocated && format.sampleRate >= 8_000 && format.sampleRate <= 768_000 &&
            format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved && format.channelCount == 2
    }

    override func allocateRenderResources() throws {
        guard let handle = state.handle else { throw HostError.message("The VST2 plug-in is closed.") }
        state.sampleRate = outputs[0].format.sampleRate
        var error = [CChar](repeating: 0, count: 2048)
        guard st_vst2_prepare(handle, state.sampleRate, VST2RenderState.maximumFrames, &error, error.count) != 0 else {
            throw HostError.message(Self.errorText(error, fallback: "The VST2 plug-in could not start processing."))
        }
        try super.allocateRenderResources()
    }
    override func deallocateRenderResources() {
        if let handle = state.handle { st_vst2_suspend(handle) }
        super.deallocateRenderResources()
    }
    /// Graph epoch resets discard queued note-offs; release held notes explicitly,
    /// as VST3AudioUnit does, instead of relying on the plug-in honouring CC 123.
    override func reset() {
        if let handle = state.handle { _ = st_vst2_reset(handle) }
        super.reset()
    }

    /// The render callback (see STAURenderUnit); `context` is this unit's render state.
    private static let render: STAURenderFunction = { context, flags, timestamp, frames, bus, output, events, pull in
        guard bus == 0 else { return kAudioUnitErr_InvalidElement }
        guard let context else { return kAudioUnitErr_Uninitialized }
        guard timestamp.pointee.mSampleTime.isFinite, abs(timestamp.pointee.mSampleTime) < 1e15 else { return kAudio_ParamError }
        return Unmanaged<VST2RenderState>.fromOpaque(context)._withUnsafeGuaranteedRef {
            $0.render(flags: flags, timestamp: timestamp, frames: frames, output: output, events: events, pull: pull)
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

    func documentState() throws -> [String: Any] {
        guard let handle = state.handle, let descriptor else { throw HostError.message("No VST2 plug-in is loaded.") }
        var bytes: UnsafeMutableRawPointer?, count = 0
        var error = [CChar](repeating: 0, count: 2048)
        guard st_vst2_copy_state(handle, &bytes, &count, &error, error.count) != 0, let bytes else {
            throw HostError.message(Self.errorText(error, fallback: "The VST2 state could not be saved."))
        }
        defer { st_vst2_free(bytes) }
        return ["format": "SuperTracker.VST2", "version": 1, "id": descriptor.id, "bundlePath": descriptor.bundlePath,
                "data": Data(bytes: bytes, count: count)]
    }

    func restoreDocumentState(_ value: [String: Any]) throws {
        guard let handle = state.handle, let descriptor, value["format"] as? String == "SuperTracker.VST2",
              value["version"] as? Int == 1, (value["id"] as? String)?.uppercased() == descriptor.id.uppercased(),
              let data = value["data"] as? Data, !data.isEmpty else {
            throw HostError.message("The saved VST2 state does not match this plug-in.")
        }
        var error = [CChar](repeating: 0, count: 2048)
        let result = data.withUnsafeBytes { st_vst2_restore_state(handle, $0.baseAddress, data.count, &error, error.count) }
        guard result != 0 else { throw HostError.message(Self.errorText(error, fallback: "The VST2 plug-in rejected its saved state.")) }
        publishAllParameters()
    }

    /// Applies a raw VST2 chunk such as an NKS PCHK payload.
    func restoreChunk(_ data: Data, preset: Bool) throws {
        guard let handle = state.handle, !data.isEmpty else { throw HostError.message("No VST2 plug-in is loaded.") }
        var error = [CChar](repeating: 0, count: 2048)
        let result = data.withUnsafeBytes { st_vst2_restore_chunk(handle, $0.baseAddress, data.count, preset ? 1 : 0, &error, error.count) }
        guard result != 0 else { throw HostError.message(Self.errorText(error, fallback: "The VST2 plug-in rejected this preset.")) }
        publishAllParameters()
    }

    private func publishAllParameters() {
        guard let handle = state.handle, let tree = parameters else { return }
        publishing = true
        defer { publishing = false }
        for parameter in tree.allParameters {
            let value = AUValue(st_vst2_parameter_value(handle, Int32(truncatingIfNeeded: parameter.address)))
            if parameter.value != value { parameter.setValue(value, originator: nil) }
        }
    }

    var parameterIDs: Set<UInt32> { Set(parameters?.allParameters.map { UInt32(truncatingIfNeeded: $0.address) } ?? []) }

    var status: [String: Any] {
        ["format": "VST2", "uniqueID": descriptor.map { VST3Catalogue.fourCC($0.uniqueID) } ?? "",
         "droppedEvents": state.droppedEvents, "needsReload": false]
    }

    override func requestViewController(completionHandler: @escaping (NSViewController?) -> Void) {
        let create = { [weak self] in
            guard let self else { completionHandler(nil); return }
            let controller = VST2EditorController(state: self.state)
            completionHandler(controller.attach() ? controller : nil)
        }
        if Thread.isMainThread { create() } else { DispatchQueue.main.async(execute: create) }
    }

    private static func errorText(_ chars: [CChar], fallback: String) -> String {
        let text = String(cString: chars)
        return text.isEmpty ? fallback : text
    }
}

/// Render-side owner: preallocated buffers and event arrays only.
private final class VST2RenderState {
    static let maximumFrames: UInt32 = 16_384
    private static let eventCapacity = 2_048
    var handle: OpaquePointer?
    var sampleRate: Double = 48_000
    var isInstrument = false
    var musicalContext: AUHostMusicalContextBlock?
    var transportContext: AUHostTransportStateBlock?
    private let flags = UnsafeMutablePointer<Int64>.allocate(capacity: 2)
    var bypassed: Bool {
        get { OSAtomicAdd64Barrier(0, flags) != 0 }
        set { let desired: Int64 = newValue ? 1 : 0; let old = OSAtomicAdd64Barrier(0, flags); _ = OSAtomicCompareAndSwap64Barrier(old, desired, flags) }
    }
    var droppedEvents: Int64 { OSAtomicAdd64Barrier(0, flags.advanced(by: 1)) }
    private let left = UnsafeMutablePointer<Float>.allocate(capacity: Int(maximumFrames))
    private let right = UnsafeMutablePointer<Float>.allocate(capacity: Int(maximumFrames))
    private let inputSamples = UnsafeMutablePointer<Float>.allocate(capacity: Int(maximumFrames) * 2)
    private let inputList = AudioBufferList.allocate(maximumBuffers: 2)
    private let midi = UnsafeMutablePointer<STVST2MIDI>.allocate(capacity: eventCapacity)
    private let changes = UnsafeMutablePointer<STVST2ParameterChange>.allocate(capacity: eventCapacity)

    init() {
        flags.initialize(repeating: 0, count: 2)
        left.initialize(repeating: 0, count: Int(Self.maximumFrames)); right.initialize(repeating: 0, count: Int(Self.maximumFrames))
        inputSamples.initialize(repeating: 0, count: Int(Self.maximumFrames) * 2)
        midi.initialize(repeating: STVST2MIDI(), count: Self.eventCapacity)
        changes.initialize(repeating: STVST2ParameterChange(), count: Self.eventCapacity)
    }
    func close() {
        guard let existing = handle else { return }
        handle = nil
        st_vst2_destroy(existing)
    }
    deinit {
        close()
        flags.deallocate(); left.deallocate(); right.deallocate(); inputSamples.deallocate()
        free(inputList.unsafeMutablePointer); midi.deallocate(); changes.deallocate()
    }

    func render(flags actionFlags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, timestamp: UnsafePointer<AudioTimeStamp>,
                frames: UInt32, output: UnsafeMutablePointer<AudioBufferList>, events: UnsafePointer<AURenderEvent>?,
                pull: UnsafeRawPointer?) -> AUAudioUnitStatus {
        guard let handle else { return kAudioUnitErr_Uninitialized }
        guard frames <= Self.maximumFrames else { return kAudioUnitErr_TooManyFramesToProcess }
        let buffers = UnsafeMutableAudioBufferListPointer(output), bytes = frames * UInt32(MemoryLayout<Float>.size)
        guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
        if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(left); buffers[0].mDataByteSize = bytes }
        if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(right); buffers[1].mDataByteSize = bytes }
        guard let outLeft = buffers[0].mData?.assumingMemoryBound(to: Float.self),
              let outRight = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
        var sourceLeft: UnsafePointer<Float>?, sourceRight: UnsafePointer<Float>?
        if !isInstrument {
            guard let pull else { return kAudioUnitErr_NoConnection }
            let list = UnsafeMutableAudioBufferListPointer(inputList.unsafeMutablePointer)
            list.unsafeMutablePointer.pointee.mNumberBuffers = 2
            list[0] = AudioBuffer(mNumberChannels: 1, mDataByteSize: bytes, mData: inputSamples)
            list[1] = AudioBuffer(mNumberChannels: 1, mDataByteSize: bytes, mData: inputSamples.advanced(by: Int(Self.maximumFrames)))
            var inputFlags = AudioUnitRenderActionFlags()
            let result = st_au_pull(pull, &inputFlags, timestamp, frames, 0, list.unsafeMutablePointer)
            guard result == noErr else { return result }
            guard list.count == 2, list[0].mDataByteSize >= bytes, list[1].mDataByteSize >= bytes,
                  let l = list[0].mData?.assumingMemoryBound(to: Float.self), let r = list[1].mData?.assumingMemoryBound(to: Float.self) else {
                return kAudioUnitErr_FormatNotSupported
            }
            sourceLeft = UnsafePointer(l); sourceRight = UnsafePointer(r)
        }
        var midiCount = 0, parameterCount = 0
        var next = events
        let blockStart = Int64(timestamp.pointee.mSampleTime)
        while let event = next {
            let sample = event.pointee.head.eventSampleTime
            let relative = sample == AUEventSampleTimeImmediate ? 0 : max(0, min(Int64(frames) - 1, sample - blockStart))
            switch event.pointee.head.eventType {
            case .MIDI:
                let message = event.pointee.MIDI
                if message.length >= 1, message.length <= 3, midiCount < Self.eventCapacity {
                    midi[midiCount] = STVST2MIDI(sample_offset: Int32(relative), status: message.data.0,
                        data1: message.length > 1 ? message.data.1 : 0, data2: message.length > 2 ? message.data.2 : 0)
                    midiCount += 1
                } else { OSAtomicIncrement64Barrier(flags.advanced(by: 1)) }
            case .parameter, .parameterRamp:
                let parameter = event.pointee.parameter
                if parameter.parameterAddress <= UInt64(Int32.max), parameter.value.isFinite, parameterCount < Self.eventCapacity {
                    changes[parameterCount] = STVST2ParameterChange(sample_offset: Int32(relative), index: Int32(parameter.parameterAddress), value: parameter.value)
                    parameterCount += 1
                }
            default: break
            }
            next = event.pointee.head.next.map { UnsafePointer($0) }
        }
        var transport = STVST2Transport()
        let context = VST3TransportContext.make(timestamp: timestamp.pointee, musical: musicalContext, transport: transportContext)
        transport.sample_position = Double(context.sample_position)
        transport.sample_rate = sampleRate
        transport.tempo = context.tempo
        transport.numerator = context.numerator; transport.denominator = context.denominator
        transport.beat_position = context.beat_position; transport.bar_start_beat = context.bar_start_beat
        transport.musical_valid = context.valid_fields & UInt32(ST_VST3_TRANSPORT_MUSIC_VALID) != 0 ? 1 : 0
        transport.playing = context.playing; transport.recording = context.recording; transport.cycling = context.cycling
        transport.cycle_start_beat = context.cycle_start_beat; transport.cycle_end_beat = context.cycle_end_beat
        let result = st_vst2_process(handle, sourceLeft, sourceRight, outLeft, outRight, frames,
                                     midi, UInt32(midiCount), changes, UInt32(parameterCount), &transport)
        guard result != 0 else { memset(outLeft, 0, Int(bytes)); memset(outRight, 0, Int(bytes)); return kAudioUnitErr_CannotDoInCurrentContext }
        if bypassed {
            if let sourceLeft, let sourceRight { memcpy(outLeft, sourceLeft, Int(bytes)); memcpy(outRight, sourceRight, Int(bytes)) }
            else { memset(outLeft, 0, Int(bytes)); memset(outRight, 0, Int(bytes)) }
        }
        buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
        actionFlags.pointee.remove(.unitRenderAction_OutputIsSilence)
        return noErr
    }

    // Editor helpers (main thread).
    func attachEditor(_ view: NSView) -> NSSize? {
        guard let handle else { return nil }
        var width: Int32 = 640, height: Int32 = 420
        var error = [CChar](repeating: 0, count: 1024)
        guard st_vst2_attach_editor(handle, Unmanaged.passUnretained(view).toOpaque(), &width, &height, &error, error.count) != 0 else { return nil }
        return NSSize(width: Int(max(1, width)), height: Int(max(1, height)))
    }
    func detachEditor() { if let handle { st_vst2_detach_editor(handle) } }
    func idleEditor() -> NSSize? {
        guard let handle else { return nil }
        var width: Int32 = 0, height: Int32 = 0
        return st_vst2_editor_idle(handle, &width, &height) != 0 ? NSSize(width: Int(width), height: Int(height)) : nil
    }
}

private final class VST2EditorController: NSViewController {
    private let state: VST2RenderState
    private var attached = false
    private var idle: Timer?
    init(state: VST2RenderState) { self.state = state; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
    override func loadView() { view = NSView(frame: NSRect(x: 0, y: 0, width: 640, height: 420)) }
    func attach() -> Bool {
        guard !attached else { return true }
        guard let size = state.attachEditor(view) else { return false }
        attached = true
        view.setFrameSize(size); preferredContentSize = size
        let timer = Timer(timeInterval: 1.0 / 30.0, repeats: true) { [weak self] _ in
            guard let self, let size = self.state.idleEditor() else { return }
            self.view.setFrameSize(size); self.preferredContentSize = size
            self.view.window?.setContentSize(size)
        }
        idle = timer
        RunLoop.main.add(timer, forMode: .common)
        return true
    }
    override func viewDidAppear() { super.viewDidAppear(); _ = attach() }
    override func viewDidDisappear() {
        idle?.invalidate(); idle = nil
        if attached { state.detachEditor(); attached = false }
        super.viewDidDisappear()
    }
    deinit { idle?.invalidate(); if attached { state.detachEditor() } }
}

import AppKit
import AVFoundation
import AudioToolbox
import Darwin
import TrackerEngine
#if canImport(AURenderShim)
import AURenderShim
#endif

/// First-party DSP presented through the same rack contract as hosted plug-ins.
/// Its identifiers and state are ours; no NI binary state compatibility is implied.
final class BuiltinEffectsAudioUnit: STAURenderUnit {
    private static let description = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x73746678, componentManufacturer: 0x53705472, componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(BuiltinEffectsAudioUnit.self, as: description, name: "SuperTracker: Studio Effects", version: 1)
    }()
    static func isAdapterDescription(_ value: AudioComponentDescription) -> Bool {
        value.componentManufacturer == description.componentManufacturer && value.componentSubType == description.componentSubType
    }
    static func dspID(_ id: String) -> String { id.hasPrefix("stfx:") ? String(id.dropFirst(5)) : id }
    static func has(id: String) -> Bool { st_builtin_effect_find(dspID(id)) != nil }
    static func catalogue() -> [[String: Any]] {
        (0..<st_builtin_effect_count()).compactMap { index in
            guard let ptr = st_builtin_effect_info(index) else { return nil }
            let info = ptr.pointee
            return ["id": "stfx:" + String(cString: info.id), "name": String(cString: info.name),
                    "manufacturer": "SuperTracker", "format": "Builtin", "kind": "effect",
                    "category": String(cString: info.category), "isInstrument": false]
        }
    }
    static func drumModels() -> [[String: Any]] {
        (0..<st_drum_model_count()).compactMap { index in
            guard let info = st_drum_model_info(index)?.pointee else { return nil }
            let id = String(cString: info.id), model = st_drum_model_for_id(info.id)
            let parameters: [[String: Any]] = (0..<info.parameterCount).compactMap { parameter in
                guard let p = st_drum_parameter_info(model, parameter)?.pointee else { return nil }
                return ["index": Int(parameter), "id": String(cString: p.id), "name": String(cString: p.name), "unit": String(cString: p.unit),
                    "min": p.minimum, "max": p.maximum, "defaultValue": st_drum_parameter_default(model, parameter), "defaultPhysical": p.defaultValue,
                    "logarithmic": p.flags & UInt32(ST_BUILTIN_PARAMETER_LOG) != 0,
                    "stepCount": p.flags & UInt32(ST_BUILTIN_PARAMETER_INTEGER) != 0 ? Int(p.maximum - p.minimum) : 0]
            }
            return ["id": id, "model": model, "name": String(cString: info.name), "parameters": parameters]
        }
    }
    static func makeNode(id: String, sampleRate: Double) throws -> AVAudioUnit {
        _ = registration
        let node = AVAudioUnitEffect(audioComponentDescription: description)
        guard let unit = node.auAudioUnit as? BuiltinEffectsAudioUnit else { throw TrackerError.message("The studio effect adapter could not be created.") }
        try unit.configure(id: dspID(id), sampleRate: sampleRate)
        return node
    }
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree?
    private let renderState = BuiltinEffectsRenderState()
    private(set) var effectID = ""
    private var publishingParameters = false
    private(set) var lastStateError: String?
    override init(componentDescription: AudioComponentDescription, options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let input = try AUAudioUnitBus(format: format), output = try AUAudioUnitBus(format: format)
        input.supportedChannelCounts = [2]; output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        maximumFramesToRender = BuiltinEffectsRenderState.capacity
        setRenderFunction(Self.render, context: Unmanaged.passUnretained(renderState).toOpaque())
    }
    /// The render callback (see STAURenderUnit): no per-render closure or allocation.
    private static let render: STAURenderFunction = { context, flags, timestamp, frames, bus, output, events, pull in
        guard bus == 0 else { return kAudioUnitErr_InvalidElement }
        guard let context else { return kAudioUnitErr_Uninitialized }
        guard timestamp.pointee.mSampleTime.isFinite, abs(timestamp.pointee.mSampleTime) < 1e15 else { return kAudio_ParamError }
        return Unmanaged<BuiltinEffectsRenderState>.fromOpaque(context)._withUnsafeGuaranteedRef {
            $0.render(flags: flags, timestamp: timestamp, frames: frames, output: output, events: events, pull: pull)
        }
    }
    private func configure(id: String, sampleRate: Double) throws {
        guard !renderResourcesAllocated, let info = st_builtin_effect_find(id) else { throw TrackerError.message("Unknown studio effect: \(id).") }
        effectID = id
        try renderState.prepare(id: id, sampleRate: sampleRate)
        if info.pointee.flags & UInt32(ST_BUILTIN_EFFECT_SIDECHAIN) != 0 {
            let bus = try AUAudioUnitBus(format: inputs[0].format); bus.supportedChannelCounts = [2]
            bus.name = "Sidechain"
            inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [inputs[0], bus])
            renderState.sidechain = true
        }
        var list: [AUParameter] = []
        for index in 0..<info.pointee.parameterCount {
            guard let metadata = st_builtin_effect_parameter_info(id, index) else { continue }
            let p = metadata.pointee
            let param = AUParameterTree.createParameter(withIdentifier: String(cString: p.id), name: String(cString: p.name),
                address: AUParameterAddress(index), min: 0, max: 1, unit: .generic, unitName: String(cString: p.unit),
                flags: [.flag_IsReadable, .flag_IsWritable], valueStrings: nil, dependentParameters: nil)
            param.value = AUValue(st_builtin_effect_to_normalized(id, index, p.defaultValue))
            list.append(param)
        }
        parameters = AUParameterTree.createTree(withChildren: list)
        let dsp = renderState
        parameters?.implementorValueObserver = { [weak self] parameter, value in
            guard self?.publishingParameters == false, value.isFinite else { return }
            dsp.publishParameter(UInt32(parameter.address), value: Double(value))
        }
        parameters?.implementorValueProvider = { parameter in
            guard let handle = dsp.handle else { return 0 }
            return AUValue(st_builtin_effect_get_parameter(handle, UInt32(parameter.address)))
        }
        parameters?.implementorStringFromValueCallback = { parameter, value in
            let normalized = Double(value?.pointee ?? parameter.value)
            let index = UInt32(parameter.address)
            let raw = st_builtin_effect_from_normalized(id, index, normalized)
            guard let metadata = st_builtin_effect_parameter_info(id, index) else { return String(format: "%.3f", raw) }
            let p = metadata.pointee
            if p.labelCount > 0, let label = st_builtin_effect_parameter_label(id, index, UInt32(max(0, min(Double(p.labelCount - 1), raw.rounded() - p.minimum)))) { return String(cString: label) }
            let unit = String(cString: p.unit)
            return String(format: p.flags & UInt32(ST_BUILTIN_PARAMETER_INTEGER) != 0 ? "%.0f" : "%.2f", raw) + (unit.isEmpty ? "" : " " + unit)
        }
    }
    /// AUParameter caches its last control-side value. Publish a DSP snapshot
    /// when the rack reads controls, without writing that older snapshot back
    /// over a concurrently scheduled automation event.
    func synchronizeParameterValues() {
        guard let handle = renderState.handle else { return }
        publishingParameters = true
        defer { publishingParameters = false }
        for parameter in parameters?.allParameters ?? [] {
            let value = AUValue(st_builtin_effect_get_parameter(handle, UInt32(parameter.address)))
            if parameter.value != value { parameter.setValue(value, originator: nil) }
        }
    }
    func parameterStepCount(address: AUParameterAddress) -> Int? {
        guard address < 64, let p = st_builtin_effect_parameter_info(effectID, UInt32(address))?.pointee else { return nil }
        if p.flags & UInt32(ST_BUILTIN_PARAMETER_TOGGLE) != 0 { return 1 }
        if p.flags & UInt32(ST_BUILTIN_PARAMETER_INTEGER) != 0 { return max(1, Int((p.maximum - p.minimum).rounded())) }
        return 0
    }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var parameterTree: AUParameterTree? { get { parameters } set { parameters = newValue } }
    override var canProcessInPlace: Bool { false }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var shouldBypassEffect: Bool { get { renderState.bypassed } set { renderState.bypassed = newValue } }
    override var latency: TimeInterval { renderState.handle.map { Double(st_builtin_effect_latency($0)) / renderState.sampleRate } ?? 0 }
    override var tailTime: TimeInterval { renderState.handle.map { st_builtin_effect_tail_seconds($0) } ?? 0 }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && (8_000...384_000).contains(format.sampleRate) && format.channelCount == 2 && !format.isInterleaved && format.commonFormat == .pcmFormatFloat32
    }
    override func allocateRenderResources() throws {
        let sr = outputs[0].format.sampleRate
        if sr != renderState.sampleRate { try renderState.prepare(id: effectID, sampleRate: sr) }
        refreshHostContext()
        try super.allocateRenderResources()
    }
    func refreshHostContext() {
        renderState.musicalContext = musicalContextBlock
        renderState.transportContext = transportStateBlock
    }
    override func reset() {
        super.reset()
        renderState.reset()
    }
    func documentState() throws -> [String: Any] {
        guard let handle = renderState.handle, let info = st_builtin_effect_find(effectID) else { throw TrackerError.message("No studio effect is loaded.") }
        var values: [String: Double] = [:]
        for index in 0..<info.pointee.parameterCount {
            if let p = st_builtin_effect_parameter_info(effectID, index) { values[String(cString: p.pointee.id)] = Double(AUValue(st_builtin_effect_get_parameter(handle, index))) }
        }
        return ["format": "SuperTracker.BuiltinFX", "version": 1, "id": effectID, "parameters": values]
    }
    func restoreDocumentState(_ state: [String: Any]) throws {
        guard state["format"] as? String == "SuperTracker.BuiltinFX", state["version"] as? Int == 1,
              state["id"] as? String == effectID, let values = state["parameters"] as? [String: Double],
              let info = st_builtin_effect_find(effectID) else { throw TrackerError.message("The saved effect state does not match this studio effect.") }
        var changes: [(AUParameter, Double)] = []
        for index in 0..<info.pointee.parameterCount {
            guard let p = st_builtin_effect_parameter_info(effectID, index), let value = values[String(cString: p.pointee.id)] else { continue }
            guard value.isFinite, (0...1).contains(value), let parameter = parameters?.parameter(withAddress: UInt64(index)) else { throw TrackerError.message("The saved effect contains an invalid parameter.") }
            changes.append((parameter, value))
        }
        guard changes.count == values.count else { throw TrackerError.message("The saved effect contains an unknown parameter.") }
        publishingParameters = true
        defer { publishingParameters = false }
        for (parameter, value) in changes {
            parameter.value = AUValue(value)
            renderState.publishParameter(UInt32(parameter.address), value: Double(AUValue(value)))
        }
    }
    override var fullState: [String: Any]? {
        get { try? documentState() }
        set { do { if let newValue { try restoreDocumentState(newValue) }; lastStateError = nil } catch { lastStateError = error.localizedDescription } }
    }
    override var fullStateForDocument: [String: Any]? { get { fullState } set { fullState = newValue } }
    /// Scheduled automation goes to the render state's own queue, not AUAudioUnit's event
    /// schedule, so it reaches the DSP both when this unit renders as its own node and when
    /// a BuiltinEffectsChain runs it inside a shared insert-chain node.
    override var scheduleParameterBlock: AUScheduleParameterBlock { renderState.scheduleBlock }
    /// Opaque handle for BuiltinEffectsChain.setMembers: this unit's DSP, parameters,
    /// automation queue and bypass state, rendered in place by the chain.
    var chainMember: AnyObject { renderState }
}


/// Scheduled parameter automation for one built-in effect: a bounded multi-producer queue
/// (render-thread automation, step-FX locks and control-thread schedules may overlap), drained
/// by the single renderer into a time-ordered heap. Never allocates or waits after init.
private final class BuiltinEventQueue {
    struct Event { var time: Int64 = 0; var value: Float = 0; var address: UInt32 = 0; var ramp: UInt32 = 0; var order: Int64 = 0 }
    static let capacity = 4_096
    private let queue = UnsafeMutablePointer<Event>.allocate(capacity: capacity)
    private let sequences = UnsafeMutablePointer<Int64>.allocate(capacity: capacity)
    private let enqueuePosition = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let dropped = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private var dequeuePosition: Int64 = 0
    private let pending = UnsafeMutablePointer<Event>.allocate(capacity: capacity)
    private(set) var pendingCount = 0
    init() {
        queue.initialize(repeating: Event(), count: Self.capacity); pending.initialize(repeating: Event(), count: Self.capacity)
        sequences.initialize(repeating: 0, count: Self.capacity)
        for index in 0..<Self.capacity { sequences[index] = Int64(index) }
        enqueuePosition.initialize(to: 0); dropped.initialize(to: 0)
    }
    deinit {
        queue.deallocate(); pending.deallocate(); sequences.deallocate(); enqueuePosition.deallocate(); dropped.deallocate()
    }
    var droppedEvents: Int { Int(OSAtomicAdd64Barrier(0, dropped)) }
    /// Any thread, including the render thread. Sequence-tagged bounded MPSC publication;
    /// a full queue counts a dropped event instead of waiting.
    func push(time: Int64, ramp: UInt32, address: UInt32, value: Float) {
        var position = OSAtomicAdd64Barrier(0, enqueuePosition)
        for _ in 0..<32 {
            let slot = Int(position & Int64(Self.capacity - 1))
            let difference = OSAtomicAdd64Barrier(0, sequences.advanced(by: slot)) &- position
            if difference == 0 {
                if OSAtomicCompareAndSwap64Barrier(position, position &+ 1, enqueuePosition) {
                    queue[slot] = Event(time: time, value: value, address: address, ramp: ramp, order: position)
                    _ = OSAtomicCompareAndSwap64Barrier(position, position &+ 1, sequences.advanced(by: slot))
                    return
                }
            } else if difference < 0 { break }
            position = OSAtomicAdd64Barrier(0, enqueuePosition)
        }
        OSAtomicIncrement64Barrier(dropped)
    }
    /// Render thread: moves published events into the heap. Immediate-relative times
    /// (AUEventSampleTimeImmediate + offset) become absolute from `blockStart`.
    @inline(__always) func drain(blockStart: Int64) {
        // Fast path: nothing published since the last drain.
        let slot0 = Int(dequeuePosition & Int64(Self.capacity - 1))
        guard OSAtomicAdd64Barrier(0, sequences.advanced(by: slot0)) == dequeuePosition &+ 1 else { return }
        for _ in 0..<Self.capacity {
            let slot = Int(dequeuePosition & Int64(Self.capacity - 1))
            guard OSAtomicAdd64Barrier(0, sequences.advanced(by: slot)) == dequeuePosition &+ 1 else { break }
            var event = queue[slot]
            _ = OSAtomicCompareAndSwap64Barrier(dequeuePosition &+ 1, dequeuePosition &+ Int64(Self.capacity), sequences.advanced(by: slot))
            dequeuePosition = dequeuePosition &+ 1
            if event.time >= AUEventSampleTimeImmediate && event.time <= AUEventSampleTimeImmediate + 1_000_000 {
                event.time = blockStart + (event.time - AUEventSampleTimeImmediate)
            }
            guard pendingCount < Self.capacity else { OSAtomicIncrement64Barrier(dropped); continue }
            var position = pendingCount
            while position > 0 {
                let parent = (position - 1) / 2
                if !precedes(event, pending[parent]) { break }
                pending[position] = pending[parent]; position = parent
            }
            pending[position] = event; pendingCount += 1
        }
    }
    @inline(__always) private func precedes(_ a: Event, _ b: Event) -> Bool { a.time < b.time || (a.time == b.time && a.order < b.order) }
    /// Render thread: the earliest pending event, if any.
    @inline(__always) var first: Event? { pendingCount > 0 ? pending[0] : nil }
    @inline(__always) func removeFirst() {
        pendingCount -= 1
        guard pendingCount > 0 else { return }
        let replacement = pending[pendingCount]
        var position = 0
        while position * 2 + 1 < pendingCount {
            var child = position * 2 + 1
            if child + 1 < pendingCount && precedes(pending[child + 1], pending[child]) { child += 1 }
            if !precedes(pending[child], replacement) { break }
            pending[position] = pending[child]; position = child
        }
        pending[position] = replacement
    }
}

private final class BuiltinEffectsRenderState {
    static let capacity: UInt32 = 16_384
    var handle: OpaquePointer?
    var sampleRate: Double = 48_000
    var sidechain = false
    var musicalContext: AUHostMusicalContextBlock?
    var transportContext: AUHostTransportStateBlock?
    let scheduled = BuiltinEventQueue()
    private(set) lazy var scheduleBlock: AUScheduleParameterBlock = { [scheduled] time, ramp, address, value in
        guard address < 64, value.isFinite else { return }
        scheduled.push(time: time, ramp: ramp, address: UInt32(address), value: value)
    }
    private let bypass = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    var bypassed: Bool {
        get { OSAtomicAdd64Barrier(0, bypass) != 0 }
        set { let value: Int64 = newValue ? 1 : 0; while true { let old = OSAtomicAdd64Barrier(0, bypass); if OSAtomicCompareAndSwap64Barrier(old, value, bypass) { break } } }
    }
    private let buffers = UnsafeMutablePointer<Float>.allocate(capacity: Int(capacity) * 8)
    private let inputList = AudioBufferList.allocate(maximumBuffers: 2)
    private let sideList = AudioBufferList.allocate(maximumBuffers: 2)
    private var dryDelay: UnsafeMutablePointer<Float>?
    private var delayLength = 0, delayCursor = 0
    private var wet: Float = 1
    /// A bypassed insert whose crossfade has finished is not processed at all (a
    /// Maschine import keeps disabled modules as bypassed inserts). Its state is
    /// cleared when it is engaged again, so it starts from silence, not a stale tail.
    private var dormant = false
    private let rampRemaining = UnsafeMutablePointer<UInt32>.allocate(capacity: 64)
    private let rampValue = UnsafeMutablePointer<Double>.allocate(capacity: 64)
    private let rampDelta = UnsafeMutablePointer<Double>.allocate(capacity: 64)
    private var activeRamps = 0
    private let changeGeneration = UnsafeMutablePointer<Int64>.allocate(capacity: 64)
    private let changeValue = UnsafeMutablePointer<Int64>.allocate(capacity: 64)
    private let consumedGeneration = UnsafeMutablePointer<Int64>.allocate(capacity: 64)
    /// Bumped after any parameter publication, so a callback with nothing new skips
    /// the per-parameter barrier reads.
    private let anyChange = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private var consumedAnyChange: Int64 = 0
    init() { bypass.initialize(to: 0); anyChange.initialize(to: 0); buffers.initialize(repeating: 0, count: Int(Self.capacity) * 8); rampRemaining.initialize(repeating:0,count:64); rampValue.initialize(repeating:0,count:64); rampDelta.initialize(repeating:0,count:64); changeGeneration.initialize(repeating:0,count:64); changeValue.initialize(repeating:0,count:64); consumedGeneration.initialize(repeating:0,count:64) }
    deinit { if let handle { st_builtin_effect_destroy(handle) }; anyChange.deallocate(); dryDelay?.deallocate(); buffers.deallocate(); bypass.deallocate(); rampRemaining.deallocate(); rampValue.deallocate(); rampDelta.deallocate(); changeGeneration.deallocate(); changeValue.deallocate(); consumedGeneration.deallocate(); free(inputList.unsafeMutablePointer); free(sideList.unsafeMutablePointer) }
    var latencyFrames: Int { handle.map { Int(st_builtin_effect_latency($0)) } ?? 0 }
    /// Control-side changes replace an unfinished ramp for this parameter at
    /// the next callback. Only the render thread mutates the ramp arrays.
    func publishParameter(_ index: UInt32, value: Double) {
        guard index < 64, let handle else { return }
        let pointer = changeValue.advanced(by: Int(index)), bits = Int64(bitPattern: value.bitPattern)
        while true { let old = OSAtomicAdd64Barrier(0, pointer); if OSAtomicCompareAndSwap64Barrier(old, bits, pointer) { break } }
        _ = st_builtin_effect_set_parameter(handle, index, value)
        OSAtomicIncrement64Barrier(changeGeneration.advanced(by: Int(index)))
        OSAtomicIncrement64Barrier(anyChange)
    }
    private func consumeParameterChanges(_ handle: OpaquePointer) {
        // Read before the per-parameter scan: a publication racing this scan bumps
        // anyChange again afterwards and is picked up by the next callback.
        let any = OSAtomicAdd64Barrier(0, anyChange)
        guard any != consumedAnyChange else { return }
        consumedAnyChange = any
        for index in 0..<64 {
            let pointer = changeGeneration.advanced(by: index), generation = OSAtomicAdd64Barrier(0, pointer)
            guard generation != consumedGeneration[index] else { continue }
            let bits = OSAtomicAdd64Barrier(0, changeValue.advanced(by: index))
            guard generation == OSAtomicAdd64Barrier(0, pointer) else { continue }
            if rampRemaining[index] > 0 { rampRemaining[index] = 0; activeRamps -= 1 }
            _ = st_builtin_effect_set_parameter(handle, UInt32(index), Double(bitPattern: UInt64(bitPattern: bits)))
            consumedGeneration[index] = generation
        }
    }
    func prepare(id: String, sampleRate sr: Double) throws {
        guard let next = st_builtin_effect_create(id, sr, Self.capacity) else { throw TrackerError.message("Could not prepare the studio effect.") }
        if let handle {
            if let info = st_builtin_effect_find(id) { for i in 0..<info.pointee.parameterCount { _ = st_builtin_effect_set_parameter(next, i, st_builtin_effect_get_parameter(handle, i)) } }
            st_builtin_effect_destroy(handle)
        }
        handle = next; sampleRate = sr
        dryDelay?.deallocate(); delayLength = Int(st_builtin_effect_latency(next)); delayCursor = 0
        dryDelay = delayLength > 0 ? .allocate(capacity: delayLength * 2) : nil
        dryDelay?.initialize(repeating: 0, count: delayLength * 2)
    }
    func reset() { if let handle { st_builtin_effect_reset(handle) }; dryDelay?.update(repeating: 0, count: delayLength * 2); delayCursor = 0; wet = bypassed ? 0 : 1; rampRemaining.update(repeating:0,count:64); activeRamps=0 }
    private func setup(_ list: UnsafeMutableAudioBufferListPointer, frames: UInt32, left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>) {
        list.unsafeMutablePointer.pointee.mNumberBuffers = 2
        list[0] = AudioBuffer(mNumberChannels: 1, mDataByteSize: frames * 4, mData: left)
        list[1] = AudioBuffer(mNumberChannels: 1, mDataByteSize: frames * 4, mData: right)
    }
    private func process(_ handle: OpaquePointer, left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, scL: UnsafePointer<Float>?, scR: UnsafePointer<Float>?, offset: UInt32, count: UInt32) {
        guard count > 0 else { return }
        if activeRamps == 0 {
            _ = st_builtin_effect_process(handle,left.advanced(by:Int(offset)),right.advanced(by:Int(offset)),scL?.advanced(by:Int(offset)),scR?.advanced(by:Int(offset)),count)
            return
        }
        for frame in Int(offset)..<Int(offset + count) {
            if activeRamps == 0 {
                _ = st_builtin_effect_process(handle,left.advanced(by:frame),right.advanced(by:frame),scL?.advanced(by:frame),scR?.advanced(by:frame),UInt32(Int(offset + count)-frame))
                return
            }
            for index in 0..<64 where rampRemaining[index] > 0 { _ = st_builtin_effect_set_parameter(handle,UInt32(index),rampValue[index]) }
            _ = st_builtin_effect_process(handle,left.advanced(by:frame),right.advanced(by:frame),scL?.advanced(by:frame),scR?.advanced(by:frame),1)
            for index in 0..<64 where rampRemaining[index] > 0 {
                rampValue[index] += rampDelta[index]; rampRemaining[index] -= 1
                if rampRemaining[index] == 0 { activeRamps -= 1; _ = st_builtin_effect_set_parameter(handle,UInt32(index),rampValue[index]) }
            }
        }
    }
    /// Its own node: pulls the main (and sidechain) input, processes, writes the output.
    func render(flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, timestamp: UnsafePointer<AudioTimeStamp>, frames: UInt32,
                output: UnsafeMutablePointer<AudioBufferList>, events: UnsafePointer<AURenderEvent>?, pull: UnsafeRawPointer?) -> AUAudioUnitStatus {
        guard handle != nil else { return kAudioUnitErr_Uninitialized }
        guard frames <= Self.capacity else { return kAudioUnitErr_TooManyFramesToProcess }
        guard frames > 0 else { return noErr }
        guard let pull else { return kAudioUnitErr_NoConnection }
        let out = UnsafeMutableAudioBufferListPointer(output), input = UnsafeMutableAudioBufferListPointer(inputList.unsafeMutablePointer)
        guard out.count == 2 else { return kAudioUnitErr_FormatNotSupported }
        let stride = Int(Self.capacity), bytes = Int(frames) * 4
        let left = buffers, right = buffers.advanced(by: stride)
        setup(input, frames: frames, left: buffers.advanced(by: stride * 4), right: buffers.advanced(by: stride * 5))
        var pullFlags = AudioUnitRenderActionFlags()
        let status = st_au_pull(pull, &pullFlags, timestamp, frames, 0, input.unsafeMutablePointer)
        guard status == noErr else { return status }
        guard input.count == 2, input[0].mDataByteSize >= bytes, input[1].mDataByteSize >= bytes,
              let l = input[0].mData?.assumingMemoryBound(to: Float.self), let r = input[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudioUnitErr_FormatNotSupported }
        memcpy(left, l, bytes); memcpy(right, r, bytes)
        var scL: UnsafePointer<Float>?, scR: UnsafePointer<Float>?
        if sidechain {
            let list = UnsafeMutableAudioBufferListPointer(sideList.unsafeMutablePointer)
            setup(list, frames: frames, left: buffers.advanced(by: stride * 6), right: buffers.advanced(by: stride * 7))
            var auxFlags = AudioUnitRenderActionFlags()
            if st_au_pull(pull, &auxFlags, timestamp, frames, 1, list.unsafeMutablePointer) == noErr, list.count == 2,
               list[0].mDataByteSize >= bytes, list[1].mDataByteSize >= bytes,
               let al = list[0].mData?.assumingMemoryBound(to: Float.self), let ar = list[1].mData?.assumingMemoryBound(to: Float.self) {
                scL = UnsafePointer(al); scR = UnsafePointer(ar)
            }
        }
        processInPlace(left: left, right: right, frames: frames, sampleTime: Int64(timestamp.pointee.mSampleTime),
                       sidechainLeft: scL, sidechainRight: scR, renderEvents: events)
        for channel in 0..<2 {
            let source = channel == 0 ? left : right
            if let destination = out[channel].mData { memcpy(destination, source, bytes) } else { out[channel].mData = UnsafeMutableRawPointer(source) }
            out[channel].mDataByteSize = UInt32(bytes)
        }
        flags.pointee.remove(.unitRenderAction_OutputIsSilence)
        return noErr
    }
    /// Processes `frames` of `left`/`right` in place: parameter publications, scheduled and
    /// passed-in automation (sample accurate, with ramps), the effect, and the latency-aligned
    /// bypass crossfade. Its own node and BuiltinEffectsChain render exactly this.
    /// Render thread only; allocation and lock free.
    func processInPlace(left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>, frames: UInt32, sampleTime start: Int64,
                        sidechainLeft scL: UnsafePointer<Float>?, sidechainRight scR: UnsafePointer<Float>?, renderEvents: UnsafePointer<AURenderEvent>?) {
        guard let handle, frames > 0, frames <= Self.capacity else { return }
        consumeParameterChanges(handle)
        scheduled.drain(blockStart: start)
        let stride = Int(Self.capacity)
        let dryL = buffers.advanced(by: stride * 2), dryR = buffers.advanced(by: stride * 3)
        // The aligned dry path only reaches the output during a bypass crossfade (or
        // while bypassed). Fully engaged, the latency ring is still fed so a later
        // crossfade stays aligned, but no dry block is assembled or mixed.
        let target: Float = bypassed ? 0 : 1
        let mixesDry = wet != 1 || target != 1
        let count = Int(frames)
        let l = left, r = right
        if let delay = dryDelay, delayLength > 0 {
            if mixesDry {
                for i in 0..<count {
                    let dl = l[i].isFinite ? l[i] : 0, dr = r[i].isFinite ? r[i] : 0
                    dryL[i] = delay[delayCursor]; dryR[i] = delay[delayLength + delayCursor]
                    delay[delayCursor] = dl; delay[delayLength + delayCursor] = dr
                    delayCursor += 1; if delayCursor == delayLength { delayCursor = 0 }
                }
            } else {
                let delayR = delay.advanced(by: delayLength)
                var i = 0
                while i < count {
                    let run = min(count - i, delayLength - delayCursor)
                    for n in 0..<run {
                        let dl = l[i + n], dr = r[i + n]
                        delay[delayCursor + n] = dl.isFinite ? dl : 0; delayR[delayCursor + n] = dr.isFinite ? dr : 0
                    }
                    i += run; delayCursor += run; if delayCursor == delayLength { delayCursor = 0 }
                }
            }
        } else if mixesDry {
            for i in 0..<count { dryL[i] = l[i].isFinite ? l[i] : 0; dryR[i] = r[i].isFinite ? r[i] : 0 }
        }
        var tempo = 120.0
        if let musicalContext, musicalContext(&tempo, nil, nil, nil, nil, nil), tempo.isFinite { st_builtin_effect_set_tempo(handle, tempo) }
        var cursor: UInt32 = 0, next = renderEvents, eventCount = 0
        // Finished bypass crossfade: output is exactly the (delayed) dry signal, so the
        // processor is skipped; parameter events still land, without ramps.
        let idle = bypassed && wet <= 0
        if idle {
            if !dormant { dormant = true; rampRemaining.update(repeating:0,count:64); activeRamps = 0 }
        } else if dormant { dormant = false; st_builtin_effect_reset(handle) }
        let end = start + Int64(frames)
        func apply(_ address: UInt64, _ value: Float, ramp: UInt32) {
            guard address < 64, value.isFinite else { return }
            let index = Int(address), target = max(0, min(1, Double(value)))
            if rampRemaining[index] > 0 { activeRamps -= 1; rampRemaining[index] = 0 }
            if !idle, ramp > 0 {
                rampValue[index] = st_builtin_effect_get_parameter(handle, UInt32(index))
                rampRemaining[index] = ramp
                rampDelta[index] = (target - rampValue[index]) / Double(ramp)
                activeRamps += 1
            } else { _ = st_builtin_effect_set_parameter(handle, UInt32(index), target) }
        }
        while eventCount < 4096 {
            // The earlier of the next passed-in event and the next scheduled one; passed-in first on a tie.
            var listOffset: UInt32 = .max
            if let event = next {
                let time = event.pointee.head.eventSampleTime
                listOffset = UInt32(time == AUEventSampleTimeImmediate ? 0 : max(0, min(Int64(frames), time - start)))
            }
            var queueOffset: UInt32 = .max
            if let event = scheduled.first, event.time < end { queueOffset = UInt32(max(0, event.time - start)) }
            if listOffset == .max && queueOffset == .max { break }
            let offset = min(listOffset, queueOffset)
            if offset > cursor { if !idle { process(handle,left:left,right:right,scL:scL,scR:scR,offset:cursor,count:offset-cursor) }; cursor = offset }
            if listOffset <= queueOffset, let event = next {
                let head = event.pointee.head
                if head.eventType == .parameter || head.eventType == .parameterRamp {
                    let parameter = event.pointee.parameter
                    apply(parameter.parameterAddress, parameter.value, ramp: head.eventType == .parameterRamp ? parameter.rampDurationSampleFrames : 0)
                }
                next = head.next.map { UnsafePointer($0) }
            } else if let event = scheduled.first {
                scheduled.removeFirst()
                apply(UInt64(event.address), event.value, ramp: event.ramp)
            }
            eventCount += 1
        }
        if cursor < frames && !idle { process(handle,left:left,right:right,scL:scL,scR:scR,offset:cursor,count:frames-cursor) }
        if mixesDry {
            let delta = Float(1 / max(1, sampleRate * 0.005))
            for i in 0..<count { wet += max(-delta, min(delta, target - wet)); left[i] = dryL[i] + (left[i] - dryL[i]) * wet; right[i] = dryR[i] + (right[i] - dryR[i]) * wet }
        }
    }
}

/// Several consecutive built-in inserts of one strip in ONE graph node: the chain pulls its
/// input once and runs each member's processInPlace back to back on the same buffer. The
/// members stay ordinary BuiltinEffectsAudioUnit instances (parameters, automation, state,
/// bypass, latency and editors are theirs); only their rendering moves here. The member list
/// is replaced in place (setMembers) without touching graph connections: the renderer picks
/// up the new list at its next callback, and retired lists are released once no callback
/// can still be using them.
final class BuiltinEffectsChain {
    private static let description = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x73746368, componentManufacturer: 0x53705472, componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(BuiltinEffectsChainUnit.self, as: description, name: "SuperTracker: Studio Effects Chain", version: 1)
    }()
    let node: AVAudioUnitEffect
    private let unit: BuiltinEffectsChainUnit
    init(sampleRate: Double) throws {
        _ = Self.registration
        node = AVAudioUnitEffect(audioComponentDescription: Self.description)
        guard let unit = node.auAudioUnit as? BuiltinEffectsChainUnit,
              let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) else {
            throw TrackerError.message("The studio effect chain could not be created.")
        }
        self.unit = unit
        try unit.inputBusses[0].setFormat(format); try unit.outputBusses[0].setFormat(format)
    }
    /// Control thread. `members` are BuiltinEffectsAudioUnit.chainMember handles in processing order.
    func setMembers(_ members: [AnyObject]) throws {
        let states = members.compactMap { $0 as? BuiltinEffectsRenderState }
        guard states.count == members.count, Set(states.map(ObjectIdentifier.init)).count == states.count else {
            throw TrackerError.message("A studio effect chain accepts each built-in effect once.")
        }
        unit.state.publish(states)
    }
    /// The members' identities in processing order (control thread).
    var members: [AnyObject] { unit.state.members }
    /// Sum of the members' algorithmic latencies, in frames (informational: routing plans
    /// each member's latency itself).
    var latencyFrames: Int { unit.state.members.reduce(0) { $0 + $1.latencyFrames } }
    /// Control thread: release member lists no callback can still be using.
    func collectRetired() { unit.state.collectRetired() }
}

private final class BuiltinChainProgram {
    let members: [BuiltinEffectsRenderState]
    let pointers: UnsafeMutablePointer<Unmanaged<BuiltinEffectsRenderState>>
    init(_ members: [BuiltinEffectsRenderState]) {
        self.members = members
        pointers = .allocate(capacity: max(1, members.count))
        for (index, member) in members.enumerated() { (pointers + index).initialize(to: .passUnretained(member)) }
    }
    deinit { pointers.deallocate() }
}

private final class BuiltinChainRenderState {
    static let capacity: UInt32 = 16_384
    private let program = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let rendering = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private var current: BuiltinChainProgram?
    private var retired: [BuiltinChainProgram] = []
    private let buffers = UnsafeMutablePointer<Float>.allocate(capacity: Int(capacity) * 2)
    init() { program.initialize(to: 0); rendering.initialize(to: 0); buffers.initialize(repeating: 0, count: Int(Self.capacity) * 2) }
    deinit { program.deallocate(); rendering.deallocate(); buffers.deallocate() }
    var members: [BuiltinEffectsRenderState] { current?.members ?? [] }
    func publish(_ members: [BuiltinEffectsRenderState]) {
        let next = BuiltinChainProgram(members)
        if let current { retired.append(current) }
        current = next
        let bits = Int64(bitPattern: UInt64(UInt(bitPattern: Unmanaged.passUnretained(next).toOpaque())))
        while true { let old = OSAtomicAdd64Barrier(0, program); if OSAtomicCompareAndSwap64Barrier(old, bits, program) { break } }
        collectRetired()
    }
    /// A callback registers (rendering) before loading the program, so once the new program
    /// is published and no callback is registered, no callback can hold an older one.
    func collectRetired() {
        guard !retired.isEmpty, OSAtomicAdd64Barrier(0, rendering) == 0 else { return }
        retired.removeAll()
    }
    func reset() { for member in members { member.reset() } }
    func render(flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, timestamp: UnsafePointer<AudioTimeStamp>, frames: UInt32,
                output: UnsafeMutablePointer<AudioBufferList>, pull: UnsafeRawPointer?) -> AUAudioUnitStatus {
        guard frames <= Self.capacity else { return kAudioUnitErr_TooManyFramesToProcess }
        guard frames > 0 else { return noErr }
        guard let pull else { return kAudioUnitErr_NoConnection }
        let out = UnsafeMutableAudioBufferListPointer(output)
        guard out.count == 2 else { return kAudioUnitErr_FormatNotSupported }
        let bytes = Int(frames) * 4
        let left = buffers, right = buffers.advanced(by: Int(Self.capacity))
        // Pull straight into the output buffers (our own when the caller supplied none),
        // then process in place. A pull that substitutes its own buffers is copied first.
        let ownLeft = out[0].mData == nil ? UnsafeMutableRawPointer(left) : out[0].mData!
        let ownRight = out[1].mData == nil ? UnsafeMutableRawPointer(right) : out[1].mData!
        out[0].mData = ownLeft; out[1].mData = ownRight
        out[0].mDataByteSize = UInt32(bytes); out[1].mDataByteSize = UInt32(bytes)
        let status = st_au_pull(pull, flags, timestamp, frames, 0, output)
        guard status == noErr else { return status }
        guard out[0].mDataByteSize >= bytes, out[1].mDataByteSize >= bytes,
              let pulledLeft = out[0].mData, let pulledRight = out[1].mData else { return kAudioUnitErr_FormatNotSupported }
        if pulledLeft != ownLeft { memcpy(ownLeft, pulledLeft, bytes); out[0].mData = ownLeft }
        if pulledRight != ownRight { memcpy(ownRight, pulledRight, bytes); out[1].mData = ownRight }
        out[0].mDataByteSize = UInt32(bytes); out[1].mDataByteSize = UInt32(bytes)
        let l = ownLeft.assumingMemoryBound(to: Float.self), r = ownRight.assumingMemoryBound(to: Float.self)
        if flags.pointee.contains(.unitRenderAction_OutputIsSilence) {
            // A silent pull may leave stale data; the members expect real zeros.
            l.update(repeating: 0, count: Int(frames)); r.update(repeating: 0, count: Int(frames))
        }
        let start = Int64(timestamp.pointee.mSampleTime)
        OSAtomicIncrement64Barrier(rendering)
        let bits = OSAtomicAdd64Barrier(0, program)
        if bits != 0 {
            let current = Unmanaged<BuiltinChainProgram>.fromOpaque(UnsafeRawPointer(bitPattern: UInt(UInt64(bitPattern: bits)))!)
            current._withUnsafeGuaranteedRef { program in
                for index in 0..<program.members.count {
                    program.pointers[index]._withUnsafeGuaranteedRef {
                        $0.processInPlace(left: l, right: r, frames: frames, sampleTime: start, sidechainLeft: nil, sidechainRight: nil, renderEvents: nil)
                    }
                }
            }
        }
        OSAtomicDecrement64Barrier(rendering)
        flags.pointee.remove(.unitRenderAction_OutputIsSilence)
        return noErr
    }
}

private final class BuiltinEffectsChainUnit: STAURenderUnit {
    let state = BuiltinChainRenderState()
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    override init(componentDescription: AudioComponentDescription, options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let input = try AUAudioUnitBus(format: format), output = try AUAudioUnitBus(format: format)
        input.supportedChannelCounts = [2]; output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        maximumFramesToRender = BuiltinChainRenderState.capacity
        setRenderFunction(Self.render, context: Unmanaged.passUnretained(state).toOpaque())
        // No parameters, scheduled events or observers of its own: each member drains its own queue.
        setRendersDirectly(true)
    }
    private static let render: STAURenderFunction = { context, flags, timestamp, frames, bus, output, _, pull in
        guard bus == 0 else { return kAudioUnitErr_InvalidElement }
        guard let context else { return kAudioUnitErr_Uninitialized }
        guard timestamp.pointee.mSampleTime.isFinite, abs(timestamp.pointee.mSampleTime) < 1e15 else { return kAudio_ParamError }
        return Unmanaged<BuiltinChainRenderState>.fromOpaque(context)._withUnsafeGuaranteedRef {
            $0.render(flags: flags, timestamp: timestamp, frames: frames, output: output, pull: pull)
        }
    }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var canProcessInPlace: Bool { true }
    override var latency: TimeInterval {
        let sr = outputs[0].format.sampleRate
        return sr > 0 ? Double(state.members.reduce(0) { $0 + $1.latencyFrames }) / sr : 0
    }
    override var tailTime: TimeInterval { 0 }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = max(BuiltinChainRenderState.capacity, newValue) }
    }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 && !format.isInterleaved && format.commonFormat == .pcmFormatFloat32
    }
    override func reset() { super.reset(); state.reset() }
}

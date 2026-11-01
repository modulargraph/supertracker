import Accelerate
import AVFoundation
import AudioToolbox
import Darwin
#if canImport(AURenderShim)
import AURenderShim
#endif

/// Original zero-latency track/send processing. Defaults belong to the graph's control
/// thread; scheduling and reset publication are safe during rendering. The bounded
/// queue accepts multiple producers and never allocates or waits for a lock.
final class AudioMixAutomation {
    let node: AVAudioUnitEffect
    private let unit: SuperTrackerMixAutomationUnit
    private static let component = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x73746D78, componentManufacturer: 0x53705472,
        componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(SuperTrackerMixAutomationUnit.self, as: component,
            name: "SuperTracker: Sample-Accurate Mix", version: 1)
    }()

    /// By default, capacity covers four controls every 32 samples through two seconds
    /// of compensation plus a maximum render block. Single-target sends may use 4096
    /// events at 48 kHz. All storage is allocated here, before the graph can render.
    init(sampleRate: Double, eventCapacity: Int? = nil) throws {
        guard sampleRate.isFinite, sampleRate >= 8_000, sampleRate <= 768_000,
              let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) else {
            throw NSError(domain: "SuperTracker.MixAutomation", code: Int(kAudioUnitErr_FormatNotSupported))
        }
        _ = Self.registration
        node = AVAudioUnitEffect(audioComponentDescription: Self.component)
        guard let implementation = node.auAudioUnit as? SuperTrackerMixAutomationUnit else {
            throw NSError(domain: "SuperTracker.MixAutomation", code: Int(kAudioUnitErr_FailedInitialization))
        }
        unit = implementation
        let maximumPending = Int(ceil((ceil(sampleRate * 2) + 16_384) / 32)) * 4
        unit.state.allocateEventStorage(requested: eventCapacity ?? maximumPending)
        try unit.inputBusses[0].setFormat(format)
        try unit.outputBusses[0].setFormat(format)
    }

    /// Only genuinely changed defaults override their corresponding timeline value.
    /// Updating master volume, for example, does not erase automated track volume.
    func setDefaults(trackVolume: Float, instrumentVolume: Float, masterVolume: Float,
                     trackPan: Float, instrumentPan: Float, muted: Bool) {
        unit.state.setDefaults(trackVolume: trackVolume, instrumentVolume: instrumentVolume,
            masterVolume: masterVolume, trackPan: trackPan, instrumentPan: instrumentPan, muted: muted)
    }

    /// Configures a send node: unity track/instrument/master controls, centered pan,
    /// and this separate send gain. Targets 4 and 5 automate that same send gain.
    func setGain(_ gain: Float) { unit.state.setSendDefault(gain) }

    /// Schedule raw (not normalized) values at an absolute engine sample timestamp.
    /// The legacy AUEventSampleTimeImmediate + offset convention is also supported.
    /// Targets: 0 track volume, 1 track pan, 23 instrument volume, 24 instrument pan,
    /// and 4/5 send gain. Events outside these targets or with nonfinite values are ignored.
    func schedule(target: Int, value: Float, sampleTime: Int64) {
        unit.state.schedule(target: target, value: value, sampleTime: sampleTime)
    }

    /// Solo/anchor gate, multiplied into the gain. Unlike the send gain (targets 4/5)
    /// no scheduled event can change it, so one node can be both a send and its gate.
    func setSoloGain(_ gain: Float) { unit.state.setSoloDefault(gain) }

    /// Cancels even future queued events, then restores the latest live defaults on
    /// the next render. Epochs prevent an old producer from reviving cancelled events.
    func reset() { unit.state.requestReset() }

    /// Output peak metering inside the render (no tap, no buffer copy). While enabled,
    /// each render folds the block's output peaks into a pending maximum.
    var meteringEnabled: Bool {
        get { unit.state.meteringEnabled }
        set { unit.state.meteringEnabled = newValue }
    }
    /// Control thread: the output peaks since the previous call (zero when disabled).
    func takePeaks() -> (left: Float, right: Float) { unit.state.takePeaks() }

    var droppedEvents: Int { unit.state.droppedEvents }
    var eventCapacity: Int { unit.state.eventCapacity }
}

private final class SuperTrackerMixAutomationUnit: STAURenderUnit {
    let state = SampleAccurateMixState()
    private var input: AUAudioUnitBus!
    private var output: AUAudioUnitBus!
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        input = try AUAudioUnitBus(format: format); output = try AUAudioUnitBus(format: format)
        input.supportedChannelCounts = [2]; output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        maximumFramesToRender = 16_384
        setRenderFunction(Self.render, context: Unmanaged.passUnretained(state).toOpaque())
        // No parameters, scheduled events or render observers: render without the event schedule.
        setRendersDirectly(true)
    }

    /// The render callback (see STAURenderUnit); `context` is this unit's state.
    private static let render: STAURenderFunction = { context, flags, timestamp, frames, bus, output, _, pull in
        guard bus == 0 else { return kAudioUnitErr_InvalidElement }
        guard let context, let pull else { return kAudioUnitErr_NoConnection }
        return Unmanaged<SampleAccurateMixState>.fromOpaque(context)._withUnsafeGuaranteedRef {
            $0.render(flags: flags, timestamp: timestamp, frames: frames, output: output, pull: pull)
        }
    }

    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var canProcessInPlace: Bool { true }
    override var latency: TimeInterval { 0 }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = max(16_384, newValue) }
    }

    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 &&
            format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }

    override func allocateRenderResources() throws {
        guard input.format == output.format else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(kAudioUnitErr_FormatNotSupported))
        }
        state.allocateScratch(frames: Int(maximumFramesToRender))
        state.requestReset()
        try super.allocateRenderResources()
    }

    override func reset() { state.requestReset(); super.reset() }

}

private struct MixEvent {
    var time: Int64 = 0
    var epoch: Int64 = 0
    var order: Int64 = 0
    var index: Int32 = 0
    var value: Float = 0
}

private final class SampleAccurateMixState {
    private var queueCapacity = 4_096
    private var queue = UnsafeMutablePointer<MixEvent>.allocate(capacity: 4_096)
    private var sequences = UnsafeMutablePointer<Int64>.allocate(capacity: 4_096)
    private let enqueuePosition = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let epoch = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let dropped = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private var dequeuePosition: Int64 = 0
    private var pending = UnsafeMutablePointer<MixEvent>.allocate(capacity: 4_096)
    private var pendingCount = 0
    private var renderEpoch: Int64 = -1

    // Each word packs a Float and its revision. The outer sequence makes a control
    // update coherent; a renderer never spins if the control thread is mid-update.
    // Index 7 is the solo gate: a default only, never a scheduled target.
    private static let controls = 8
    private let defaults = UnsafeMutablePointer<Int64>.allocate(capacity: controls)
    private let defaultSequence = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let candidateDefaults = UnsafeMutablePointer<Int64>.allocate(capacity: controls)
    private let appliedDefaults = UnsafeMutablePointer<Int64>.allocate(capacity: controls)
    private let values = UnsafeMutablePointer<Float>.allocate(capacity: controls)
    // Pending output peaks (Float bit patterns, nonnegative) and the metering switch.
    private let peaks = UnsafeMutablePointer<Int32>.allocate(capacity: 2)
    private let metering = UnsafeMutablePointer<Int32>.allocate(capacity: 1)
    private var scratchLeft = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchRight = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchCapacity = 16_384

    init() {
        queue.initialize(repeating: MixEvent(), count: queueCapacity)
        pending.initialize(repeating: MixEvent(), count: queueCapacity)
        sequences.initialize(repeating: 0, count: queueCapacity)
        for index in 0..<queueCapacity { sequences[index] = Int64(index) }
        enqueuePosition.initialize(to: 0); epoch.initialize(to: 0); dropped.initialize(to: 0)
        defaultSequence.initialize(to: 0)
        defaults.initialize(repeating: 0, count: Self.controls)
        candidateDefaults.initialize(repeating: 0, count: Self.controls)
        appliedDefaults.initialize(repeating: 0, count: Self.controls)
        values.initialize(repeating: 0, count: Self.controls)
        peaks.initialize(repeating: 0, count: 2); metering.initialize(to: 0)
        for index in 0..<Self.controls {
            let value: Float = index < 3 || index >= 6 ? 1 : 0
            defaults[index] = Int64(value.bitPattern)
            appliedDefaults[index] = defaults[index]
            values[index] = value
        }
        scratchLeft.initialize(repeating: 0, count: scratchCapacity)
        scratchRight.initialize(repeating: 0, count: scratchCapacity)
    }

    deinit {
        queue.deinitialize(count: queueCapacity); queue.deallocate()
        pending.deinitialize(count: queueCapacity); pending.deallocate()
        sequences.deinitialize(count: queueCapacity); sequences.deallocate()
        enqueuePosition.deinitialize(count: 1); enqueuePosition.deallocate()
        epoch.deinitialize(count: 1); epoch.deallocate()
        dropped.deinitialize(count: 1); dropped.deallocate()
        defaultSequence.deinitialize(count: 1); defaultSequence.deallocate()
        defaults.deinitialize(count: Self.controls); defaults.deallocate()
        candidateDefaults.deinitialize(count: Self.controls); candidateDefaults.deallocate()
        appliedDefaults.deinitialize(count: Self.controls); appliedDefaults.deallocate()
        values.deinitialize(count: Self.controls); values.deallocate()
        peaks.deinitialize(count: 2); peaks.deallocate()
        metering.deinitialize(count: 1); metering.deallocate()
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
    }

    var droppedEvents: Int { Int(OSAtomicAdd64Barrier(0, dropped)) }
    var eventCapacity: Int { queueCapacity }

    /// Construction only: both event structures use a power-of-two bound. The ceiling
    /// covers the constructor's maximum 768 kHz sample rate without integer overflow.
    func allocateEventStorage(requested: Int) {
        let desired = max(256, min(262_144, requested))
        var capacity = 256
        while capacity < desired { capacity *= 2 }
        guard capacity != queueCapacity else { return }
        queue.deinitialize(count: queueCapacity); queue.deallocate()
        pending.deinitialize(count: queueCapacity); pending.deallocate()
        sequences.deinitialize(count: queueCapacity); sequences.deallocate()
        queueCapacity = capacity
        queue = .allocate(capacity: capacity); pending = .allocate(capacity: capacity)
        sequences = .allocate(capacity: capacity)
        queue.initialize(repeating: MixEvent(), count: capacity)
        pending.initialize(repeating: MixEvent(), count: capacity)
        sequences.initialize(repeating: 0, count: capacity)
        for index in 0..<capacity { sequences[index] = Int64(index) }
        pendingCount = 0; dequeuePosition = 0
        mixAtomicStore(0, enqueuePosition)
    }

    private func normalized(_ value: Float, index: Int) -> Float {
        guard value.isFinite else { return 0 }
        if index == 3 || index == 4 { return max(-1, min(1, value)) }
        if index == 5 { return value > 0 ? 1 : 0 }
        if index == 7 { return max(0, min(1, value)) }
        return max(0, min(index == 6 ? 4 : 2, value))
    }

    private func setDefault(_ value: Float, index: Int) {
        let pointer = defaults.advanced(by: index)
        let old = UInt64(bitPattern: OSAtomicAdd64Barrier(0, pointer))
        let bits = normalized(value, index: index).bitPattern
        guard UInt32(truncatingIfNeeded: old) != bits else { return }
        let revision = UInt32(truncatingIfNeeded: old >> 32) &+ 1
        let replacement = Int64(bitPattern: UInt64(revision) << 32 | UInt64(bits))
        mixAtomicStore(replacement, pointer)
    }

    func setDefaults(trackVolume: Float, instrumentVolume: Float, masterVolume: Float,
                     trackPan: Float, instrumentPan: Float, muted: Bool) {
        OSAtomicIncrement64Barrier(defaultSequence)
        setDefault(trackVolume, index: 0); setDefault(instrumentVolume, index: 1)
        setDefault(masterVolume, index: 2); setDefault(trackPan, index: 3)
        setDefault(instrumentPan, index: 4); setDefault(muted ? 1 : 0, index: 5)
        setDefault(1, index: 6)
        OSAtomicIncrement64Barrier(defaultSequence)
    }

    func setSendDefault(_ gain: Float) {
        OSAtomicIncrement64Barrier(defaultSequence)
        for index in 0..<3 { setDefault(1, index: index) }
        for index in 3..<6 { setDefault(0, index: index) }
        setDefault(gain, index: 6)
        OSAtomicIncrement64Barrier(defaultSequence)
    }

    func setSoloDefault(_ gain: Float) {
        OSAtomicIncrement64Barrier(defaultSequence)
        setDefault(gain, index: 7)
        OSAtomicIncrement64Barrier(defaultSequence)
    }

    func requestReset() { OSAtomicIncrement64Barrier(epoch) }

    var meteringEnabled: Bool {
        get { OSAtomicAdd32Barrier(0, metering) != 0 }
        set {
            var old = OSAtomicAdd32Barrier(0, metering)
            while !OSAtomicCompareAndSwap32Barrier(old, newValue ? 1 : 0, metering) { old = OSAtomicAdd32Barrier(0, metering) }
            if !newValue { _ = takePeaks() }
        }
    }

    func takePeaks() -> (left: Float, right: Float) {
        func exchange(_ pointer: UnsafeMutablePointer<Int32>) -> Float {
            var old = OSAtomicAdd32Barrier(0, pointer)
            while !OSAtomicCompareAndSwap32Barrier(old, 0, pointer) { old = OSAtomicAdd32Barrier(0, pointer) }
            return Float(bitPattern: UInt32(bitPattern: old))
        }
        return (exchange(peaks), exchange(peaks.advanced(by: 1)))
    }

    /// Render thread: the finite peak of one channel's block folded into its pending maximum.
    @inline(__always) private func meter(_ samples: UnsafePointer<Float>, count: Int, channel: Int) {
        var peak: Float = 0
        vDSP_maxmgv(samples, 1, &peak, vDSP_Length(count))
        if !peak.isFinite {
            // Like the tap meters: non-finite samples are skipped, never latched.
            peak = 0
            for frame in 0..<count where samples[frame].isFinite { peak = max(peak, abs(samples[frame])) }
        }
        guard peak > 0 else { return }
        let pointer = peaks.advanced(by: channel), replacement = Int32(bitPattern: peak.bitPattern)
        var previous = OSAtomicAdd32Barrier(0, pointer)
        while replacement > previous {
            if OSAtomicCompareAndSwap32Barrier(previous, replacement, pointer) { return }
            previous = OSAtomicAdd32Barrier(0, pointer)
        }
    }

    func allocateScratch(frames: Int) {
        let count = max(16_384, frames)
        guard count != scratchCapacity else { return }
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
        scratchCapacity = count
        scratchLeft = .allocate(capacity: count); scratchRight = .allocate(capacity: count)
        scratchLeft.initialize(repeating: 0, count: count)
        scratchRight.initialize(repeating: 0, count: count)
    }

    func schedule(target: Int, value: Float, sampleTime: Int64) {
        guard value.isFinite else { return }
        let index: Int
        switch target {
        case 0: index = 0
        case 1: index = 3
        case 23: index = 1
        case 24: index = 4
        case 4, 5: index = 6
        default: return
        }
        let event = MixEvent(time: sampleTime, epoch: OSAtomicAdd64Barrier(0, epoch),
            index: Int32(index), value: normalized(value, index: index))
        var position = OSAtomicAdd64Barrier(0, enqueuePosition)
        // Sequence-tagged bounded MPSC publication. A stalled producer cannot make
        // the renderer spin; it will see the published event on a subsequent call.
        for _ in 0..<32 {
            let slot = Int(position & Int64(queueCapacity - 1))
            let sequence = OSAtomicAdd64Barrier(0, sequences.advanced(by: slot))
            let difference = sequence &- position
            if difference == 0 {
                if OSAtomicCompareAndSwap64Barrier(position, position &+ 1, enqueuePosition) {
                    var published = event
                    published.order = position
                    queue[slot] = published
                    // This producer exclusively owns the slot until publication.
                    // Its sequence cannot change, so publication needs one CAS.
                    _ = OSAtomicCompareAndSwap64Barrier(position, position &+ 1, sequences.advanced(by: slot))
                    return
                }
            } else if difference < 0 { break }
            position = OSAtomicAdd64Barrier(0, enqueuePosition)
        }
        OSAtomicIncrement64Barrier(dropped)
    }

    private func prepareTimeline(blockStart: Int64) {
        let currentEpoch = OSAtomicAdd64Barrier(0, epoch)
        if renderEpoch != currentEpoch {
            renderEpoch = currentEpoch; pendingCount = 0
            for index in 0..<Self.controls { values[index] = Float(bitPattern: UInt32(truncatingIfNeeded: appliedDefaults[index])) }
        }
        let before = OSAtomicAdd64Barrier(0, defaultSequence)
        if before & 1 == 0 {
            for index in 0..<Self.controls { candidateDefaults[index] = OSAtomicAdd64Barrier(0, defaults.advanced(by: index)) }
            if OSAtomicAdd64Barrier(0, defaultSequence) == before {
                for index in 0..<Self.controls where candidateDefaults[index] != appliedDefaults[index] {
                    appliedDefaults[index] = candidateDefaults[index]
                    values[index] = Float(bitPattern: UInt32(truncatingIfNeeded: candidateDefaults[index]))
                }
            }
        }
        for _ in 0..<queueCapacity {
            let slot = Int(dequeuePosition & Int64(queueCapacity - 1))
            guard OSAtomicAdd64Barrier(0, sequences.advanced(by: slot)) == dequeuePosition &+ 1 else { break }
            var event = queue[slot]
            // No producer can reuse this slot before this release publication.
            _ = OSAtomicCompareAndSwap64Barrier(dequeuePosition &+ 1,
                dequeuePosition &+ Int64(queueCapacity), sequences.advanced(by: slot))
            dequeuePosition = dequeuePosition &+ 1
            guard event.epoch == renderEpoch else { continue }
            if event.time >= AUEventSampleTimeImmediate && event.time <= AUEventSampleTimeImmediate + 1_000_000 {
                event.time = blockStart + (event.time - AUEventSampleTimeImmediate)
            }
            guard pendingCount < queueCapacity else { OSAtomicIncrement64Barrier(dropped); continue }
            // A stable min-heap supports out-of-order publication without quadratic
            // work when a large future automation batch arrives in reverse order.
            var position = pendingCount
            while position > 0 {
                let parent = (position - 1) / 2
                if !precedes(event, pending[parent]) { break }
                pending[position] = pending[parent]; position = parent
            }
            pending[position] = event; pendingCount += 1
        }
    }

    private func precedes(_ a: MixEvent, _ b: MixEvent) -> Bool {
        a.time < b.time || (a.time == b.time && a.order < b.order)
    }

    private func applyFirstEvent() {
        let event = pending[0]
        values[Int(event.index)] = event.value
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

    func render(flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, timestamp: UnsafePointer<AudioTimeStamp>,
                frames: AUAudioFrameCount, output: UnsafeMutablePointer<AudioBufferList>,
                pull: UnsafeRawPointer) -> AUAudioUnitStatus {
        let count = Int(frames)
        guard count <= scratchCapacity else { return kAudioUnitErr_TooManyFramesToProcess }
        let time = timestamp.pointee.mSampleTime
        guard time.isFinite, abs(time) < 1e15 else { return kAudio_ParamError }
        let buffers = UnsafeMutableAudioBufferListPointer(output)
        guard buffers.count == 2, buffers[0].mNumberChannels == 1,
              buffers[1].mNumberChannels == 1 else { return kAudioUnitErr_FormatNotSupported }
        let bytes = UInt32(count * MemoryLayout<Float>.size)
        if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(scratchLeft); buffers[0].mDataByteSize = bytes }
        if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(scratchRight); buffers[1].mDataByteSize = bytes }
        guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes else { return kAudio_ParamError }
        let result = st_au_pull(pull, flags, timestamp, frames, 0, output)
        guard result == noErr else { return result }
        guard buffers.count == 2, let left = buffers[0].mData?.assumingMemoryBound(to: Float.self),
              let right = buffers[1].mData?.assumingMemoryBound(to: Float.self),
              buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes else { return kAudio_ParamError }
        let blockStart = Int64(time)
        prepareTimeline(blockStart: blockStart)
        let silent = flags.pointee.contains(.unitRenderAction_OutputIsSilence)
        // A strip whose processor is also its output's solo gate meters before the gate, like a
        // separate gate node would: while soloed out it still shows its level, then outputs silence.
        let gatedMeter = values[7] == 0 && OSAtomicAdd32Barrier(0, metering) != 0
        if gatedMeter { values[7] = 1 }
        // Piecewise-constant controls: events split the block into segments, each one a
        // vector multiply by the same gain and pan factors the per-sample form used, in
        // the same order (sample*gain, then *pan factor). Unity factors are skipped, so
        // a unity strip passes its input through untouched; zero gain writes silence.
        var frame = 0, audible = false
        while frame < count {
            while pendingCount > 0 && pending[0].time <= blockStart + Int64(frame) { applyFirstEvent() }
            var end = count
            if pendingCount > 0 { end = min(count, Int(pending[0].time - blockStart)) }
            let length = end - frame
            let gain = values[5] > 0 ? 0 : values[0] * values[1] * values[2] * values[6] * values[7]
            let pan = max(-1, min(1, values[3] + values[4]))
            if silent || gain == 0 {
                (left + frame).update(repeating: 0, count: length); (right + frame).update(repeating: 0, count: length)
            } else {
                audible = true
                var leftFactor: Float = pan > 0 ? 1 - pan : 1, rightFactor: Float = pan < 0 ? 1 + pan : 1, scale = gain
                let n = vDSP_Length(length)
                if scale != 1 { vDSP_vsmul(left + frame, 1, &scale, left + frame, 1, n); vDSP_vsmul(right + frame, 1, &scale, right + frame, 1, n) }
                if leftFactor != 1 { vDSP_vsmul(left + frame, 1, &leftFactor, left + frame, 1, n) }
                if rightFactor != 1 { vDSP_vsmul(right + frame, 1, &rightFactor, right + frame, 1, n) }
            }
            frame = end
        }
        if audible && OSAtomicAdd32Barrier(0, metering) != 0 {
            meter(left, count: count, channel: 0); meter(right, count: count, channel: 1)
        }
        if gatedMeter {
            values[7] = 0
            if audible { left.update(repeating: 0, count: count); right.update(repeating: 0, count: count); audible = false }
        }
        // A block whose every segment was silenced is reported as silence downstream.
        if audible { flags.pointee.remove(.unitRenderAction_OutputIsSilence) }
        else { flags.pointee.insert(.unitRenderAction_OutputIsSilence) }
        buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
        return noErr
    }
}

private func mixAtomicStore(_ value: Int64, _ pointer: UnsafeMutablePointer<Int64>) {
    var old = OSAtomicAdd64Barrier(0, pointer)
    while !OSAtomicCompareAndSwap64Barrier(old, value, pointer) { old = OSAtomicAdd64Barrier(0, pointer) }
}

import AVFoundation
import AudioToolbox
import Darwin
#if canImport(AURenderShim)
import AURenderShim
#endif

/// A host-owned delay for aligning parallel Audio Unit paths. Construct and connect on
/// the graph's control thread. `frames` and `reset()` may also be called while rendering.
/// The node deliberately reports zero latency: the rack accounts for this delay itself.
final class AudioDelayCompensator {
    let node: AVAudioUnitEffect
    let maximumFrames: Int
    private let unit: SuperTrackerPluginDelay

    private static let description = AudioComponentDescription(
        componentType: kAudioUnitType_Effect, componentSubType: 0x73747064,
        componentManufacturer: 0x53705472, componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(SuperTrackerPluginDelay.self, as: description,
            name: "SuperTracker: Plugin Delay Compensation", version: 1)
    }()

    init(sampleRate: Double, maximumFrames: Int? = nil) throws {
        guard sampleRate.isFinite, sampleRate >= 8_000, sampleRate <= 768_000,
              let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) else {
            throw NSError(domain: "SuperTracker.DelayCompensation", code: Int(kAudioUnitErr_FormatNotSupported),
                userInfo: [NSLocalizedDescriptionKey: "Delay compensation requires a supported stereo sample rate."])
        }
        _ = Self.registration
        // AVAudioUnitEffect supports synchronous creation of registered, in-process
        // AUAudioUnit subclasses. No semaphore or run-loop pumping is needed here.
        node = AVAudioUnitEffect(audioComponentDescription: Self.description)
        guard let implementation = node.auAudioUnit as? SuperTrackerPluginDelay else {
            throw NSError(domain: "SuperTracker.DelayCompensation", code: Int(kAudioUnitErr_FailedInitialization),
                userInfo: [NSLocalizedDescriptionKey: "Could not instantiate the delay compensation Audio Unit."])
        }
        unit = implementation
        self.maximumFrames = max(0, min(1_536_000, maximumFrames ?? Int(ceil(sampleRate * 2))))
        try unit.configure(format: format, maximumFrames: self.maximumFrames)
    }

    /// The requested delay, clamped to the preallocated capacity. A change during
    /// playback crossfades between old and new read positions over 256 samples.
    var frames: Int {
        get { unit.delayFrames }
        set { unit.delayFrames = newValue }
    }

    /// Discards buffered audio on the next render call, without racing the audio thread.
    /// The requested delay takes effect immediately after that reset, without a fade.
    func reset() { unit.reset() }
}

/// The AU layer owns formats and buffer allocation; the DSP state alone is captured by
/// the render block. There are no Swift arrays, locks, or allocations in that block.
private final class SuperTrackerPluginDelay: STAURenderUnit {
    private var input: AUAudioUnitBus!
    private var output: AUAudioUnitBus!
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private let dsp = StereoCompensationDelay()

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        input = try AUAudioUnitBus(format: format)
        output = try AUAudioUnitBus(format: format)
        input.supportedChannelCounts = [2]
        output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        maximumFramesToRender = 16_384
        setRenderFunction(Self.render, context: Unmanaged.passUnretained(dsp).toOpaque())
        // No parameters, scheduled events or render observers: render without the event schedule.
        setRendersDirectly(true)
    }

    /// The render callback (see STAURenderUnit); `context` is this unit's delay state.
    private static let render: STAURenderFunction = { context, flags, timestamp, frameCount, bus, buffers, _, pull in
        guard bus == 0 else { return kAudioUnitErr_InvalidElement }
        guard let context, let pull else { return kAudioUnitErr_NoConnection }
        return Unmanaged<StereoCompensationDelay>.fromOpaque(context)._withUnsafeGuaranteedRef {
            $0.render(flags: flags, timestamp: timestamp, frameCount: frameCount, output: buffers, pullInput: pull)
        }
    }

    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var canProcessInPlace: Bool { true }
    override var latency: TimeInterval { 0 }
    // An upstream pull is not necessarily the offline engine's requested quantum:
    // Apple's delay can request 4,097 input frames for a 4,096-frame output block.
    // Retain headroom even if AVAudioEngine propagates its smaller output quantum.
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = max(16_384, newValue) }
    }

    var delayFrames: Int {
        get { dsp.requestedFrames }
        set { dsp.requestedFrames = newValue }
    }

    func configure(format: AVAudioFormat, maximumFrames: Int) throws {
        guard !renderResourcesAllocated else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(kAudioUnitErr_CannotDoInCurrentContext))
        }
        try input.setFormat(format)
        try output.setFormat(format)
        dsp.allocateDelay(maximumFrames: maximumFrames)
    }

    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 &&
            format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved &&
            format.sampleRate.isFinite && format.sampleRate > 0
    }

    override func allocateRenderResources() throws {
        guard input.format == output.format, input.format.channelCount == 2,
              input.format.commonFormat == .pcmFormatFloat32, !input.format.isInterleaved else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(kAudioUnitErr_FormatNotSupported))
        }
        dsp.allocateScratch(frames: max(16_384, Int(maximumFramesToRender)))
        dsp.requestReset()
        try super.allocateRenderResources()
    }

    override func reset() {
        dsp.requestReset()
        super.reset()
    }

}

private final class StereoCompensationDelay {
    private let requested = UnsafeMutablePointer<Int32>.allocate(capacity: 1)
    private let resetRevision = UnsafeMutablePointer<Int32>.allocate(capacity: 1)
    private var left = UnsafeMutablePointer<Float>.allocate(capacity: 1)
    private var right = UnsafeMutablePointer<Float>.allocate(capacity: 1)
    private var scratchLeft = UnsafeMutablePointer<Float>.allocate(capacity: 1)
    private var scratchRight = UnsafeMutablePointer<Float>.allocate(capacity: 1)
    private var capacity = 1
    private var maximumDelay = 0
    private var scratchCapacity = 1
    private var cursor = 0
    private var currentDelay = 0
    private var targetDelay = 0
    private var fadePosition = 256
    private var lastResetRevision: Int32 = 0
    private var hasRendered = false
    private let fadeLength = 256
    private static let blockHeadroom = 4_096

    init() {
        requested.initialize(to: 0)
        resetRevision.initialize(to: 0)
        left.initialize(to: 0); right.initialize(to: 0)
        scratchLeft.initialize(to: 0); scratchRight.initialize(to: 0)
    }

    deinit {
        requested.deinitialize(count: 1); requested.deallocate()
        resetRevision.deinitialize(count: 1); resetRevision.deallocate()
        left.deinitialize(count: capacity); left.deallocate()
        right.deinitialize(count: capacity); right.deallocate()
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
    }

    var requestedFrames: Int {
        get { Int(OSAtomicAdd32Barrier(0, requested)) }
        set {
            let value = Int32(max(0, min(maximumDelay, newValue)))
            var old = OSAtomicAdd32Barrier(0, requested)
            while !OSAtomicCompareAndSwap32Barrier(old, value, requested) {
                old = OSAtomicAdd32Barrier(0, requested)
            }
        }
    }

    // Called only before rendering is allocated, never from the render callback.
    func allocateDelay(maximumFrames: Int) {
        left.deinitialize(count: capacity); left.deallocate()
        right.deinitialize(count: capacity); right.deallocate()
        // Block headroom: the steady block-copy path below needs `delay + block <= capacity`
        // (an exactly sized ring forced every compensated edge onto the per-sample loop).
        maximumDelay = maximumFrames
        capacity = maximumFrames + 1 + Self.blockHeadroom
        left = .allocate(capacity: capacity); right = .allocate(capacity: capacity)
        left.initialize(repeating: 0, count: capacity)
        right.initialize(repeating: 0, count: capacity)
        requestedFrames = requestedFrames
        requestReset()
    }

    func allocateScratch(frames: Int) {
        let count = max(1, frames)
        guard count != scratchCapacity else { return }
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
        scratchCapacity = count
        scratchLeft = .allocate(capacity: count); scratchRight = .allocate(capacity: count)
        scratchLeft.initialize(repeating: 0, count: count)
        scratchRight.initialize(repeating: 0, count: count)
    }

    func requestReset() { OSAtomicIncrement32Barrier(resetRevision) }

    func render(flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>,
                timestamp: UnsafePointer<AudioTimeStamp>, frameCount: AUAudioFrameCount,
                output: UnsafeMutablePointer<AudioBufferList>,
                pullInput: UnsafeRawPointer) -> AUAudioUnitStatus {
        let count = Int(frameCount)
        guard count <= scratchCapacity else { return kAudioUnitErr_TooManyFramesToProcess }
        let buffers = UnsafeMutableAudioBufferListPointer(output)
        guard buffers.count == 2, buffers[0].mNumberChannels == 1,
              buffers[1].mNumberChannels == 1 else { return kAudioUnitErr_FormatNotSupported }
        let bytes = UInt32(count * MemoryLayout<Float>.size)
        if buffers[0].mData == nil {
            buffers[0].mData = UnsafeMutableRawPointer(scratchLeft)
            buffers[0].mDataByteSize = bytes
        }
        if buffers[1].mData == nil {
            buffers[1].mData = UnsafeMutableRawPointer(scratchRight)
            buffers[1].mDataByteSize = bytes
        }
        guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes else {
            return kAudio_ParamError
        }
        let status = st_au_pull(pullInput, flags, timestamp, frameCount, 0, output)
        guard status == noErr else { return status }
        guard let outputLeft = buffers[0].mData?.assumingMemoryBound(to: Float.self),
              let outputRight = buffers[1].mData?.assumingMemoryBound(to: Float.self),
              buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes else {
            return kAudio_ParamError
        }

        let revision = OSAtomicAdd32Barrier(0, resetRevision)
        let desired = requestedFrames
        if !hasRendered || revision != lastResetRevision {
            // Clearing belongs to the audio thread, so even a concurrent transport
            // reset cannot race ring-buffer reads. Both loops have a fixed upper bound.
            memset(left, 0, capacity * MemoryLayout<Float>.size)
            memset(right, 0, capacity * MemoryLayout<Float>.size)
            cursor = 0; currentDelay = desired; targetDelay = desired
            fadePosition = fadeLength; lastResetRevision = revision; hasRendered = true
        }
        let inputIsSilent = flags.pointee.contains(.unitRenderAction_OutputIsSilence)
        if desired == 0 && currentDelay == 0 && targetDelay == 0 && fadePosition == fadeLength {
            // Zero delay (the usual case: no plug-in latency to compensate) is a pass-
            // through. The ring still records the block, exactly as the per-sample path
            // would, so a later delay change crossfades from the right history. With no
            // delay there is no tail, so a silent input stays flagged silent.
            var written = 0
            while written < count {
                let chunk = min(count - written, capacity - cursor)
                if inputIsSilent {
                    (left + cursor).update(repeating: 0, count: chunk); (right + cursor).update(repeating: 0, count: chunk)
                } else {
                    (left + cursor).update(from: outputLeft + written, count: chunk)
                    (right + cursor).update(from: outputRight + written, count: chunk)
                }
                written += chunk; cursor += chunk
                if cursor == capacity { cursor = 0 }
            }
            if inputIsSilent { outputLeft.update(repeating: 0, count: count); outputRight.update(repeating: 0, count: count) }
            buffers[0].mDataByteSize = bytes
            buffers[1].mDataByteSize = bytes
            return noErr
        }
        // Silent upstream audio can still leave an audible tail in our delay line.
        flags.pointee.remove(.unitRenderAction_OutputIsSilence)
        if fadePosition == fadeLength && targetDelay == desired && currentDelay == desired && desired + count <= capacity {
            // Steady compensation (a constant plug-in latency elsewhere in the mix): the
            // per-sample loop below reduces to writing the block into the ring and reading
            // it back `desired` frames earlier. Block copies give identical samples; every
            // read position precedes the writes it could otherwise overlap.
            let start = cursor
            var written = 0
            while written < count {
                let chunk = min(count - written, capacity - cursor)
                if inputIsSilent {
                    (left + cursor).update(repeating: 0, count: chunk); (right + cursor).update(repeating: 0, count: chunk)
                } else {
                    (left + cursor).update(from: outputLeft + written, count: chunk)
                    (right + cursor).update(from: outputRight + written, count: chunk)
                }
                written += chunk; cursor += chunk
                if cursor == capacity { cursor = 0 }
            }
            var read = start - desired
            if read < 0 { read += capacity }
            var copied = 0
            while copied < count {
                let chunk = min(count - copied, capacity - read)
                (outputLeft + copied).update(from: left + read, count: chunk)
                (outputRight + copied).update(from: right + read, count: chunk)
                copied += chunk; read += chunk
                if read == capacity { read = 0 }
            }
            buffers[0].mDataByteSize = bytes
            buffers[1].mDataByteSize = bytes
            return noErr
        }
        for frame in 0..<count {
            left[cursor] = inputIsSilent ? 0 : outputLeft[frame]
            right[cursor] = inputIsSilent ? 0 : outputRight[frame]
            if fadePosition == fadeLength && targetDelay != desired {
                currentDelay = targetDelay; targetDelay = desired; fadePosition = 0
            }
            var oldRead = cursor - currentDelay
            if oldRead < 0 { oldRead += capacity }
            if fadePosition < fadeLength {
                var newRead = cursor - targetDelay
                if newRead < 0 { newRead += capacity }
                let blend = Float(fadePosition + 1) / Float(fadeLength)
                outputLeft[frame] = left[oldRead] + (left[newRead] - left[oldRead]) * blend
                outputRight[frame] = right[oldRead] + (right[newRead] - right[oldRead]) * blend
                fadePosition += 1
                if fadePosition == fadeLength { currentDelay = targetDelay }
            } else {
                outputLeft[frame] = left[oldRead]
                outputRight[frame] = right[oldRead]
            }
            cursor += 1
            if cursor == capacity { cursor = 0 }
        }
        buffers[0].mDataByteSize = bytes
        buffers[1].mDataByteSize = bytes
        return noErr
    }
}

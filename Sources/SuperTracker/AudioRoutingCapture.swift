import AVFoundation
import AudioToolbox
import Darwin
#if canImport(AURenderShim)
import AURenderShim
#endif

/// An original, transparent AU for simultaneous offline output/stem capture.
/// Configure and drain only while the graph is quiescent (after renderOffline
/// returns). AVAudioEngine serializes this node's render calls. This deliberately
/// has no tap, worker, independent render pull, or cross-thread consumer.
final class AudioRoutingCapture {
    enum CaptureError: LocalizedError, Equatable {
        case invalidConfiguration(String)
        case invalidTimestamp
        case gap(expected: Int64, received: Int64)
        case overflow(capacity: Int, pending: Int, incoming: Int)
        case upstreamRender(Int32)
        case invalidBuffers
        var errorDescription: String? {
            switch self {
            case .invalidConfiguration(let text): return text
            case .invalidTimestamp: return "Stem capture received an invalid sample timestamp."
            case .gap(let expected, let received): return "Stem capture missed frames: expected \(expected), received \(received)."
            case .overflow(let capacity, let pending, let incoming):
                return "Stem capture overflow: \(pending) pending and \(incoming) incoming frames exceed its \(capacity)-frame ring."
            case .upstreamRender(let status): return "Stem capture's upstream processor failed with status \(status)."
            case .invalidBuffers: return "Stem capture received an unsupported audio buffer layout."
            }
        }
    }
    struct Chunk: Equatable {
        /// Logical timeline index, after subtracting this stage's arrival latency.
        let startFrame: Int64
        let frameCount: AVAudioFrameCount
    }

    let node: AVAudioUnitEffect
    let capacityFrames: Int
    private let unit: RoutingCaptureAudioUnit
    private let format: AVAudioFormat
    private static let description = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x73747263, componentManufacturer: 0x53705472, componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(RoutingCaptureAudioUnit.self, as: description,
            name: "SuperTracker: Routing Capture", version: 1)
    }()

    init(sampleRate: Double, capacityFrames: Int = 131_072) throws {
        guard sampleRate.isFinite, (8_000...768_000).contains(sampleRate), (1...4_194_304).contains(capacityFrames),
              let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) else {
            throw CaptureError.invalidConfiguration("Capture requires a supported stereo sample rate and 1–4194304 ring frames.")
        }
        _ = Self.registration
        node = AVAudioUnitEffect(audioComponentDescription: Self.description)
        guard let implementation = node.auAudioUnit as? RoutingCaptureAudioUnit else {
            throw CaptureError.invalidConfiguration("Could not instantiate the routing capture Audio Unit.")
        }
        self.format = format; self.capacityFrames = capacityFrames; unit = implementation
        try unit.configure(format: format, capacityFrames: capacityFrames)
    }

    /// Starts a new capture interval [startFrame, startFrame + frameLimit).
    /// A nil limit captures until explicitly disabled. Pre-roll and already
    /// captured prefixes are ignored. Reconfiguration clears unread data/errors.
    /// Call only while this node cannot render; resources may remain allocated.
    func configureCapture(enabled: Bool, arrivalFrames: Int64, startFrame: Int64 = 0,
                          frameLimit: Int64? = nil) throws {
        guard (0...9_000_000_000_000_000).contains(arrivalFrames), (0...9_000_000_000_000_000).contains(startFrame),
              frameLimit == nil || frameLimit! >= 0 else {
            throw CaptureError.invalidConfiguration("Capture arrival/start/limit must be nonnegative supported frame counts.")
        }
        let limit = frameLimit ?? (Int64.max - startFrame)
        let (end, overflow) = startFrame.addingReportingOverflow(limit)
        guard !overflow else { throw CaptureError.invalidConfiguration("Capture frame interval overflows its timeline.") }
        unit.dsp.begin(enabled: enabled, arrivalFrames: arrivalFrames, startFrame: startFrame, endFrame: end)
    }

    /// Explicit synchronous offline authority for wrappers which omit timestamp
    /// validity flags. Supply AVAudioEngine.manualRenderingSampleTime immediately
    /// before renderOffline, and always end the window when that call returns.
    /// Windows must progress contiguously, or repeat exactly for a render retry.
    func beginRenderQuantum(sampleTime: Int64, frameCount: Int) throws {
        guard (0...9_000_000_000_000_000).contains(sampleTime), (1...65_536).contains(frameCount) else {
            throw CaptureError.invalidConfiguration("Capture render windows require a nonnegative master sample time and 1–65536 frames.")
        }
        try unit.dsp.beginQuantum(sampleTime: sampleTime, frameCount: frameCount)
    }

    func endRenderQuantum() { unit.dsp.endQuantum() }

    /// Read after each master renderOffline call until availableFrames reaches 0.
    /// This never renders the graph. The caller retains and reuses the PCM buffer.
    @discardableResult func drain(into buffer: AVAudioPCMBuffer) throws -> Chunk {
        if let error { throw error }
        guard buffer.format == format, buffer.frameCapacity > 0, let channels = buffer.floatChannelData else {
            throw CaptureError.invalidConfiguration("Capture drain requires a nonempty buffer in the capture node's stereo Float32 format.")
        }
        let start = unit.dsp.nextReadFrame
        let count = unit.dsp.drain(left: channels[0], right: channels[1], maximum: Int(buffer.frameCapacity))
        buffer.frameLength = AVAudioFrameCount(count)
        return Chunk(startFrame: start, frameCount: AVAudioFrameCount(count))
    }

    /// All status access, like drain(), is restricted to quiescent graph boundaries.
    var availableFrames: Int { unit.dsp.bufferedFrames }
    var capturedFrames: Int64 { unit.dsp.nextWriteFrame - unit.dsp.startFrame }
    var drainedFrames: Int64 { unit.dsp.nextReadFrame - unit.dsp.startFrame }
    var unflaggedFallbackCount: UInt64 { unit.dsp.unflaggedFallbackCount }
    var error: CaptureError? {
        switch unit.dsp.failure {
        case 0: return nil
        case 1: return .invalidTimestamp
        case 2: return .gap(expected: unit.dsp.failureExpected, received: unit.dsp.failureReceived)
        case 3: return .overflow(capacity: capacityFrames, pending: unit.dsp.failurePending, incoming: unit.dsp.failureIncoming)
        case 4: return .upstreamRender(unit.dsp.failureStatus)
        default: return .invalidBuffers
        }
    }

    /// Captured scalar evidence from the first invalid timestamp. Dictionary and
    /// text construction happen only when the quiescent control thread reads it.
    /// Nonfinite sample times remain JSON-safe; the raw bit pattern is preserved.
    var diagnostics: [String: Any] {
        let sampleTime = unit.dsp.failureSampleTime
        let sampleTimeValue: Any = sampleTime.isFinite ? sampleTime as Any : String(describing: sampleTime)
        let after = unit.dsp.failureSampleTimeAfterPull
        let afterValue: Any = after.isFinite ? after as Any : String(describing: after)
        return ["error": error?.localizedDescription ?? "", "failureCode": unit.dsp.failure,
                "sampleTime": sampleTimeValue, "sampleTimeBits": String(sampleTime.bitPattern, radix: 16),
                "timeFlags": unit.dsp.failureTimeFlags, "frameCount": unit.dsp.failureFrameCount,
                "sampleTimeAfterPull": afterValue, "sampleTimeBitsAfterPull": String(after.bitPattern, radix: 16),
                "timeFlagsAfterPull": unit.dsp.failureTimeFlagsAfterPull,
                "failureWindowActive": unit.dsp.failureWindowActive,
                "failureWindowSampleTime": unit.dsp.failureWindowSampleTime,
                "failureWindowFrameCount": unit.dsp.failureWindowFrameCount,
                "renderWindowActive": unit.dsp.windowActive, "renderWindowSampleTime": unit.dsp.windowSampleTime,
                "renderWindowFrameCount": unit.dsp.windowFrameCount, "unflaggedFallbackCount": unflaggedFallbackCount,
                "sampleRate": format.sampleRate, "capturedFrames": capturedFrames,
                "drainedFrames": drainedFrames, "availableFrames": availableFrames]
    }
}

private final class RoutingCaptureAudioUnit: STAURenderUnit {
    let dsp = RoutingCaptureDSP()
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
        maximumFramesToRender = 65_536
        setRenderFunction(Self.render, context: Unmanaged.passUnretained(dsp).toOpaque())
        // No parameters, scheduled events or render observers: render without the event schedule.
        setRendersDirectly(true)
    }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var canProcessInPlace: Bool { true }
    override var latency: TimeInterval { 0 }
    override var tailTime: TimeInterval { 0 }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { 65_536 }
        set { super.maximumFramesToRender = 65_536 }
    }
    func configure(format: AVAudioFormat, capacityFrames: Int) throws {
        guard !renderResourcesAllocated else { throw AudioRoutingCapture.CaptureError.invalidConfiguration("Capture storage is already allocated.") }
        try input.setFormat(format); try output.setFormat(format)
        dsp.allocate(capacityFrames: capacityFrames)
    }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 && format.commonFormat == .pcmFormatFloat32 &&
            !format.isInterleaved && format.sampleRate.isFinite && (8_000...768_000).contains(format.sampleRate)
    }
    override func allocateRenderResources() throws {
        guard input.format == output.format, input.format.channelCount == 2,
              input.format.commonFormat == .pcmFormatFloat32, !input.format.isInterleaved else {
            throw AudioRoutingCapture.CaptureError.invalidBuffers
        }
        try super.allocateRenderResources()
    }
    // Capture epochs are explicit. Engine reset/prepare must not unexpectedly
    // discard already captured frames or change the configured interval.
    private static let render: STAURenderFunction = { context, flags, timestamp, frames, bus, output, _, pull in
        guard bus == 0 else { return kAudioUnitErr_InvalidElement }
        guard let context, let pull else { return kAudioUnitErr_NoConnection }
        return Unmanaged<RoutingCaptureDSP>.fromOpaque(context)._withUnsafeGuaranteedRef {
            $0.render(flags: flags, timestamp: timestamp, frames: frames, output: output, pull: pull)
        }
    }
}

/// One render writer and one consumer used only between completed graph renders.
/// All failure records are integers; Error/string construction occurs outside DSP.
private final class RoutingCaptureDSP {
    private var left = UnsafeMutablePointer<Float>.allocate(capacity: 1)
    private var right = UnsafeMutablePointer<Float>.allocate(capacity: 1)
    private let scratchLeft = UnsafeMutablePointer<Float>.allocate(capacity: 65_536)
    private let scratchRight = UnsafeMutablePointer<Float>.allocate(capacity: 65_536)
    private var capacity = 1
    private var writeIndex = 0
    private var readIndex = 0
    private var enabled = false
    private var arrivalFrames: Int64 = 0
    private var endFrame: Int64 = Int64.max
    private(set) var startFrame: Int64 = 0
    private(set) var nextWriteFrame: Int64 = 0
    private(set) var nextReadFrame: Int64 = 0
    private(set) var bufferedFrames = 0
    private(set) var failure = 0
    private(set) var failureExpected: Int64 = 0
    private(set) var failureReceived: Int64 = 0
    private(set) var failurePending = 0
    private(set) var failureIncoming = 0
    private(set) var failureStatus: Int32 = 0
    private(set) var failureSampleTime: Double = 0
    private(set) var failureTimeFlags: UInt32 = 0
    private(set) var failureFrameCount: UInt32 = 0
    private(set) var failureSampleTimeAfterPull: Double = 0
    private(set) var failureTimeFlagsAfterPull: UInt32 = 0
    private(set) var failureWindowActive = false
    private(set) var failureWindowSampleTime: Int64 = 0
    private(set) var failureWindowFrameCount = 0
    private(set) var windowActive = false
    private(set) var windowSampleTime: Int64 = 0
    private(set) var windowFrameCount = 0
    private var hasPreviousWindow = false
    private(set) var unflaggedFallbackCount: UInt64 = 0

    init() {
        left.initialize(to: 0); right.initialize(to: 0)
        scratchLeft.initialize(repeating: 0, count: 65_536); scratchRight.initialize(repeating: 0, count: 65_536)
    }
    deinit {
        left.deinitialize(count: capacity); left.deallocate(); right.deinitialize(count: capacity); right.deallocate()
        scratchLeft.deinitialize(count: 65_536); scratchLeft.deallocate()
        scratchRight.deinitialize(count: 65_536); scratchRight.deallocate()
    }
    func allocate(capacityFrames: Int) {
        left.deinitialize(count: capacity); left.deallocate(); right.deinitialize(count: capacity); right.deallocate()
        capacity = capacityFrames
        left = .allocate(capacity: capacity); right = .allocate(capacity: capacity)
        left.initialize(repeating: 0, count: capacity); right.initialize(repeating: 0, count: capacity)
    }
    func begin(enabled: Bool, arrivalFrames: Int64, startFrame: Int64, endFrame: Int64) {
        self.enabled = enabled; self.arrivalFrames = arrivalFrames; self.startFrame = startFrame; self.endFrame = endFrame
        nextWriteFrame = startFrame; nextReadFrame = startFrame; bufferedFrames = 0; writeIndex = 0; readIndex = 0
        failure = 0; failureExpected = 0; failureReceived = 0; failurePending = 0; failureIncoming = 0; failureStatus = 0
        failureSampleTime = 0; failureTimeFlags = 0; failureFrameCount = 0
        failureSampleTimeAfterPull = 0; failureTimeFlagsAfterPull = 0
        failureWindowActive = false; failureWindowSampleTime = 0; failureWindowFrameCount = 0
        windowActive = false; windowSampleTime = 0; windowFrameCount = 0; hasPreviousWindow = false; unflaggedFallbackCount = 0
    }
    func beginQuantum(sampleTime: Int64, frameCount: Int) throws {
        guard !windowActive else { throw AudioRoutingCapture.CaptureError.invalidConfiguration("End the prior capture render window before beginning another.") }
        if hasPreviousWindow {
            let retry = sampleTime == windowSampleTime && frameCount == windowFrameCount
            let progress = sampleTime == windowSampleTime + Int64(windowFrameCount)
            guard retry || progress else {
                throw AudioRoutingCapture.CaptureError.invalidConfiguration("Capture master render windows must progress contiguously or repeat exactly for a retry.")
            }
        }
        windowSampleTime = sampleTime; windowFrameCount = frameCount; windowActive = true; hasPreviousWindow = true
    }
    func endQuantum() { windowActive = false }
    func drain(left destinationLeft: UnsafeMutablePointer<Float>, right destinationRight: UnsafeMutablePointer<Float>, maximum: Int) -> Int {
        let count = min(maximum, bufferedFrames), first = min(count, capacity - readIndex)
        if first > 0 {
            memcpy(destinationLeft, left.advanced(by: readIndex), first * MemoryLayout<Float>.size)
            memcpy(destinationRight, right.advanced(by: readIndex), first * MemoryLayout<Float>.size)
        }
        if count > first {
            memcpy(destinationLeft.advanced(by: first), left, (count - first) * MemoryLayout<Float>.size)
            memcpy(destinationRight.advanced(by: first), right, (count - first) * MemoryLayout<Float>.size)
        }
        readIndex = (readIndex + count) % capacity; bufferedFrames -= count; nextReadFrame += Int64(count)
        return count
    }
    private func fail(_ code: Int, status: Int32 = 0) {
        if enabled, failure == 0 { failure = code; failureStatus = status }
    }
    func render(flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, timestamp: UnsafePointer<AudioTimeStamp>,
                frames: AUAudioFrameCount, output: UnsafeMutablePointer<AudioBufferList>, pull: UnsafeRawPointer) -> AUAudioUnitStatus {
        // An upstream pull may reuse the timestamp's backing storage. This render
        // request belongs to its incoming time, captured before invoking upstream.
        let incomingSampleTime = timestamp.pointee.mSampleTime
        let incomingTimeFlags = timestamp.pointee.mFlags
        guard frames <= 65_536 else { fail(5); return kAudioUnitErr_TooManyFramesToProcess }
        let buffers = UnsafeMutableAudioBufferListPointer(output), bytes = frames * UInt32(MemoryLayout<Float>.size)
        guard buffers.count == 2, buffers[0].mNumberChannels == 1, buffers[1].mNumberChannels == 1 else {
            fail(5); return kAudioUnitErr_FormatNotSupported
        }
        if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(scratchLeft); buffers[0].mDataByteSize = bytes }
        if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(scratchRight); buffers[1].mDataByteSize = bytes }
        guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes else { fail(5); return kAudio_ParamError }
        let result = st_au_pull(pull, flags, timestamp, frames, 0, output)
        guard result == noErr else { fail(4, status: result); return result }
        guard buffers.count == 2, buffers[0].mNumberChannels == 1, buffers[1].mNumberChannels == 1,
              buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes,
              let sourceLeft = buffers[0].mData?.assumingMemoryBound(to: Float.self),
              let sourceRight = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { fail(5); return kAudio_ParamError }
        guard enabled, failure == 0, frames > 0, nextWriteFrame < endFrame else { return noErr }
        let sampleTime = incomingSampleTime
        let numericTimeIsValid = sampleTime.isFinite && abs(sampleTime) <= 9_000_000_000_000_000 && sampleTime.rounded(.towardZero) == sampleTime
        let flaggedTimeIsValid = incomingTimeFlags.contains(.sampleTimeValid) && numericTimeIsValid
        // Never infer a clock from an unflagged callback. The caller's independent
        // synchronous master window supplies it, and only a matching whole-window
        // pull (possibly with bounded lookahead) is eligible. Split/offset requests
        // deliberately fail instead of inventing their position.
        let useMasterWindow = !incomingTimeFlags.contains(.sampleTimeValid) && numericTimeIsValid && windowActive &&
            sampleTime == Double(windowSampleTime) && Int(frames) >= windowFrameCount
        guard flaggedTimeIsValid || useMasterWindow else {
            failureSampleTime = sampleTime; failureTimeFlags = incomingTimeFlags.rawValue; failureFrameCount = frames
            failureSampleTimeAfterPull = timestamp.pointee.mSampleTime
            failureTimeFlagsAfterPull = timestamp.pointee.mFlags.rawValue
            failureWindowActive = windowActive; failureWindowSampleTime = windowSampleTime; failureWindowFrameCount = windowFrameCount
            fail(1); return noErr
        }
        if useMasterWindow { unflaggedFallbackCount &+= 1 }
        let renderFrame = useMasterWindow ? windowSampleTime : Int64(sampleTime)
        let logicalStart = renderFrame - arrivalFrames
        let logicalEnd = logicalStart + Int64(frames)
        // Covers pre-roll and complete repeated pulls, including repeats after drain.
        guard logicalEnd > nextWriteFrame else { return noErr }
        guard logicalStart <= nextWriteFrame else {
            failureExpected = nextWriteFrame; failureReceived = logicalStart; fail(2); return noErr
        }
        let count = Int(min(logicalEnd, endFrame) - nextWriteFrame)
        guard count <= capacity - bufferedFrames else {
            failurePending = bufferedFrames; failureIncoming = count; fail(3); return noErr
        }
        let offset = Int(nextWriteFrame - logicalStart), first = min(count, capacity - writeIndex)
        let silent = flags.pointee.contains(.unitRenderAction_OutputIsSilence)
        if silent {
            memset(left.advanced(by: writeIndex), 0, first * MemoryLayout<Float>.size)
            memset(right.advanced(by: writeIndex), 0, first * MemoryLayout<Float>.size)
            if count > first { memset(left, 0, (count - first) * MemoryLayout<Float>.size); memset(right, 0, (count - first) * MemoryLayout<Float>.size) }
        } else {
            memcpy(left.advanced(by: writeIndex), sourceLeft.advanced(by: offset), first * MemoryLayout<Float>.size)
            memcpy(right.advanced(by: writeIndex), sourceRight.advanced(by: offset), first * MemoryLayout<Float>.size)
            if count > first {
                memcpy(left, sourceLeft.advanced(by: offset + first), (count - first) * MemoryLayout<Float>.size)
                memcpy(right, sourceRight.advanced(by: offset + first), (count - first) * MemoryLayout<Float>.size)
            }
        }
        writeIndex = (writeIndex + count) % capacity; bufferedFrames += count; nextWriteFrame += Int64(count)
        return noErr
    }
}

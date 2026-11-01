import Foundation
import Darwin

/// Captures recording boundaries on a single authoritative source-render clock.
/// Construct and retain on control, then call observeQuantum from exactly one
/// serialized source/master observer. It is not a collector for plug-in lookahead
/// pulls. requestStop and trySnapshot may run concurrently on other threads.
///
/// Storage is allocated once. The render path uses only scalar state and bounded
/// atomic operations: no allocation, locks, waiting, dispatch, or callbacks. The
/// owner must quiesce every reader/writer before releasing this object. Device
/// failure/timeouts are caller-level aborts, not fabricated source boundaries.
public final class AudioRecordingBoundary {
    public enum ErrorCode: Int64, Error, LocalizedError, Sendable {
        case none = 0
        case invalidSampleRate, sampleRateChanged, epochChanged
        case invalidTimestamp, invalidFrameCount, frameOverflow
        case gap, overlap, regression, renderFailed, invalidState

        public var errorDescription: String? {
            switch self {
            case .none: return nil
            case .invalidSampleRate: return "Recording source sample rate is not finite and positive."
            case .sampleRateChanged: return "Recording source sample rate changed during capture."
            case .epochChanged: return "Recording source clock epoch changed during capture."
            case .invalidTimestamp: return "Recording requires a valid integral source sample timestamp."
            case .invalidFrameCount: return "Recording source quantum must contain a positive frame count."
            case .frameOverflow: return "Recording source quantum exceeds the supported frame coordinates."
            case .gap: return "Recording source clock skipped a render quantum."
            case .overlap: return "Recording source render quanta overlap or changed length."
            case .regression: return "Recording source clock moved backwards."
            case .renderFailed: return "Recording source render failed before the accepted end."
            case .invalidState: return "Recording boundary publication is invalid."
            }
        }
    }

    public enum Observation: Equatable, Sendable {
        case started, advanced, duplicate, ended, closed, failed
    }

    public struct Snapshot: Equatable, Sendable {
        public let revision: UInt64
        public let startFrame: Int64?
        public let endFrame: Int64?
        public let lastObservedStart: Int64?
        public let lastObservedThrough: Int64?
        public let errorCode: ErrorCode
        public let errorFrame: Int64?
        public let errorFrameCount: Int64
        public let errorSampleTime: Double
        public let errorSampleRate: Double
        public let errorEpoch: UInt64
        public var errorDescription: String? { errorCode.errorDescription }
    }

    public let sampleRate: Double
    public let epoch: UInt64
    private let words: UnsafeMutablePointer<Int64>
    private let stopRequested: UnsafeMutablePointer<Int64>

    // These fields belong exclusively to the serialized render writer. Readers
    // see only atomic words published under the sequence below.
    private var start: Int64?
    private var end: Int64?
    private var lastStart: Int64?
    private var lastThrough: Int64?
    private var errorCode: ErrorCode = .none
    private var errorFrame: Int64?
    private var errorFrameCount: Int64 = 0
    private var errorSampleTime: Double = 0
    private var errorSampleRate: Double = 0
    private var errorEpoch: UInt64 = 0

    public init(sampleRate: Double, epoch: UInt64) throws {
        guard sampleRate.isFinite && sampleRate > 0 else { throw ErrorCode.invalidSampleRate }
        self.sampleRate = sampleRate
        self.epoch = epoch
        words = .allocate(capacity: 12)
        words.initialize(repeating: 0, count: 12)
        stopRequested = .allocate(capacity: 1)
        stopRequested.initialize(to: 0)
    }

    deinit {
        words.deinitialize(count: 12)
        words.deallocate()
        stopRequested.deinitialize(count: 1)
        stopRequested.deallocate()
    }

    /// One-way mailbox. Its atomic load at the beginning of a new quantum is the
    /// stop request's linearization point. Repeated requests are harmless. A stop
    /// requested before the first quantum produces [firstStart,firstStart).
    public func requestStop() {
        _ = OSAtomicCompareAndSwap64Barrier(0, 1, stopRequested)
    }

    /// Use for an already-validated integral authority, such as a manual engine's
    /// Int64 sample counter. The timestamp overload validates raw audio timestamps.
    @discardableResult
    public func observeQuantum(frame: Int64, frames: Int, sampleRate: Double,
                               epoch: UInt64, failedRender: Bool = false) -> Observation {
        if errorCode != .none { return .failed }
        if end != nil { return .closed }
        // Sample once before validating the quantum: a racing control request
        // belongs to this boundary only if this atomic read observes it.
        let stopping = OSAtomicAdd64Barrier(0, stopRequested) != 0
        return observe(frame: frame, frames: frames, sampleTime: Double(frame),
                       sampleRate: sampleRate, epoch: epoch, failedRender: failedRender,
                       stopping: stopping)
    }

    @discardableResult
    public func observeQuantum(sampleTime: Double, frames: Int, sampleRate: Double,
                               epoch: UInt64, sampleTimeValid: Bool,
                               failedRender: Bool = false) -> Observation {
        if errorCode != .none { return .failed }
        if end != nil { return .closed }
        let stopping = OSAtomicAdd64Barrier(0, stopRequested) != 0
        guard !failedRender else {
            return fail(.renderFailed, frame: nil, frames: frames, sampleTime: sampleTime,
                        sampleRate: sampleRate, epoch: epoch)
        }
        guard sampleTimeValid, sampleTime.isFinite,
              let frame = Int64(exactly: sampleTime) else {
            return fail(.invalidTimestamp, frame: nil, frames: frames, sampleTime: sampleTime,
                        sampleRate: sampleRate, epoch: epoch)
        }
        return observe(frame: frame, frames: frames, sampleTime: sampleTime,
                       sampleRate: sampleRate, epoch: epoch, failedRender: false,
                       stopping: stopping)
    }

    private func observe(frame: Int64, frames: Int, sampleTime: Double,
                         sampleRate: Double, epoch: UInt64, failedRender: Bool,
                         stopping: Bool) -> Observation {
        var invalid: ErrorCode = .none
        if failedRender { invalid = .renderFailed }
        else if !sampleRate.isFinite || sampleRate <= 0 { invalid = .invalidSampleRate }
        else if sampleRate != self.sampleRate { invalid = .sampleRateChanged }
        else if epoch != self.epoch { invalid = .epochChanged }
        else if frames <= 0 { invalid = .invalidFrameCount }
        guard invalid == .none else {
            return fail(invalid, frame: frame, frames: frames, sampleTime: sampleTime,
                        sampleRate: sampleRate, epoch: epoch)
        }
        guard let count = Int64(exactly: frames) else {
            return fail(.invalidFrameCount, frame: frame, frames: frames, sampleTime: sampleTime,
                        sampleRate: sampleRate, epoch: epoch)
        }
        let (through, overflow) = frame.addingReportingOverflow(count)
        guard !overflow else {
            return fail(.frameOverflow, frame: frame, frames: frames, sampleTime: sampleTime,
                        sampleRate: sampleRate, epoch: epoch)
        }
        if let previousStart = lastStart, let previousThrough = lastThrough {
            if frame == previousStart && through == previousThrough { return .duplicate }
            if frame < previousStart { invalid = .regression }
            else if frame < previousThrough { invalid = .overlap }
            else if frame > previousThrough { invalid = .gap }
            guard invalid == .none else {
                return fail(invalid, frame: frame, frames: frames, sampleTime: sampleTime,
                            sampleRate: sampleRate, epoch: epoch)
            }
        }
        let first = start == nil
        if first { start = frame }
        if stopping { end = frame }
        lastStart = frame
        lastThrough = through
        publish()
        return stopping ? .ended : (first ? .started : .advanced)
    }

    private func fail(_ code: ErrorCode, frame: Int64?, frames: Int,
                      sampleTime: Double, sampleRate: Double, epoch: UInt64) -> Observation {
        errorCode = code
        errorFrame = frame
        errorFrameCount = Int64(clamping: frames)
        errorSampleTime = sampleTime
        errorSampleRate = sampleRate
        errorEpoch = epoch
        publish()
        return .failed
    }

    private func publish() {
        OSAtomicIncrement64Barrier(words) // odd: readers must retry, never wait
        let flags: Int64 = (start == nil ? 0 : 1) | (end == nil ? 0 : 2) |
            (lastStart == nil ? 0 : 4) | (errorFrame == nil ? 0 : 8)
        store(flags, at: 1)
        store(start ?? 0, at: 2)
        store(end ?? 0, at: 3)
        store(lastStart ?? 0, at: 4)
        store(lastThrough ?? 0, at: 5)
        store(errorCode.rawValue, at: 6)
        store(errorFrame ?? 0, at: 7)
        store(errorFrameCount, at: 8)
        store(Int64(bitPattern: errorSampleTime.bitPattern), at: 9)
        store(Int64(bitPattern: errorSampleRate.bitPattern), at: 10)
        store(Int64(bitPattern: errorEpoch), at: 11)
        OSAtomicIncrement64Barrier(words) // even: complete publication
    }

    /// Bounded, coherent read for control/tap consumers. nil means publication
    /// raced the read; retain the previous snapshot or retry on a later callback.
    /// This must not be replaced with an unbounded spin on an audio callback.
    public func trySnapshot(maxAttempts: Int = 8) -> Snapshot? {
        guard maxAttempts > 0 else { return nil }
        for _ in 0..<min(maxAttempts, 64) {
            let before = load(0)
            if before & 1 != 0 { continue }
            let flags = load(1), start = load(2), end = load(3)
            let lastStart = load(4), lastThrough = load(5), code = load(6)
            let errorFrame = load(7), frames = load(8), time = load(9)
            let rate = load(10), epoch = load(11)
            if load(0) != before { continue }
            return Snapshot(revision: UInt64(bitPattern: before),
                startFrame: flags & 1 == 0 ? nil : start,
                endFrame: flags & 2 == 0 ? nil : end,
                lastObservedStart: flags & 4 == 0 ? nil : lastStart,
                lastObservedThrough: flags & 4 == 0 ? nil : lastThrough,
                errorCode: ErrorCode(rawValue: code) ?? .invalidState,
                errorFrame: flags & 8 == 0 ? nil : errorFrame,
                errorFrameCount: frames,
                errorSampleTime: Double(bitPattern: UInt64(bitPattern: time)),
                errorSampleRate: Double(bitPattern: UInt64(bitPattern: rate)),
                errorEpoch: UInt64(bitPattern: epoch))
        }
        return nil
    }

    @inline(__always) private func load(_ index: Int) -> Int64 {
        OSAtomicAdd64Barrier(0, words.advanced(by: index))
    }

    @inline(__always) private func store(_ value: Int64, at index: Int) {
        // There is exactly one writer to each publication word; readers perform
        // only atomic add(0). CAS therefore succeeds once without an RT loop.
        let pointer = words.advanced(by: index)
        let old = OSAtomicAdd64Barrier(0, pointer)
        _ = OSAtomicCompareAndSwap64Barrier(old, value, pointer)
    }
}

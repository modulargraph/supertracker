import Foundation

/// Extracts one exact half-open recording interval from timestamped PCM packets.
///
/// The caller supplies integral coordinates from one authoritative source clock;
/// this type never derives time from callback size, elapsed wall time, or sleeps.
/// Calls must be serialized with accepting the start/end boundaries. Returned
/// selections refer to the caller's packet and do not own or copy any PCM.
public struct AudioRecordingWindow {
    public struct Clock: Equatable, Sendable {
        /// Identifies the clock epoch, not merely the device or input name.
        public let id: String
        public let sampleRate: Double

        public init(id: String, sampleRate: Double) {
            self.id = id
            self.sampleRate = sampleRate
        }
    }

    public struct Selection: Equatable, Sendable {
        /// Offset and count in the packet passed to receive.
        public let offset: Int
        public let count: Int
        /// The source coordinate at offset. Empty selections use offset zero.
        public let sourceStartFrame: Int64
    }

    public enum WindowError: Error, Equatable, LocalizedError, Sendable {
        case invalidClock
        case clockChanged(expected: Clock, received: Clock)
        case invalidFrameCount(Int)
        case packetOverflow(startFrame: Int64, frameCount: Int)
        case invalidEnd(startFrame: Int64, endFrame: Int64)
        case intervalOverflow
        case endChanged(accepted: Int64, proposed: Int64)
        case lateEnd(selectedThrough: Int64, proposed: Int64)
        case gap(expectedFrame: Int64, receivedFrame: Int64)

        public var errorDescription: String? {
            switch self {
            case .invalidClock:
                return "Recording requires a named source clock with a finite positive sample rate."
            case let .clockChanged(expected, received):
                return "Recording source clock changed from \(expected.id) at \(expected.sampleRate) Hz to \(received.id) at \(received.sampleRate) Hz."
            case let .invalidFrameCount(count):
                return "Recording packet frame count must be positive; received \(count)."
            case let .packetOverflow(start, count):
                return "Recording packet at \(start) with \(count) frames exceeds the source frame coordinate range."
            case let .invalidEnd(start, end):
                return "Recording end \(end) precedes its accepted start \(start)."
            case .intervalOverflow:
                return "Recording interval exceeds the supported frame count."
            case let .endChanged(accepted, proposed):
                return "Recording end is already \(accepted); it cannot change to \(proposed)."
            case let .lateEnd(selected, proposed):
                return "Recording end \(proposed) arrived after audio through \(selected) had already been selected."
            case let .gap(expected, received):
                return "Recording is missing source frames starting at \(expected); the next packet starts at \(received)."
            }
        }
    }

    public let clock: Clock
    public let startFrame: Int64
    public private(set) var endFrame: Int64?
    /// Greatest exclusive end of accepted packets, including pre/post-interval audio.
    /// A duplicate or old packet can never move this watermark backwards.
    public private(set) var receivedThrough: Int64?
    /// Exclusive end of contiguous selections, initially startFrame. This is not
    /// a disk-write acknowledgement; callers must await their own writer barrier.
    public private(set) var selectedThrough: Int64
    public private(set) var selectedFrames: Int64 = 0
    /// The first failure is sticky. Failed operations do not advance either
    /// watermark or accept a new end, and all subsequent operations throw it.
    public private(set) var failure: WindowError?

    /// Completion proves that the accepted interval was selected and an actual
    /// packet reached its end. It does not prove that a downstream writer flushed.
    /// Even an empty interval waits for a packet watermark at/after its boundary.
    public var isComplete: Bool {
        guard failure == nil, let endFrame, let receivedThrough else { return false }
        return selectedThrough == endFrame && receivedThrough >= endFrame
    }

    public init(clock: Clock, startFrame: Int64, endFrame: Int64? = nil) throws {
        guard Self.valid(clock) else { throw WindowError.invalidClock }
        self.clock = clock
        self.startFrame = startFrame
        self.selectedThrough = startFrame
        self.endFrame = nil
        if let endFrame { try acceptEnd(endFrame, clock: clock) }
    }

    /// Accepts an immutable stop boundary on the source clock. Repeating the same
    /// boundary is harmless. A boundary behind already-selected PCM is an error;
    /// the helper cannot retroactively remove samples submitted to a writer.
    public mutating func acceptEnd(_ endFrame: Int64, clock: Clock) throws {
        try checkClock(clock)
        if let accepted = self.endFrame {
            guard accepted == endFrame else {
                throw record(.endChanged(accepted: accepted, proposed: endFrame))
            }
            return
        }
        guard endFrame >= startFrame else {
            throw record(.invalidEnd(startFrame: startFrame, endFrame: endFrame))
        }
        guard !endFrame.subtractingReportingOverflow(startFrame).overflow else {
            throw record(.intervalOverflow)
        }
        guard endFrame >= selectedThrough else {
            throw record(.lateEnd(selectedThrough: selectedThrough, proposed: endFrame))
        }
        self.endFrame = endFrame
    }

    /// Selects the previously unselected suffix of a packet inside [start,end).
    /// End may still be unknown. Exact duplicates and overlapping prefixes select
    /// no frames twice, even after the caller has consumed previous selections.
    /// Missing audio before the accepted start is irrelevant; any gap in the
    /// requested interval fails immediately instead of fabricating silence.
    /// The successful packet path allocates no arrays, buffers, or strings.
    public mutating func receive(startFrame packetStart: Int64, frameCount: Int,
                                 clock: Clock) throws -> Selection {
        try checkClock(clock)
        guard frameCount > 0, let length = Int64(exactly: frameCount) else {
            throw record(.invalidFrameCount(frameCount))
        }
        let (packetEnd, overflow) = packetStart.addingReportingOverflow(length)
        guard !overflow else {
            throw record(.packetOverflow(startFrame: packetStart, frameCount: frameCount))
        }

        let needsAudio = endFrame.map { selectedThrough < $0 } ?? true
        var selection = Selection(offset: 0, count: 0, sourceStartFrame: packetStart)
        var nextSelected = selectedThrough
        var nextCount = selectedFrames
        if needsAudio && packetEnd > selectedThrough {
            guard packetStart <= selectedThrough else {
                throw record(.gap(expectedFrame: selectedThrough, receivedFrame: packetStart))
            }
            let selectionEnd = endFrame.map { min(packetEnd, $0) } ?? packetEnd
            // Both differences are bounded by this positive packet's frameCount.
            let offset = Int(selectedThrough - packetStart)
            let count = Int(selectionEnd - selectedThrough)
            let (total, countOverflow) = selectedFrames.addingReportingOverflow(Int64(count))
            guard !countOverflow else { throw record(.intervalOverflow) }
            selection = Selection(offset: offset, count: count, sourceStartFrame: selectedThrough)
            nextSelected = selectionEnd
            nextCount = total
        }

        selectedThrough = nextSelected
        selectedFrames = nextCount
        receivedThrough = receivedThrough.map { max($0, packetEnd) } ?? packetEnd
        return selection
    }

    private static func valid(_ clock: Clock) -> Bool {
        clock.sampleRate.isFinite && clock.sampleRate > 0 &&
            clock.id.contains(where: { !$0.isWhitespace })
    }

    private mutating func checkClock(_ proposed: Clock) throws {
        if let failure { throw failure }
        // Equality also verifies the rate in the steady path, without allocating.
        guard proposed != clock else { return }
        guard Self.valid(proposed) else { throw record(.invalidClock) }
        throw record(.clockChanged(expected: clock, received: proposed))
    }

    private mutating func record(_ error: WindowError) -> WindowError {
        failure = error
        return error
    }
}

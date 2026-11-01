import Accelerate
import Foundation
import AVFoundation
import Darwin

/// Native post-insert metering: attach one tap after each track's Audio Units
/// (buses 0..<trackCapacity), and measure the main mixer in its existing recording
/// tap (bus trackCapacity, or measureMaster).
/// Measure selected hardware inputs through measureInput in the input recording
/// tap; their dedicated stereo slot does not affect track or master levels.
/// AVAudioNode allows only one tap per bus, so recording and master metering must
/// share that callback. Keep this object alive until every tap has been removed.
final class AudioMeters {
    private let trackCount: Int
    private var masterBus: Int { trackCount }
    private var inputBus: Int { trackCount + 1 }
    private var scalarCount: Int { (trackCount + 2) * 2 }
    private let pending: UnsafeMutablePointer<Int32>
    // Accessed only by the UI/control thread, never by the render callbacks.
    private let displayed: UnsafeMutablePointer<Float>

    /// Storage for `trackCapacity` track meters is allocated up front, so taps on
    /// tracks added later never reallocate what a render thread writes.
    init(trackCapacity: Int = 16) {
        trackCount = max(0, trackCapacity)
        let scalarCount = (trackCount + 2) * 2
        pending = .allocate(capacity: scalarCount)
        pending.initialize(repeating: 0, count: scalarCount)
        displayed = .allocate(capacity: scalarCount)
        displayed.initialize(repeating: 0, count: scalarCount)
    }

    func measureMaster(_ buffer: AVAudioPCMBuffer) { measure(buffer, bus: masterBus) }

    /// Peaks measured elsewhere (a strip processor's own output metering) for the master slot.
    func accumulateMaster(left: Float, right: Float) {
        if left.isFinite && left > 0 { accumulate(left, at: masterBus * 2) }
        if right.isFinite && right > 0 { accumulate(right, at: masterBus * 2 + 1) }
    }

    /// Finite peak magnitude of `count` samples at `stride`; non-finite samples are skipped.
    @inline(__always)
    private static func peak(_ samples: UnsafePointer<Float>, stride: Int, count: Int) -> Float {
        var peak: Float = 0
        vDSP_maxmgv(samples, vDSP_Stride(stride), &peak, vDSP_Length(count))
        if peak.isFinite { return peak }
        peak = 0
        for frame in 0..<count {
            let sample = abs(samples[frame * stride])
            if sample.isFinite { peak = max(peak, sample) }
        }
        return peak
    }

    deinit {
        pending.deinitialize(count: scalarCount)
        pending.deallocate()
        displayed.deinitialize(count: scalarCount)
        displayed.deallocate()
    }

    /// Called directly from a Float32 audio tap. No locks or heap allocations.
    /// Peaks accumulate until the next UI snapshot, so a transient cannot vanish
    /// between UI ticks. Mono is mirrored; extra channels fold into alternating
    /// left/right meters. Values above unity are retained to expose clipping.
    func measure(_ buffer: AVAudioPCMBuffer, bus: Int) {
        guard bus >= 0, bus <= masterBus,
              buffer.format.commonFormat == .pcmFormatFloat32,
              let data = buffer.floatChannelData else { return }
        let frames = Int(buffer.frameLength)
        let channels = Int(buffer.format.channelCount)
        guard frames > 0, channels > 0 else { return }
        var left: Float = 0
        var right: Float = 0
        for channel in 0..<channels {
            let peak = buffer.format.isInterleaved
                ? Self.peak(data[0].advanced(by: channel), stride: channels, count: frames)
                : Self.peak(data[channel], stride: 1, count: frames)
            if channel & 1 == 0 { left = max(left, peak) }
            else { right = max(right, peak) }
        }
        if channels == 1 { right = left }
        accumulate(left, at: bus * 2)
        accumulate(right, at: bus * 2 + 1)
    }

    /// Called from the raw hardware input tap with the channel selection captured
    /// when installing that tap. The first two zero-based device channels map to
    /// left/right; a single selected channel is mirrored. Invalid or absent
    /// selections are silent. Reading the existing selection array allocates no
    /// storage, and only the selected channels are scanned.
    func measureInput(_ buffer: AVAudioPCMBuffer, channels: [Int]) {
        guard buffer.format.commonFormat == .pcmFormatFloat32,
              let data = buffer.floatChannelData else { return }
        let frames = Int(buffer.frameLength)
        let deviceChannels = Int(buffer.format.channelCount)
        guard frames > 0, deviceChannels > 0 else { return }
        let interleaved = buffer.format.isInterleaved
        let stride = interleaved ? deviceChannels : 1
        var left: Float = 0
        var right: Float = 0
        for selected in 0..<min(channels.count, 2) {
            let channel = channels[selected]
            guard channel >= 0, channel < deviceChannels else { continue }
            let samples = interleaved ? data[0].advanced(by: channel) : data[channel]
            let peak = Self.peak(samples, stride: stride, count: frames)
            if selected == 0 { left = peak } else { right = peak }
        }
        if channels.count == 1 { right = left }
        accumulate(left, at: inputBus * 2)
        accumulate(right, at: inputBus * 2 + 1)
    }

    /// Call on the UI/control thread, normally at 20 Hz. The atomic exchange
    /// assigns racing samples to this snapshot or the next without losing them.
    /// Decay happens here, independently of tap buffer size or sample rate.
    /// `tracks` limits the track levels decayed and returned (default: all); master and
    /// input are always included.
    func snapshot(tracks limit: Int? = nil) -> (left: Float, right: Float, tracks: [Float], inputLeft: Float, inputRight: Float) {
        let count = min(trackCount, max(0, limit ?? trackCount))
        for index in Array(0..<(count * 2)) + Array((masterBus * 2)..<scalarCount) {
            let captured = Float(bitPattern: UInt32(bitPattern: exchangeZero(at: index)))
            let decayed = displayed[index] * 0.88
            displayed[index] = max(captured, decayed < 0.00001 ? 0 : decayed)
        }
        var tracks = [Float](repeating: 0, count: count)
        for track in 0..<tracks.count {
            tracks[track] = max(displayed[track * 2], displayed[track * 2 + 1])
        }
        return (displayed[masterBus * 2], displayed[masterBus * 2 + 1], tracks,
                displayed[inputBus * 2], displayed[inputBus * 2 + 1])
    }

    /// Call on the UI/control thread after stopping the graph to clear all peaks.
    func reset() {
        for index in 0..<scalarCount {
            _ = exchangeZero(at: index)
            displayed[index] = 0
        }
    }

    /// Call on the UI/control thread after removing or replacing the input tap.
    /// Track and master peak histories remain untouched.
    func resetInput() {
        for index in (inputBus * 2)..<(inputBus * 2 + 2) {
            _ = exchangeZero(at: index)
            displayed[index] = 0
        }
    }

    // OSAtomic provides scalar atomic operations on the macOS 14 target without
    // introducing a package dependency. All stored Float values are finite and
    // nonnegative, so their Int32 bit patterns retain magnitude ordering.
    @inline(__always)
    private func accumulate(_ value: Float, at index: Int) {
        let pointer = pending.advanced(by: index)
        let replacement = Int32(bitPattern: value.bitPattern)
        var previous = OSAtomicAdd32Barrier(0, pointer)
        while replacement > previous {
            if OSAtomicCompareAndSwap32Barrier(previous, replacement, pointer) { return }
            previous = OSAtomicAdd32Barrier(0, pointer)
        }
    }

    @inline(__always)
    private func exchangeZero(at index: Int) -> Int32 {
        let pointer = pending.advanced(by: index)
        var previous = OSAtomicAdd32Barrier(0, pointer)
        while !OSAtomicCompareAndSwap32Barrier(previous, 0, pointer) {
            previous = OSAtomicAdd32Barrier(0, pointer)
        }
        return previous
    }
}

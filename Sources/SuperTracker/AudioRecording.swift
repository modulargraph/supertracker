import Foundation
import AVFoundation
import AudioToolbox

enum AudioRecordingError: LocalizedError {
    case invalid(String)
    var errorDescription: String? { if case let .invalid(message) = self { return message }; return nil }
}

/// An immutable extraction plan for one recording. Input taps observe Core Audio's
/// channelMap output: hardware selections have already moved to channels zero/one.
/// Keep hardware selection validation separate from extraction to avoid remapping
/// those samples a second time, or saving unused interface channels in the WAV.
struct AudioRecordingChannelPlan {
    let sourceChannels: [Int]
    let outputFormat: AVAudioFormat
    private let sourceChannelCount: AVAudioChannelCount
    private let sourceSampleRate: Double

    init(sourceFormat: AVAudioFormat, sourceChannels: [Int]) throws {
        guard sourceFormat.sampleRate.isFinite, sourceFormat.sampleRate > 0,
              sourceFormat.channelCount > 0, sourceFormat.commonFormat == .pcmFormatFloat32 else {
            throw AudioRecordingError.invalid("Recording requires a valid Float32 audio source.")
        }
        guard (1...2).contains(sourceChannels.count),
              sourceChannels.allSatisfy({ $0 >= 0 && $0 < Int(sourceFormat.channelCount) }) else {
            throw AudioRecordingError.invalid("Choose one or two available recording channels.")
        }
        guard let format = AVAudioFormat(standardFormatWithSampleRate: sourceFormat.sampleRate,
                                         channels: AVAudioChannelCount(sourceChannels.count)) else {
            throw AudioRecordingError.invalid("Unable to create the selected recording format.")
        }
        self.sourceChannels = sourceChannels
        outputFormat = format
        sourceChannelCount = sourceFormat.channelCount
        sourceSampleRate = sourceFormat.sampleRate
    }

    static func input(sourceFormat: AVAudioFormat, selectedHardwareChannels: [Int]) throws -> Self {
        // A mono default device has only its first channel; the host's initial
        // default pair [0,1] has not necessarily been explicitly configured yet.
        let selected = sourceFormat.channelCount == 1 && selectedHardwareChannels == [0,1]
            ? [0] : selectedHardwareChannels
        guard (1...2).contains(selected.count),
              selected.allSatisfy({ $0 >= 0 && $0 < Int(sourceFormat.channelCount) }) else {
            throw AudioRecordingError.invalid("The selected audio input channels are unavailable. Choose channels for the current input device before recording.")
        }
        return try Self(sourceFormat: sourceFormat, sourceChannels: Array(0..<selected.count))
    }

    static func master(sourceFormat: AVAudioFormat) throws -> Self {
        guard sourceFormat.channelCount >= 2 else {
            throw AudioRecordingError.invalid("The master recording source must provide stereo audio.")
        }
        return try Self(sourceFormat: sourceFormat, sourceChannels: [0,1])
    }

    func copy(_ input: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
        guard input.format.commonFormat == .pcmFormatFloat32,
              input.format.channelCount == sourceChannelCount,
              input.format.sampleRate == sourceSampleRate,
              let source = input.floatChannelData else {
            throw AudioRecordingError.invalid("The recording input format changed (expected Float32 \(sourceChannelCount) channels at \(sourceSampleRate) Hz; received commonFormat \(input.format.commonFormat.rawValue), \(input.format.channelCount) channels at \(input.format.sampleRate) Hz, Float32 buffers \(input.floatChannelData != nil)). Stop recording and select the input channels again.")
        }
        guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: max(1,input.frameLength)),
              let destination = output.floatChannelData else {
            throw AudioRecordingError.invalid("Unable to allocate the recording audio buffer.")
        }
        output.frameLength = input.frameLength
        let frames = Int(input.frameLength)
        if input.format.isInterleaved {
            let stride = Int(sourceChannelCount)
            for (outputChannel,inputChannel) in sourceChannels.enumerated() {
                for frame in 0..<frames { destination[outputChannel][frame] = source[0][frame * stride + inputChannel] }
            }
        } else {
            for (outputChannel,inputChannel) in sourceChannels.enumerated() {
                memcpy(destination[outputChannel], source[inputChannel], frames * MemoryLayout<Float>.stride)
            }
        }
        return output
    }
}

// File/error state is confined to queue; the owner reads after its final barrier.
// The callback copies only the selected mono/stereo channels and enqueues them.
// Written-frame progress is an atomic scalar for UI reads.
final class AudioRecording: @unchecked Sendable {
    enum Source { case input, master }
    let source: Source
    private var file: AVAudioFile?
    private let channels: AudioRecordingChannelPlan
    let sampleRate: Double
    let channelCount: AVAudioChannelCount
    let path: String
    let instrument: Int?
    let owner: String?
    /// Nil is reserved for generated channel-extraction fixtures. The host always
    /// supplies a source boundary and timestamped tap packets.
    let boundary: AudioRecordingBoundary?
    let manualClock: Bool
    private let beforeWrite: (() throws -> Void)?
    private let boundarySnapshot:(()->AudioRecordingBoundary.Snapshot?)?
    private var window: AudioRecordingWindow?
    private var finished = false
    let queue = DispatchQueue(label:"SuperTracker.recording",qos:.userInitiated)
    private(set) var error: Error?
    private let frameCounter: UnsafeMutablePointer<Int64> = {
        let pointer = UnsafeMutablePointer<Int64>.allocate(capacity:1); pointer.initialize(to:0); return pointer
    }()
    var frames: AVAudioFramePosition { OSAtomicAdd64Barrier(0,frameCounter) }
    deinit { frameCounter.deinitialize(count:1); frameCounter.deallocate() }

    init(source: Source, file: AVAudioFile, path: String, instrument: Int?, channels: AudioRecordingChannelPlan,
         boundary: AudioRecordingBoundary? = nil, manualClock: Bool = false,
         beforeWrite: (() throws -> Void)? = nil,
         boundarySnapshot:(()->AudioRecordingBoundary.Snapshot?)? = nil, owner:String? = nil) throws {
        let format = file.processingFormat
        guard format.channelCount == channels.outputFormat.channelCount,
              format.sampleRate == channels.outputFormat.sampleRate,
              format.commonFormat == .pcmFormatFloat32, !format.isInterleaved else {
            throw AudioRecordingError.invalid("The recording file format does not match the selected input channels.")
        }
        self.source = source; self.file = file; self.path = path; self.instrument = instrument; self.owner=owner; self.channels = channels
        self.boundary = boundary; self.manualClock = manualClock; self.beforeWrite = beforeWrite; self.boundarySnapshot = boundarySnapshot
        sampleRate = format.sampleRate; channelCount = format.channelCount
    }

    func close() { finished = true; file = nil }

    struct Progress {
        let frames: Int64
        let startFrame: Int64?
        let endFrame: Int64?
        let receivedThrough: Int64?
        let complete: Bool
        let error: Error?
    }

    func requestStop() { boundary?.requestStop() }

    /// Exactly one source-render producer owns this call. Offline master renders
    /// use the host's admitted manual quantum instead of unflagged AU timestamps.
    func observeRender(_ timestamp:AudioTimeStamp,frames:AUAudioFrameCount,failed:Bool = false) {
        guard source != .input, !manualClock, let boundary else { return }
        _ = boundary.observeQuantum(sampleTime:timestamp.mSampleTime,frames:Int(frames),sampleRate:sampleRate,
            epoch:boundary.epoch,sampleTimeValid:timestamp.mFlags.contains(.sampleTimeValid),failedRender:failed)
    }

    func observeManualQuantum(frame:Int64,frames:AUAudioFrameCount,failed:Bool = false) {
        guard manualClock, let boundary else { return }
        _ = boundary.observeQuantum(frame:frame,frames:Int(frames),sampleRate:sampleRate,epoch:boundary.epoch,failedRender:failed)
    }

    /// A control-side interruption preserves the already written PCM. It never
    /// asks the render thread to wait for disk work or clears the source endpoint.
    func fail(_ failure: Error) {
        queue.async { [self] in
            guard !finished,error == nil else { return }
            do { _ = try synchronizeWindow() } catch { self.error = error }
            if error == nil && window?.isComplete != true { error = failure }
        }
    }

    func progress() async -> Progress {
        await withCheckedContinuation { continuation in
            queue.async { [self] in
                if error == nil && !finished {
                    do { _ = try synchronizeWindow() } catch { self.error = error }
                }
                continuation.resume(returning:Progress(frames:frames,startFrame:window?.startFrame,
                    endFrame:window?.endFrame,receivedThrough:window?.receivedThrough,
                    complete:window?.isComplete == true,error:error))
            }
        }
    }

    /// Final writer barrier. An explicit interruption is attached only after all
    /// earlier queued packets were processed, retaining the valid prefix on disk.
    func finish(interruption:Error? = nil) async -> Progress {
        await withCheckedContinuation { continuation in
            queue.async { [self] in
                if error == nil {
                    do { _ = try synchronizeWindow() } catch { self.error = error }
                }
                // Once every accepted sample was written, a later device/host
                // interruption cannot retroactively turn that interval partial.
                if error == nil && window?.isComplete != true, let interruption { error = interruption }
                if error == nil && boundary != nil && window?.isComplete != true {
                    error = AudioRecordingError.invalid("The recording stopped before its source-clock tail arrived.")
                }
                if error == nil && frames == 0 {
                    error = AudioRecordingError.invalid("No audio frames arrived during the accepted recording interval.")
                }
                close()
                continuation.resume(returning:Progress(frames:frames,startFrame:window?.startFrame,
                    endFrame:window?.endFrame,receivedThrough:window?.receivedThrough,
                    complete:window?.isComplete == true,error:error))
            }
        }
    }

    private func synchronizeWindow() throws -> Bool {
        guard let boundary else { return true }
        // Contention is not a clock error. Retain this packet on the serial
        // writer queue until a fresh coherent snapshot is available, preserving
        // FIFO and never selecting against an older end fence. This wait is not
        // on the render/tap thread and persistent failure is bounded.
        let deadline = ContinuousClock.now.advanced(by:.milliseconds(100))
        var coherent:AudioRecordingBoundary.Snapshot?
        repeat {
            if let boundarySnapshot { coherent = boundarySnapshot() }
            else { coherent = boundary.trySnapshot(maxAttempts:64) }
            if coherent != nil { break }
            if ContinuousClock.now >= deadline {
                throw AudioRecordingError.invalid("The recording source clock could not be read coherently within 100 milliseconds.")
            }
            Thread.sleep(forTimeInterval:0.001)
        } while true
        guard let snapshot = coherent else { return false }
        if snapshot.errorCode != .none { throw snapshot.errorCode }
        guard let start = snapshot.startFrame else { return false }
        let clock = AudioRecordingWindow.Clock(id:String(boundary.epoch),sampleRate:sampleRate)
        if window == nil { window = try AudioRecordingWindow(clock:clock,startFrame:start) }
        if let end = snapshot.endFrame { try window?.acceptEnd(end,clock:clock) }
        return true
    }

    func receive(_ input: AVAudioPCMBuffer, at time: AVAudioTime? = nil) {
        guard input.frameLength > 0 else { return }
        do {
            let copy = try channels.copy(input)
            let packetStart: Int64?
            if boundary != nil {
                guard let time, time.isSampleTimeValid, time.sampleRate == sampleRate else {
                    throw AudioRecordingError.invalid("The recording tap did not supply the expected source sample clock.")
                }
                packetStart = time.sampleTime
                if source == .input, let boundary {
                    // Input taps carry the actual captured PCM clock. The duplex
                    // HAL render observer can instead report output-rate pulls,
                    // which are neither contiguous nor rate-aligned with input.
                    // Use one serialized tap producer for both data and fences.
                    _ = boundary.observeQuantum(frame:time.sampleTime,frames:Int(input.frameLength),
                        sampleRate:time.sampleRate,epoch:boundary.epoch)
                }
            } else { packetStart = nil }
            queue.async { [self] in
                guard !finished, error == nil, let file else { return }
                do {
                    if let packetStart {
                        // A pending old tap packet may arrive before the first
                        // accepted source quantum. Its frames precede this capture.
                        guard try synchronizeWindow() else { return }
                        guard var active = window else { return }
                        let selection = try active.receive(startFrame:packetStart,frameCount:Int(copy.frameLength),clock:active.clock)
                        window = active
                        guard selection.count > 0 else { return }
                        if selection.offset > 0, let data = copy.floatChannelData {
                            for channel in 0..<Int(channelCount) {
                                memmove(data[channel],data[channel].advanced(by:selection.offset),selection.count * MemoryLayout<Float>.stride)
                            }
                        }
                        copy.frameLength = AVAudioFrameCount(selection.count)
                    }
                    try beforeWrite?()
                    try file.write(from:copy)
                    OSAtomicAdd64Barrier(Int64(copy.frameLength),frameCounter)
                }
                catch { self.error = error }
            }
        } catch {
            queue.async { [self] in if self.error == nil { self.error = error } }
        }
    }
}

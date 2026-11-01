// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Generated PCM only: no audio engine, hardware input, tap, or plug-in is opened.
enum NativeAudioRecordingTests {
    private static let frameCount = 97
    private static let sampleRate = 48_000.0

    static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("recording-channels", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var passed: [String] = []
        let multichannel = try generated(channels: 8)

        // Core Audio has already copied hardware channel 7 to tap channels 0/1.
        // Remaining tap channels are deliberately silent to detect double mapping.
        let mappedMono = try generated(channels: 8, silent: true)
        for frame in 0..<frameCount {
            set(mappedMono, channel: 0, frame: frame, value: signal(channel: 6, frame: frame))
            set(mappedMono, channel: 1, frame: frame, value: signal(channel: 6, frame: frame))
        }
        let monoPlan = try AudioRecordingChannelPlan.input(sourceFormat: mappedMono.format,
                                                          selectedHardwareChannels: [6])
        try check(monoPlan.sourceChannels == [0], "Hardware mono selection must read only remapped tap channel 0.")
        try verifyCopy(monoPlan, source: mappedMono, expectedChannels: [6])
        try record(monoPlan, source: mappedMono, expectedChannels: [6],
                   url: folder.appendingPathComponent("input-hardware-7-mono.wav"))
        passed.append("Recording hardware channel 7 from an eight-channel mapped tap produces a true mono 24-bit WAV without a second channel remap")

        let mappedStereo = try generated(channels: 8, silent: true)
        for frame in 0..<frameCount {
            set(mappedStereo, channel: 0, frame: frame, value: signal(channel: 6, frame: frame))
            set(mappedStereo, channel: 1, frame: frame, value: signal(channel: 3, frame: frame))
        }
        let stereoPlan = try AudioRecordingChannelPlan.input(sourceFormat: mappedStereo.format,
                                                            selectedHardwareChannels: [6, 3])
        try check(stereoPlan.sourceChannels == [0, 1], "Hardware stereo selection must read the remapped tap prefix.")
        try verifyCopy(stereoPlan, source: mappedStereo, expectedChannels: [6, 3])
        try record(stereoPlan, source: mappedStereo, expectedChannels: [6, 3],
                   url: folder.appendingPathComponent("input-hardware-7-4-stereo.wav"))
        passed.append("Recording ordered hardware channels 7 and 4 preserves the selected stereo PCM and writes exactly two channels")

        for interleaved in [false, true] {
            let raw = try generated(channels: 8, interleaved: interleaved)
            let plan = try AudioRecordingChannelPlan(sourceFormat: raw.format, sourceChannels: [6, 3])
            try verifyCopy(plan, source: raw, expectedChannels: [6, 3])
            try record(plan, source: raw, expectedChannels: [6, 3],
                       url: folder.appendingPathComponent("direct-7-4-\(interleaved ? "interleaved" : "planar").wav"))
        }
        passed.append("Direct recording extraction preserves ordered planar/interleaved channels and owns incoming PCM before asynchronous writing")

        let masterPlan = try AudioRecordingChannelPlan.master(sourceFormat: multichannel.format)
        try check(masterPlan.sourceChannels == [0, 1], "Master recording must select the first stereo pair.")
        try verifyCopy(masterPlan, source: multichannel, expectedChannels: [0, 1])
        try record(masterPlan, source: multichannel, expectedChannels: [0, 1],
                   url: folder.appendingPathComponent("master-stereo.wav"), recordingSource: .master)
        let monoDevice = try generated(channels: 1)
        let monoDefault = try AudioRecordingChannelPlan.input(sourceFormat: monoDevice.format,
                                                              selectedHardwareChannels: [0, 1])
        try check(monoDefault.sourceChannels == [0] && monoDefault.outputFormat.channelCount == 1,
                  "Default stereo selection on a mono device must normalize to true mono.")
        try verifyCopy(monoDefault, source: monoDevice, expectedChannels: [0])
        passed.append("Master recording remains stereo and the default input selection normalizes correctly on mono devices")

        for selection in [[], [-1], [8], [0, 8], [0, 1, 2]] {
            try rejects("Invalid direct selection \(selection)") {
                _ = try AudioRecordingChannelPlan(sourceFormat: multichannel.format, sourceChannels: selection)
            }
            try rejects("Invalid hardware selection \(selection)") {
                _ = try AudioRecordingChannelPlan.input(sourceFormat: multichannel.format,
                                                       selectedHardwareChannels: selection)
            }
        }
        let float64 = try generated(channels: 8, commonFormat: .pcmFormatFloat64, silent: true)
        try rejects("Unsupported recording source format") {
            _ = try AudioRecordingChannelPlan(sourceFormat: float64.format, sourceChannels: [0, 1])
        }
        let rateDrift = try generated(channels: 8, rate: 44_100)
        let channelDrift = try generated(channels: 4)
        try rejects("Source sample-rate drift") { _ = try stereoPlan.copy(rateDrift) }
        try rejects("Source channel-count drift") { _ = try stereoPlan.copy(channelDrift) }
        try rejects("Source PCM format drift") { _ = try masterPlan.copy(float64) }
        passed.append("Recording rejects missing, excessive and out-of-range selections, unsupported PCM, and sample-rate or channel-count drift")

        let invalidURL = folder.appendingPathComponent("invalid-buffer.wav")
        var invalidFile: AVAudioFile? = try file(for: stereoPlan, url: invalidURL)
        let invalidRecorder = try AudioRecording(source: .input, file: invalidFile!,
                                                  path: invalidURL.path, instrument: nil, channels: stereoPlan)
        invalidFile = nil
        invalidRecorder.receive(rateDrift)
        invalidRecorder.receive(mappedStereo)
        invalidRecorder.queue.sync { invalidRecorder.close() }
        try check(invalidRecorder.error != nil, "Asynchronous recording must expose invalid-buffer failure.")
        try check(invalidRecorder.frames == 0, "Invalid input must not increment recorded frames or allow later writes after the failure.")
        passed.append("The asynchronous recorder reports an invalid incoming buffer and does not count unwritten frames")
        for rate in [44_100.0,48_000.0] {
            for lostPacket in [false,true] {
                let format=AVAudioFormat(standardFormatWithSampleRate:rate,channels:1)!
                let url=folder.appendingPathComponent("tap-clock-\(Int(rate))-\(lostPacket).wav")
                let file=try AVAudioFile(forWriting:url,settings:format.settings)
                let boundary=try AudioRecordingBoundary(sampleRate:rate,epoch:42)
                let recorder=try AudioRecording(source:.input,file:file,path:url.path,instrument:nil,
                    channels:AudioRecordingChannelPlan(sourceFormat:format,sourceChannels:[0]),boundary:boundary)
                var frame:Int64=4096
                for count in [2048,2048,1024,4096] {
                    // Unrelated duplex output callbacks must never own input fences.
                    var outputTime=AudioTimeStamp();outputTime.mSampleTime=Double(frame)*44_100/rate
                    outputTime.mFlags = .sampleTimeValid
                    recorder.observeRender(outputTime,frames:256)
                    let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:AVAudioFrameCount(count))!
                    buffer.frameLength=AVAudioFrameCount(count)
                    buffer.floatChannelData![0].initialize(repeating:0.125,count:count)
                    recorder.receive(buffer,at:AVAudioTime(sampleTime:frame,atRate:rate))
                    _ = await recorder.progress() // retain a written prefix before injected loss
                    frame += Int64(count)
                }
                recorder.requestStop()
                let tail=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:2048)!
                tail.frameLength=2048
                recorder.receive(tail,at:AVAudioTime(sampleTime:frame+(lostPacket ? 256:0),atRate:rate))
                let result=await recorder.finish()
                try check(result.frames == 9216,"Tap clock did not preserve all accepted PCM: \(result.frames) frames; \(result.error?.localizedDescription ?? "no error").")
                if lostPacket {
                    try check(result.error as? AudioRecordingBoundary.ErrorCode == .gap,"Real missing input frames were not rejected.")
                } else {
                    try check(result.error == nil && result.complete && result.endFrame == frame,"Contiguous batched input failed to stop on its own clock.")
                }
            }
        }
        passed.append("Input tap clock accepts batched 44.1/48kHz PCM despite unrelated duplex render clocks, and still rejects actual missing PCM")
        return passed
    }

    private static func signal(channel: Int, frame: Int) -> Float {
        // Exact powers-of-two fractions are stable across 24-bit PCM round trips.
        Float((channel + 1) * 128 + frame % 29 - 14) / 4096
    }

    private static func generated(channels: Int, interleaved: Bool = false,
                                  rate: Double = sampleRate, commonFormat: AVAudioCommonFormat = .pcmFormatFloat32,
                                  silent: Bool = false) throws -> AVAudioPCMBuffer {
        let format: AVAudioFormat
        if channels > 2 {
            guard let layout = AVAudioChannelLayout(layoutTag: kAudioChannelLayoutTag_DiscreteInOrder | UInt32(channels)) else {
                throw failure("Cannot create generated multichannel layout.")
            }
            format = AVAudioFormat(commonFormat: commonFormat, sampleRate: rate,
                                   interleaved: interleaved, channelLayout: layout)
        } else {
            guard let simple = AVAudioFormat(commonFormat: commonFormat, sampleRate: rate,
                                             channels: AVAudioChannelCount(channels), interleaved: interleaved) else {
                throw failure("Cannot create generated PCM format.")
            }
            format = simple
        }
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frameCount)) else {
            throw failure("Cannot allocate generated PCM.")
        }
        buffer.frameLength = AVAudioFrameCount(frameCount)
        for audio in UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList) {
            if let data = audio.mData { memset(data, 0, Int(audio.mDataByteSize)) }
        }
        if !silent {
            for channel in 0..<channels {
                for frame in 0..<frameCount { set(buffer, channel: channel, frame: frame, value: signal(channel: channel, frame: frame)) }
            }
        }
        return buffer
    }

    private static func set(_ buffer: AVAudioPCMBuffer, channel: Int, frame: Int, value: Float) {
        if buffer.format.isInterleaved {
            buffer.floatChannelData![0][frame * Int(buffer.format.channelCount) + channel] = value
        } else { buffer.floatChannelData![channel][frame] = value }
    }

    private static func value(_ buffer: AVAudioPCMBuffer, channel: Int, frame: Int) -> Float {
        if buffer.format.isInterleaved {
            return buffer.floatChannelData![0][frame * Int(buffer.format.channelCount) + channel]
        }
        return buffer.floatChannelData![channel][frame]
    }

    private static func verifyCopy(_ plan: AudioRecordingChannelPlan, source: AVAudioPCMBuffer,
                                   expectedChannels: [Int]) throws {
        let copied = try plan.copy(source)
        try check(Int(copied.format.channelCount) == expectedChannels.count, "Extracted PCM must have the selected channel count.")
        try check(copied.frameLength == source.frameLength && copied.format.sampleRate == source.format.sampleRate,
                  "Extraction must preserve frame count and sample rate.")
        for (channel, expected) in expectedChannels.enumerated() {
            for frame in 0..<frameCount {
                try check(value(copied, channel: channel, frame: frame) == signal(channel: expected, frame: frame),
                          "Extracted channel \(channel), frame \(frame) did not preserve the selected source PCM.")
            }
        }
    }

    private static func file(for plan: AudioRecordingChannelPlan, url: URL) throws -> AVAudioFile {
        try AVAudioFile(forWriting: url, settings: [AVFormatIDKey: kAudioFormatLinearPCM,
            AVSampleRateKey: plan.outputFormat.sampleRate, AVNumberOfChannelsKey: plan.outputFormat.channelCount,
            AVLinearPCMBitDepthKey: 24, AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false],
            commonFormat: .pcmFormatFloat32, interleaved: false)
    }

    private static func record(_ plan: AudioRecordingChannelPlan, source: AVAudioPCMBuffer,
                               expectedChannels: [Int], url: URL, recordingSource: AudioRecording.Source = .input) throws {
        var outputFile: AVAudioFile? = try file(for: plan, url: url)
        let recorder = try AudioRecording(source: recordingSource, file: outputFile!,
                                         path: url.path, instrument: nil, channels: plan)
        outputFile = nil
        let original = (0..<Int(source.format.channelCount)).map { channel in
            (0..<Int(source.frameLength)).map { value(source, channel: channel, frame: $0) }
        }
        // The disk writer cannot run until the tap's original storage has been
        // overwritten. Correct PCM on disk proves receive owns an immediate copy.
        recorder.queue.suspend()
        recorder.receive(source)
        recorder.receive(source)
        for channel in original.indices {
            for frame in original[channel].indices { set(source, channel: channel, frame: frame, value: 0) }
        }
        recorder.queue.resume()
        recorder.queue.sync { recorder.close() }
        for channel in original.indices {
            for frame in original[channel].indices { set(source, channel: channel, frame: frame, value: original[channel][frame]) }
        }
        if let error = recorder.error { throw error }
        try check(recorder.frames == AVAudioFramePosition(frameCount * 2), "Written frame count must accumulate across callbacks.")
        try check(recorder.channelCount == expectedChannels.count && recorder.sampleRate == sampleRate && recorder.path == url.path,
                  "Recorder metadata must describe the actual output file.")
        let read = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
        try check(Int(read.fileFormat.channelCount) == expectedChannels.count,
                  "WAV file must store exactly the selected channel count, not the tap's device channel count.")
        try check((read.fileFormat.settings[AVLinearPCMBitDepthKey] as? NSNumber)?.intValue == 24,
                  "Generated recording must be a 24-bit PCM WAV.")
        try check(read.length == AVAudioFramePosition(frameCount * 2), "WAV header must contain the complete frame count after closing.")
        guard let decoded = AVAudioPCMBuffer(pcmFormat: read.processingFormat, frameCapacity: AVAudioFrameCount(read.length)) else {
            throw failure("Cannot allocate WAV verification buffer.")
        }
        try read.read(into: decoded)
        try check(Int(decoded.frameLength) == frameCount * 2, "WAV read must return every recorded frame.")
        for (channel, expected) in expectedChannels.enumerated() {
            for frame in 0..<Int(decoded.frameLength) {
                let delta = abs(value(decoded, channel: channel, frame: frame) - signal(channel: expected, frame: frame % frameCount))
                try check(delta <= 2.0 / 8_388_608, "24-bit WAV round trip changed channel \(channel), frame \(frame).")
            }
        }
    }

    private static func check(_ condition: Bool, _ message: String) throws {
        if !condition { throw failure(message) }
    }

    private static func rejects(_ message: String, action: () throws -> Void) throws {
        do { try action() }
        catch {
            try check(!error.localizedDescription.isEmpty, "Rejected recording input must explain the failure.")
            return
        }
        throw failure(message + " was accepted.")
    }

    private static func failure(_ message: String) -> NSError {
        NSError(domain: "NativeAudioRecordingTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}

#endif

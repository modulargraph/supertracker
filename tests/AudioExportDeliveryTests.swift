import Foundation
import AVFoundation
import CryptoKit
import Darwin

/// Standalone file/SRC fixture. No AVAudioEngine, devices, installed plug-ins,
/// MIDI or user projects. Build only from the agent; root owns native execution.
@main enum AudioExportDeliveryTests {
    typealias D = AudioExportDelivery
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    static var groups: [String] = []
    static var observations: [[String: Any]] = []
    static let files = FileManager.default
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !condition() { throw Failure(message: message) }
    }
    static func rejects(_ label: String, _ body: () throws -> Void) throws {
        do { try body() } catch { return }
        throw Failure(message: "Expected rejection: " + label)
    }
    static func pass(_ name: String) { groups.append(name); print("PASS delivery: " + name) }
    static func write(_ url: URL, left: [Float], right: [Float], rate: Double = 48_000, bits: Int = 32) throws {
        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(left.count))!
        buffer.frameLength = AVAudioFrameCount(left.count)
        for i in left.indices { buffer.floatChannelData![0][i] = left[i]; buffer.floatChannelData![1][i] = right[i] }
        var file: AVAudioFile? = try AVAudioFile(forWriting: url, settings: [AVFormatIDKey: kAudioFormatLinearPCM,
            AVSampleRateKey: rate, AVNumberOfChannelsKey: 2, AVLinearPCMBitDepthKey: bits,
            AVLinearPCMIsFloatKey: bits == 32], commonFormat: .pcmFormatFloat32, interleaved: false)
        try file!.write(from: buffer)
        if #available(macOS 15.0, *) { file?.close() }
        file = nil
    }
    static func read(_ url: URL) throws -> [[Float]] {
        let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
        guard file.length < 1_000_000 else { throw Failure(message: "Fixture unexpectedly exceeds read budget") }
        let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length))!
        try file.read(into: buffer)
        return (0..<2).map { Array(UnsafeBufferPointer(start: buffer.floatChannelData![$0], count: Int(buffer.frameLength))) }
    }
    static func converted(_ source: URL, _ destination: URL, frames: Int, rate: Int,
                          encoding: AudioExportFormat = .float32, inputBlock: Int = 4096, outputBlock: Int = 4096) throws -> [[Float]] {
        let settings = try D.Settings(encoding: encoding, sampleRate: rate)
        let job = try D.Job(sourceURL: source, expectedInputFrames: Int64(frames), destinationURL: destination,
                            settings: settings, inputBlockFrames: inputBlock, outputBlockFrames: outputBlock)
        var calls = 0
        while job.result == nil {
            let progress = try job.advance(maximumCalls: 3); calls += 1
            try check(calls < 1_000_000, "Converter never terminated")
            try check(progress.outputFrames <= progress.totalOutputFrames && progress.inputFrames <= frames, "Invalid progress counters")
            try check(!files.fileExists(atPath: destination.path), "An incomplete job became visible at its destination")
        }
        let result = try job.publish()
        let decoded = try read(destination)
        let file = try AVAudioFile(forReading: destination)
        let format = file.fileFormat.streamDescription.pointee
        try check(decoded[0].count == D.outputFrames(inputFrames: Int64(frames), sampleRate: rate), "Exact ceiling frame count differs")
        try check(file.length == result.outputFrames && format.mSampleRate == Double(rate) && format.mChannelsPerFrame == 2,
                  "Wrong actual WAV rate/channel count/length")
        try check(format.mBitsPerChannel == encoding.bitDepth && (format.mFormatFlags & kAudioFormatFlagIsFloat != 0) == encoding.isFloat,
                  "Wrong actual WAV encoding")
        try check(decoded.joined().allSatisfy(\.isFinite), "Non-finite delivered audio")
        observations.append(["source": source.lastPathComponent, "destination": destination.lastPathComponent,
            "inputFrames": frames, "outputFrames": result.outputFrames, "sampleRate": rate, "encoding": encoding.rawValue,
            "inputBlock": inputBlock, "outputBlock": outputBlock, "advanceCalls": calls])
        return decoded
    }
    static func run(_ root: URL) throws {
        try files.createDirectory(at: root, withIntermediateDirectories: true)
        for (input, rate, expected): (Int64, Int, Int64) in [(0, 44_100, 0), (1, 44_100, 1), (159, 44_100, 147),
            (160, 44_100, 147), (161, 44_100, 148), (6007, 96_000, 12014), (Int64.max, 48_000, Int64.max)] {
            try check(try D.outputFrames(inputFrames: input, sampleRate: rate) == expected, "Rational frame ceiling changed")
        }
        try rejects("overflow") { _ = try D.outputFrames(inputFrames: .max, sampleRate: 96_000) }
        try rejects("negative frames") { _ = try D.outputFrames(inputFrames: -1, sampleRate: 48_000) }
        try rejects("unknown rate") { _ = try D.Settings(encoding: .pcm24, sampleRate: 88_200) }
        try rejects("RIFF bound") { _ = try D.preflight(inputFrames: 1_000_000_000, settings: D.Settings(encoding: .float32, sampleRate: 96_000)) }
        let musical: Int64 = 80, total: Int64 = 160
        let splitTail = try D.outputFrames(inputFrames: total, sampleRate: 44_100) - D.outputFrames(inputFrames: musical, sampleRate: 44_100)
        try check(splitTail == 73 && splitTail != D.outputFrames(inputFrames: total - musical, sampleRate: 44_100),
                  "Absolute endpoint split must remain exact")
        pass("checked rational endpoints, overflow and RIFF admission")

        for count in [1, 2, 3, 159, 160, 161, 4095, 4097] {
            let source = root.appendingPathComponent("tiny-\(count).caf")
            let left = (0..<count).map { Float(0.25 + 0.1 * sin(Double($0) * 0.1)) }
            let right = left.map { -$0 * 0.7 }
            try write(source, left: left, right: right)
            for rate in [44_100, 48_000, 96_000] {
                for encoding in AudioExportFormat.allCases {
                    _ = try converted(source, root.appendingPathComponent("tiny-\(count)-\(rate)-\(encoding.rawValue).wav"),
                                      frames: count, rate: rate, encoding: encoding, inputBlock: 37, outputBlock: 113)
                }
            }
        }
        pass("all rates/encodings, one-frame and odd-length EOF draining with exact WAV headers")

        let count = 48_017, source = root.appendingPathComponent("source.caf")
        let left = (0..<count).map { i in Float(1.21 * sin(2 * Double.pi * 997 * Double(i) / 48_000) + 0.07 * cos(Double(i) * 0.3)) }
        let right = (0..<count).map { i in Float(0.33 * sin(2 * Double.pi * 311 * Double(i) / 48_000 + 0.2)) }
        try write(source, left: left, right: right)
        let originalBytes = try Data(contentsOf: source)
        for rate in [44_100, 48_000, 96_000] {
            let first = try converted(source, root.appendingPathComponent("block-\(rate)-a.wav"), frames: count, rate: rate)
            let second = try converted(source, root.appendingPathComponent("block-\(rate)-b.wav"), frames: count, rate: rate, inputBlock: 37, outputBlock: 113)
            let third = try converted(source, root.appendingPathComponent("block-\(rate)-c.wav"), frames: count, rate: rate, inputBlock: 8192, outputBlock: 997)
            for candidate in [second, third] {
                let error = zip(first.joined(), candidate.joined()).reduce(Float(0)) { max($0, abs($1.0 - $1.1)) }
                try check(error < 0.000002, "Chunking changed SRC by \(error) at \(rate) Hz")
                observations.append(["check": "chunk invariance", "rate": rate, "maximumError": error])
            }
            try check(first[0].map(abs).max()! > 1.1, "Float32 overshoot was clipped or normalized")
            if rate == 48_000 {
                try check(zip(left, first[0]).allSatisfy { $0.bitPattern == $1.bitPattern } && zip(right, first[1]).allSatisfy { $0.bitPattern == $1.bitPattern }, "48 kHz Float32 delivery is not bit-exact")
            }
        }
        try check(try Data(contentsOf: source) == originalBytes, "The intermediate source was modified")
        pass("chunk-invariant persistent SRC, Float32 overshoot and bit-exact 48 kHz bypass")

        let impulseURL = root.appendingPathComponent("impulse.caf")
        var impulseL = [Float](repeating: 0, count: 9600), impulseR = impulseL
        impulseL[3200] = 1; impulseR[4800] = 0.5
        try write(impulseURL, left: impulseL, right: impulseR)
        for rate in [44_100, 48_000, 96_000] {
            let samples = try converted(impulseURL, root.appendingPathComponent("impulse-\(rate).wav"), frames: 9600, rate: rate)
            for channel in 0..<2 {
                let peak = samples[channel].indices.max { abs(samples[channel][$0]) < abs(samples[channel][$1]) }!
                let expected = (channel == 0 ? 3200 : 4800) * rate / 48_000
                try check(peak == expected, "Priming shifted impulse from \(expected) to \(peak) at \(rate)")
                observations.append(["check": "impulse alignment", "rate": rate, "channel": channel, "peakFrame": peak])
            }
        }
        pass("zero-origin impulse timing and independent stereo/stem phase alignment")

        let toneURL = root.appendingPathComponent("tones.caf"), toneCount = 48_000
        let low = (0..<toneCount).map { Float(0.5 * sin(2 * Double.pi * 1000 * Double($0) / 48_000)) }
        let high = (0..<toneCount).map { Float(0.5 * sin(2 * Double.pi * 23_000 * Double($0) / 48_000)) }
        try write(toneURL, left: low, right: high)
        let reduced = try converted(toneURL, root.appendingPathComponent("tones-44100.wav"), frames: toneCount, rate: 44_100)
        let interior = 2000..<(reduced[0].count - 2000)
        let mse = interior.reduce(0.0) { sum, i in let expected = 0.5 * sin(2 * Double.pi * 1000 * Double(i) / 44_100); return sum + pow(Double(reduced[0][i]) - expected, 2) } / Double(interior.count)
        let aliasRMS = sqrt(interior.reduce(0.0) { $0 + pow(Double(reduced[1][$1]), 2) } / Double(interior.count))
        try check(sqrt(mse) < 0.001, "1 kHz SRC phase/amplitude error is \(sqrt(mse))")
        try check(aliasRMS < 0.00035, "23 kHz anti-alias rejection is insufficient: RMS \(aliasRMS)")
        observations.append(["check": "tone quality", "toneRMSError": sqrt(mse), "aliasRMS": aliasRMS])
        for encoding in [AudioExportFormat.pcm16, .pcm24] {
            let integer = try converted(toneURL, root.appendingPathComponent("tones-44100-\(encoding.rawValue).wav"), frames: toneCount, rate: 44_100, encoding: encoding)
            let tolerance = 2 / pow(2.0, Double(encoding.bitDepth - 1)) + 0.0000001
            let error = zip(reduced.joined(), integer.joined()).reduce(0.0) { max($0, abs(Double($1.0) - Double($1.1))) }
            try check(error <= tolerance, "Integer delivery exceeds quantization tolerance")
        }
        pass("passband fidelity, >60 dB stopband alias rejection and one final integer quantization")

        let cancelDestination = root.appendingPathComponent("cancel.wav")
        let cancel = try D.Job(sourceURL: source, expectedInputFrames: Int64(count), destinationURL: cancelDestination,
                               settings: D.Settings(encoding: .pcm24, sampleRate: 96_000), inputBlockFrames: 37, outputBlockFrames: 113)
        try cancel.advance(maximumCalls: 1); cancel.discard()
        try check(!files.fileExists(atPath: cancelDestination.path), "Cancellation published an incomplete destination")
        try rejects("canceled job") { try cancel.advance() }
        let collision = root.appendingPathComponent("collision.wav"), sentinel = Data("existing file must survive".utf8)
        let job = try D.Job(sourceURL: source, expectedInputFrames: Int64(count), destinationURL: collision, settings: D.Settings(encoding: .float32))
        while job.result == nil { try job.advance() }
        try sentinel.write(to: collision)
        try rejects("late target collision") { try job.publish() }; job.discard()
        try check(try Data(contentsOf: collision) == sentinel, "Late collision damaged existing bytes")
        try rejects("existing target") { _ = try D.Job(sourceURL: source, expectedInputFrames: Int64(count), destinationURL: collision, settings: D.Settings(encoding: .pcm16)) }
        pass("resumable cancellation, exclusive inner publication and existing-file preservation")

        try rejects("source extent mismatch") { _ = try D.Job(sourceURL: source, expectedInputFrames: Int64(count + 1), destinationURL: cancelDestination, settings: D.Settings(encoding: .pcm24)) }
        try rejects("zero capacity") { _ = try D.Job(sourceURL: source, expectedInputFrames: Int64(count), destinationURL: cancelDestination, settings: D.Settings(encoding: .pcm24), inputBlockFrames: 0) }
        try rejects("source destination alias") { _ = try D.Job(sourceURL: source, expectedInputFrames: Int64(count), destinationURL: source, settings: D.Settings(encoding: .pcm24)) }
        let wrongRate = root.appendingPathComponent("wrong-rate.caf")
        try write(wrongRate, left: [0.1, 0.2], right: [0.1, 0.2], rate: 44_100)
        try rejects("non48k source") { _ = try D.Job(sourceURL: wrongRate, expectedInputFrames: 2, destinationURL: cancelDestination, settings: D.Settings(encoding: .pcm24)) }
        let bad = root.appendingPathComponent("nan.caf")
        try write(bad, left: [.nan, 0.2], right: [0.1, 0.2])
        let invalid = try D.Job(sourceURL: bad, expectedInputFrames: 2, destinationURL: cancelDestination, settings: D.Settings(encoding: .float32, sampleRate: 96_000))
        try rejects("nonfinite PCM") { try invalid.advance() }
        try check(!files.fileExists(atPath: cancelDestination.path), "Failed delivery became visible")
        let leftover = try files.contentsOfDirectory(atPath: root.path).filter { $0.hasPrefix(".delivery-") }
        try check(leftover.isEmpty, "Private delivery directories leaked: \(leftover)")
        try check(try Data(contentsOf: source) == originalBytes, "Failure/cancellation changed original source bytes")
        pass("malformed sources, finite PCM and complete owned-temporary cleanup")
    }
    static func main() {
        let root = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "test-output/export-delivery/native-run", isDirectory: true)
        var failure: String?
        do { try run(root) } catch { failure = error.localizedDescription; fputs("FAIL delivery: \(failure!)\n", stderr) }
        let report: [String: Any] = ["complete": failure == nil, "groups": groups, "observations": observations,
            "error": failure as Any? ?? NSNull(), "audioEngine": false, "devices": false, "installedPlugins": false]
        try? files.createDirectory(at: root, withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: root.appendingPathComponent("report.json"), options: .atomic)
        }
        if failure != nil { exit(1) }
        print("PASS \(groups.count) delivery groups; file conversion only")
    }
}

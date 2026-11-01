import Foundation
import AVFoundation
import Darwin

/// Offline file delivery only. The engine, transport and plug-in graph stay at
/// 48 kHz. Jobs are single-owner control-thread objects, never render callbacks.
enum AudioExportDelivery {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { "Audio delivery: " + message }
    }
    struct Settings {
        let encoding: AudioExportFormat
        let sampleRate: Int
        init(encoding: AudioExportFormat, sampleRate: Int = 48_000) throws {
            guard [44_100, 48_000, 96_000].contains(sampleRate) else { throw fail("Choose 44.1, 48 or 96 kHz.") }
            self.encoding = encoding; self.sampleRate = sampleRate
        }
        var fileSettings: [String: Any] {
            [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: sampleRate,
             AVNumberOfChannelsKey: 2, AVLinearPCMBitDepthKey: encoding.bitDepth,
             AVLinearPCMIsFloatKey: encoding.isFloat, AVLinearPCMIsBigEndianKey: false]
        }
        var metadata: [String: Any] {
            ["container": "wav", "encoding": encoding.rawValue, "sampleRate": sampleRate,
             "bitDepth": encoding.bitDepth, "isFloat": encoding.isFloat, "renderSampleRate": 48_000]
        }
    }
    struct Progress {
        let inputFrames: Int64
        let outputFrames: Int64
        let totalOutputFrames: Int64
        let complete: Bool
    }
    struct Result {
        let inputFrames: Int64
        let outputFrames: Int64
        let settings: Settings
        var metadata: [String: Any] {
            var result = settings.metadata
            result["inputFrames"] = inputFrames; result["outputFrames"] = outputFrames
            return result
        }
    }

    /// Exact ceiling of the absolute endpoint. A musical/tail split must use
    /// Q(musicalFrames) and Q(totalFrames)-Q(musicalFrames), not rounded pieces.
    static func outputFrames(inputFrames: Int64, sampleRate: Int) throws -> Int64 {
        guard inputFrames >= 0 else { throw fail("Negative source frame count.") }
        let numerator: Int64, denominator: Int64
        switch sampleRate {
        case 44_100: numerator = 147; denominator = 160
        case 48_000: numerator = 1; denominator = 1
        case 96_000: numerator = 2; denominator = 1
        default: throw fail("Unsupported delivery sample rate.")
        }
        let whole = (inputFrames / denominator).multipliedReportingOverflow(by: numerator)
        let remainder = (inputFrames % denominator * numerator + denominator - 1) / denominator
        let result = whole.partialValue.addingReportingOverflow(remainder)
        guard !whole.overflow, !result.overflow else { throw fail("Delivery frame count exceeds its integer range.") }
        return result.partialValue
    }

    /// Conservative RIFF/WAVE cap, including room for headers/chunks. This does
    /// not claim RF64 support; private source CAF files have no RIFF size limit.
    static func preflight(inputFrames: Int64, settings: Settings) throws -> Int64 {
        guard inputFrames > 0 else { throw fail("The source file is empty.") }
        let count = try outputFrames(inputFrames: inputFrames, sampleRate: settings.sampleRate)
        let size = count.multipliedReportingOverflow(by: Int64(2 * settings.encoding.bitDepth / 8))
        guard !size.overflow, size.partialValue <= Int64(UInt32.max) - 4_096 else {
            throw fail("The requested WAV exceeds the supported RIFF size. Export a shorter range or smaller delivery format.")
        }
        return count
    }

    /// A resumable conversion into an owned sibling directory. Between advance
    /// calls the host can yield and cancel while keeping its export gate active.
    /// publish() only targets an absent inner transaction path. The outer export
    /// transaction owns final destination replacement after transport restoration.
    final class Job {
        let sourceURL: URL
        let destinationURL: URL
        let settings: Settings
        let expectedInputFrames: Int64
        let expectedOutputFrames: Int64
        private(set) var result: Result?
        private(set) var published = false
        private let ownedDirectory: URL
        private let stagingURL: URL
        private var reader: AVAudioFile?
        private var writer: AVAudioFile?
        private var converter: AVAudioConverter?
        private let inputBuffer: AVAudioPCMBuffer
        private let outputBuffer: AVAudioPCMBuffer
        private var inputFrames: Int64 = 0
        private var writtenFrames: Int64 = 0
        private var converterFrames: Int64 = 0
        private var eofOffered = false
        private var inactive = false
        private var inputError: Error?
        private var stalledCalls = 0

        init(sourceURL: URL, expectedInputFrames: Int64, destinationURL: URL, settings: Settings,
             inputBlockFrames: Int = 4_096, outputBlockFrames: Int = 4_096) throws {
            guard sourceURL.isFileURL, destinationURL.isFileURL,
                  sourceURL.standardizedFileURL != destinationURL.standardizedFileURL,
                  destinationURL.pathExtension.lowercased() == "wav",
                  (1...65_536).contains(inputBlockFrames), (1...65_536).contains(outputBlockFrames) else {
                throw fail("Invalid delivery path or block capacity.")
            }
            let files = FileManager.default
            guard !files.fileExists(atPath: destinationURL.path),
                  try destinationURL.deletingLastPathComponent().resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
                throw fail("The private delivery destination must be absent in an existing directory.")
            }
            let outputCount = try preflight(inputFrames: expectedInputFrames, settings: settings)
            let source = try AVAudioFile(forReading: sourceURL, commonFormat: .pcmFormatFloat32, interleaved: false)
            let description = source.fileFormat.streamDescription.pointee
            guard sourceURL.pathExtension.lowercased() == "caf", source.length == expectedInputFrames,
                  description.mFormatID == kAudioFormatLinearPCM, description.mSampleRate == 48_000,
                  description.mChannelsPerFrame == 2, description.mBitsPerChannel == 32,
                  description.mFormatFlags & kAudioFormatFlagIsFloat != 0 else {
                throw fail("Expected the complete stereo Float32/48 kHz CAF render with its exact admitted frame count.")
            }
            guard let targetFormat = AVAudioFormat(standardFormatWithSampleRate: Double(settings.sampleRate), channels: 2),
                  let input = AVAudioPCMBuffer(pcmFormat: source.processingFormat, frameCapacity: AVAudioFrameCount(inputBlockFrames)),
                  let output = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: AVAudioFrameCount(outputBlockFrames)) else {
                throw fail("Cannot allocate bounded delivery buffers.")
            }
            var conversion: AVAudioConverter?
            if settings.sampleRate != 48_000 {
                guard let value = AVAudioConverter(from: source.processingFormat, to: targetFormat) else { throw fail("The selected sample-rate converter is unavailable.") }
                value.sampleRateConverterAlgorithm = AVSampleRateConverterAlgorithm_Mastering
                value.sampleRateConverterQuality = AVAudioQuality.max.rawValue
                // SDK contract: normal priming assumes silent prehistory and
                // generates zero output latency. EOF supplies trailing silence.
                value.primeMethod = .normal
                value.dither = false // Float32 -> Float32; file writer owns quantization.
                conversion = value
            }
            let directory = destinationURL.deletingLastPathComponent().appendingPathComponent(".delivery-" + UUID().uuidString, isDirectory: true)
            try files.createDirectory(at: directory, withIntermediateDirectories: false)
            let staged = directory.appendingPathComponent("audio.wav")
            let destination: AVAudioFile
            do { destination = try AVAudioFile(forWriting: staged, settings: settings.fileSettings, commonFormat: .pcmFormatFloat32, interleaved: false) }
            catch { try? files.removeItem(at: directory); throw error }
            self.sourceURL = sourceURL; self.destinationURL = destinationURL
            self.settings = settings; self.expectedInputFrames = expectedInputFrames; expectedOutputFrames = outputCount
            ownedDirectory = directory; stagingURL = staged; reader = source; writer = destination
            inputBuffer = input; outputBuffer = output; converter = conversion
        }

        deinit { discard() }

        @discardableResult func advance(maximumCalls: Int = 8) throws -> Progress {
            guard (1...256).contains(maximumCalls), !inactive else { throw fail("The delivery job is inactive or its work budget is invalid.") }
            if result != nil { return progress }
            do {
                for _ in 0..<maximumCalls {
                    if let converter {
                        outputBuffer.frameLength = 0
                        let beforeInput = inputFrames, beforeOutput = converterFrames
                        var error: NSError?
                        let status = converter.convert(to: outputBuffer, error: &error) { [self] requested, state in
                            do {
                                if inputFrames == expectedInputFrames { eofOffered = true; state.pointee = .endOfStream; return nil }
                                guard requested > 0 else { state.pointee = .noDataNow; return nil }
                                try readInput(maximum: min(Int(requested), Int(inputBuffer.frameCapacity)))
                                state.pointee = .haveData
                                return inputBuffer
                            } catch { inputError = error; state.pointee = .endOfStream; return nil }
                        }
                        if let error = inputError ?? error { throw error }
                        if status == .error { throw fail("Sample-rate conversion failed.") }
                        try consume(outputBuffer)
                        if status == .endOfStream {
                            guard eofOffered else { throw fail("The converter ended before receiving the complete source.") }
                            try finish(); break
                        }
                        stalledCalls = beforeInput == inputFrames && beforeOutput == converterFrames ? stalledCalls + 1 : 0
                        guard stalledCalls < 8 else { throw fail("The sample-rate converter stopped making progress.") }
                    } else {
                        // A same-rate Float32 delivery is copied exactly; no SRC,
                        // normalization, limiter, or intermediate quantization.
                        if inputFrames == expectedInputFrames { try finish(); break }
                        try readInput(maximum: Int(inputBuffer.frameCapacity))
                        try consume(inputBuffer)
                        if inputFrames == expectedInputFrames { try finish(); break }
                    }
                }
                return progress
            } catch { discard(); throw error }
        }

        /// Synchronous, no await. Exclusivity prevents a late collision from
        /// overwriting a file. The surrounding transaction publishes user paths.
        @discardableResult func publish() throws -> Result {
            guard let result, !inactive, !published else { throw fail("Delivery must finish and verify before publication.") }
            let status = stagingURL.path.withCString { source in destinationURL.path.withCString { target in
                renameatx_np(AT_FDCWD, source, AT_FDCWD, target, UInt32(RENAME_EXCL))
            } }
            guard status == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno), userInfo: [NSLocalizedDescriptionKey: "Cannot publish the private delivery WAV without replacing another file."]) }
            published = true; inactive = true
            // A successful publication is not undone by best-effort removal of
            // its now-empty private directory. Never remove destinationURL.
            try? FileManager.default.removeItem(at: ownedDirectory)
            return result
        }

        /// Cancel/failure/deinit cleanup owns only the private sibling directory.
        /// A published file belongs to its caller and survives Job deallocation.
        func discard() {
            closeFiles(); converter = nil; inactive = true
            try? FileManager.default.removeItem(at: ownedDirectory)
        }

        private var progress: Progress {
            Progress(inputFrames: inputFrames, outputFrames: writtenFrames, totalOutputFrames: expectedOutputFrames, complete: result != nil)
        }
        private func readInput(maximum: Int) throws {
            guard let reader, maximum > 0 else { throw fail("Source input is unavailable.") }
            let count = AVAudioFrameCount(min(Int64(maximum), expectedInputFrames - inputFrames))
            inputBuffer.frameLength = 0
            try reader.read(into: inputBuffer, frameCount: count)
            guard inputBuffer.frameLength == count, count > 0 else { throw fail("The intermediate render changed or ended early.") }
            try requireFinite(inputBuffer)
            inputFrames += Int64(count)
        }
        private func consume(_ buffer: AVAudioPCMBuffer) throws {
            try requireFinite(buffer)
            let available = Int64(buffer.frameLength)
            converterFrames += available
            guard converterFrames <= expectedOutputFrames + 262_144 else {
                throw fail("The converter produced unbounded output after the source endpoint.")
            }
            let keep = min(available, expectedOutputFrames - writtenFrames)
            guard keep >= 0 else { throw fail("Delivery output exceeded its admitted frame count.") }
            if keep > 0 {
                let originalLength = buffer.frameLength
                buffer.frameLength = AVAudioFrameCount(keep)
                defer { buffer.frameLength = originalLength }
                guard let writer else { throw fail("The delivery writer is unavailable.") }
                try writer.write(from: buffer)
                writtenFrames += keep
            }
        }
        private func finish() throws {
            guard inputFrames == expectedInputFrames, writtenFrames == expectedOutputFrames,
                  converterFrames >= expectedOutputFrames else {
                throw fail("The converter did not supply the exact admitted output duration (\(writtenFrames)/\(expectedOutputFrames) frames).")
            }
            closeFiles(); converter = nil
            let file = try AVAudioFile(forReading: stagingURL, commonFormat: .pcmFormatFloat32, interleaved: false)
            let format = file.fileFormat.streamDescription.pointee
            guard file.length == expectedOutputFrames, format.mFormatID == kAudioFormatLinearPCM,
                  format.mSampleRate == Double(settings.sampleRate), format.mChannelsPerFrame == 2,
                  format.mBitsPerChannel == UInt32(settings.encoding.bitDepth),
                  (format.mFormatFlags & kAudioFormatFlagIsFloat != 0) == settings.encoding.isFloat else {
                throw fail("The completed WAV header does not match its delivery settings.")
            }
            let header = try FileHandle(forReadingFrom: stagingURL)
            defer { try? header.close() }
            let signature = try header.read(upToCount: 12) ?? Data()
            guard signature.count == 12, String(data: signature.prefix(4), encoding: .ascii) == "RIFF",
                  String(data: signature.suffix(4), encoding: .ascii) == "WAVE" else { throw fail("The completed delivery is not a RIFF/WAVE file.") }
            result = Result(inputFrames: expectedInputFrames, outputFrames: expectedOutputFrames, settings: settings)
        }
        private func closeFiles() {
            if #available(macOS 15.0, *) { writer?.close(); reader?.close() }
            writer = nil; reader = nil // ARC closes files on the macOS 14 path.
        }
        private func requireFinite(_ buffer: AVAudioPCMBuffer) throws {
            guard let channels = buffer.floatChannelData, buffer.format.channelCount == 2 else { throw fail("Expected a stereo Float32 buffer.") }
            for channel in 0..<2 {
                for frame in 0..<Int(buffer.frameLength) where !channels[channel][frame].isFinite {
                    throw fail("Non-finite audio cannot be delivered.")
                }
            }
        }
    }
    private static func fail(_ message: String) -> Failure { Failure(message: message) }
}

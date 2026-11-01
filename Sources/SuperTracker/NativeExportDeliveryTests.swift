// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox

/// Bounded public-command delivery checks. Invoke after NativeExportFormatTests
/// so its independently generated 48 kHz Float32 files remain the PCM reference.
/// Uses private files, an offline graph and one original in-process delay AU.
enum NativeExportDeliveryTests {
    private static let internalRate = 48_000
    private static let sourceFrames = 24_001
    private static let delayFrames = 65
    private static let component = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x646C3635, componentManufacturer: 0x53744476, componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(ExportDeliveryDelayUnit.self, as: component,
            name: "SuperTracker Test: Delivery 65-frame delay", version: 1)
    }()
    private struct PCM {
        let path: String
        let left: [Float]
        let right: [Float]
    }
    private static func expectedFrames(_ input: Int, _ rate: Int) -> Int {
        (input * rate + internalRate - 1) / internalRate
    }

    @MainActor static func run(output: URL) async throws -> [String] {
        _ = registration
        let fm = FileManager.default, folder = output.appendingPathComponent("export-delivery", isDirectory: true)
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        var passed: [String] = [], observations: [[String: Any]] = [], complete = false
        defer {
            let report: [String: Any] = ["complete": complete, "checks": passed, "observations": observations,
                "physicalAudio": false, "installedPlugins": false, "graphMode": "offline",
                "renderSampleRate": internalRate, "sourceFrames": sourceFrames]
            if let bytes = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? bytes.write(to: folder.appendingPathComponent("report.json"), options: .atomic)
            }
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Export delivery regression: " + message) }
        }
        let source = folder.appendingPathComponent("original-source.wav")
        let left = (0..<24_000).map { index -> Float in
            let time = Double(index) / Double(internalRate)
            return Float(0.37 * sin(2 * .pi * 233 * time) + 0.081 * cos(2 * .pi * 619 * time))
        }
        let right = (0..<24_000).map { index -> Float in
            let time = Double(index) / Double(internalRate)
            return Float(0.31 * sin(2 * .pi * 311 * time + 0.2) + 0.067 * cos(2 * .pi * 997 * time))
        }
        try AudioSamples.write(DecodedSample(path: source.path, sampleRate: Double(internalRate),
            left: left, right: right, originalChannels: 2), to: source)
        let originalSource = try Data(contentsOf: source)
        func project(_ routed: Bool, delayed: Bool = false) -> [String: Any] {
            let rows: [Any] = [["note": 60, "instrument": 0, "velocity": 113, "gate": 4], NSNull(), NSNull(), NSNull()]
            var value: [String: Any] = ["version": 1, "name": "Original delivery fixture", "bpm": 120,
                "linesPerBeat": 4, "masterVolume": 0.71, "activePattern": 0, "loopSong": false, "metronome": false,
                "tracks": [["routingID": "encoding-source", "name": "Original PCM", "instrument": 0,
                    "volume": 0.63, "pan": 0.0, "mute": false, "solo": false, "delay": 0.0, "reverb": 0.0]],
                "instruments": [["type": "sample", "samplePath": source.path, "rootNote": 60, "volume": 0.83,
                    "mode": "oneshot", "attack": 0.0, "decay": 0.0, "sustain": 1.0, "release": 0.001]],
                "patterns": [["id": 0, "length": 4, "steps": [rows], "automation": []]],
                "sequence": [0], "audioClips": [], "audioPlugins": [], "delay": ["mix": 0.0], "reverb": ["mix": 0.0]]
            if routed {
                value["routing"] = ["version": 1, "mixModel": "standard", "buses": [],
                    "outputs": ["track:encoding-source": "master"], "sends": [], "sidechains": []] as [String: Any]
            }
            if delayed {
                let id = "\(component.componentType):\(component.componentSubType):\(component.componentManufacturer)"
                value["audioPlugins"] = [["owner": "track:encoding-source", "track": 0,
                    "kind": "effect", "slot": 0, "component": id]]
            }
            return value
        }
        func inspect(_ path: String, rate: Int, encoding: AudioExportFormat, frames: Int) throws -> PCM {
            let url = URL(fileURLWithPath: path), bytes = try Data(contentsOf: url)
            try require(bytes.count >= 12 && String(data: bytes.prefix(4), encoding: .ascii) == "RIFF" &&
                String(data: bytes.subdata(in: 8..<12), encoding: .ascii) == "WAVE", "Not a RIFF/WAVE file: \(path)")
            let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
            let header = file.fileFormat.streamDescription.pointee
            try require(header.mFormatID == kAudioFormatLinearPCM && header.mSampleRate == Double(rate) &&
                header.mChannelsPerFrame == 2 && header.mBitsPerChannel == UInt32(encoding.bitDepth) &&
                (header.mFormatFlags & kAudioFormatFlagIsFloat != 0) == encoding.isFloat,
                "Actual WAV header differs from requested \(rate) / \(encoding.rawValue).")
            try require(file.length == frames, "Expected Q(N)=\(frames), got \(file.length) frames at \(rate).")
            guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(frames)) else {
                throw AudioSampleError.invalid("Cannot allocate delivery inspection buffer.")
            }
            try file.read(into: buffer)
            try require(Int(buffer.frameLength) == frames, "Short file read.")
            let channels = buffer.floatChannelData!
            let pcm = PCM(path: path, left: Array(UnsafeBufferPointer(start: channels[0], count: frames)),
                right: Array(UnsafeBufferPointer(start: channels[1], count: frames)))
            let all = pcm.left + pcm.right, peak = all.reduce(Float(0)) { max($0, abs($1)) }
            try require(all.allSatisfy(\.isFinite) && peak > 0.01 && peak < 0.8, "Output must be finite, audible and unclipped.")
            observations.append(["path": path, "sampleRate": rate, "encoding": encoding.rawValue,
                "headerBits": header.mBitsPerChannel, "frames": frames, "peak": peak])
            return pcm
        }
        func difference(_ a: PCM, _ b: PCM, prefix: Int? = nil) -> Double {
            let count = prefix ?? a.left.count
            guard a.left.count >= count, b.left.count >= count else { return .infinity }
            var result = 0.0
            for index in 0..<count {
                result = max(result, abs(Double(a.left[index]) - Double(b.left[index])))
                result = max(result, abs(Double(a.right[index]) - Double(b.right[index])))
            }
            return result
        }
        func tree(_ directory: URL) throws -> [String: Data] {
            var result: [String: Data] = [:]
            guard let entries = fm.enumerator(at: directory, includingPropertiesForKeys: [.isRegularFileKey, .isDirectoryKey]) else { return result }
            for case let url as URL in entries {
                let attributes = try url.resourceValues(forKeys: [.isRegularFileKey, .isDirectoryKey])
                if attributes.isRegularFile == true {
                    result[String(url.path.dropFirst(directory.path.count))] = try Data(contentsOf: url)
                } else if attributes.isDirectory == true {
                    result[String(url.path.dropFirst(directory.path.count)) + "/"] = Data()
                }
            }
            return result
        }
        for routed in [false, true] {
            let label = routed ? "routed" : "legacy", directory = folder.appendingPathComponent(label, isDirectory: true)
            try fm.createDirectory(at: directory, withIntermediateDirectories: true)
            let graph = AVAudioEngine(), format = AVAudioFormat(standardFormatWithSampleRate: Double(internalRate), channels: 2)!
            try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
            let host = try AudioHost(audioGraph: graph, midiInputIDs: [])
            defer { host.shutdown() }
            func apply(delayed: Bool = false) async throws {
                _ = try await host.handle("engine.project", payload: ["project": project(routed, delayed: delayed), "replace": true, "awaitPlugins": true])
                await host.pluginRestorationBarrier()?.value
            }
            func render(_ rate: Int, _ encoding: AudioExportFormat, mode: String = "song", delayed: Bool = false, suffix: String) async throws -> ([String: Any], [PCM]) {
                try await apply(delayed: delayed)
                let destination = directory.appendingPathComponent(suffix + (mode == "stems" ? "" : ".wav"), isDirectory: mode == "stems")
                var payload: [String: Any] = ["mode": mode, "path": destination.path, "sampleRate": rate,
                    "encoding": encoding.rawValue, "tail": 1.0 / Double(internalRate)]
                if mode == "stems" { payload["owners"] = ["track:encoding-source", "master"] }
                guard let result = try await host.handle("audio.export", payload: payload) as? [String: Any] else {
                    throw AudioSampleError.invalid("Missing delivery completion metadata.")
                }
                try require((result["sampleRate"] as? NSNumber)?.intValue == rate && result["encoding"] as? String == encoding.rawValue &&
                    result["bitDepth"] as? Int == encoding.bitDepth && result["isFloat"] as? Bool == encoding.isFloat,
                    "Completion metadata does not match requested delivery.")
                try require((result["sourceFrames"] as? NSNumber)?.intValue == sourceFrames,
                    "Completion must retain the exact musical-plus-tail extent in 48 kHz source frames.")
                let paths = result["paths"] as? [String] ?? []
                try require(paths.count == (mode == "stems" ? 2 : 1), "Incorrect output file count.")
                try require(!graph.isRunning && graph.isInManualRenderingMode && graph.manualRenderingFormat.sampleRate == Double(internalRate),
                    "Delivery changed the stopped graph or internal render rate.")
                return (result, try paths.map { try inspect($0, rate: rate, encoding: encoding, frames: expectedFrames(sourceFrames, rate)) })
            }
            var floatReferences: [Int: PCM] = [:]
            for rate in [44_100, 48_000, 96_000] {
                for encoding in [AudioExportFormat.float32, .pcm16, .pcm24] {
                    let (_, files) = try await render(rate, encoding, suffix: "song-\(rate)-\(encoding.rawValue)")
                    if encoding == .float32 { floatReferences[rate] = files[0] }
                    else {
                        let error = difference(files[0], floatReferences[rate]!)
                        let tolerance = 2 / pow(2, Double(encoding.bitDepth - 1)) + 0.0000001
                        try require(error <= tolerance, "\(rate)/\(encoding.rawValue) quantization exceeded \(tolerance): \(error).")
                    }
                }
            }
            passed.append("\(label): nine rate/encoding combinations have actual stereo WAV headers, Q(24001) frames, finite audio and bounded quantization")
            let referencePath = output.appendingPathComponent("export-format/\(label)/song-float32.wav").path
            let reference = try inspect(referencePath, rate: internalRate, encoding: .float32, frames: 24_000)
            try require(difference(floatReferences[internalRate]!, reference, prefix: 24_000) == 0,
                "48 kHz delivery changed the existing encoding fixture's exact Float32 PCM.")
            passed.append("\(label): 48 kHz delivery is sample-identical to existing NativeExportFormatTests reference PCM")
            // Pattern selection uses the same delivery boundary, exercised at the fractional endpoint rate.
            let (_, pattern) = try await render(44_100, .float32, mode: "pattern", suffix: "pattern-44100")
            try require(difference(pattern[0], floatReferences[44_100]!) == 0, "Pattern export changed the identical one-pattern song.")
            passed.append("\(label): pattern delivery uses the same resampling and exact endpoint as its one-pattern song")
            if routed {
                var arrivals: [String: Int] = [:]
                for rate in [44_100, 48_000, 96_000] {
                    let (_, dry) = try await render(rate, .float32, mode: "stems", suffix: "stems-dry-\(rate)")
                    let (result, delayed) = try await render(rate, .float32, mode: "stems", delayed: true, suffix: "stems-delay-\(rate)")
                    let stems = result["stems"] as? [[String: Any]] ?? []
                    try require(stems.count == 2 && result["renderPasses"] as? Int == 1, "Selected stems must be captured simultaneously.")
                    for (index, stem) in stems.enumerated() {
                        let owner = stem["owner"] as? String ?? "", arrival = (stem["arrivalFrames"] as? NSNumber)?.intValue ?? -1
                        try require((stem["frames"] as? NSNumber)?.intValue == expectedFrames(sourceFrames, rate) &&
                            (stem["sourceFrames"] as? NSNumber)?.intValue == sourceFrames &&
                            (stem["sampleRate"] as? NSNumber)?.intValue == rate &&
                            (stem["arrivalSampleRate"] as? NSNumber)?.intValue == internalRate,
                            "Stem delivery/source/arrival metadata has inconsistent units.")
                        if let original = arrivals[owner] { try require(arrival == original, "PDC arrival was rerounded into delivery samples.") }
                        else { arrivals[owner] = arrival }
                        try require(arrival >= delayFrames, "The original delayed insert must exercise nonzero 48 kHz arrival metadata.")
                        try require(difference(dry[index], delayed[index]) < 0.000003,
                            "PDC or delivery shifted \(owner) at \(rate) compared with its latency-free reference.")
                    }
                    observations.append(["kind": "simultaneous-stems", "sampleRate": rate, "stems": stems])
                }
                passed.append("Routed simultaneous stems at all rates retain 65-frame PDC in 48 kHz units and match aligned latency-free PCM")
            }
            try await apply()
            let bounce = directory.appendingPathComponent("bounce-known-delivery-ignored.wav")
            _ = try await host.handle("audio.bounceTrack", payload: ["path": bounce.path, "track": 0,
                "tail": 0, "sampleRate": 96_000, "encoding": "pcm16"])
            _ = try inspect(bounce.path, rate: internalRate, encoding: .float32, frames: 24_000)
            passed.append("\(label): editable bounce remains Float32/48 kHz when given valid 96 kHz/PCM16 delivery choices")

            try await apply()
            let existing = directory.appendingPathComponent("preserved.wav")
            try Data(contentsOf: URL(fileURLWithPath: floatReferences[internalRate]!.path)).write(to: existing)
            let original = try Data(contentsOf: existing)
            let snapshot = try JSONSerialization.data(withJSONObject: try await host.handle("engine.snapshot", payload: [:]), options: .sortedKeys)
            for rate: Any in [32_000, 88_200, 44_100.5, true, "44100", NSNull()] {
                for destination in [existing, directory.appendingPathComponent("invalid-new.wav")] {
                    var failed = false
                    do { _ = try await host.handle("audio.export", payload: ["mode": "song", "path": destination.path, "sampleRate": rate, "tail": 0]) }
                    catch { failed = true }
                    try require(failed, "An invalid sample rate was accepted: \(rate).")
                    try require(try Data(contentsOf: existing) == original, "Validation overwrote an existing delivery.")
                    try require(!fm.fileExists(atPath: directory.appendingPathComponent("invalid-new.wav").path), "Validation published a partial file.")
                }
            }
            let invalidDestination = directory.appendingPathComponent("destination-is-folder", isDirectory: true)
            try fm.createDirectory(at: invalidDestination, withIntermediateDirectories: true)
            try Data("preserve unrelated file".utf8).write(to: invalidDestination.appendingPathComponent("sentinel"))
            let beforeFailure = try tree(directory)
            var rejected = false
            do { _ = try await host.handle("audio.export", payload: ["mode": "song", "path": invalidDestination.path, "sampleRate": 44_100, "tail": 0]) }
            catch { rejected = true }
            try require(rejected && (try tree(directory)) == beforeFailure, "A failed destination changed files or left staging artifacts.")
            let after = try JSONSerialization.data(withJSONObject: try await host.handle("engine.snapshot", payload: [:]), options: .sortedKeys)
            try require(snapshot == after, "Invalid or failed exports changed the project.")
            passed.append("\(label): invalid delivery rates and failed destination preserve existing bytes, absent targets, project and staging cleanliness")

            if routed {
                // A nonempty stems destination is never replaced. Its refusal is
                // separate from cancellation after an empty directory is reserved.
                let refused = directory.appendingPathComponent("nonempty-stems", isDirectory: true)
                try fm.createDirectory(at: refused, withIntermediateDirectories: true)
                try original.write(to: refused.appendingPathComponent("01-Original PCM.wav"))
                try Data("existing stem folder".utf8).write(to: refused.appendingPathComponent("sentinel.txt"))
                let beforeRefusal = try tree(directory)
                var reachedProgress = false, refusedNonempty = false
                host.onEvent = { event, _ in if event == "audio.exportProgress" { reachedProgress = true } }
                do {
                    _ = try await host.handle("audio.export", payload: ["mode": "stems", "path": refused.path,
                        "owners": ["track:encoding-source", "master"], "sampleRate": 96_000, "encoding": "pcm24", "tail": 2])
                } catch { refusedNonempty = true }
                host.onEvent = nil
                try require(refusedNonempty && !reachedProgress && (try tree(directory)) == beforeRefusal,
                    "A nonempty stem folder must be refused before progress and remain byte-for-byte intact.")
                passed.append("Routed nonempty stem destination is refused before progress without modifying existing files")
            }
            // A long enough tail reaches the production progress/yield seam.
            // Legacy cancellation waits for conversion, after native rendering;
            // routed cancellation starts with an existing empty reservation.
            let cancelPath = routed ? directory.appendingPathComponent("cancel-stems", isDirectory: true) : existing
            if routed {
                try fm.createDirectory(at: cancelPath, withIntermediateDirectories: true)
                try Data("unrelated sibling must survive".utf8).write(to: directory.appendingPathComponent("cancel-outside-sentinel.txt"))
            }
            let beforeCancel = try tree(directory)
            var requested = false, cancelError: String?, cancellation: Task<Void, Never>?
            host.onEvent = { event, payload in
                guard event == "audio.exportProgress", !requested else { return }
                guard routed || payload["phase"] as? String == "delivery" else { return }
                requested = true
                cancellation = Task(priority: .high) { @MainActor in
                    do { _ = try await host.handle("transport.stop", payload: [:]) }
                    catch { cancelError = error.localizedDescription }
                }
            }
            var failure: String?
            do {
                var payload: [String: Any] = ["mode": routed ? "stems" : "song", "path": cancelPath.path,
                    "sampleRate": 96_000, "encoding": "pcm24", "tail": 2]
                if routed { payload["owners"] = ["track:encoding-source", "master"] }
                _ = try await host.handle("audio.export", payload: payload)
            } catch { failure = error.localizedDescription }
            await cancellation?.value; host.onEvent = nil
            try require(requested && cancelError == nil && failure?.localizedCaseInsensitiveContains("cancel") == true,
                "Progress cancellation did not complete: \(failure ?? "success") / \(cancelError ?? "no Stop error").")
            try require(try tree(directory) == beforeCancel, "Cancellation changed existing output or published partial files/staging.")
            try require(!graph.isRunning && host.status()["playing"] as? Bool == false, "Cancellation restarted the stopped graph.")
            passed.append("\(label): \(routed ? "render cancellation preserves an empty reserved stem directory and unrelated sibling" : "conversion-phase cancellation preserves the existing WAV") with no partial publication or staging artifacts")
        }
        try require(try Data(contentsOf: source) == originalSource, "Delivery modified source audio.")
        complete = true
        return passed
    }
}

/// Original 65-frame pipeline: its noninteger delivered length at 44.1 kHz
/// catches accidental PDC rescaling before the common file conversion boundary.
private final class ExportDeliveryDelayUnit: AUAudioUnit {
    private var inputs: AUAudioUnitBusArray!, outputs: AUAudioUnitBusArray!
    private var scratch: AVAudioPCMBuffer!
    private let left = UnsafeMutablePointer<Float>.allocate(capacity: 65)
    private let right = UnsafeMutablePointer<Float>.allocate(capacity: 65)
    private var cursor = 0
    override init(componentDescription: AudioComponentDescription, options: AudioComponentInstantiationOptions = []) throws {
        left.initialize(repeating: 0, count: 65); right.initialize(repeating: 0, count: 65)
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [try AUAudioUnitBus(format: format)])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [try AUAudioUnitBus(format: format)])
        maximumFramesToRender = 65_536
    }
    deinit { left.deinitialize(count: 65); right.deinitialize(count: 65); left.deallocate(); right.deallocate() }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var canProcessInPlace: Bool { true }
    override var latency: TimeInterval { 65.0 / 48_000 }
    override func allocateRenderResources() throws {
        try super.allocateRenderResources()
        scratch = AVAudioPCMBuffer(pcmFormat: outputs[0].format, frameCapacity: maximumFramesToRender)!
        reset()
    }
    override func reset() {
        super.reset(); cursor = 0
        for index in 0..<65 { left[index] = 0; right[index] = 0 }
    }
    override var internalRenderBlock: AUInternalRenderBlock {
        { [unowned self] flags, time, frames, bus, output, _, pull in
            guard bus == 0, let pull, frames <= self.maximumFramesToRender else { return kAudioUnitErr_TooManyFramesToProcess }
            let buffers = UnsafeMutableAudioBufferListPointer(output)
            guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
            for channel in 0..<2 where buffers[channel].mData == nil {
                buffers[channel].mData = UnsafeMutableRawPointer(self.scratch.floatChannelData![channel])
                buffers[channel].mDataByteSize = frames * 4
            }
            let result = pull(flags, time, frames, 0, output)
            guard result == noErr else { return result }
            guard let a = buffers[0].mData?.assumingMemoryBound(to: Float.self), let b = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            let silent = flags.pointee.contains(.unitRenderAction_OutputIsSilence)
            for frame in 0..<Int(frames) {
                let oldA = self.left[self.cursor], oldB = self.right[self.cursor]
                self.left[self.cursor] = silent ? 0 : a[frame]; self.right[self.cursor] = silent ? 0 : b[frame]
                a[frame] = oldA; b[frame] = oldB; self.cursor = (self.cursor + 1) % 65
            }
            flags.pointee.remove(.unitRenderAction_OutputIsSilence)
            return noErr
        }
    }
}

#endif

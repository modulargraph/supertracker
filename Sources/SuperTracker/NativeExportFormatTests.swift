// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Actual host/writer acceptance with original PCM and an injected offline graph.
/// No installed plug-ins, external MIDI destinations, or physical audio devices.
enum NativeExportFormatTests {
    private struct Rendered {
        let path: String
        let samples: [Float]
    }

    @MainActor static func run(output: URL) async throws -> [String] {
        let files = FileManager.default
        let folder = output.appendingPathComponent("export-format", isDirectory: true)
        try files.createDirectory(at: folder, withIntermediateDirectories: true)
        let rate = 48_000.0, frames = 24_000
        let sourceURL = folder.appendingPathComponent("original-source.wav")
        let left = (0..<frames).map { index -> Float in
            let time = Double(index) / rate
            return Float(0.37 * sin(2 * .pi * 233 * time) + 0.081 * cos(2 * .pi * 619 * time))
        }
        let right = (0..<frames).map { index -> Float in
            let time = Double(index) / rate
            return Float(0.31 * sin(2 * .pi * 311 * time + 0.2) + 0.067 * cos(2 * .pi * 997 * time))
        }
        try AudioSamples.write(DecodedSample(path: sourceURL.path, sampleRate: rate, left: left, right: right, originalChannels: 2), to: sourceURL)
        let sourceBytes = try Data(contentsOf: sourceURL)
        var passed: [String] = [], observations: [[String: Any]] = []
        var complete = false
        defer {
            let report: [String: Any] = ["complete": complete, "checks": passed, "observations": observations,
                "physicalAudio": false, "installedPlugins": false, "graphMode": "offline", "sampleRate": rate]
            if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("report.json"), options: .atomic)
            }
        }
        func require(_ value: Bool, _ message: String) throws {
            if !value { throw AudioSampleError.invalid("Export encoding regression: " + message) }
        }
        func project(routed: Bool) -> [String: Any] {
            let rows: [Any] = [["note": 60, "instrument": 0, "velocity": 113, "gate": 4], NSNull(), NSNull(), NSNull()]
            var result: [String: Any] = ["version": 1, "name": "Original encoding fixture", "bpm": 120,
                "linesPerBeat": 4, "masterVolume": 0.71, "activePattern": 0, "loopSong": false, "metronome": false,
                "tracks": [["routingID": "encoding-source", "name": "Original PCM", "instrument": 0,
                    "volume": 0.63, "pan": 0.0, "mute": false, "solo": false, "delay": 0.0, "reverb": 0.0]],
                "instruments": [["type": "sample", "samplePath": sourceURL.path, "rootNote": 60, "volume": 0.83,
                    "mode": "oneshot", "attack": 0.0, "decay": 0.0, "sustain": 1.0, "release": 0.001]],
                "patterns": [["id": 0, "length": 4, "steps": [rows], "automation": []]], "sequence": [0],
                "audioPlugins": [], "audioClips": [], "delay": ["mix": 0.0], "reverb": ["mix": 0.0]]
            if routed {
                result["routing"] = ["version": 1, "mixModel": "standard", "buses": [],
                    "outputs": ["track:encoding-source": "master"], "sends": [], "sidechains": []] as [String: Any]
            }
            return result
        }
        func inspect(_ path: String, encoding: AudioExportFormat) throws -> Rendered {
            let url = URL(fileURLWithPath: path), bytes = try Data(contentsOf: url)
            try require(bytes.count >= 12 && String(data: bytes.prefix(4), encoding: .ascii) == "RIFF" &&
                String(data: bytes.subdata(in: 8..<12), encoding: .ascii) == "WAVE", "The file is not a RIFF/WAVE container: \(path).")
            let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
            let header = file.fileFormat.streamDescription.pointee
            try require(header.mFormatID == kAudioFormatLinearPCM && header.mBitsPerChannel == UInt32(encoding.bitDepth),
                "Header bit depth does not match \(encoding.rawValue): \(header).")
            try require((header.mFormatFlags & kAudioFormatFlagIsFloat != 0) == encoding.isFloat,
                "Header integer/float representation does not match \(encoding.rawValue).")
            try require(header.mSampleRate == rate && header.mChannelsPerFrame == 2 && file.length == frames,
                "The actual file must contain exactly \(frames) stereo frames at 48 kHz: \(path).")
            let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(frames))!
            try file.read(into: buffer)
            try require(Int(buffer.frameLength) == frames, "The entire file could not be decoded.")
            let channels = buffer.floatChannelData!
            let samples = Array(UnsafeBufferPointer(start: channels[0], count: frames)) + Array(UnsafeBufferPointer(start: channels[1], count: frames))
            try require(samples.allSatisfy(\.isFinite), "The output contains non-finite samples.")
            let peak = samples.reduce(Float(0)) { max($0, abs($1)) }
            try require(peak > 0.01 && peak < 0.8, "The deterministic fixture must be audible and unclipped (peak \(peak)).")
            observations.append(["path": path, "encoding": encoding.rawValue, "headerBits": header.mBitsPerChannel,
                "headerFloat": header.mFormatFlags & kAudioFormatFlagIsFloat != 0, "frames": file.length, "peak": peak])
            return Rendered(path: path, samples: samples)
        }
        for routed in [false, true] {
            let name = routed ? "routed" : "legacy"
            let directory = folder.appendingPathComponent(name, isDirectory: true)
            try files.createDirectory(at: directory, withIntermediateDirectories: true)
            let graph = AVAudioEngine(), format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
            try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
            let host = try AudioHost(audioGraph: graph, midiInputIDs: [])
            defer { host.shutdown() }
            let sourceProject = project(routed: routed)
            func apply() async throws {
                _ = try await host.handle("engine.project", payload: ["project": sourceProject, "replace": true, "awaitPlugins": true])
                await host.pluginRestorationBarrier()?.value
            }
            func render(mode: String, encoding: AudioExportFormat?, suffix: String) async throws -> [Rendered] {
                try await apply()
                let url = directory.appendingPathComponent(mode == "stems" ? suffix : suffix + ".wav", isDirectory: mode == "stems")
                var payload: [String: Any] = ["mode": mode, "path": url.path, "tail": 0, "sampleRate": rate]
                if let encoding { payload["encoding"] = encoding.rawValue }
                if routed && mode == "stems" { payload["owners"] = ["track:encoding-source", "master"] }
                let result = try await host.handle("audio.export", payload: payload) as? [String: Any] ?? [:]
                let expected = encoding ?? .pcm24
                try require(result["encoding"] as? String == expected.rawValue && result["container"] as? String == "wav" &&
                    (result["sampleRate"] as? NSNumber)?.doubleValue == rate && result["bitDepth"] as? Int == expected.bitDepth &&
                    result["isFloat"] as? Bool == expected.isFloat, "Completion metadata disagrees with the requested \(name) \(mode) encoding.")
                let paths = result["paths"] as? [String] ?? []
                try require(paths.count == (routed && mode == "stems" ? 2 : 1), "An export returned the wrong number of files.")
                try require(!graph.isRunning && graph.isInManualRenderingMode && graph.manualRenderingMode == .offline,
                    "Export changed the isolated stopped graph's mode or running state.")
                return try paths.map { try inspect($0, encoding: expected) }
            }
            for mode in ["song", "pattern", "stems"] {
                let reference = try await render(mode: mode, encoding: .float32, suffix: mode + "-float32")
                for encoding in [AudioExportFormat.pcm16, .pcm24] {
                    let rendered = try await render(mode: mode, encoding: encoding, suffix: mode + "-" + encoding.rawValue)
                    try require(rendered.count == reference.count, "The comparison file counts differ.")
                    for index in rendered.indices {
                        let actual = rendered[index].samples, expected = reference[index].samples
                        try require(actual.count == expected.count, "Integer conversion changed duration.")
                        let error = zip(actual, expected).reduce(0.0) { max($0, abs(Double($1.0) - Double($1.1))) }
                        // Allow two integer LSBs for converter rounding/dither, plus one small Float32 rounding allowance.
                        let tolerance = 2.0 / pow(2.0, Double(encoding.bitDepth - 1)) + 0.0000001
                        try require(error <= tolerance, "\(name) \(mode) \(encoding.rawValue) differs from Float32 by \(error), above \(tolerance).")
                        let scale = pow(2.0, Double(encoding.bitDepth - 1))
                        let gridError = actual.reduce(0.0) { max($0, abs(Double($1) * scale - (Double($1) * scale).rounded())) }
                        try require(gridError < 0.001, "Decoded integer samples do not lie on the expected PCM grid.")
                        observations.append(["path": rendered[index].path, "reference": reference[index].path,
                            "maximumAbsoluteError": error, "tolerance": tolerance, "quantizationGridError": gridError])
                    }
                }
                _ = try await render(mode: mode, encoding: nil, suffix: mode + "-default24")
                passed.append("\(name) \(mode): actual PCM16/PCM24/Float32 WAV headers, exact stereo duration, bounded quantization versus Float32 and omitted-encoding PCM24 default")
            }
            for requested in [String?.none, "pcm16", "pcm24", "float32"] {
                try await apply()
                let destination = directory.appendingPathComponent("bounce-" + (requested ?? "default") + ".wav")
                var payload: [String: Any] = ["path": destination.path, "track": 0, "tail": 0]
                if let requested { payload["encoding"] = requested }
                let result = try await host.handle("audio.bounceTrack", payload: payload) as? [String: Any] ?? [:]
                try require(result["path"] as? String == destination.path && (result["duration"] as? NSNumber)?.doubleValue == 0.5,
                    "Internal track bounce lost its file path or half-second duration.")
                _ = try inspect(destination.path, encoding: .float32)
            }
            passed.append("\(name) track bounce remains Float32 for absent and all valid delivery encoding fields")
            try await apply()
            let existing = directory.appendingPathComponent("song-float32.wav")
            let originalBytes = try Data(contentsOf: existing)
            let snapshot = try await host.handle("engine.snapshot", payload: [:])
            let before = try JSONSerialization.data(withJSONObject: snapshot, options: .sortedKeys)
            let invalid: [[String: Any]] = [["encoding": "pcm32"], ["encoding": "PCM24"], ["encoding": true],
                ["encoding": 24], ["encoding": NSNull()], ["sampleRate": true], ["sampleRate": 44_101],
                ["sampleRate": "48000"], ["sampleRate": NSNull()], ["container": "aiff"]]
            for (index, fields) in invalid.enumerated() {
                for mode in ["song", "stems"] {
                    let destination = directory.appendingPathComponent("invalid-\(index)-\(mode)" + (mode == "song" ? ".wav" : ""))
                    try require(!files.fileExists(atPath: destination.path), "Invalid-fixture destination must not exist before validation.")
                    var payload = fields; payload["mode"] = mode; payload["path"] = destination.path; payload["tail"] = 0
                    var rejected = false
                    do { _ = try await host.handle("audio.export", payload: payload) } catch { rejected = true }
                    try require(rejected && !files.fileExists(atPath: destination.path), "An invalid encoding/rate created a file or stem folder.")
                }
                var overwrite = fields; overwrite["mode"] = "song"; overwrite["path"] = existing.path; overwrite["tail"] = 0
                var rejected = false
                do { _ = try await host.handle("audio.export", payload: overwrite) } catch { rejected = true }
                let bytes = try Data(contentsOf: existing)
                try require(rejected && bytes == originalBytes, "Invalid export validation modified an existing valid WAV.")
                try require(!graph.isRunning && host.status()["playing"] as? Bool == false,
                    "Invalid encoding validation changed the stopped transport.")
            }
            let after = try JSONSerialization.data(withJSONObject: try await host.handle("engine.snapshot", payload: [:]), options: .sortedKeys)
            try require(before == after, "Invalid encoding/rate requests mutated the desired project.")
            passed.append("\(name) invalid encoding/container/rate rejection preserves the project, stopped graph, existing WAV bytes and absent file/stem destinations")
        }
        let unchanged = try Data(contentsOf: sourceURL)
        try require(unchanged == sourceBytes, "Export modified original source audio.")
        complete = true
        return passed
    }
}

#endif

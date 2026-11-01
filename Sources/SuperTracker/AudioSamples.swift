import Foundation
import CoreFoundation
import AVFoundation

/// Path selection is pure so development and tests never spill edits into the
/// user's normal support folder when a private session root is configured.
enum SampleEditPaths {
    static func directory(payload: [String: Any], environment: [String: String], applicationSupport: URL) -> URL {
        if let path = payload["outputDirectory"] as? String { return URL(fileURLWithPath: path, isDirectory: true) }
        let support = environment["SUPERTRACKER_SUPPORT_DIR"].flatMap { $0.hasPrefix("/") ? URL(fileURLWithPath: $0, isDirectory: true) : nil }
            ?? applicationSupport.appendingPathComponent("SuperTracker", isDirectory: true)
        return support.appendingPathComponent("Edits", isDirectory: true)
    }
}

/// Validate all in-memory input before an edit can allocate a slice or write a
/// derived file. Finite selection positions retain the editor's clamping policy.
enum SampleEditSelection {
    enum Failure: LocalizedError {
        case invalid(String)
        var errorDescription: String? { if case .invalid(let message) = self { return message }; return nil }
    }
    static func range(source: DecodedSample, payload: [String: Any]) throws -> Range<Int> {
        let count = source.left.count
        guard (1...100_000_000).contains(count), source.right.count == count,
              source.sampleRate.isFinite, source.sampleRate > 0,
              source.left.allSatisfy(\.isFinite), source.right.allSatisfy(\.isFinite) else {
            throw Failure.invalid("The sample has invalid or empty audio data. Nothing was edited.")
        }
        func position(_ key: String, fallback: Double) throws -> Double {
            guard let raw = payload[key] else { return fallback }
            guard let number = raw as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite else {
                throw Failure.invalid("Sample \(key) must be a finite numeric position. Nothing was edited.")
            }
            return max(0, min(1, number.doubleValue))
        }
        let lower = try position("start", fallback: 0), upper = try position("end", fallback: 1)
        let first = min(count - 1, Int(lower * Double(count)))
        let last = max(first + 1, min(count, Int(upper * Double(count))))
        return first..<last
    }
}

struct DecodedSample {
    let path: String
    let sampleRate: Double
    var left: [Float]
    var right: [Float]
    let originalChannels: Int
    /// Single-cycle frame size detected for Wavetable mode (file metadata or size), nil unknown.
    var cycleFrames: Int? = nil

    var duration: Double { Double(left.count) / sampleRate }
    func info() -> [String: Any] {
        let bins = min(512, left.count)
        var peaks = [Float](repeating: 0, count: bins)
        if bins > 0 {
            for i in left.indices {
                let bin = min(bins - 1, i * bins / left.count)
                peaks[bin] = max(peaks[bin], max(abs(left[i]), abs(right[i])))
            }
        }
        return ["path": path, "name": URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent,
                "duration": duration, "sampleRate": sampleRate, "channels": originalChannels,
                "frames": left.count, "peaks": peaks, "wavetableFrameSize": cycleFrames ?? 0]
    }
}

extension DecodedSample {
    /// Sample-editor query: min/max waveform columns for a zoomed window of the sample and optional
    /// zero-crossing snapping of marker positions. Positions are normalized 0...1 over the whole sample.
    func inspect(payload: [String: Any]) -> [String: Any] {
        let count = left.count
        var result: [String: Any] = ["path": path, "frames": count, "sampleRate": sampleRate]
        guard count > 0 else { result["min"] = [Float](); result["max"] = [Float](); return result }
        let number = { (key: String, fallback: Double) -> Double in
            let value = (payload[key] as? NSNumber)?.doubleValue ?? fallback
            return value.isFinite ? value : fallback
        }
        let start = max(0, min(1, number("start", 0))), end = max(start, min(1, number("end", 1)))
        let bins = Int(max(1, min(4096, number("bins", 1000))))
        let first = min(count - 1, Int(start * Double(count) + 1e-6))
        let last = max(first + 1, min(count, Int((end * Double(count) - 1e-6).rounded(.up))))
        let span = last - first
        var minimum = [Float](repeating: 0, count: bins), maximum = [Float](repeating: 0, count: bins)
        for bin in 0..<bins {
            let a = first + bin * span / bins, z = max(a + 1, min(last, first + (bin + 1) * span / bins))
            var lo = Float.greatestFiniteMagnitude, hi = -Float.greatestFiniteMagnitude
            for i in a..<z { lo = min(lo, min(left[i], right[i])); hi = max(hi, max(left[i], right[i])) }
            minimum[bin] = lo; maximum[bin] = hi
        }
        result["start"] = Double(first) / Double(count); result["end"] = Double(last) / Double(count)
        result["min"] = minimum; result["max"] = maximum
        if let positions = payload["snap"] as? [NSNumber] {
            let requestedDirection = number("direction", 0)
            let direction = requestedDirection <= -1 ? -1 : requestedDirection >= 1 ? 1 : 0
            // Clamp in seconds before multiplying or converting. A JSON number
            // can be finite but still exceed the integer domain by many orders.
            let rate = sampleRate.isFinite && sampleRate > 0 ? sampleRate : 1
            let seconds = min(max(0, number("window", 0.02)), Double(count) / rate)
            let window = Int(min(Double(count), max(1, seconds * rate)))
            result["snapped"] = positions.map { zeroCrossing(near: $0.doubleValue, direction: direction, window: window) }
        }
        return result
    }

    /// Nearest zero crossing (on the summed channels) to `position`; direction -1/1 searches only
    /// backward/forward, 0 picks the closest. Returns the original position when none is in range.
    func zeroCrossing(near position: Double, direction: Int, window: Int) -> Double {
        let count = left.count
        let normalized = position.isFinite ? max(0, min(1, position)) : 0
        guard count > 1 else { return normalized }
        let frame = (normalized * Double(count)).rounded()
        let origin = frame >= Double(count - 1) ? count - 1 : Int(frame)
        let reach = max(1, min(count, window))
        let value = { (i: Int) -> Float in left[i] + right[i] }
        // A crossing lies between frames i-1 and i.
        let crossing = { (i: Int) -> Bool in (value(i - 1) < 0) != (value(i) < 0) }
        var forward: Int? = nil, backward: Int? = nil
        if direction >= 0, origin + 1 < count {
            for i in (origin + 1)...(origin + min(count - 1 - origin, reach)) where crossing(i) {
                forward = direction > 0 || abs(value(i)) <= abs(value(i - 1)) ? i : i - 1; break
            }
        }
        if direction <= 0, origin >= 1 {
            for i in stride(from: origin, through: max(1, origin - min(origin, reach) + 1), by: -1) where crossing(i) {
                backward = direction < 0 || abs(value(i - 1)) <= abs(value(i)) ? i - 1 : i; break
            }
        }
        let found: Int?
        switch (forward, backward) {
        case let (f?, b?): found = abs(f - origin) <= abs(origin - b) ? f : b
        case let (f?, nil): found = f
        case let (nil, b?): found = b
        default: found = nil
        }
        if let found { return Double(found) / Double(count) }
        return normalized
    }
}

enum AudioSampleError: LocalizedError {
    case invalid(String)
    var errorDescription: String? { if case .invalid(let message) = self { return message }; return nil }
}

enum AudioSamples {
    /// Maps a project sample reference to a local file (the app installs one for
    /// built-in samples, see BuiltinSamples). The decoded sample keeps the reference.
    nonisolated(unsafe) static var referenceResolver: ((String) throws -> String)?
    /// Detects a wavetable's frame size from the file and its frame count (installed by the app).
    nonisolated(unsafe) static var cycleDetector: ((_ file: String, _ frames: Int) -> Int?)?

    static func decode(_ path: String) throws -> DecodedSample {
        let location = try referenceResolver?(path) ?? path
        // AVAudioFile reports a missing file as an opaque 'what' (2003334207) error.
        guard FileManager.default.fileExists(atPath: location) else {
            throw AudioSampleError.invalid("Sample file not found: \((path as NSString).lastPathComponent). Use Relink missing project media to locate it.")
        }
        let file = try AVAudioFile(forReading: URL(fileURLWithPath: location), commonFormat: .pcmFormatFloat32, interleaved: false)
        guard file.length > 0, file.length <= 100_000_000 else {
            throw AudioSampleError.invalid("Sample must contain audio and be shorter than 100 million frames. Split long recordings before loading them as instruments.")
        }
        let format = file.processingFormat
        let capacity: AVAudioFrameCount = 65536
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else {
            throw AudioSampleError.invalid("Cannot allocate sample decoding buffer.")
        }
        var left: [Float] = []; var right: [Float] = []
        left.reserveCapacity(Int(file.length)); right.reserveCapacity(Int(file.length))
        while file.framePosition < file.length {
            try file.read(into: buffer, frameCount: min(capacity, AVAudioFrameCount(file.length - file.framePosition)))
            guard let channels = buffer.floatChannelData, buffer.frameLength > 0 else { break }
            let count = Int(buffer.frameLength)
            let second = channels[format.channelCount > 1 ? 1 : 0]
            for index in 0..<count {
                let l = channels[0][index], r = second[index]
                left.append(l.isFinite ? l : 0); right.append(r.isFinite ? r : 0)
            }
        }
        var sample = DecodedSample(path: path, sampleRate: format.sampleRate, left: left, right: right, originalChannels: Int(format.channelCount))
        sample.cycleFrames = cycleDetector?(location, left.count)
        return sample
    }

    static func edit(_ source: DecodedSample, payload: [String: Any]) throws -> DecodedSample {
        let operation = payload["operation"] as? String ?? payload["action"] as? String ?? "trim"
        var sample = source
        let selectionRange = try SampleEditSelection.range(source: source, payload: payload)
        let start = selectionRange.lowerBound, end = selectionRange.upperBound
        switch operation {
        case "stretch":
            let selection = DecodedSample(path:source.path,sampleRate:source.sampleRate,left:Array(source.left[start..<end]),right:Array(source.right[start..<end]),originalChannels:source.originalChannels)
            let stretched = try AudioStretch.process(selection,ratio:AudioStretch.ratio(payload:payload))
            sample.left = Array(source.left[..<start]) + stretched.left + Array(source.left[end...])
            sample.right = Array(source.right[..<start]) + stretched.right + Array(source.right[end...])
        case "trim", "crop":
            sample.left = Array(sample.left[start..<end]); sample.right = Array(sample.right[start..<end])
        case "reverse":
            sample.left[start..<end].reverse(); sample.right[start..<end].reverse()
        case "normalize":
            var peak: Float = 0
            for i in start..<end { peak = max(peak, max(abs(sample.left[i]), abs(sample.right[i]))) }
            let gain = Float(payload["peak"] as? Double ?? 0.98) / max(0.000001, peak)
            for i in start..<end { sample.left[i] *= gain; sample.right[i] *= gain }
        case "gain":
            let gain = Float(pow(10, min(24, max(-96, payload["decibels"] as? Double ?? 0)) / 20))
            for i in start..<end { sample.left[i] *= gain; sample.right[i] *= gain }
        case "fadeIn", "fadeOut":
            // "linear" (default), "smooth" (S-curve) or "exponential" (perceptually even) fade shapes.
            let curve = payload["curve"] as? String ?? "linear"
            for i in start..<end {
                let t = Float(i - start) / Float(max(1, end - start - 1))
                let x = operation == "fadeIn" ? t : 1 - t
                let gain = curve == "smooth" ? 0.5 - 0.5 * cos(Float.pi * x) : curve == "exponential" ? x * x * x : x
                sample.left[i] *= gain; sample.right[i] *= gain
            }
        case "removeDC":
            let meanL = sample.left[start..<end].reduce(0, +) / Float(end - start)
            let meanR = sample.right[start..<end].reduce(0, +) / Float(end - start)
            for i in start..<end { sample.left[i] -= meanL; sample.right[i] -= meanR }
        case "silence":
            for i in start..<end { sample.left[i] = 0; sample.right[i] = 0 }
        default: throw AudioSampleError.invalid("Unknown sample edit: \(operation).")
        }
        let directory = SampleEditPaths.directory(payload: payload, environment: ProcessInfo.processInfo.environment,
            applicationSupport: FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0])
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let name = URL(fileURLWithPath: source.path).deletingPathExtension().lastPathComponent
        let destination = directory.appendingPathComponent("\(name)-\(operation)-\(UUID().uuidString.prefix(8)).wav")
        try write(sample, to: destination)
        return DecodedSample(path: destination.path, sampleRate: sample.sampleRate, left: sample.left, right: sample.right, originalChannels: 2)
    }

    static func write(_ sample: DecodedSample, to url: URL) throws {
        guard let format = AVAudioFormat(standardFormatWithSampleRate: sample.sampleRate, channels: 2),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 65536) else {
            throw AudioSampleError.invalid("Cannot allocate sample output buffer.")
        }
        let settings: [String: Any] = [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: sample.sampleRate,
            AVNumberOfChannelsKey: 2, AVLinearPCMBitDepthKey: 24, AVLinearPCMIsFloatKey: false,
            AVLinearPCMIsBigEndianKey: false, AVLinearPCMIsNonInterleaved: false]
        let file = try AVAudioFile(forWriting: url, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false)
        var offset = 0
        while offset < sample.left.count {
            let count = min(65536, sample.left.count - offset)
            buffer.frameLength = AVAudioFrameCount(count)
            if let channels = buffer.floatChannelData {
                for i in 0..<count { channels[0][i] = sample.left[offset + i]; channels[1][i] = sample.right[offset + i] }
            }
            try file.write(from: buffer); offset += count
        }
    }
}

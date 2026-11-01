import Foundation

// Sample-editor backend: zoomed waveform columns, zero-crossing snapping and
// source-preserving region edits (trim, fades, normalize, reverse, gain).
@main
struct SampleEditorTests {
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() { throw NSError(domain: "SampleEditorTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    }
    static func sample(_ left: [Float], _ right: [Float]? = nil, path: String = "generated.wav") -> DecodedSample {
        DecodedSample(path: path, sampleRate: 48_000, left: left, right: right ?? left, originalChannels: 2)
    }

    static func main() throws {
        // A 100-frame-period sine: crossings every 50 frames.
        let count = 48_000
        let tone = (0..<count).map { Float(0.5 * sin(2 * Double.pi * (Double($0) + 0.25) / 100)) }
        let source = sample(tone)

        // Whole-file overview and a zoomed window.
        let overview = source.inspect(payload: ["bins": 480])
        let maxima = overview["max"] as! [Float], minima = overview["min"] as! [Float]
        try require(maxima.count == 480 && minima.count == 480, "Overview has the requested column count")
        try require(maxima.allSatisfy { abs($0 - 0.5) < 0.01 } && minima.allSatisfy { abs($0 + 0.5) < 0.01 }, "Each 100-frame column spans the full sine")
        let zoom = source.inspect(payload: ["start": 0.5, "end": 0.5 + 50.0 / Double(count), "bins": 50])
        let zoomMax = zoom["max"] as! [Float], zoomMin = zoom["min"] as! [Float]
        try require(zip(zoomMax, zoomMin).allSatisfy { $0 == $1 }, "At one frame per column the envelope is the sample value")
        try require(zip(zoomMax, tone[24_000..<24_050]).allSatisfy { $0 == $1 }, "Zoomed columns are the exact frames of the window")
        try require(abs((zoom["start"] as! Double) - 0.5) < 1e-9, "Zoom reports its aligned start")
        let tiny = source.inspect(payload: ["start": 0.25, "end": 0.25, "bins": 64])
        try require((tiny["max"] as! [Float]).count == 64, "A zero-width window still answers")
        try require(sample([]).inspect(payload: [:])["frames"] as? Int == 0, "Empty sample answers without columns")

        // Zero-crossing snapping. Crossings of this tone sit between frames 24 and 25, 74 and 75, ...
        func crossing(_ frame: Int, _ direction: Int) -> Int {
            Int((source.zeroCrossing(near: Double(frame) / Double(count), direction: direction, window: 960) * Double(count)).rounded())
        }
        func isZero(_ frame: Int) -> Bool { abs(tone[frame]) < 0.032 }  // within one frame step of the crossing
        let near = crossing(10_010, 0)
        try require(isZero(near) && abs(near - 10_010) <= 26, "Nearest crossing is a near-zero frame close by (got \(near))")
        let ahead = crossing(10_010, 1), behind = crossing(10_010, -1)
        try require(ahead > 10_010 && ahead - 10_010 <= 50 && isZero(ahead), "Forward snap moves forward to the next crossing (got \(ahead))")
        try require(behind < 10_010 && 10_010 - behind <= 50 && isZero(behind), "Backward snap moves backward (got \(behind))")
        try require(crossing(ahead, 1) > ahead, "Repeated forward snapping always progresses")
        try require(crossing(behind, -1) < behind, "Repeated backward snapping always progresses")
        let silent = sample([Float](repeating: 0.25, count: 1000))
        try require(silent.zeroCrossing(near: 0.5, direction: 0, window: 100) == 0.5, "No crossing keeps the position")
        let snapped = source.inspect(payload: ["bins": 1, "snap": [0.1, 0.2], "direction": 0])["snapped"] as! [Double]
        try require(snapped.count == 2 && snapped.allSatisfy { isZero(Int(($0 * Double(count)).rounded())) }, "Batch snapping through inspect")

        // Region edits write a new file and leave the source untouched.
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("SampleEditorTests-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let sourceURL = folder.appendingPathComponent("source.wav")
        let ramp = (0..<4800).map { Float($0) / 4800 * 0.5 }
        try AudioSamples.write(sample(ramp, ramp.map { -$0 }), to: sourceURL)
        let original = try Data(contentsOf: sourceURL)
        let decoded = try AudioSamples.decode(sourceURL.path)
        func edit(_ payload: [String: Any]) throws -> DecodedSample {
            var full = payload; full["outputDirectory"] = folder.path
            let result = try AudioSamples.edit(decoded, payload: full)
            try require(result.path != sourceURL.path && FileManager.default.fileExists(atPath: result.path), "Edit writes a new file")
            let reread = try AudioSamples.decode(result.path)
            try require(reread.left.count == result.left.count, "Written edit decodes with the same length")
            return reread
        }
        let trimmed = try edit(["operation": "trim", "start": 0.25, "end": 0.75])
        try require(trimmed.left.count == 2400 && abs(trimmed.left[0] - 0.125) < 1e-4, "Trim keeps exactly the region")
        let normalized = try edit(["operation": "normalize", "start": 0.0, "end": 0.5, "peak": 1.0])
        try require(abs(normalized.left[2399] - 1) < 1e-3 && abs(normalized.left[4799] - ramp[4799]) < 1e-4, "Normalize scales the region to its peak and leaves the rest")
        let reversed = try edit(["operation": "reverse", "start": 0.5, "end": 1.0])
        try require(abs(reversed.left[2400] - ramp[4799]) < 1e-4 && abs(reversed.left[100] - ramp[100]) < 1e-4, "Reverse flips only the region")
        let quieter = try edit(["operation": "gain", "decibels": -6.0206])
        try require(abs(quieter.left[4000] - ramp[4000] / 2) < 1e-3 && abs(quieter.right[4000] + ramp[4000] / 2) < 1e-3, "Gain applies to both channels")
        for curve in ["linear", "smooth", "exponential"] {
            let faded = try edit(["operation": "fadeIn", "start": 0.0, "end": 0.5, "curve": curve])
            try require(abs(faded.left[0]) < 1e-6 && abs(faded.left[2399] - ramp[2399]) < 1e-3 && abs(faded.left[3000] - ramp[3000]) < 1e-4, "\(curve) fade-in starts silent and leaves later audio")
            let out = try edit(["operation": "fadeOut", "start": 0.5, "end": 1.0, "curve": curve])
            try require(abs(out.left[4799]) < 1e-6 && abs(out.left[2400] - ramp[2400]) < 1e-3, "\(curve) fade-out ends silent")
        }
        let half = try edit(["operation": "fadeIn", "start": 0.0, "end": 1.0, "curve": "exponential"])
        try require(half.left[2400] < ramp[2400] * 0.2, "Exponential fade is slower at the start than linear")
        let after = try Data(contentsOf: sourceURL)
        try require(after == original, "Source file bytes are unchanged after all edits")
        print("Sample editor: overview/zoom columns, zero-crossing snapping and 11 source-preserving region edits passed.")
    }
}

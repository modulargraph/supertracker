// swiftc -O -swift-version 5 -module-cache-path /tmp/supertracker-swift-cache \
// Sources/SuperTracker/AudioSamples.swift Sources/SuperTracker/AudioStretch.swift \
// Sources/SuperTracker/SampleSlicing.swift tests/SampleSlicingTests.swift -o /tmp/supertracker-slicing-tests
import Foundation
import Darwin

@main struct SampleSlicingTests {
    static let rate = 48_000.0
    static func sample(_ values: [Float], rate: Double = rate, inverted: Bool = false) -> DecodedSample {
        DecodedSample(path: "/unchanged/recording.wav", sampleRate: rate, left: values, right: inverted ? values.map { -$0 } : values, originalChannels: 2)
    }
    static func noise(_ seed: inout UInt32) -> Double { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; return Double(seed) / Double(UInt32.max) * 2 - 1 }
    static func drum(_ audio: inout [Float], at time: Double, snare: Bool, amplitude: Double = 0.7) {
        let start = Int(time * rate); var seed: UInt32 = 12345
        for n in 0..<Int(rate * 0.15) where start + n < audio.count {
            let t = Double(n) / rate, tone = sin(2 * Double.pi * (snare ? 190 : 75) * t)
            let value = (snare ? noise(&seed) * 0.85 + tone * 0.15 : tone) * exp(-t * (snare ? 38 : 30))
            audio[start + n] += Float(value * amplitude)
        }
    }
    static func points(_ source: DecodedSample, _ payload: [String: Any] = [:]) throws -> [Double] {
        let result = try SampleSlicing.analyze(source, payload: payload)
        let points = result["slicePoints"] as! [Double]
        precondition(points.first == 0 && points.allSatisfy { $0.isFinite && $0 >= 0 && $0 < 1 })
        precondition(points == points.sorted() && Set(points).count == points.count)
        precondition(result["sliceCount"] as! Int == points.count)
        return points
    }
    static func main() throws {
        setbuf(stdout, nil)
        var breakAudio = [Float](repeating: 0, count: Int(rate * 2))
        let attacks = [0.0, 0.25, 0.5, 0.875, 1.125, 1.5]
        for (index, time) in attacks.enumerated() { drum(&breakAudio, at: time, snare: index % 2 != 0) }
        let source = sample(breakAudio), original = source.left
        let detected = try points(source).map { $0 * source.duration }
        print("Drum markers seconds: \(detected)")
        precondition(detected.count == attacks.count)
        for time in attacks { precondition(detected.contains { abs($0 - time) < 0.012 }, "Missing drum at \(time)") }
        precondition(source.left == original && source.path == "/unchanged/recording.wav")
        let inverted = try points(sample(breakAudio, inverted: true)), normal = try points(source); precondition(inverted == normal)
        print("PASS kick/snare attacks, stereo phase robustness, source preservation")

        var seed: UInt32 = 981, noisy = (0..<Int(rate * 3)).map { _ in Float(noise(&seed) * 0.025) }
        let steadyNoise = try points(sample(noisy)); precondition(steadyNoise == [0])
        let silence = try points(sample([Float](repeating: 0, count: 48000))); precondition(silence == [0])
        let invalidPCM = try points(sample([Float.nan, .infinity, -.infinity, 0])); precondition(invalidPCM == [0])
        drum(&noisy, at: 0.5, snare: false); drum(&noisy, at: 1.25, snare: true)
        let noiseHits = try points(sample(noisy)).map { $0 * 3 }
        precondition(noiseHits.count == 3 && noiseHits.contains { abs($0 - 0.5) < 0.012 } && noiseHits.contains { abs($0 - 1.25) < 0.012 })
        print("PASS steady noise/silence rejection and drums over noise")

        var synth = [Float](repeating: 0, count: 48000)
        for frame in 9600..<40000 { synth[frame] = Float(sin(Double(frame - 9600) * 2 * .pi * 220 / rate) * min(1, Double(frame - 9600) / 144) * 0.6) }
        let note = try points(sample(synth)); print("Synth markers: \(note)"); precondition(note.count == 2 && abs(note[1] - 0.2) < 0.012)
        let region = try points(source, ["start": 0.25, "end": 0.75])
        precondition(region.count == 3 && abs(region[1] - 0.375) < 0.012 && abs(region[2] - 0.625) < 0.012)
        print("PASS single sustained synth note and selection-relative markers")

        let limited = try SampleSlicing.analyze(source, payload: ["maximumSlices": 3])
        precondition(limited["sliceCount"] as! Int == 3 && limited["capped"] as! Bool)
        let spaced = try points(source, ["minimumGapSeconds": 0.4]).map { $0 * 2 }
        for pair in zip(spaced, spaced.dropFirst()) { precondition(pair.1 - pair.0 >= 0.4 - 1 / rate) }
        let one = try points(source, ["maximumSlices": 1]); precondition(one == [0])
        let low = try points(sample(noisy), ["sensitivity": 0]), high = try points(sample(noisy), ["sensitivity": 1])
        precondition(high.count >= low.count)
        do { _ = try points(source, ["start": 0.8, "end": 0.2]); preconditionFailure("Invalid range accepted") } catch { }
        let sanitized = try SampleSlicing.analyze(source, payload: ["sensitivity": Double.nan, "minimumGapSeconds": -5, "maximumSlices": 500])
        precondition(sanitized["sensitivity"] as! Double == 0.5 && sanitized["minimumGapSeconds"] as! Double == 0.005 && sanitized["maximumSlices"] as! Int == 128)
        print("PASS slice limit, minimum gap, sensitivity and invalid-input bounds")

        let long = sample([Float](repeating: 0, count: 1_000_000), rate: 1000)
        let start = Date(), result = try SampleSlicing.analyze(long, payload: [:])
        precondition((result["analysisWindows"] as! Int) <= 262_144 && result["sliceCount"] as! Int == 1)
        print("PASS bounded long-file analysis: \(result["analysisWindows"]!) windows in \(Date().timeIntervalSince(start)) seconds")
        print("Sample slicing: 5 suites passed.")
    }
}

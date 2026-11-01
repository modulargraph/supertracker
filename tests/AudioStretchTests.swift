import Foundation

@main
struct AudioStretchTests {
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() { throw NSError(domain: "AudioStretchTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    }

    static func sample(_ left: [Float], _ right: [Float]? = nil, rate: Double = 48_000) -> DecodedSample {
        DecodedSample(path: "generated.wav", sampleRate: rate, left: left, right: right ?? left, originalChannels: 2)
    }

    static func frequency(_ values: [Float], rate: Double) -> Double {
        let trim = min(values.count / 4, Int(rate * 0.080))
        let end = values.count - trim
        var crossings: [Int] = []
        guard end > trim + 1 else { return 0 }
        for i in (trim + 1)..<end where values[i - 1] <= 0 && values[i] > 0 { crossings.append(i) }
        guard let first = crossings.first, let last = crossings.last, last > first else { return 0 }
        return Double(crossings.count - 1) * rate / Double(last - first)
    }

    static func main() throws {
        let rate = 48_000.0
        let tone = (0..<48_000).map { Float(0.7 * sin(2 * Double.pi * 440 * Double($0) / rate)) }
        let original = sample(tone, tone.map { -$0 })
        for ratio in [0.25, 0.5, 0.75, 1.5, 2.0, 4.0] {
            let result = try AudioStretch.process(original, ratio: ratio)
            try require(result.left.count == Int((Double(tone.count) * ratio).rounded()), "Exact output length at ratio \(ratio)")
            try require(result.sampleRate == rate, "Sample rate remains unchanged")
            let hz = frequency(result.left, rate: rate)
            try require(abs(hz - 440) < 4.4, "Pitch preserved within 1% at ratio \(ratio): \(hz) Hz")
            try require(zip(result.left, result.right).allSatisfy { abs($0 + $1) < 0.000001 }, "Stereo anti-phase relationship preserved")
            try require(result.left.allSatisfy { $0.isFinite && abs($0) <= 0.701 }, "Finite bounded audio")
            let middle = result.left.dropFirst(result.left.count / 4).prefix(result.left.count / 2)
            let rms = sqrt(middle.reduce(0.0) { $0 + Double($1 * $1) } / Double(middle.count))
            try require(rms > 0.40, "Sustained tone remains audible at ratio \(ratio): rms \(rms)")
            print("PASS stretch sustained tone ratio \(ratio), pitch \(String(format: "%.2f", hz)) Hz")
        }
        let unchanged = try AudioStretch.process(original, ratio: 1)
        try require(unchanged.left == original.left && unchanged.right == original.right, "Unity stretch is lossless")
        try require(original.left == tone, "Source PCM remains unchanged")
        for fundamental in [55.0, 110.0, 997.0] {
            let values = (0..<48_000).map { Float(0.6 * sin(2 * Double.pi * fundamental * Double($0) / rate)) }
            for ratio in [0.5, 2.0] {
                let result = try AudioStretch.process(sample(values), ratio: ratio)
                let hz = frequency(result.left, rate: rate)
                try require(abs(hz - fundamental) / fundamental < 0.01,
                            "Bass/treble pitch retained: \(fundamental) Hz -> \(hz), ratio \(ratio)")
            }
        }
        print("PASS stretch sustained bass/treble pitch at 55, 110 and 997 Hz")

        // Wideband decaying drum-like attacks placed at known times. Verify timing
        // around target locations and contrast against silence between attacks.
        var percussion = [Float](repeating: 0, count: 96_000)
        let attacks = [0.20, 0.65, 1.10, 1.55]
        var random: UInt32 = 0x12345678
        for onset in attacks {
            let offset = Int(onset * rate)
            for n in 0..<2_880 {
                random = random &* 1_664_525 &+ 1_013_904_223
                let noise = Double(random >> 8) / Double(0x00ffffff) * 2 - 1
                let envelope = exp(-Double(n) / 560)
                percussion[offset + n] = Float(0.8 * envelope * (0.65 * sin(2 * .pi * 130 * Double(n) / rate) + 0.35 * noise))
            }
        }
        func rmsNear(_ values: [Float], _ time: Double, radius: Double) -> Double {
            let lower = max(0, Int((time - radius) * rate)), upper = min(values.count, Int((time + radius) * rate))
            guard upper > lower else { return 0 }
            return sqrt(values[lower..<upper].reduce(0.0) { $0 + Double($1 * $1) } / Double(upper - lower))
        }
        for ratio in [0.5, 1.5, 2.0] {
            let result = try AudioStretch.process(sample(percussion), ratio: ratio)
            for onset in attacks {
                let attackEnergy = rmsNear(result.left, onset * ratio + 0.010, radius: 0.070)
                let gapEnergy = rmsNear(result.left, (onset + 0.23) * ratio, radius: 0.040)
                try require(attackEnergy > 0.025, "Rhythmic attack retained near stretched time at \(onset), ratio \(ratio): \(attackEnergy)")
                try require(gapEnergy < attackEnergy * 0.20, "Rhythmic attacks remain distinct, ratio \(ratio)")
            }
            print("PASS stretch rhythmic transients ratio \(ratio)")
        }
        for ratio in [0.25, 4.0] {
            let silence = try AudioStretch.process(sample([Float](repeating: 0, count: 2048)), ratio: ratio)
            try require(silence.left.allSatisfy { $0 == 0 }, "Silent input remains silent")
        }
        let short = try AudioStretch.process(sample([0.1, -0.1, 0.2]), ratio: 2)
        try require(short.left.count == 6 && short.left.allSatisfy(\.isFinite), "Short input handled")
        let bpmRatio = try AudioStretch.ratio(payload: ["sourceBPM":120, "targetBPM":90])
        try require(abs(bpmRatio - 4.0 / 3.0) < 1e-9, "Tempo conversion direction")
        for invalid in [Double.nan, Double.infinity, 0, -1, 0.24, 4.01] {
            var rejected = false
            do { _ = try AudioStretch.process(original, ratio: invalid) } catch { rejected = true }
            try require(rejected, "Invalid ratio rejected")
        }
        var rejected = false
        do { _ = try AudioStretch.ratio(payload: ["sourceBPM":120, "targetBPM":0]) } catch { rejected = true }
        try require(rejected, "Zero tempo rejected")
        rejected = false
        do { _ = try AudioStretch.process(sample([.nan, 0]), ratio: 2) } catch { rejected = true }
        try require(rejected, "Non-finite input rejected")
        rejected = false
        do { _ = try AudioStretch.process(sample([0, 1], [0]), ratio: 2) } catch { rejected = true }
        try require(rejected, "Mismatched channels rejected")
        rejected = false
        do { _ = try AudioStretch.process(sample([0, 1], rate: 0), ratio: 2) } catch { rejected = true }
        try require(rejected, "Invalid sample rate rejected")
        print("PASS stretch: exact duration, pitch, stereo, rhythm, identity, source preservation and validation")
    }
}

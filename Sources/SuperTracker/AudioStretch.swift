import Foundation

/// Original offline waveform-similarity overlap-add stretcher. Playback rate inside
/// each grain remains one: changing duration does not transpose the source. Left and
/// right share every chosen offset, preserving their phase relationship. This is a
/// general-purpose time-domain algorithm, not formant correction or transient warping.
enum AudioStretch {
    enum StretchError: LocalizedError {
        case invalid(String)
        var errorDescription: String? { if case .invalid(let message) = self { return message }; return nil }
    }

    /// ratio is output duration / input duration. 120 BPM -> 90 BPM is 120 / 90.
    static func ratio(payload: [String: Any]) throws -> Double {
        let value: Double
        if let explicit = payload["ratio"] as? NSNumber {
            value = explicit.doubleValue
        } else if let source = payload["sourceBPM"] as? NSNumber, let target = payload["targetBPM"] as? NSNumber {
            guard source.doubleValue.isFinite, target.doubleValue.isFinite,
                  source.doubleValue > 0, target.doubleValue > 0 else {
                throw StretchError.invalid("Source and target tempo must be positive finite numbers.")
            }
            value = source.doubleValue / target.doubleValue
        } else {
            throw StretchError.invalid("Set a stretch ratio, or both the source and target tempo.")
        }
        guard value.isFinite, (0.25...4).contains(value) else {
            throw StretchError.invalid("Stretch ratio must be between 0.25 and 4 (25–400% duration).")
        }
        return value
    }

    /// CPU-heavy offline work: invoke off the UI and audio render threads. The caller
    /// writes the returned PCM to a new file using the normal non-destructive edit path.
    static func process(_ source: DecodedSample, ratio: Double) throws -> DecodedSample {
        guard ratio.isFinite, (0.25...4).contains(ratio) else {
            throw StretchError.invalid("Stretch ratio must be between 0.25 and 4 (25–400% duration).")
        }
        let inputCount = source.left.count
        guard inputCount > 0, source.right.count == inputCount,
              source.sampleRate.isFinite, source.sampleRate > 0 else {
            throw StretchError.invalid("The sample has invalid audio data.")
        }
        guard source.left.allSatisfy(\.isFinite), source.right.allSatisfy(\.isFinite) else {
            throw StretchError.invalid("The sample contains non-finite audio values.")
        }
        let outputLength = Double(inputCount) * ratio
        guard outputLength <= 100_000_000 else {
            throw StretchError.invalid("The stretched sample exceeds the 100 million frame limit. Split the recording first.")
        }
        if ratio == 1 { return source }
        let outputCount = max(1, Int(outputLength.rounded()))
        // Very short material has no stable periodic context. Extending its one
        // available grain is still preferable to resampling and changing its pitch.
        if inputCount < 16 {
            let left = (0..<outputCount).map { source.left[min($0, inputCount - 1)] }
            let right = (0..<outputCount).map { source.right[min($0, inputCount - 1)] }
            return DecodedSample(path: source.path, sampleRate: source.sampleRate, left: left,
                                 right: right, originalChannels: source.originalChannels)
        }

        // About 40 ms balances sustained tones and rhythmic material. The search is
        // half a window, bounded in source time instead of accumulating timing drift.
        let desiredWindow = max(64, min(4096, Int(source.sampleRate * 0.040)))
        let window = min(desiredWindow, inputCount)
        let hop = max(4, window / 4)
        let overlap = window - hop
        let searchRadius = max(8, window / 2)
        let lastInputStart = max(0, inputCount - window)
        let correlationStride = max(1, overlap / 128)
        let coarseStride = max(1, Int(source.sampleRate / 12_000))
        let weight = (0..<window).map { index -> Float in
            // Half-sample offset gives nonzero endpoint weights for edge normalization.
            let s = sin(Double.pi * (Double(index) + 0.5) / Double(window))
            return Float(s * s)
        }
        var left = [Float](repeating: 0, count: outputCount)
        var right = [Float](repeating: 0, count: outputCount)
        var normalization = [Float](repeating: 0, count: outputCount)
        var previousStart = 0
        var outputStart = 0

        while outputStart < outputCount {
            let expected = min(lastInputStart, max(0, Int((Double(outputStart) / ratio).rounded())))
            var inputStart = expected
            if outputStart > 0 && lastInputStart > 0 {
                let lower = max(0, expected - searchRadius)
                let upper = min(lastInputStart, expected + searchRadius)
                let referenceStart = min(lastInputStart + hop, previousStart + hop)
                var referenceEnergy: Double = 0
                for n in stride(from: 0, to: overlap, by: correlationStride) {
                    let l = Double(source.left[referenceStart + n])
                    let r = Double(source.right[referenceStart + n])
                    referenceEnergy += l * l + r * r
                }
                if referenceEnergy > 1e-12 {
                    // Both channels contribute separately so anti-phase stereo cannot
                    // cancel into a silent analysis signal.
                    func similarity(_ candidate: Int) -> Double {
                        var dot: Double = 0, energy: Double = 0
                        for n in stride(from: 0, to: overlap, by: correlationStride) {
                            let l = Double(source.left[candidate + n]), r = Double(source.right[candidate + n])
                            dot += l * Double(source.left[referenceStart + n]) + r * Double(source.right[referenceStart + n])
                            energy += l * l + r * r
                        }
                        guard energy > 1e-12 else { return -2 }
                        let correlation = dot / sqrt(referenceEnergy * energy)
                        // Prefer nearby equally good matches. This avoids time drift
                        // on periodic content and excessive movement of attacks.
                        return correlation - 0.002 * Double(abs(candidate - expected)) / Double(searchRadius)
                    }
                    var bestScore = similarity(expected)
                    for candidate in stride(from: lower, through: upper, by: coarseStride) {
                        let score = similarity(candidate)
                        if score > bestScore { bestScore = score; inputStart = candidate }
                    }
                    let refineLower = max(lower, inputStart - coarseStride)
                    let refineUpper = min(upper, inputStart + coarseStride)
                    if refineLower <= refineUpper {
                        for candidate in refineLower...refineUpper {
                            let score = similarity(candidate)
                            if score > bestScore { bestScore = score; inputStart = candidate }
                        }
                    }
                }
            }

            let available = min(window, outputCount - outputStart)
            for n in 0..<available {
                let gain = weight[n]
                left[outputStart + n] += source.left[inputStart + n] * gain
                right[outputStart + n] += source.right[inputStart + n] * gain
                normalization[outputStart + n] += gain
            }
            previousStart = inputStart
            outputStart += hop
        }
        for index in left.indices {
            let gain = max(1e-12, normalization[index])
            left[index] /= gain
            right[index] /= gain
        }
        return DecodedSample(path: source.path, sampleRate: source.sampleRate, left: left,
                             right: right, originalChannels: source.originalChannels)
    }
}

import Foundation

/// Offline onset analysis. Returns marker positions only; never edits source PCM.
/// Memory is bounded by 262,144 analysis windows even for the decoder's longest file.
enum SampleSlicing {
    private struct Candidate { let frame: Int; let strength: Double }
    private static let maximumWindows = 262_144

    static func analyze(_ sample: DecodedSample, payload: [String: Any]) throws -> [String: Any] {
        guard sample.sampleRate.isFinite, (1_000...768_000).contains(sample.sampleRate),
              !sample.left.isEmpty, sample.left.count == sample.right.count,
              sample.left.count <= 100_000_000 else {
            throw AudioSampleError.invalid("Transient analysis requires a valid stereo sample of at most 100 million frames.")
        }
        func number(_ key: String, _ fallback: Double, _ lower: Double, _ upper: Double) -> Double {
            min(upper, max(lower, PayloadValue.number(payload, key, fallback)))
        }
        let sensitivity = number("sensitivity", 0.5, 0, 1)
        let gapSeconds = number("minimumGapSeconds", 0.04, 0.005, 2)
        let maximumSlices = Int(number("maximumSlices", 128, 1, 128).rounded())
        let start = number("start", 0, 0, 1), end = number("end", 1, 0, 1)
        guard end > start else { throw AudioSampleError.invalid("Set the sample end after its start before detecting transients.") }
        let first = min(sample.left.count - 1, Int(start * Double(sample.left.count)))
        let last = min(sample.left.count, max(first + 1, Int(end * Double(sample.left.count))))
        let count = last - first
        let hop = max(1, Int((sample.sampleRate * 0.001).rounded()), (count + maximumWindows - 1) / maximumWindows)
        let windows = (count + hop - 1) / hop
        let gap = max(1, Int((gapSeconds * sample.sampleRate).rounded()))
        var energy = [Double](repeating: 0, count: windows)
        var brightness = [Double](repeating: 0, count: windows)
        let highpass = exp(-2 * Double.pi * 25 / sample.sampleRate)
        var previousLeft = 0.0, previousRight = 0.0, filteredLeft = 0.0, filteredRight = 0.0
        var peakEnergy = 0.0, peakBrightness = 0.0
        // Stereo power avoids cancelling anti-phase drums. A 25 Hz high-pass avoids
        // treating a sustained DC offset as musical energy; the difference band also
        // finds a snare attack on top of a bass note without FFT allocation or setup.
        for window in 0..<windows {
            let begin = first + window * hop, finish = min(last, begin + hop)
            var sum = 0.0, highSum = 0.0
            for frame in begin..<finish {
                let sourceLeft = sample.left[frame], sourceRight = sample.right[frame]
                let left = sourceLeft.isFinite ? min(8, max(-8, Double(sourceLeft))) : 0
                let right = sourceRight.isFinite ? min(8, max(-8, Double(sourceRight))) : 0
                let nextLeft = left - previousLeft + highpass * filteredLeft
                let nextRight = right - previousRight + highpass * filteredRight
                let differenceLeft = nextLeft - filteredLeft, differenceRight = nextRight - filteredRight
                sum += nextLeft * nextLeft + nextRight * nextRight
                highSum += differenceLeft * differenceLeft + differenceRight * differenceRight
                previousLeft = left; previousRight = right; filteredLeft = nextLeft; filteredRight = nextRight
            }
            let divisor = Double(2 * (finish - begin))
            energy[window] = sqrt(sum / divisor); brightness[window] = sqrt(highSum / divisor)
            peakEnergy = max(peakEnergy, energy[window]); peakBrightness = max(peakBrightness, brightness[window])
        }
        var markers = [0.0], candidateCount = 0
        if peakEnergy > 0.000_001 && maximumSlices > 1 && windows > 2 {
            let history = max(2, Int((0.04 * sample.sampleRate / Double(hop)).rounded()))
            let amplitudeFloor = max(0.000_001, peakEnergy * (0.045 - sensitivity * 0.04))
            let brightnessFloor = max(0.000_001, peakBrightness * (0.045 - sensitivity * 0.04))
            let threshold = 2.9 - sensitivity * 1.35
            var novelty = [Double](repeating: 0, count: windows)
            var sum = 0.0, highSum = 0.0
            for window in 0..<windows {
                let prior = Double(max(1, min(window, history)))
                let ratio = energy[window] > amplitudeFloor ? energy[window] / max(amplitudeFloor, sum / prior) : 0
                let highRatio = brightness[window] > brightnessFloor ? brightness[window] / max(brightnessFloor, highSum / prior) : 0
                // A bright release/cut edge is not a new attack: require rising energy.
                novelty[window] = ratio >= 1.1 ? max(ratio, highRatio * 0.9) : 0
                sum += energy[window]; highSum += brightness[window]
                if window >= history { sum -= energy[window - history]; highSum -= brightness[window - history] }
            }
            var candidates: [Candidate] = []
            candidates.reserveCapacity(min(windows / 2, count / gap + 1))
            for window in 1..<(windows - 1) {
                let strength = novelty[window]
                guard strength >= threshold, strength >= novelty[window - 1], strength > novelty[window + 1] else { continue }
                // Back up to the attack foot, using only a bounded 8 ms window. The
                // returned frame remains inside the selected region; no zero-crossing
                // snapping moves markers past the attack or changes the source file.
                var onset = window
                let lookback = max(1, Int(0.008 * sample.sampleRate / Double(hop)))
                let floor = max(amplitudeFloor, energy[window] * 0.12)
                while onset > max(0, window - lookback), energy[onset - 1] > floor,
                      energy[onset - 1] <= energy[onset] * 1.15 { onset -= 1 }
                let frame = onset * hop
                guard frame >= gap, count - frame >= max(1, hop) else { continue }
                let candidate = Candidate(frame: frame, strength: strength)
                if let previous = candidates.last, frame - previous.frame < gap {
                    if frame >= previous.frame && strength > previous.strength { candidates[candidates.count - 1] = candidate }
                } else { candidates.append(candidate) }
            }
            candidateCount = candidates.count
            if candidates.count > maximumSlices - 1 {
                candidates.sort { $0.strength == $1.strength ? $0.frame < $1.frame : $0.strength > $1.strength }
                candidates.removeSubrange((maximumSlices - 1)...)
            }
            candidates.sort { $0.frame < $1.frame }
            markers += candidates.map { Double($0.frame) / Double(count) }
        }
        return ["method": "transient", "slicePoints": markers, "slices": markers, "sliceCount": markers.count,
                "sensitivity": sensitivity, "minimumGapSeconds": gapSeconds, "maximumSlices": maximumSlices,
                "start": start, "end": end, "duration": Double(count) / sample.sampleRate,
                "analyzedFrames": count, "analysisWindows": windows, "analysisWindowSeconds": Double(hop) / sample.sampleRate,
                "candidateCount": candidateCount, "capped": candidateCount + 1 > maximumSlices]
    }
}

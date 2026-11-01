// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation

/// JSON-to-render integration checks. These exercise the host commands used by the
/// editor, while the engine tests separately cover the modulation DSP itself.
enum NativeModulationTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("modulation", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let host = try AudioHost(midiInputIDs: [])
        defer { host.shutdown() }
        var failures: [String] = []
        var measurements: [String: Any] = [:]
        func check(_ condition: Bool, _ message: String) {
            if !condition { failures.append(message) }
        }
        func rmsDifference(_ a: DecodedSample, _ b: DecodedSample) -> Double {
            let count = min(a.left.count, b.left.count)
            guard count > 0 else { return 0 }
            var sum = 0.0
            for index in 0..<count {
                let left = Double(a.left[index] - b.left[index])
                let right = Double(a.right[index] - b.right[index])
                sum += left * left + right * right
            }
            return sqrt(sum / Double(count * 2))
        }
        func stereoDifference(_ sample: DecodedSample, from: Double, to: Double) -> Double {
            let first = max(0, min(sample.left.count, Int(from * sample.sampleRate)))
            let last = max(first, min(sample.left.count, Int(to * sample.sampleRate)))
            guard last > first else { return 0 }
            var sum = 0.0
            for index in first..<last {
                let value = Double(sample.left[index] - sample.right[index])
                sum += value * value
            }
            return sqrt(sum / Double(last - first))
        }
        func lane(_ target: String, _ points: [[String: Double]]) -> [String: Any] {
            ["track": 0, "target": target, "interpolation": "step", "points": points]
        }
        func render(_ name: String, instrument: [String: Any], automation: [[String: Any]] = [], gate: Double = 4) async throws -> DecodedSample {
            var rows = [Any](repeating: NSNull(), count: 8)
            rows[0] = ["note": 60, "instrument": 0, "velocity": 110, "gate": gate]
            let project: [String: Any] = ["version": 1, "name": "Modulation verification", "bpm": 120,
                "linesPerBeat": 4, "masterVolume": 0.7, "activePattern": 0, "loopSong": false,
                "tracks": [["instrument": 0, "volume": 0.7, "pan": 0, "delaySend": 0, "reverbSend": 0]],
                "instruments": [instrument], "patterns": [["id": 0, "length": 8, "steps": [rows], "automation": automation]],
                "sequence": [0], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
            let file = folder.appendingPathComponent(name + ".wav")
            _ = try await host.handle("audio.export", payload: ["path": file.path, "mode": "song", "tail": 0])
            let result = try AudioSamples.decode(file.path)
            check(abs(result.duration - 1) < 0.001, "\(name) must render one complete second.")
            check(result.left.allSatisfy { $0.isFinite } && result.right.allSatisfy { $0.isFinite }, "\(name) contains non-finite PCM.")
            check(result.left.contains { abs($0) > 0.005 }, "\(name) must contain audible synth PCM.")
            return result
        }

        // A half-second held saw with an independent amplitude release exposes
        // every filter ADSR stage, including filter release after note-off.
        let filterInstrument: [String: Any] = ["type": "synth", "waveform": "saw", "volume": 0.5,
            "attack": 0, "decay": 0, "sustain": 1, "release": 0.45, "cutoff": 250, "resonance": 0.1,
            "filterAttack": 0.01, "filterDecay": 0.1, "filterSustain": 0.2, "filterRelease": 0.05,
            "filterEnvAmount": 36, "lfoRate": 0, "lfoDepth": 0, "lfo2Rate": 0, "lfo2Depth": 0]
        let filterBaseline = try await render("filter-envelope-baseline", instrument: filterInstrument)
        let filterChanges: [(String, Double)] = [("filterAttack", 0.2), ("filterDecay", 0.35),
            ("filterSustain", 0.9), ("filterRelease", 0.4), ("filterEnvAmount", 0)]
        for (target, value) in filterChanges {
            var changed = filterInstrument
            changed[target] = value
            let direct = try await render(target + "-instrument", instrument: changed)
            let automated = try await render(target + "-automation", instrument: filterInstrument,
                automation: [lane(target, [["row": 0, "value": value]])])
            let audibleChange = rmsDifference(filterBaseline, direct)
            let mappingError = rmsDifference(direct, automated)
            measurements[target] = ["parameterChangeRMS": audibleChange, "automationMappingErrorRMS": mappingError]
            check(audibleChange > 0.0001, "The mapped \(target) instrument parameter must audibly alter its envelope stage.")
            check(mappingError < 0.000002, "Automation target \(target) must match setting the same instrument parameter; RMS error \(mappingError).")
        }

        // Keep LFO1 active on pitch. LFO2 must independently modulate pan without
        // replacing that first oscillator, and its triangle shape starts on the left.
        var lfoInstrument: [String: Any] = ["type": "synth", "waveform": "sine", "volume": 0.5,
            "attack": 0, "decay": 0, "sustain": 1, "release": 0.01, "cutoff": 18_000,
            "filterEnvAmount": 0, "lfoRate": 1.3, "lfoDepth": 0.02, "lfoTarget": "pitch", "lfoShape": "sine",
            "lfo2Rate": 4, "lfo2Depth": 0, "lfo2Target": "pan", "lfo2Shape": "triangle"]
        let mono = try await render("lfo1-pitch", instrument: lfoInstrument, gate: 7.5)
        lfoInstrument["lfo2Depth"] = 0.8
        let stereo = try await render("lfo1-pitch-lfo2-pan", instrument: lfoInstrument, gate: 7.5)
        let monoDifference = stereoDifference(mono, from: 0.01, to: 0.9)
        let stereoAmount = stereoDifference(stereo, from: 0.01, to: 0.9)
        let onsetFrames = Int(stereo.sampleRate * 0.01)
        let onsetLeftEnergy = stereo.left.prefix(onsetFrames).reduce(0.0) { $0 + Double($1) * Double($1) }
        let onsetRightEnergy = stereo.right.prefix(onsetFrames).reduce(0.0) { $0 + Double($1) * Double($1) }
        check(monoDifference < 0.000001 && stereoAmount > 0.01,
              "Independent LFO2 pan must create stereo movement while LFO1 pitch alone remains centered.")
        check(onsetLeftEnergy > onsetRightEnergy * 4,
              "The mapped LFO2 triangle shape must begin with a strong leftward pan.")
        var withoutLFO1 = lfoInstrument
        withoutLFO1["lfoDepth"] = 0
        let panOnly = try await render("lfo2-pan-alone", instrument: withoutLFO1, gate: 7.5)
        let independentDifference = rmsDifference(stereo, panOnly)
        check(independentDifference > 0.001, "Enabling LFO2 must preserve the audible contribution of LFO1.")
        measurements["independentLFOs"] = ["lfo1StereoDifferenceRMS": monoDifference,
            "lfo2StereoDifferenceRMS": stereoAmount, "lfo1ContributionRMS": independentDifference,
            "triangleOnsetLeftEnergy": onsetLeftEnergy, "triangleOnsetRightEnergy": onsetRightEnergy]

        // Constant automation must produce the same PCM as the corresponding
        // instrument setting. This catches either lane being mapped to LFO1.
        for (target, value) in [("lfo2Rate", 7.0), ("lfo2Depth", 0.3)] {
            var changed = lfoInstrument
            changed[target] = value
            let direct = try await render(target + "-instrument", instrument: changed, gate: 7.5)
            let automated = try await render(target + "-automation", instrument: lfoInstrument,
                automation: [lane(target, [["row": 0, "value": value]])], gate: 7.5)
            let audibleChange = rmsDifference(stereo, direct)
            let mappingError = rmsDifference(direct, automated)
            measurements[target] = ["parameterChangeRMS": audibleChange, "automationMappingErrorRMS": mappingError]
            check(audibleChange > 0.001, "Mapped \(target) must audibly change the second LFO.")
            check(mappingError < 0.000002, "Automation target \(target) must match the instrument setting; RMS error \(mappingError).")
        }

        let stepped = try await render("lfo2-depth-step", instrument: lfoInstrument,
            automation: [lane("lfo2Depth", [["row": 0, "value": 0], ["row": 4, "value": 0.8]])], gate: 7.5)
        let beforeStep = stereoDifference(stepped, from: 0.05, to: 0.45)
        let afterStep = stereoDifference(stepped, from: 0.55, to: 0.9)
        measurements["lfo2DepthStep"] = ["beforeStepStereoRMS": beforeStep, "afterStepStereoRMS": afterStep]
        check(beforeStep < 0.000001 && afterStep > 0.01,
              "LFO2 depth automation must keep early rows centered and introduce panning at row4.")

        // At 120 BPM, half a quarter-note beat per cycle is exactly 4 Hz. Compare
        // synced rendering against that independent free-rate reference for each
        // slot, then verify both beat-lane addresses and the inactive Hz lanes.
        for prefix in ["lfo", "lfo2"] {
            let syncKey = prefix + "Sync", beatsKey = prefix + "Beats", rateKey = prefix + "Rate"
            var syncInstrument: [String: Any] = ["type": "synth", "waveform": "sine", "volume": 0.5,
                "attack": 0, "decay": 0, "sustain": 1, "release": 0.01, "cutoff": 18_000,
                "filterEnvAmount": 0, "lfoRate": 0, "lfoDepth": 0, "lfoTarget": "pan", "lfoShape": "triangle",
                "lfo2Rate": 0, "lfo2Depth": 0, "lfo2Target": "pan", "lfo2Shape": "triangle"]
            syncInstrument[prefix + "Depth"] = 0.8
            syncInstrument[rateKey] = 0.75
            syncInstrument[beatsKey] = 0.5
            // Omitting Sync must retain the legacy free-Hz behavior.
            let free = try await render(prefix + "-sync-disabled", instrument: syncInstrument, gate: 7.5)
            syncInstrument[syncKey] = true
            let synced = try await render(prefix + "-sync-half-beat", instrument: syncInstrument, gate: 7.5)
            var freeReference = syncInstrument
            freeReference[syncKey] = false
            freeReference[rateKey] = 4.0
            let expected = try await render(prefix + "-sync-reference-4hz", instrument: freeReference, gate: 7.5)
            let enabledDifference = rmsDifference(free, synced)
            let syncRateError = rmsDifference(synced, expected)
            check(enabledDifference > 0.001 && syncRateError < 0.000002,
                  "\(prefix) Sync and Beats JSON must select the correct tempo-derived rate while omitted Sync preserves free Hz.")

            var slowerInstrument = syncInstrument
            slowerInstrument[beatsKey] = 1.0
            let slower = try await render(prefix + "-sync-one-beat", instrument: slowerInstrument, gate: 7.5)
            let beatAutomated = try await render(prefix + "-sync-beat-automation", instrument: syncInstrument,
                automation: [lane(beatsKey, [["row": 0, "value": 1]])], gate: 7.5)
            let beatDifference = rmsDifference(synced, slower)
            let beatMappingError = rmsDifference(slower, beatAutomated)
            check(beatDifference > 0.001 && beatMappingError < 0.000002,
                  "Automation target \(beatsKey) must audibly change the synced cycle and match its instrument setting.")

            let hzAutomated = try await render(prefix + "-sync-inactive-hz-lane", instrument: syncInstrument,
                automation: [lane(rateKey, [["row": 0, "value": 17], ["row": 4, "value": 3]])], gate: 7.5)
            let inactiveHzError = rmsDifference(synced, hzAutomated)
            check(inactiveHzError < 0.000002,
                  "Automation target \(rateKey) must remain inaudible while \(syncKey) is enabled.")
            measurements[prefix + "Sync"] = ["syncEnabledDifferenceRMS": enabledDifference,
                "tempoRateReferenceErrorRMS": syncRateError, "beatChangeRMS": beatDifference,
                "beatAutomationMappingErrorRMS": beatMappingError, "inactiveHzAutomationErrorRMS": inactiveHzError]
        }

        measurements["failures"] = failures
        try JSONSerialization.data(withJSONObject: measurements, options: [.prettyPrinted, .sortedKeys])
            .write(to: folder.appendingPathComponent("measurements.json"))
        guard failures.isEmpty else {
            throw AudioSampleError.invalid("Modulation integration: " + failures.joined(separator: " "))
        }
        return ["Filter ADSR and amount JSON parameters audibly control an independent filter envelope",
                "All seven filter-envelope and LFO2 automation targets reproduce their instrument parameter PCM",
                "Independent LFO1 and LFO2 remain audible together and LFO2 depth automation starts on its assigned row",
                "Both synced LFOs follow beat settings and beat automation while their free-Hz automation remains inactive"]
    }
}

#endif

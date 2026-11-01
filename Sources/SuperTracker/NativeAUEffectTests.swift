// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AudioToolbox

/// End-to-end effect verification through the same commands used by the application.
/// Fixtures are rendered offline: no test audio is sent to the user's speakers.
enum NativeAUEffectTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("effects", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let host = try AudioHost(midiInputIDs: [])
        defer { host.shutdown() }
        var passed: [String] = []
        var measurements: [String: Any] = [:]
        var failures: [String] = []
        func check(_ condition: Bool, _ message: String) throws {
            if !condition { failures.append(message) }
        }
        func energy(_ sample: DecodedSample, from: Double = 0, to: Double? = nil) -> Double {
            let first = max(0, min(sample.left.count, Int(from * sample.sampleRate)))
            let last = max(first, min(sample.left.count, Int((to ?? sample.duration) * sample.sampleRate)))
            guard last > first else { return 0 }
            var sum = 0.0
            for index in first..<last {
                sum += Double(sample.left[index]) * Double(sample.left[index])
                sum += Double(sample.right[index]) * Double(sample.right[index])
            }
            return sqrt(sum / Double((last - first) * 2))
        }
        func difference(_ a: DecodedSample, _ b: DecodedSample) -> Double {
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
        func render(_ name: String, bounce: Bool = false) async throws -> DecodedSample {
            let file = folder.appendingPathComponent(name + ".wav")
            _ = try await host.handle(bounce ? "audio.bounceTrack" : "audio.export",
                payload: ["path": file.path, "mode": "song", "track": 0, "tail": 1.0])
            let sample = try AudioSamples.decode(file.path)
            try check(sample.left.allSatisfy { $0.isFinite } && sample.right.allSatisfy { $0.isFinite },
                      "\(name) produced non-finite samples.")
            try check(abs(sample.duration - 2) < 0.002, "\(name) should contain a one-second pattern plus one-second tail.")
            return sample
        }
        func load(_ identifier: String, kind: String = "effect", slot: Int = 0) async throws {
            _ = try await host.handle("plugin.load", payload: ["track": 0, "kind": kind, "slot": slot, "component": identifier])
        }
        func set(_ address: AudioUnitParameterID, _ value: Double, slot: Int = 0) async throws {
            _ = try await host.handle("plugin.parameter", payload: ["track": 0, "kind": "effect", "slot": slot,
                "address": String(address), "value": value, "normalized": false])
        }
        func apply(_ project: [String: Any]) async throws {
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        }

        // A short harmonic pulse has a precise end for tail tests and enough spectral
        // content to expose nonlinear/filter ordering. No random source or external asset.
        let sourceURL = folder.appendingPathComponent("source-pulse.wav")
        let rate = 48_000.0
        var pulse = [Float](repeating: 0, count: 2_400)
        for index in pulse.indices {
            let time = Double(index) / rate
            let envelope = sin(Double.pi * Double(index) / Double(pulse.count))
            pulse[index] = Float(envelope * (sin(2 * .pi * 220 * time) * 0.5 + sin(2 * .pi * 2_730 * time) * 0.22))
        }
        try AudioSamples.write(DecodedSample(path: sourceURL.path, sampleRate: rate, left: pulse, right: pulse, originalChannels: 2), to: sourceURL)
        var rows = [Any](repeating: NSNull(), count: 8)
        rows[0] = ["note": 60, "instrument": 0, "velocity": 110, "gate": 0.9]
        var project: [String: Any] = ["version": 1, "name": "Effect verification", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Pulse", "instrument": 0, "volume": 0.7, "pan": 0, "delaySend": 0, "reverbSend": 0]],
            "instruments": [["type": "sample", "samplePath": sourceURL.path, "rootNote": 60, "volume": 0.8,
                             "mode": "oneshot", "attack": 0, "decay": 0, "sustain": 1, "release": 0.001, "cutoff": 20_000]],
            "patterns": [["id": 0, "length": 8, "steps": [rows], "automation": []]], "sequence": [0],
            "audioPlugins": [], "delay": ["time": 0.25, "feedback": 0.5, "mix": 0],
            "reverb": ["size": 0.7, "damping": 0.4, "mix": 0]]
        try await apply(project)
        let available = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
        func component(_ name: String) throws -> String {
            guard let entry = available.first(where: { ($0["name"] as? String ?? "").localizedCaseInsensitiveCompare(name) == .orderedSame }),
                  let identifier = entry["id"] as? String else {
                throw AudioSampleError.invalid("AU effect integration requires the macOS system Audio Unit \(name).")
            }
            return identifier
        }
        let delayID = try component("AUDelay"), distortionID = try component("AUDistortion")
        let lowpassID = try component("AULowpass"), instrumentID = try component("DLSMusicDevice")
        let dry = try await render("sample-dry")
        try check(energy(dry, to: 0.1) > 0.01 && energy(dry, from: 0.15) < 0.000001,
                  "The dry pulse must be audible and end before delay measurements begin.")

        try await load(delayID)
        try await set(kDelayParam_WetDryMix, 100)
        try await set(kDelayParam_DelayTime, 0.2)
        try await set(kDelayParam_Feedback, 0)
        try await set(kDelayParam_LopassCutoff, 20_000)
        let delayed = try await render("insert-delay-200ms")
        let delayedTail = energy(delayed, from: 0.18, to: 0.32)
        try check(energy(delayed, to: 0.12) < energy(dry, to: 0.12) * 0.05 && delayedTail > 0.005,
                  "AUDelay at 100% wet must move the pulse into the 200ms echo window.")
        _ = try await host.handle("plugin.parameter", payload: ["track": 0, "kind": "effect", "slot": 0, "bypass": true])
        let bypassed = try await render("insert-delay-bypassed")
        let bypassDifference = difference(dry, bypassed)
        try check(bypassDifference < 0.00001, "Bypassing AUDelay must reproduce dry PCM; RMS difference \(bypassDifference).")
        _ = try await host.handle("plugin.parameter", payload: ["track": 0, "kind": "effect", "slot": 0, "bypass": false])
        try await set(kDelayParam_DelayTime, 0.4)
        let slower = try await render("insert-delay-400ms")
        try check(energy(slower, from: 0.18, to: 0.32) < delayedTail * 0.05 && energy(slower, from: 0.38, to: 0.52) > 0.005,
                  "Changing the delay-time parameter must move the audible echo from 200ms to 400ms.")
        measurements["delay200msEchoRMS"] = delayedTail
        measurements["bypassDryDifferenceRMS"] = bypassDifference
        measurements["delayTimeChangeDifferenceRMS"] = difference(delayed, slower)
        passed.append("AUDelay insertion, bypass and delay-time edits audibly alter the expected PCM windows")

        // Track bounce must traverse actual insert processing rather than copying the
        // original sample. Here the dry onset disappears and the delayed onset remains.
        let insertBounce = try await render("sample-insert-bounce", bounce: true)
        try check(difference(insertBounce, slower) < 0.00001 && energy(insertBounce, from: 0.38, to: 0.52) > 0.005,
                  "Track bounce must include the active AUDelay insert.")
        passed.append("Track bounce retains audible Audio Unit insert processing")

        _ = try await host.handle("plugin.remove", payload: ["track": 0, "kind": "effect", "slot": 0])
        try await load(distortionID)
        for (address, value): (AudioUnitParameterID, Double) in [
            (kDistortionParam_DelayMix, 0), (kDistortionParam_DecimationMix, 0),
            (kDistortionParam_LinearTerm, 1), (kDistortionParam_SquaredTerm, 0),
            (kDistortionParam_CubicTerm, 18), (kDistortionParam_PolynomialMix, 100),
            (kDistortionParam_RingModMix, 0), (kDistortionParam_SoftClipGain, 8), (kDistortionParam_FinalMix, 100)] {
            try await set(address, value)
        }
        try await load(lowpassID, slot: 1)
        try await set(kLowPassParam_CutoffFrequency, 450, slot: 1)
        try await set(kLowPassParam_Resonance, 0, slot: 1)
        let distortedThenFiltered = try await render("chain-distortion-lowpass")
        let chainSnapshot = try await host.handle("engine.snapshot", payload: [:]) as! [String: Any]
        var reversedChain = chainSnapshot["audioPlugins"] as! [[String: Any]]
        for index in reversedChain.indices { reversedChain[index]["slot"] = 1 - (reversedChain[index]["slot"] as! Int) }
        project["audioPlugins"] = reversedChain
        try await apply(project)
        let filteredThenDistorted = try await render("chain-lowpass-distortion")
        let orderDifference = difference(distortedThenFiltered, filteredThenDistorted)
        try check(energy(distortedThenFiltered) > 0.0001 && energy(filteredThenDistorted) > 0.0001 && orderDifference > 0.0001,
                  "Reversing distortion and lowpass must change audible PCM; RMS difference \(orderDifference).")
        measurements["chainOrderDifferenceRMS"] = orderDifference
        passed.append("Serial AU effect order changes rendered audio for distortion and lowpass")

        // Verify each send independently. A nonzero delay cannot hide a broken reverb,
        // and vice versa. Bounce compares include the native DSP return source bus.
        project["audioPlugins"] = []
        try await apply(project)
        let nativeDryBounce = try await render("native-bounce-dry", bounce: true)
        var tracks = project["tracks"] as! [[String: Any]]
        tracks[0]["delaySend"] = 0.9; project["tracks"] = tracks
        project["delay"] = ["time": 0.25, "feedback": 0.55, "mix": 0.8]
        try await apply(project)
        let nativeDelayBounce = try await render("native-bounce-delay-send", bounce: true)
        tracks[0]["delaySend"] = 0; tracks[0]["reverbSend"] = 0.9; project["tracks"] = tracks
        project["delay"] = ["mix": 0]; project["reverb"] = ["size": 0.8, "damping": 0.25, "mix": 0.8]
        try await apply(project)
        let nativeReverbBounce = try await render("native-bounce-reverb-send", bounce: true)
        let nativeDelayTail = energy(nativeDelayBounce, from: 0.2)
        let nativeReverbTail = energy(nativeReverbBounce, from: 0.2)
        try check(nativeDelayTail > 0.0001 && nativeReverbTail > 0.00001 && energy(nativeDryBounce, from: 0.2) < 0.000001,
                  "Native track bounces must include independently audible delay and reverb tails.")
        measurements["nativeDelaySendTailRMS"] = nativeDelayTail
        measurements["nativeReverbSendTailRMS"] = nativeReverbTail
        passed.append("Native track bounce includes separately verified delay and reverb send tails")

        // Repeat the send test with a sequenced AU instrument: it takes a different
        // signal path from native voices and must reach both auxiliary return buses.
        tracks[0]["delaySend"] = 0; tracks[0]["reverbSend"] = 0; project["tracks"] = tracks
        project["delay"] = ["mix": 0]; project["reverb"] = ["mix": 0]
        project["instruments"] = [["type": "au", "volume": 0.8, "pan": 0]]
        project["audioPlugins"] = [["track": 0, "kind": "instrument", "slot": 0, "component": instrumentID]]
        try await apply(project)
        let auDryBounce = try await render("au-bounce-dry", bounce: true)
        let dryTail = energy(auDryBounce, from: 0.5)
        tracks[0]["delaySend"] = 0.9; project["tracks"] = tracks
        project["delay"] = ["time": 0.3, "feedback": 0.65, "mix": 0.9]
        try await apply(project)
        let auDelayBounce = try await render("au-bounce-delay-send", bounce: true)
        tracks[0]["delaySend"] = 0; tracks[0]["reverbSend"] = 0.9; project["tracks"] = tracks
        project["delay"] = ["mix": 0]; project["reverb"] = ["size": 0.8, "damping": 0.25, "mix": 0.9]
        try await apply(project)
        let auReverbBounce = try await render("au-bounce-reverb-send", bounce: true)
        let auDelayTail = energy(auDelayBounce, from: 0.5), auReverbTail = energy(auReverbBounce, from: 0.5)
        try check(energy(auDryBounce) > 0.0001 && auDelayTail > max(0.000001, dryTail * 1.2)
            && auReverbTail > max(0.000001, dryTail * 1.2),
                  "AU instrument bounces must include both send returns; dry/delay/reverb tail RMS \(dryTail)/\(auDelayTail)/\(auReverbTail).")
        measurements["auDryTailRMS"] = dryTail
        measurements["auDelaySendTailRMS"] = auDelayTail
        measurements["auReverbSendTailRMS"] = auReverbTail
        passed.append("AU instrument track bounce includes separately verified delay and reverb send tails")
        measurements["failures"] = failures
        let data = try JSONSerialization.data(withJSONObject: measurements, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
        if !failures.isEmpty { throw AudioSampleError.invalid("AU effect integration: " + failures.joined(separator: "\n")) }
        passed += try await NativeAUMixTimingTests.run(output: output)
        return passed
    }
}

#endif

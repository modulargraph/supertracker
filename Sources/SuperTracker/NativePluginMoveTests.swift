// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AudioToolbox

/// Effect moves through public host commands, including real offline PCM. Only
/// Apple AUs and an original harmonic sample are used; no audio is played live.
enum NativePluginMoveTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("plugin-move", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var measurements: [String: Any] = [:], passed: [String] = []
        defer {
            measurements["passed"] = passed
            if let data = try? JSONSerialization.data(withJSONObject: measurements, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Plug-in move regression: " + message) }
        }
        var instantiations = 0
        var resumeFailure:Error?
        let host = try AudioHost(beforePluginInstantiation: { instantiations += 1 },beforeEffectOrderResume:{
            if let error = resumeFailure { throw error }
        }, midiInputIDs: [])
        defer { host.shutdown() }
        let available = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
        func component(_ name: String) throws -> String {
            guard let found = available.first(where: { ($0["name"] as? String) == name }), let id = found["id"] as? String else {
                throw AudioSampleError.invalid("Move tests require the macOS Audio Unit " + name + ".")
            }
            return id
        }
        let delayID = try component("AUDelay"), instrumentID = try component("DLSMusicDevice")
        let distortionID = try component("AUDistortion"), lowpassID = try component("AULowpass")
        func snapshot(_ host: AudioHost) async throws -> [String: Any] {
            try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
        }
        func apply(_ host: AudioHost, _ project: [String: Any]) async throws {
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
            await host.pluginRestorationBarrier()?.value
        }
        func load(_ host: AudioHost, _ id: String, track: Int = 0, kind: String = "effect", slot: Int) async throws {
            _ = try await host.handle("plugin.load", payload: ["track": track, "kind": kind, "slot": slot, "component": id])
        }
        func parameter(_ host: AudioHost, _ address: AudioUnitParameterID, _ value: Double,
                       track: Int = 0, slot: Int) async throws {
            _ = try await host.handle("plugin.parameter", payload: ["track": track, "kind": "effect", "slot": slot,
                "address": String(address), "normalized": false, "value": value])
        }
        func descriptors(_ project: [String: Any], track: Int = 0, kind: String = "effect") -> [[String: Any]] {
            (project["audioPlugins"] as? [[String: Any]] ?? []).filter {
                ($0["track"] as? Int) == track && ($0["kind"] as? String) == kind
            }.sorted { ($0["slot"] as? Int ?? -1) < ($1["slot"] as? Int ?? -1) }
        }
        func verifyDescriptors(_ actual: [[String: Any]], _ expected: [[String: Any]], _ label: String) throws {
            let actual = actual.sorted { ($0["slot"] as? Int ?? -1) < ($1["slot"] as? Int ?? -1) }
            let expected = expected.sorted { ($0["slot"] as? Int ?? -1) < ($1["slot"] as? Int ?? -1) }
            try require(actual.count == expected.count, "\(label) changed the number of instances.")
            for (a, b) in zip(actual, expected) {
                try require(a["instanceID"] as? String == b["instanceID"] as? String &&
                            a["instanceID"] is String && a["component"] as? String == b["component"] as? String &&
                            a["slot"] as? Int == b["slot"] as? Int && a["track"] as? Int == b["track"] as? Int &&
                            a["kind"] as? String == b["kind"] as? String &&
                            (a["bypass"] as? Bool ?? false) == (b["bypass"] as? Bool ?? false),
                            "\(label) changed instance identity, target, component, or bypass.")
                try require(equivalentState(a["state"] as? String, b["state"] as? String),
                            "\(label) replaced an instance's opaque state.")
            }
        }
        func verifyProject(_ actual: [String: Any], _ expected: [String: Any], _ label: String) throws {
            try verifyDescriptors(descriptors(actual), descriptors(expected), label)
            try verifyDescriptors(descriptors(actual, kind: "instrument"), descriptors(expected, kind: "instrument"), label + " instrument")
            try verifyDescriptors(descriptors(actual, track: 1), descriptors(expected, track: 1), label + " other track")
            let a = actual["patterns"] as? [[String: Any]] ?? [], b = expected["patterns"] as? [[String: Any]] ?? []
            try require(a.count == b.count, "\(label) changed pattern count.")
            for (left, right) in zip(a, b) {
                try require(NSArray(array: left["automation"] as? [Any] ?? []).isEqual(to: right["automation"] as? [Any] ?? []),
                            "\(label) changed or missed an automation target in a pattern.")
            }
        }
        func remapped(_ project: [String: Any], _ slots: [Int: Int]) -> [String: Any] {
            var result = project
            var plugins = project["audioPlugins"] as? [[String: Any]] ?? []
            for index in plugins.indices where plugins[index]["track"] as? Int == 0 && plugins[index]["kind"] as? String == "effect" {
                if let slot = plugins[index]["slot"] as? Int, let replacement = slots[slot] { plugins[index]["slot"] = replacement }
            }
            result["audioPlugins"] = plugins
            var patterns = project["patterns"] as? [[String: Any]] ?? []
            for index in patterns.indices {
                var lanes = patterns[index]["automation"] as? [[String: Any]] ?? []
                for lane in lanes.indices where lanes[lane]["track"] as? Int == 0 {
                    var parts = (lanes[lane]["target"] as? String ?? "").split(separator: ":").map(String.init)
                    let position: Int?
                    if parts.count == 3 && parts[0] == "plugin" { position = 1 }
                    else if parts.count == 4 && parts[0] == "plugin" && parts[1] == "effect" { position = 2 }
                    else { position = nil }
                    if let position, let slot = Int(parts[position]), let replacement = slots[slot] {
                        parts[position] = String(replacement); lanes[lane]["target"] = parts.joined(separator: ":")
                    }
                }
                patterns[index]["automation"] = lanes
            }
            result["patterns"] = patterns
            return result
        }
        func lane(_ target: String, track: Int = 0, value: Double = 0.25) -> [String: Any] {
            ["track": track, "target": target, "interpolation": "step", "points": [["row": 0, "value": value]]]
        }
        let rows = [Any](repeating: NSNull(), count: 4)
        let base: [String: Any] = ["version": 1, "name": "Sparse AU move verification", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 0.5, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Moved", "instrument": 0, "volume": 0.7], ["name": "Unchanged", "instrument": 1, "volume": 0.7]],
            "instruments": [["type": "au", "volume": 0.5], ["type": "synth", "volume": 0.5]],
            "patterns": [["id": 0, "length": 4, "steps": [rows, rows], "automation": []]],
            "sequence": [0], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        try await apply(host, base)
        try await load(host, instrumentID, kind: "instrument", slot: 0)
        for (slot, time) in [(0, 0.11), (3, 0.33), (7, 0.77)] {
            try await load(host, delayID, slot: slot)
            try await parameter(host, kDelayParam_DelayTime, time, slot: slot)
        }
        _ = try await host.handle("plugin.parameter", payload: ["track": 0, "kind": "effect", "slot": 3, "bypass": true])
        try await load(host, delayID, track: 1, slot: 3)
        try await parameter(host, kDelayParam_DelayTime, 0.91, track: 1, slot: 3)
        var original = try await snapshot(host)
        original["patterns"] = [
            ["id": 0, "length": 4, "steps": [rows, rows], "automation": [
                lane("plugin:effect:0:0"), lane("plugin:3:0"), lane("plugin:effect:7:0"),
                lane("plugin:instrument:0:0"), lane("plugin:effect:3:0", track: 1), lane("volume")]],
            ["id": 1, "length": 4, "steps": [rows, rows], "automation": [
                lane("plugin:7:0", value: 0.73), lane("plugin:effect:3:0", value: 0.33),
                lane("plugin:effect:0:0", value: 0.12), lane("plugin:instrument:0:1")]]]
        try await apply(host, original)
        original = try await snapshot(host)
        let initialLoads = instantiations
        let ids = descriptors(original).compactMap { $0["instanceID"] as? String }
        try require(initialLoads == 5 && ids.count == 3 && Set(ids).count == 3,
                    "Fixture must contain three distinct same-component effects, one AU instrument, and one unrelated effect.")
        let forwardMap = [0: 7, 3: 0, 7: 3], forwardExpected = remapped(original, [0: 7, 3: 0, 7: 3])
        let forward = try await host.handle("plugin.move", payload: ["track": 0, "fromSlot": 0, "toSlot": 7]) as? [String: Any] ?? [:]
        try require(forward["moved"] as? Bool == true && forward["track"] as? Int == 0 &&
                    forward["fromSlot"] as? Int == 0 && forward["toSlot"] as? Int == 7,
                    "Move result omitted its committed target.")
        try verifyDescriptors(forward["previousPlugins"] as? [[String: Any]] ?? [], descriptors(original), "forward previousPlugins")
        try verifyDescriptors(forward["plugins"] as? [[String: Any]] ?? [], descriptors(forwardExpected), "forward plugins")
        let map = forward["slotMap"] as? [[String: Any]] ?? []
        try require(map.count == 3, "The three-effect move did not return all slot mappings.")
        for entry in map {
            let from = entry["fromSlot"] as? Int ?? -1, to = entry["toSlot"] as? Int ?? -1
            let old = descriptors(original).first { $0["slot"] as? Int == from }
            try require(forwardMap[from] == to && old?["instanceID"] as? String == entry["instanceID"] as? String,
                        "slotMap did not follow the original instance identity.")
        }
        let movedSnapshot = try await snapshot(host)
        try verifyProject(movedSnapshot, forwardExpected, "sparse forward move")
        try require(descriptors(movedSnapshot).compactMap { $0["slot"] as? Int } == [0, 3, 7] && instantiations == initialLoads,
                    "Moving compacted numeric gaps or instantiated another plug-in.")
        _ = try await host.handle("plugin.move", payload: ["track": 0, "fromSlot": 7, "toSlot": 0])
        try verifyProject(try await snapshot(host), original, "sparse backward move")
        try require(instantiations == initialLoads, "Backward movement recreated a plug-in.")
        measurements["forwardResult"] = forward
        passed.append("Sparse same-component effect moves preserve instance IDs, distinct state, bypass, instrument, and all-pattern automation")

        // The actual undo/redo mechanism sends complete project snapshots. Stable
        // instance identities distinguish equal-component effects during permutation.
        for (name, project) in [("redo", movedSnapshot), ("undo", original),
                                ("three-way-permutation", remapped(original, [0: 3, 3: 7, 7: 0])), ("restore-original", original)] {
            try await apply(host, project)
            try verifyProject(try await snapshot(host), project, name)
            try require(instantiations == initialLoads, "\(name) instantiated instead of permuting existing instances.")
        }
        measurements["instantiationsAfterMoveUndoRedo"] = instantiations
        passed.append("Undo, redo, and three-way saved-instance permutations preserve AU identities and settings without re-instantiation")

        let invalidRequests: [[String: Any]] = [
            ["track": 0, "fromSlot": 0, "toSlot": 2], ["track": 0, "fromSlot": 2, "toSlot": 7],
            ["track": 0, "fromSlot": -1, "toSlot": 7], ["track": 0, "fromSlot": 0, "toSlot": 8],
            ["track": -1, "fromSlot": 0, "toSlot": 7], ["track": 16, "fromSlot": 0, "toSlot": 7],
            ["track": 1, "fromSlot": 0, "toSlot": 3], ["fromSlot": 0, "toSlot": 7]]
        var rejected = 0
        for request in invalidRequests {
            var didReject = false
            do { _ = try await host.handle("plugin.move", payload: request) } catch { didReject = true }
            try require(didReject, "An invalid move was accepted: \(request)")
            rejected += 1
            try verifyProject(try await snapshot(host), original, "invalid move \(request)")
            try require(instantiations == initialLoads, "An invalid move entered plug-in instantiation.")
        }
        measurements["invalidRequestsRejected"] = rejected
        passed.append("Empty, missing, out-of-range, and invalid-track effect moves leave the project, automation, and instances unchanged")
        // A desired descriptor for another installation/instance must never be
        // silently replaced with the same component currently in that numeric slot.
        for (field,value) in [("bundlePath","/fixture/OtherInstallation.vst3"),("format","VST3")] {
            var incompatible = original
            var plugins = incompatible["audioPlugins"] as! [[String:Any]]
            let index = plugins.firstIndex { ($0["track"] as? Int) == 0 && ($0["kind"] as? String) == "effect" && ($0["slot"] as? Int) == 0 }!
            plugins[index][field] = value; incompatible["audioPlugins"] = plugins
            try await apply(host,incompatible)
            let before = try await snapshot(host), loadsBefore = instantiations
            var denied = false
            do { _ = try await host.handle("plugin.move",payload:["track":0,"fromSlot":0,"toSlot":7]) } catch { denied = true }
            let after = try await snapshot(host)
            try require(denied && instantiations == loadsBefore,"An incompatible saved \(field) reordered the live chain.")
            try verifyProject(after,before,"incompatible saved identity")
            try require(descriptors(after).first?[field] as? String == value,"A rejected move overwrote the requested installation or instance identity.")
            try await apply(host,original)
        }
        // A new valid ID is now an actual replacement request. Once restoration
        // finishes it is no longer a desired/live mismatch: moving that exact new
        // instance must succeed without constructing it again.
        var replacement = original
        var replacementPlugins = replacement["audioPlugins"] as! [[String:Any]]
        let replacementIndex = replacementPlugins.firstIndex { ($0["track"] as? Int) == 0 && ($0["kind"] as? String) == "effect" && ($0["slot"] as? Int) == 0 }!
        let replacementID = UUID().uuidString
        replacementPlugins[replacementIndex]["instanceID"] = replacementID
        replacement["audioPlugins"] = replacementPlugins
        let loadsBeforeReplacement = instantiations
        try await apply(host,replacement)
        try verifyProject(try await snapshot(host),replacement,"completed exact-identity replacement")
        try require(instantiations == loadsBeforeReplacement+1,"A new saved ID did not instantiate exactly its replacement.")
        let loadsBeforeReplacementMove = instantiations
        _ = try await host.handle("plugin.move",payload:["track":0,"fromSlot":0,"toSlot":7])
        try verifyProject(try await snapshot(host),remapped(replacement,forwardMap),"move completed replacement")
        try require(instantiations == loadsBeforeReplacementMove,"Moving the completed replacement instantiated another unit.")
        _ = try await host.handle("plugin.move",payload:["track":0,"fromSlot":7,"toSlot":0])
        try verifyProject(try await snapshot(host),replacement,"inverse completed replacement move")
        try require(instantiations == loadsBeforeReplacementMove,"Returning the completed replacement instantiated another unit.")
        measurements["completedIdentityReplacement"] = ["requestedID":replacementID,"instantiationsBefore":loadsBeforeReplacement,
            "instantiationsAfterRestore":loadsBeforeReplacementMove,"instantiationsAfterMoves":instantiations]
        try await apply(host,original)

        // Preserve the original rejection intent with an identity that truly
        // remains unavailable after restoration, rather than a successfully loaded
        // valid UUID. Its exact unsatisfied descriptor must survive the move.
        var unavailable = original
        var unavailablePlugins = unavailable["audioPlugins"] as! [[String:Any]]
        let unavailableID = UUID().uuidString
        let invalidState = Data("original unavailable-state fixture".utf8).base64EncodedString()
        unavailablePlugins[replacementIndex]["instanceID"] = unavailableID
        unavailablePlugins[replacementIndex]["state"] = invalidState
        unavailable["audioPlugins"] = unavailablePlugins
        try await apply(host,unavailable)
        let beforeUnavailableMove = try await snapshot(host), loadsBeforeUnavailableMove = instantiations
        var unavailableDenied = false
        do { _ = try await host.handle("plugin.move",payload:["track":0,"fromSlot":0,"toSlot":7]) } catch { unavailableDenied = true }
        let afterUnavailableMove = try await snapshot(host)
        try require(unavailableDenied && instantiations == loadsBeforeUnavailableMove,"An unavailable requested identity reordered or recreated the live chain.")
        try verifyProject(afterUnavailableMove,beforeUnavailableMove,"unavailable requested identity")
        let retainedUnavailable = descriptors(afterUnavailableMove).first { $0["slot"] as? Int == 0 }
        try require(retainedUnavailable?["instanceID"] as? String == unavailableID && retainedUnavailable?["state"] as? String == invalidState,
                    "Rejected movement overwrote the unavailable identity or opaque state.")
        measurements["unavailableIdentityMove"] = ["requestedID":unavailableID,"denied":unavailableDenied,
            "instantiationsBefore":loadsBeforeUnavailableMove,"instantiationsAfter":instantiations]
        try await apply(host,original)
        passed.append("Incompatible installations and unavailable identities reject movement while completed identity replacements move without recreation")
        let injected = NSError(domain:"SuperTracker.ReorderFixture",code:731,userInfo:[NSLocalizedDescriptionKey:"Original reorder resume fixture failure"])
        let loadsBeforeRollback = instantiations
        resumeFailure = injected
        var caught:NSError?
        do { _ = try await host.handle("plugin.move",payload:["track":0,"fromSlot":0,"toSlot":7]) }
        catch { caught = error as NSError }
        resumeFailure = nil
        try require(caught?.domain == injected.domain && caught?.code == injected.code,"Reorder rollback replaced the original failure.")
        try verifyProject(try await snapshot(host),original,"failure after reconnect and project remap")
        try require(instantiations == loadsBeforeRollback,"Rollback recreated a plug-in instance.")
        let lifetime = host.status()["endpointLifetime"] as? [String:Any] ?? [:]
        try require(lifetime["activeReaders"] as? Int == 0 && lifetime["retiredEndpoints"] as? Int == 0 && lifetime["retiredUnits"] as? Int == 0,
                    "Rollback published retired units or retained unbalanced render readers.")
        measurements["injectedRollback"] = ["instantiationsBefore":loadsBeforeRollback,"instantiationsAfter":instantiations,"originalErrorDomain":caught?.domain ?? "", "originalErrorCode":caught?.code ?? 0,"endpointLifetime":lifetime]
        passed.append("A failure after reconnection and automation remap restores the original chain and project without replacing instances or the error")
        await host.shutdownAndWait()

        // Nonlinear distortion and lowpass do not commute. Move the existing nodes,
        // render each order, and then return them to their original chain order.
        var audioInstantiations = 0
        let audioHost = try AudioHost(beforePluginInstantiation: { audioInstantiations += 1 }, midiInputIDs: [])
        defer { audioHost.shutdown() }
        let source = folder.appendingPathComponent("harmonic-pulse.wav"), rate = 48_000.0
        var pulse = [Float](repeating: 0, count: 2400)
        for index in pulse.indices {
            let time = Double(index) / rate, envelope = sin(.pi * Double(index) / Double(pulse.count))
            pulse[index] = Float(envelope * (0.5 * sin(2 * .pi * 220 * time) + 0.22 * sin(2 * .pi * 2730 * time)))
        }
        try AudioSamples.write(DecodedSample(path: source.path, sampleRate: rate, left: pulse, right: pulse, originalChannels: 2), to: source)
        var audioRows = [Any](repeating: NSNull(), count: 8)
        audioRows[0] = ["note": 60, "instrument": 0, "velocity": 110, "gate": 0.9]
        let audioProject: [String: Any] = ["version": 1, "name": "Audible effect moves", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Pulse", "instrument": 0, "volume": 0.7, "pan": 0, "delaySend": 0, "reverbSend": 0]],
            "instruments": [["type": "sample", "samplePath": source.path, "rootNote": 60, "volume": 0.8,
                "mode": "oneshot", "attack": 0, "decay": 0, "sustain": 1, "release": 0.001, "cutoff": 20_000]],
            "patterns": [["id": 0, "length": 8, "steps": [audioRows], "automation": []]], "sequence": [0],
            "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        try await apply(audioHost, audioProject)
        try await load(audioHost, distortionID, slot: 0)
        for (address, value): (AudioUnitParameterID, Double) in [
            (kDistortionParam_DelayMix, 0), (kDistortionParam_DecimationMix, 0), (kDistortionParam_LinearTerm, 1),
            (kDistortionParam_SquaredTerm, 0), (kDistortionParam_CubicTerm, 18), (kDistortionParam_PolynomialMix, 100),
            (kDistortionParam_RingModMix, 0), (kDistortionParam_SoftClipGain, 8), (kDistortionParam_FinalMix, 100)] {
            try await parameter(audioHost, address, value, slot: 0)
        }
        try await load(audioHost, lowpassID, slot: 5)
        try await parameter(audioHost, kLowPassParam_CutoffFrequency, 450, slot: 5)
        try await parameter(audioHost, kLowPassParam_Resonance, 0, slot: 5)
        let beforeAudioMove = try await snapshot(audioHost)
        func render(_ name: String) async throws -> DecodedSample {
            let file = folder.appendingPathComponent(name + ".wav")
            _ = try await audioHost.handle("audio.export", payload: ["path": file.path, "mode": "song", "tail": 0.5])
            let pcm = try AudioSamples.decode(file.path)
            try require(pcm.left.allSatisfy(\.isFinite) && pcm.right.allSatisfy(\.isFinite) && abs(pcm.duration - 1.5) < 0.002,
                        "Effect-order rendering produced invalid samples or duration.")
            return pcm
        }
        let originalPCM = try await render("distortion-then-lowpass")
        _ = try await audioHost.handle("plugin.move", payload: ["track": 0, "fromSlot": 0, "toSlot": 5])
        let reversedPCM = try await render("lowpass-then-distortion")
        _ = try await audioHost.handle("plugin.move", payload: ["track": 0, "fromSlot": 5, "toSlot": 0])
        let restoredPCM = try await render("restored-distortion-then-lowpass")
        let changed = rmsDifference(originalPCM, reversedPCM), restored = rmsDifference(originalPCM, restoredPCM)
        measurements["orderDifferenceRMS"] = changed; measurements["restoredOrderDifferenceRMS"] = restored
        measurements["audioInstantiations"] = audioInstantiations
        try require(rms(originalPCM) > 0.0001 && rms(reversedPCM) > 0.0001 && changed > 0.0001,
                    "Moving distortion and lowpass did not produce a meaningful audible order change: \(changed).")
        try require(restored < 0.00001, "The inverse move failed to restore original PCM: \(restored).")
        try verifyDescriptors(descriptors(try await snapshot(audioHost)), descriptors(beforeAudioMove), "audible inverse move")
        try require(audioInstantiations == 2, "Audible effect moves recreated processing instances.")
        passed.append("Moving existing distortion and lowpass nodes changes offline PCM, while the inverse move restores the original audio")
        await audioHost.shutdownAndWait()
        return passed
    }

    private static func equivalentState(_ left: String?, _ right: String?) -> Bool {
        guard let left,let right else { return false }
        if left == right { return true } // Includes exact unavailable opaque intent.
        guard let a = Data(base64Encoded:left),let b = Data(base64Encoded:right) else { return false }
        return AudioAUStateReuse.equivalent(a,b)
    }
    private static func rms(_ sample: DecodedSample) -> Double {
        guard !sample.left.isEmpty else { return 0 }
        var sum = 0.0
        for index in sample.left.indices { sum += pow(Double(sample.left[index]), 2) + pow(Double(sample.right[index]), 2) }
        return sqrt(sum / Double(sample.left.count * 2))
    }
    private static func rmsDifference(_ left: DecodedSample, _ right: DecodedSample) -> Double {
        guard left.left.count == right.left.count, !left.left.isEmpty else { return .infinity }
        var sum = 0.0
        for index in left.left.indices {
            sum += pow(Double(left.left[index] - right.left[index]), 2) + pow(Double(left.right[index] - right.right[index]), 2)
        }
        return sqrt(sum / Double(left.left.count * 2))
    }
}

#endif

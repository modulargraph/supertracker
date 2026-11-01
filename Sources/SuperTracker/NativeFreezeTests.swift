// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import TrackerEngine

/// `--freeze-self-test <dir>`: user Track Freeze end to end with a built-in
/// synth and Apple's AUDelay, in the flat mixer and in standard routing.
/// Run with SUPERTRACKER_SUPPORT_DIR pointing at a private temporary folder.
enum NativeFreezeTests {
    struct Comparison { let frames: Int; let maxAbs: Double; let rms: Double; let reference: Double }

    @MainActor static func run(output: URL) async throws -> [String: Any] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        guard let support = ProcessInfo.processInfo.environment["SUPERTRACKER_SUPPORT_DIR"], support.hasPrefix("/") else {
            throw TrackerError.message("FREEZE TEST: set SUPERTRACKER_SUPPORT_DIR to a private temporary folder.")
        }
        var checks: [[String: Any]] = []
        func record(_ name: String, _ details: [String: Any] = [:]) {
            var value = details; value["name"] = name; value["passed"] = true; checks.append(value)
            print("PASS \(name) \(details.isEmpty ? "" : String(describing: details))"); fflush(stdout)
        }
        checks.append(contentsOf: try pureChecks().map { name in print("PASS \(name)"); return ["name": name, "passed": true] })
        for check in try enginePatternChecks() { record(check.name, check.details) }
        let only = ProcessInfo.processInfo.environment["SUPERTRACKER_FREEZE_ONLY"]
        for (routed, instrument) in [(false, false), (true, false), (false, true)] {
            if let only, only != (routed ? "routed" : "flat") + (instrument ? "-au-instrument" : "") { continue }
            let name = (routed ? "routed" : "flat") + (instrument ? "-au-instrument" : "")
            let details = try await scenario(output: output.appendingPathComponent(name), routed: routed, auInstrument: instrument, record: record)
            checks.append(["name": name + " scenario", "passed": true, "details": details])
        }
        if only == nil || only == "recorded" {
            let details = try await recordedScenario(output: output.appendingPathComponent("recorded"), record: record)
            checks.append(["name": "recorded scenario", "passed": true, "details": details])
        }
        if only == nil || only == "latency" {
            let details = try await latencyScenario(output: output.appendingPathComponent("latency"), record: record)
            checks.append(["name": "latency/auto-tail/recorded scenario", "passed": true, "details": details])
        }
        let report: [String: Any] = ["passed": true, "checks": checks, "generatedAt": ISO8601DateFormatter().string(from: Date())]
        try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
            .write(to: output.appendingPathComponent("freeze-test-report.json"), options: .atomic)
        print("Track freeze tests passed. Evidence: \(output.path)")
        return report
    }

    static func require(_ condition: Bool, _ message: String) throws {
        if !condition { throw TrackerError.message("FREEZE TEST: " + message) }
    }

    static func compare(_ a: DecodedSample, _ b: DecodedSample, frames: Int) -> Comparison {
        compare((a.left, a.right), (b.left, b.right), frames: frames)
    }

    static func compare(_ a: ([Float], [Float]), _ b: ([Float], [Float]), frames: Int, from start: Int = 0) -> Comparison {
        var maxAbs = 0.0, sum = 0.0, ref = 0.0
        for i in start..<(start + frames) {
            for (x, y) in [(a.0[i], b.0[i]), (a.1[i], b.1[i])] {
                let d = Double(x) - Double(y); maxAbs = max(maxAbs, abs(d)); sum += d * d; ref += Double(x) * Double(x)
            }
        }
        return Comparison(frames: frames, maxAbs: maxAbs, rms: sqrt(sum / Double(max(1, 2 * frames))), reference: sqrt(ref / Double(max(1, 2 * frames))))
    }

    static func peak(_ s: DecodedSample, from start: Int) -> Double {
        var value = 0.0
        for i in start..<s.left.count { value = max(value, Double(abs(s.left[i])), Double(abs(s.right[i]))) }
        return value
    }

    static func project(routed: Bool, delay: String, instrument: String? = nil) -> [String: Any] {
        func pattern(_ id: Int, _ offset: Int) -> [String: Any] {
            var lead = [Any](repeating: NSNull(), count: 16), bass = [Any](repeating: NSNull(), count: 16)
            for row in stride(from: 0, to: 16, by: 4) {
                lead[row] = ["note": 64 + offset + row / 4 * 3, "instrument": 0, "velocity": 100, "gate": 1.5] as [String: Any]
                bass[row + 2] = ["note": 40 + offset, "instrument": 1, "velocity": 90, "gate": 1.0] as [String: Any]
            }
            // Live mix commands/automation on the frozen track must still apply after
            // the print. Only standard routing places the fader after the inserts, so
            // time-varying mix moves are compared exactly there; the flat mixer
            // applies its fader before the inserts (documented difference).
            if routed { lead[6] = ["fx1": ["type": "volume", "value": 0.6]] as [String: Any] }
            var leadVolume: [String: Any] = ["id": "vol\(id)", "target": "volume", "name": "Lead volume", "interpolation": "linear",
                "points": [["row": 0, "value": 0.5], ["row": 15, "value": 0.9]]]
            if routed { leadVolume["owner"] = "track:lead" } else { leadVolume["track"] = 0 }
            return ["id": id, "name": "Freeze \(id + 1)", "length": 16, "steps": [lead, bass], "automation": routed ? [leadVolume] : []]
        }
        var lead: [String: Any] = ["id": 0, "name": "Lead", "volume": 0.8, "pan": 0, "instrument": 0, "delaySend": 0, "reverbSend": 0]
        var bass: [String: Any] = ["id": 1, "name": "Bass", "volume": 0.7, "pan": -0.3, "instrument": 1, "delaySend": 0.2, "reverbSend": 0.1]
        var plugin: [String: Any] = ["track": 0, "kind": "effect", "slot": 0, "component": delay, "instanceID": "freeze-test-delay"]
        if routed { lead["routingID"] = "lead"; bass["routingID"] = "bass"; plugin["owner"] = "track:lead" }
        var value: [String: Any] = ["version": 1, "name": "Freeze test", "bpm": 120.0, "linesPerBeat": 4, "swing": 0, "masterVolume": 0.6, "activePattern": 0,
            "tracks": [lead, bass],
            "instruments": [["id": 0, "name": "Saw", "type": "synth", "waveform": "saw", "volume": 0.5, "attack": 0.005, "decay": 0.2, "sustain": 0.5, "release": 0.3, "cutoff": 4000],
                            ["id": 1, "name": "Square", "type": "synth", "waveform": "square", "volume": 0.4, "attack": 0.005, "decay": 0.1, "sustain": 0.4, "release": 0.05]],
            "patterns": [pattern(0, 0), pattern(1, 5)], "sequence": [0, 1], "loopSong": false,
            "delay": ["time": 0.25, "feedback": 0.2, "mix": 0.2], "reverb": ["size": 0.3, "damping": 0.4, "mix": 0.15],
            "audioPlugins": [plugin]]
        if routed { value["routing"] = ["version": 1, "mixModel": "standard", "buses": [], "sends": [], "sidechains": []] }
        if let instrument {
            var instruments = value["instruments"] as! [[String: Any]]; instruments[0]["type"] = "au"; value["instruments"] = instruments
            var synth: [String: Any] = ["track": 0, "kind": "instrument", "slot": 0, "component": instrument, "instanceID": "freeze-test-instrument"]
            if routed { synth["owner"] = "track:lead" }
            value["audioPlugins"] = [synth] + (value["audioPlugins"] as! [[String: Any]])
        }
        return value
    }

    @MainActor static func scenario(output: URL, routed: Bool, auInstrument: Bool, record: (String, [String: Any]) -> Void) async throws -> [String: Any] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let label = (routed ? "routed" : "flat") + (auInstrument ? "+AU instrument" : "")
        // This fixture loads only Apple's Audio Units. Keep even cache reads and
        // any future VST discovery confined to a fresh scenario-owned directory.
        let catalogueFolder = output.appendingPathComponent("catalogues-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: catalogueFolder, withIntermediateDirectories: true)
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots = []; vst3Configuration.cacheURL = catalogueFolder.appendingPathComponent("vst3.json")
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots = []; vst2Configuration.cacheURL = catalogueFolder.appendingPathComponent("vst2.json")
        let host = try AudioHost(vst3Catalogue: VST3Catalogue(configuration: vst3Configuration), midiInputIDs: [],
                                 vst2Catalogue: VST2Catalogue(configuration: vst2Configuration))
        defer { host.shutdown() }
        let plugins = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
        try require(plugins.allSatisfy { ["AU", "Builtin"].contains($0["format"] as? String ?? "") }, "\(label): empty private VST catalogues returned a VST descriptor")
        guard let delay = plugins.first(where: { ($0["name"] as? String ?? "").localizedCaseInsensitiveContains("AUDelay") && $0["type"] as? String == "effect" && $0["manufacturer"] as? String == "Apple" }),
              let delayID = delay["id"] as? String else { throw TrackerError.message("FREEZE TEST: Apple AUDelay is unavailable.") }
        var instrumentID: String?
        if auInstrument {
            guard let dls = plugins.first(where: { ($0["name"] as? String ?? "").localizedCaseInsensitiveContains("DLSMusicDevice") && $0["manufacturer"] as? String == "Apple" }), let id = dls["id"] as? String else {
                throw TrackerError.message("FREEZE TEST: Apple DLSMusicDevice is unavailable.")
            }
            instrumentID = id
        }
        var canonical = project(routed: routed, delay: delayID, instrument: instrumentID)
        _ = try await host.handle("engine.project", payload: ["project": canonical, "replace": true, "forcePluginState": true, "awaitPlugins": true])
        // A non-default insert setting proves Unfreeze restores the exact state.
        let target: [String: Any] = routed ? ["owner": "track:lead", "track": 0, "kind": "effect", "slot": 0] : ["track": 0, "kind": "effect", "slot": 0]
        let parameters = try await host.handle("plugin.parameters", payload: target) as? [[String: Any]] ?? []
        guard let time = parameters.first(where: { ($0["name"] as? String ?? "").localizedCaseInsensitiveContains("time") }) ?? parameters.first,
              let address = time["address"] as? String else { throw TrackerError.message("FREEZE TEST: AUDelay exposes no parameters.") }
        var setting = target; setting["address"] = address; setting["value"] = 0.13
        let applied = try await host.handle("plugin.parameter", payload: setting) as? [String: Any] ?? [:]
        let appliedValue = (applied["value"] as? NSNumber)?.doubleValue ?? -1
        let snapshot = try await host.handle("engine.snapshot", payload: [:]) as! [String: Any]
        canonical["audioPlugins"] = snapshot["audioPlugins"]
        _ = try await host.handle("engine.project", payload: ["project": canonical, "awaitPlugins": true])

        func export(_ name: String, tail: Double = 2) async throws -> DecodedSample {
            let url = output.appendingPathComponent(name + ".wav")
            try? FileManager.default.removeItem(at: url)
            _ = try await host.handle("audio.export", payload: ["path": url.path, "mode": "song", "tail": tail])
            return try AudioSamples.decode(url.path)
        }
        let live = try await export("live")
        try require(NativeSelfTest.energy(live.left) > 0.001, "\(label): live export is silent")
        // Pattern mode references (live chain): pattern export of each pattern, and
        // the track's print-style render of pattern 0 looping three times.
        func exportPattern(_ name: String, project value: [String: Any], pattern: Int) async throws -> DecodedSample {
            var copy = value; copy["activePattern"] = pattern
            _ = try await host.handle("engine.project", payload: ["project": copy, "awaitPlugins": true])
            let url = output.appendingPathComponent(name + ".wav")
            try? FileManager.default.removeItem(at: url)
            _ = try await host.handle("audio.export", payload: ["path": url.path, "mode": "pattern", "tail": 2])
            return try AudioSamples.decode(url.path)
        }
        let livePatterns = [try await exportPattern("live-pattern0", project: canonical, pattern: 0),
                            try await exportPattern("live-pattern1", project: canonical, pattern: 1)]
        let loopReference: DecodedSample
        do {
            let render = AudioFreezeTrack.patternRenderProject(try AudioFreezeTrack.renderProject(canonical, index: 0), pattern: 0, passes: 3)
            _ = try await host.handle("engine.project", payload: ["project": render, "awaitPlugins": true])
            let url = output.appendingPathComponent("live-loop3.wav")
            try? FileManager.default.removeItem(at: url)
            _ = try await host.handle("audio.export", payload: ["path": url.path, "mode": "track", "track": 0, "tail": 2])
            loopReference = try AudioSamples.decode(url.path)
        }
        _ = try await host.handle("engine.project", payload: ["project": canonical, "awaitPlugins": true])

        // (Freeze while playing is refused in freezeTrack; not exercised here so
        // the test never plays through the user's audio device.)

        let started = Date()
        let frozenResult = try await host.handle("freeze.track", payload: ["track": 0, "tail": 2]) as! [String: Any]
        let freezeSeconds = Date().timeIntervalSince(started)
        let recordValue = frozenResult["record"] as! [String: Any]
        let frozenProject = frozenResult["project"] as! [String: Any]
        let recordParsed = try AudioFreezeTrack.Record.parse(recordValue)
        try require(frozenResult["releasedPlugins"] as? Int == (auInstrument ? 2 : 1), "\(label): the plug-ins were not released")
        do { _ = try await host.handle("plugin.parameters", payload: target); throw TrackerError.message("FREEZE TEST: dormant AUDelay is still loaded") }
        catch let error as TrackerError { throw error } catch { /* expected: instance unloaded */ }
        // Canonical state is retained: the snapshot still lists the dormant plug-in with state.
        let frozenSnapshot = try await host.handle("engine.snapshot", payload: [:]) as! [String: Any]
        let retained = (frozenSnapshot["audioPlugins"] as? [[String: Any]] ?? []).first { $0["instanceID"] as? String == "freeze-test-delay" }
        try require(retained?["state"] is String, "\(label): dormant plug-in state missing from snapshot")
        try require(((frozenSnapshot["tracks"] as? [[String: Any]])?.first?["frozen"]) != nil, "\(label): snapshot lost the freeze record")
        let leadSteps = ((frozenSnapshot["patterns"] as? [[String: Any]])?.first?["steps"] as? [[Any]])?.first ?? []
        try require(leadSteps.contains { ($0 as? [String: Any])?["note"] != nil }, "\(label): snapshot must keep the frozen notes")
        do {
            _ = try await host.handle("plugin.load", payload: ["track": 0, "kind": "effect", "slot": 1, "component": delayID] + (routed ? ["owner": "track:lead"] : [:]))
            throw TrackerError.message("FREEZE TEST: loading into a frozen track must fail")
        } catch let error as AudioFreezeTrack.Failure { try require(error.message.contains("Unfreeze"), "unexpected: \(error.message)") }
        let status = host.freezeStatus()["tracks"] as? [[String: Any]] ?? []
        try require(status.first?["available"] as? Bool == true && status.first?["stale"] as? Bool == false, "\(label): status \(status)")
        try require(recordParsed.patternPrints.map(\.pattern) == [0, 1] && status.first?["patternPlayback"] as? Bool == true,
                    "\(label): pattern prints \(recordParsed.patternPrints.map(\.pattern)), status \(status)")
        for command in ["step.preview", "note.on"] {
            do {
                _ = try await host.handle(command, payload: command == "note.on" ? ["track": 0, "instrument": 0, "note": 60, "velocity": 100] : ["track": 0, "step": ["note": 60]])
                throw TrackerError.message("FREEZE TEST: \(command) on a frozen track must explain itself")
            } catch let error as AudioFreezeTrack.Failure { try require(error.message.contains("Unfreeze it to audition"), "unexpected: \(error.message)") }
        }
        record("\(label): freeze renders, unloads the insert and keeps canonical state",
               ["freezeSeconds": freezeSeconds, "printFrames": recordParsed.frames, "musicalFrames": recordParsed.musicalFrames,
                "tailFrames": recordParsed.tailFrames, "musicalEndBeat": recordParsed.musicalEndBeat])

        let frozen = try await export("frozen")
        // Export never loops: the song carries the printed tail, then the export tail.
        let liveMusical = live.left.count - 96_000
        try require(abs(frozen.left.count - (live.left.count + Int(recordParsed.tailFrames))) <= 2,
                    "\(label): frozen export length \(frozen.left.count), live \(live.left.count), tail \(recordParsed.tailFrames)")
        let same = compare(live, frozen, frames: live.left.count)
        let afterPeak = peak(frozen, from: live.left.count)
        try require(same.maxAbs < 1e-3, "\(label): frozen vs live max diff \(same.maxAbs) (rms \(same.rms), ref \(same.reference))")
        try require(afterPeak < 1e-3, "\(label): material after the live export end: \(afterPeak)")
        record("\(label): frozen export matches live export", ["frames": same.frames, "maxAbs": same.maxAbs, "rms": same.rms,
            "referenceRMS": same.reference, "liveMusicalFrames": liveMusical, "peakAfterLiveEnd": afterPeak])

        // Pattern mode: frozen pattern export == live pattern export (full mix).
        for (pattern, reference) in livePatterns.enumerated() {
            let frozenPattern = try await exportPattern("frozen-pattern\(pattern)", project: frozenProject, pattern: pattern)
            let print = recordParsed.patternPrints[pattern]
            let headTail = Int(print.frames - print.passFrames)
            try require(abs(frozenPattern.left.count - (reference.left.count + headTail)) <= 2,
                        "\(label): frozen pattern \(pattern) export length \(frozenPattern.left.count), live \(reference.left.count), tail \(headTail)")
            let diff = compare(reference, frozenPattern, frames: reference.left.count)
            let after = peak(frozenPattern, from: reference.left.count)
            try require(NativeSelfTest.energy(reference.left) > 0.001 && diff.maxAbs < 1e-3 && after < 1e-3,
                        "\(label): frozen vs live pattern \(pattern) export max diff \(diff.maxAbs) (rms \(diff.rms), ref \(diff.reference)), after \(after)")
            record("\(label): frozen pattern \(pattern) export matches live pattern export",
                   ["frames": diff.frames, "maxAbs": diff.maxAbs, "rms": diff.rms, "referenceRMS": diff.reference, "peakAfterLiveEnd": after])
        }
        _ = try await host.handle("engine.project", payload: ["project": frozenProject, "awaitPlugins": true])
        // Pattern loop wrap: first pass from the head print, then the steady-state
        // loop print twice (tail wrapped into it), then its tail == live pattern 0
        // looping three times through the live instrument and inserts.
        do {
            let print = recordParsed.patternPrints[0]
            let head = try AudioSamples.decode(print.path), loop = try AudioSamples.decode(print.loopPath)
            try require(head.sampleRate == loop.sampleRate, "\(label): print sample rates differ")
            let pass = Int(print.passFrames)
            try require(print.passes == 2 && pass == 96_000 && loop.left.count >= pass, "\(label): pattern print geometry \(print.dictionary)")
            var left: [Float] = [], right: [Float] = []
            left += head.left[0..<pass]; right += head.right[0..<pass]
            left += loop.left[0..<pass]; right += loop.right[0..<pass]
            left += loop.left; right += loop.right
            let frames = min(left.count, loopReference.left.count)
            let reference = (loopReference.left, loopReference.right)
            let diff = compare(reference, (left, right), frames: frames)
            let wrap = compare(reference, (left, right), frames: 9_600, from: 2 * pass - 4_800)
            let segments = (0..<4).map { k in compare(reference, (left, right), frames: min(pass, frames - k * pass), from: k * pass) }
            // Some instruments (e.g. DLSMusicDevice) do not repeat exactly from pass to
            // pass even live; the loop print cannot be closer than that variation.
            let selfVariation = compare((Array(loopReference.left[pass..<(2 * pass)]), Array(loopReference.right[pass..<(2 * pass)])),
                                        (Array(loopReference.left[(2 * pass)..<(3 * pass)]), Array(loopReference.right[(2 * pass)..<(3 * pass)])), frames: pass)
            let tolerance = max(1e-3, 1.5 * selfVariation.maxAbs)
            try require(segments[1].maxAbs < 1e-4, "\(label): first loop wrap (head -> loop) differs: \(segments[1].maxAbs)")
            try require(NativeSelfTest.energy(loopReference.left) > 0.001 && diff.maxAbs < tolerance && abs(left.count - loopReference.left.count) <= 2,
                        "\(label): frozen pattern loop vs live loop max diff \(diff.maxAbs) (rms \(diff.rms)), frames \(left.count)/\(loopReference.left.count), per pass \(segments.map { ($0.maxAbs, $0.reference) })")
            record("\(label): frozen pattern loop (head, wrapped loop x2, tail) matches the live track looping",
                   ["frames": frames, "maxAbs": diff.maxAbs, "rms": diff.rms, "referenceRMS": diff.reference,
                    "wrapWindowMaxAbs": wrap.maxAbs, "wrapWindowReferenceRMS": wrap.reference, "loopFrames": print.loopFrames, "passFrames": pass,
                    "perPassMaxAbs": segments.map(\.maxAbs), "livePass2vs3MaxAbs": selfVariation.maxAbs, "tolerance": tolerance])
        }

        // Save / Open retains the freeze and the print.
        let store = try ProjectStore(directory: output.appendingPathComponent("store"))
        var saved = frozenProject; saved["audioPlugins"] = frozenSnapshot["audioPlugins"]
        try store.update(saved)
        let file = output.appendingPathComponent("Frozen.supertracker")
        _ = try store.save(to: file)
        let reopened = try store.load(file)
        let reopenedRecord = try AudioFreezeTrack.Record.parse((reopened["tracks"] as? [[String: Any]])?.first?["frozen"])
        try require(reopenedRecord.path != recordParsed.path && reopenedRecord.path.contains(".media/"), "\(label): print was not collected: \(reopenedRecord.path)")
        try require(reopenedRecord.patternPrints.count == recordParsed.patternPrints.count
                    && reopenedRecord.patternPrints.allSatisfy { $0.path.contains(".media/") && $0.loopPath.contains(".media/") },
                    "\(label): pattern prints were not collected: \(reopenedRecord.patternPrints.map(\.dictionary))")
        _ = try await host.handle("engine.project", payload: ["project": reopened, "replace": true, "forcePluginState": true, "awaitPlugins": true])
        let reloaded = try await export("frozen-reopened")
        let reopenCompare = compare(frozen, reloaded, frames: min(frozen.left.count, reloaded.left.count))
        try require(reopenCompare.maxAbs < 1e-5 && reloaded.left.count == frozen.left.count, "\(label): reopened export differs \(reopenCompare.maxAbs)")
        do { _ = try await host.handle("plugin.parameters", payload: target); throw TrackerError.message("FREEZE TEST: reopened frozen track loaded its insert") }
        catch let error as TrackerError { throw error } catch {}
        record("\(label): Save/Open keeps the track frozen", ["maxAbs": reopenCompare.maxAbs, "collectedPrint": reopenedRecord.path])

        // Unfreeze restores the live chain with its exact plug-in state.
        let unfrozen = try await host.handle("freeze.unfreeze", payload: ["track": 0]) as! [String: Any]
        try require((unfrozen["problems"] as? [String] ?? ["?"]).isEmpty && unfrozen["restoredPlugins"] as? Int == (auInstrument ? 2 : 1), "\(label): unfreeze \(unfrozen)")
        let restoredParameters = try await host.handle("plugin.parameters", payload: target) as? [[String: Any]] ?? []
        let restoredValue = (restoredParameters.first { $0["address"] as? String == address }?["value"] as? NSNumber)?.doubleValue ?? -2
        try require(abs(restoredValue - appliedValue) < 1e-6, "\(label): restored delay setting \(restoredValue) != \(appliedValue)")
        let relive = try await export("unfrozen")
        let liveAgain = compare(live, relive, frames: min(live.left.count, relive.left.count))
        try require(relive.left.count == live.left.count && liveAgain.maxAbs < 1e-5, "\(label): unfrozen export differs \(liveAgain.maxAbs)")
        record("\(label): Unfreeze restores the live insert and state", ["parameter": time["name"] ?? address, "value": restoredValue, "maxAbs": liveAgain.maxAbs])

        // Undo of Freeze == republishing the pre-freeze canonical project; Redo republishes the frozen one.
        let undoTarget = reopened // frozen project as Redo
        _ = try await host.handle("engine.project", payload: ["project": undoTarget, "awaitPlugins": true])
        do { _ = try await host.handle("plugin.parameters", payload: target); throw TrackerError.message("FREEZE TEST: redo kept the insert loaded") }
        catch let error as TrackerError { throw error } catch {}
        var undone = reopened
        var tracks = undone["tracks"] as! [[String: Any]]; tracks[0].removeValue(forKey: "frozen"); undone["tracks"] = tracks
        _ = try await host.handle("engine.project", payload: ["project": undone, "awaitPlugins": true])
        _ = try await host.handle("plugin.parameters", payload: target)
        record("\(label): Undo/Redo republish frozen and live chains", [:])
        return ["live": live.left.count, "frozen": frozen.left.count,
                "catalogueIsolation": ["directory": catalogueFolder.path, "vst2Roots": [], "vst3Roots": [],
                                       "selection": "Apple Audio Units only; no VST cache reads or discovery"]]
    }

    /// Flat mixer with a deterministic 257-frame latency insert on the lead:
    /// * freezing the plug-in-free bass (auto tail) keeps the lead's compensation and lands
    ///   the bass print exactly where the compensated live bass played;
    /// * freezing the lead too releases the only latency (plan 0) and still matches live,
    ///   with the flat pre-insert gain printed and divided out;
    /// * freeze by recording (driven on the manual graph, monitor off) captures the same print,
    ///   sample-aligned through the compensated strip's tap and the native clock.
    @MainActor static func latencyScenario(output: URL, record: (String, [String: Any]) -> Void) async throws -> [String: Any] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let catalogueFolder = output.appendingPathComponent("catalogues-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: catalogueFolder, withIntermediateDirectories: true)
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots = []; vst3Configuration.cacheURL = catalogueFolder.appendingPathComponent("vst3.json")
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots = []; vst2Configuration.cacheURL = catalogueFolder.appendingPathComponent("vst2.json")
        // A manual (offline) graph: no audio device, deterministic quanta. The recorded take
        // then drives the graph itself through the same tap/clock alignment as real time.
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: vst3Configuration), midiInputIDs: [],
                                 vst2Catalogue: VST2Catalogue(configuration: vst2Configuration))
        defer { host.shutdown() }
        var canonical = project(routed: false, delay: NativeLatencyTests.longLatencyEffect())
        canonical["audioPlugins"] = [["track": 0, "kind": "effect", "slot": 0, "component": NativeLatencyTests.longLatencyEffect(), "instanceID": "freeze-latency"]]
        _ = try await host.handle("engine.project", payload: ["project": canonical, "replace": true, "forcePluginState": true, "awaitPlugins": true])
        let snapshot = try await host.handle("engine.snapshot", payload: [:]) as! [String: Any]
        canonical["audioPlugins"] = snapshot["audioPlugins"]
        _ = try await host.handle("engine.project", payload: ["project": canonical, "awaitPlugins": true])
        func export(_ name: String) async throws -> DecodedSample {
            let url = output.appendingPathComponent(name + ".wav")
            try? FileManager.default.removeItem(at: url)
            _ = try await host.handle("audio.export", payload: ["path": url.path, "mode": "song", "tail": 6])
            return try AudioSamples.decode(url.path)
        }
        func frames(_ value: Any?) -> Int { ((value as? [String: Any])?["frames"] as? NSNumber)?.intValue ?? -1 }
        let live = try await export("live")
        let liveLatency = frames(host.rack.latencyStatus())
        try require(liveLatency == 257 && NativeSelfTest.energy(live.left) > 0.001, "latency fixture: plan \(liveLatency)")

        let bass = try await host.handle("freeze.track", payload: ["track": 1, "maxTail": 6]) as! [String: Any]
        let bassRecord = try AudioFreezeTrack.Record.parse(bass["record"])
        try require(bassRecord.tailMode == "auto" && bassRecord.tailFrames > 0 && bassRecord.tailFrames < 48_000,
                    "auto tail of the short square release: \(bassRecord.tailFrames) frames")
        try require(frames(bass["compensationAfter"]) == 257, "bass freeze must keep the lead's compensation: \(bass["compensationAfter"] ?? "nil")")
        let frozenBass = try await export("frozen-bass")
        let bassDiff = compare(live, frozenBass, frames: live.left.count)
        try require(bassDiff.maxAbs < 1e-3, "frozen bass (lead live with 257-frame latency) vs live: \(bassDiff.maxAbs)")
        record("Latency: frozen plug-in-free track lands with the compensated live mix (auto tail)",
               ["maxAbs": bassDiff.maxAbs, "autoTailFrames": bassRecord.tailFrames, "plan": 257])

        let lead = try await host.handle("freeze.track", payload: ["track": 0, "tail": 1]) as! [String: Any]
        let leadRecord = try AudioFreezeTrack.Record.parse(lead["record"])
        try require(abs(leadRecord.printGain - 0.48) < 1e-9, "flat pre-insert print gain \(leadRecord.printGain)")
        try require(frames(lead["compensationAfter"]) == 0, "freezing the only latency plug-in must release its compensation: \(lead["compensationAfter"] ?? "nil")")
        let frozenBoth = try await export("frozen-both")
        let bothDiff = compare(live, frozenBoth, frames: live.left.count)
        try require(bothDiff.maxAbs < 1e-3, "both frozen (plan 0) vs live: \(bothDiff.maxAbs)")
        record("Latency: freezing the latency track releases its compensation and still matches live",
               ["maxAbs": bothDiff.maxAbs, "printGain": leadRecord.printGain])

        // Freeze by recording, against the offline lead print (kept on disk after Unfreeze).
        _ = try await host.handle("freeze.unfreeze", payload: ["track": 0])
        try require(frames(host.rack.latencyStatus()) == 257, "unfreeze must restore the 257-frame plan")
        let offline = try AudioSamples.decode(leadRecord.path)
        let started = Date()
        let taken = try await host.handle("freeze.track", payload: ["track": 0, "tail": 1, "method": "record", "testAnySource": true,
                                                                    "monitor": false, "patterns": false]) as! [String: Any]
        let takenRecord = try AudioFreezeTrack.Record.parse(taken["record"])
        let recorded = try AudioSamples.decode(takenRecord.path)
        try require(takenRecord.method == "recorded" && takenRecord.musicalFrames == leadRecord.musicalFrames
                    && recorded.left.count == Int(takenRecord.musicalFrames) + 48_000 && takenRecord.patternPrints.isEmpty,
                    "recorded print geometry \(takenRecord.dictionary)")
        let span = Int(leadRecord.musicalFrames)
        var best = (lag: 0, maxAbs: Double.infinity)
        for lag in -512...512 {
            let start = max(0, -lag) + 1_024, count = span - 2_048
            var worst = 0.0
            for i in stride(from: start, to: start + count, by: 7) { worst = max(worst, Double(abs(recorded.left[i + lag] - offline.left[i]))); if worst > best.maxAbs { break } }
            if worst < best.maxAbs { best = (lag, worst) }
        }
        let aligned = compare(offline, recorded, frames: span)
        try require(best.lag == 0 && aligned.maxAbs < 1e-3 && aligned.reference > 0.001,
                    "recorded print is misaligned or different: best lag \(best.lag) (\(best.maxAbs)), lag-0 max diff \(aligned.maxAbs), ref \(aligned.reference)")
        let frozenRecorded = try await export("frozen-recorded")
        let recordedDiff = compare(live, frozenRecorded, frames: live.left.count)
        try require(recordedDiff.maxAbs < 1e-3, "recorded frozen lead vs live: \(recordedDiff.maxAbs)")
        record("Freeze by recording: real-time take is sample-aligned with the offline print and the live mix",
               ["lag": best.lag, "maxAbs": aligned.maxAbs, "referenceRMS": aligned.reference, "exportMaxAbs": recordedDiff.maxAbs,
                "seconds": Date().timeIntervalSince(started)])

        // A routed project (the bass through a group bus with an insert): the take plays flat,
        // and still equals the routed offline print; the bus comes back for playback.
        _ = try await host.handle("freeze.unfreeze", payload: ["track": 0])
        _ = try await host.handle("freeze.unfreeze", payload: ["track": 1])
        var routed = project(routed: true, delay: NativeLatencyTests.longLatencyEffect())
        routed["routing"] = ["version": 1, "mixModel": "standard", "sends": [], "sidechains": [],
                             "buses": [["id": "group", "name": "Group", "type": "group", "volume": 0.9, "pan": 0, "mute": false, "solo": false]],
                             "outputs": ["track:bass": "bus:group", "bus:group": "master"]]
        routed["audioPlugins"] = [["owner": "track:lead", "kind": "effect", "slot": 0, "component": NativeLatencyTests.longLatencyEffect(), "instanceID": "freeze-latency"],
                                  ["owner": "bus:group", "kind": "effect", "slot": 0, "component": NativeLatencyTests.longLatencyEffect(), "instanceID": "bus-latency"]]
        _ = try await host.handle("engine.project", payload: ["project": routed, "replace": true, "forcePluginState": true, "awaitPlugins": true])
        routed["audioPlugins"] = (try await host.handle("engine.snapshot", payload: [:]) as! [String: Any])["audioPlugins"]
        _ = try await host.handle("engine.project", payload: ["project": routed, "awaitPlugins": true])
        let routedLive = try await export("routed-live")
        let routedOffline = try AudioFreezeTrack.Record.parse((try await host.handle("freeze.track", payload: ["track": 0, "tail": 1]) as! [String: Any])["record"])
        _ = try await host.handle("freeze.unfreeze", payload: ["track": 0])
        let routedTaken = try AudioFreezeTrack.Record.parse((try await host.handle("freeze.track", payload: ["track": 0, "tail": 1, "method": "record", "testAnySource": true,
                                                                                                         "monitor": false, "patterns": false]) as! [String: Any])["record"])
        let routedPrint = compare(try AudioSamples.decode(routedOffline.path), try AudioSamples.decode(routedTaken.path), frames: Int(routedOffline.musicalFrames))
        try require(routedTaken.mixMode == "routed" && routedPrint.maxAbs < 1e-3 && routedPrint.reference > 0.001,
                    "routed recorded print vs offline: \(routedPrint.maxAbs) (ref \(routedPrint.reference), mode \(routedTaken.mixMode))")
        try require(host.rack.hasRouting, "the bus must come back after the take")
        let routedFrozen = try await export("routed-frozen-recorded")
        let routedDiff = compare(routedLive, routedFrozen, frames: routedLive.left.count)
        try require(routedDiff.maxAbs < 1e-3, "routed recorded frozen lead vs live: \(routedDiff.maxAbs)")
        record("Freeze by recording: a routed project (group bus) records flat and matches its offline print and live mix",
               ["maxAbs": routedPrint.maxAbs, "exportMaxAbs": routedDiff.maxAbs])
        return ["live": live.left.count]
    }

    /// Freeze by recording without any audio device or export: a manual graph, the lead
    /// (native synth) recorded through its strip while the bass holds a 257-frame insert, so
    /// the lead's strip is delayed by the compensation the take must undo. The recorded print
    /// must equal a direct engine render of the print project, sample-aligned; while frozen
    /// the plan keeps the bass's 257 frames and the lead's line input stays closed.
    @MainActor static func recordedScenario(output: URL, record: (String, [String: Any]) -> Void) async throws -> [String: Any] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let catalogueFolder = output.appendingPathComponent("catalogues-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: catalogueFolder, withIntermediateDirectories: true)
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots = []; vst3Configuration.cacheURL = catalogueFolder.appendingPathComponent("vst3.json")
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots = []; vst2Configuration.cacheURL = catalogueFolder.appendingPathComponent("vst2.json")
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: vst3Configuration), midiInputIDs: [],
                                 vst2Catalogue: VST2Catalogue(configuration: vst2Configuration))
        defer { host.shutdown() }
        var canonical = project(routed: false, delay: "x")
        canonical["audioPlugins"] = [["track": 1, "kind": "effect", "slot": 0, "component": NativeLatencyTests.longLatencyEffect(), "instanceID": "freeze-latency"]]
        _ = try await host.handle("engine.project", payload: ["project": canonical, "replace": true, "forcePluginState": true, "awaitPlugins": true])
        let snapshot = try await host.handle("engine.snapshot", payload: [:]) as! [String: Any]
        canonical["audioPlugins"] = snapshot["audioPlugins"]
        _ = try await host.handle("engine.project", payload: ["project": canonical, "awaitPlugins": true])
        try require((host.rack.latencyStatus()["frames"] as? NSNumber)?.intValue == 257, "the bass insert must set a 257-frame plan")
        let started = Date()
        let taken = try await host.handle("freeze.track", payload: ["track": 0, "tail": 1, "method": "record", "testAnySource": true,
                                                                    "monitor": false, "patterns": false]) as! [String: Any]
        let takenRecord = try AudioFreezeTrack.Record.parse(taken["record"])
        let recorded = try AudioSamples.decode(takenRecord.path)
        let musical = Int(takenRecord.musicalFrames)
        try require(takenRecord.method == "recorded" && musical == 192_000 && recorded.left.count == musical + 48_000,
                    "recorded print geometry \(takenRecord.dictionary)")
        try require((taken["compensationAfter"] as? [String: Any])?["frames"] as? NSNumber == 257, "plan after recording \(taken["compensationAfter"] ?? "nil")")
        var reference = try AudioFreezeTrack.renderProject(canonical, index: 0)
        var tracks = reference["tracks"] as! [[String: Any]]; tracks[1]["mute"] = true; reference["tracks"] = tracks
        reference["loopSong"] = false
        let expected = try engineRender(reference, song: true, row: 0, frames: musical + 48_000, stopAt: musical)
        var best = (lag: 0, maxAbs: Double.infinity)
        for lag in -600...600 {
            var worst = 0.0
            for i in stride(from: 1_024, to: musical - 1_024, by: 5) {
                worst = max(worst, Double(abs(recorded.left[i + lag] - expected.0[i]))); if worst >= best.maxAbs { break }
            }
            if worst < best.maxAbs { best = (lag, worst) }
        }
        let aligned = compare(expected, (recorded.left, recorded.right), frames: musical + 48_000)
        try require(best.lag == 0 && aligned.maxAbs < 1e-4 && aligned.reference > 0.001,
                    "recorded print vs engine render: best lag \(best.lag) (\(best.maxAbs)), lag-0 max diff \(aligned.maxAbs) (rms \(aligned.rms), ref \(aligned.reference))")
        record("Freeze by recording: take through the compensated strip is sample-aligned (257-frame plan undone)",
               ["lag": best.lag, "maxAbs": aligned.maxAbs, "rms": aligned.rms, "referenceRMS": aligned.reference, "seconds": Date().timeIntervalSince(started)])
        // Unfreeze restores the live track (and keeps the bass plan).
        let unfrozen = try await host.handle("freeze.unfreeze", payload: ["track": 0]) as! [String: Any]
        try require((unfrozen["problems"] as? [String] ?? ["?"]).isEmpty, "unfreeze \(unfrozen)")
        try require((host.rack.latencyStatus()["frames"] as? NSNumber)?.intValue == 257, "plan after unfreeze")
        record("Freeze by recording: Unfreeze restores the live track", [:])
        return ["musicalFrames": musical]
    }

    static func pureChecks() throws -> [String] {
        var names: [String] = []
        let base = project(routed: false, delay: "aufx:dely:appl")
        try require(AudioFreezeTrack.runtimeProject(base).count == base.count, "unfrozen project must pass through")
        var frozen = base
        var tracks = frozen["tracks"] as! [[String: Any]]
        tracks[0]["frozen"] = AudioFreezeTrack.Record(id: "x", path: "/tmp/print.wav", sha256: "", sampleRate: 48_000, frames: 300_000,
            musicalFrames: 192_000, tailFrames: 108_000, musicalEndBeat: 8, tailSeconds: 2.25, dormantPluginIDs: ["freeze-test-delay"],
            fingerprint: try AudioFreezeTrack.fingerprint(base, index: 0), mixMode: "flat", createdAt: "").dictionary
        frozen["tracks"] = tracks
        let runtime = try AudioFreezeTrack.runtimeProject(frozen)
        try require((runtime["audioPlugins"] as? [Any])?.isEmpty == true, "dormant plug-in must be removed")
        let steps = ((runtime["patterns"] as? [[String: Any]])?.first?["steps"] as? [[Any]]) ?? []
        try require(steps[0].allSatisfy { ($0 as? [String: Any])?["note"] == nil }, "frozen notes must be removed")
        let routedRuntime = try AudioFreezeTrack.runtimeProject({ () -> [String: Any] in
            var value = project(routed: true, delay: "x"); var t = value["tracks"] as! [[String: Any]]; t[0]["frozen"] = (frozen["tracks"] as! [[String: Any]])[0]["frozen"]; value["tracks"] = t; return value }())
        let routedPattern = (routedRuntime["patterns"] as? [[String: Any]])?.first
        try require((((routedPattern?["steps"] as? [[Any]]) ?? [])[0][6] as? [String: Any])?["fx1"] != nil, "mix step FX must stay live")
        try require((routedPattern?["automation"] as? [Any])?.count == 1, "mix automation must stay live")
        try require(steps[1].contains { ($0 as? [String: Any])?["note"] != nil }, "other track notes must remain")
        let lanes = ((runtime["patterns"] as? [[String: Any]])?.first?["automation"] as? [[String: Any]]) ?? []
        try require(lanes.isEmpty, "flat fixture has no lanes")
        let clip = (runtime["audioClips"] as? [[String: Any]])?.first
        try require(clip?["path"] as? String == "/tmp/print.wav" && clip?["track"] as? Int == 0, "print clip missing")
        names.append("Pure: runtime projection removes source, keeps mix")
        let noLoop = AudioFreezeTrack.engineProject(runtime)
        try require(((noLoop["audioClips"] as? [[String: Any]])?.first?["durationBeats"] as? Double) == 0, "non-looping song plays the whole print tail")
        var looping = runtime; looping["loopSong"] = true
        let loopClip = (AudioFreezeTrack.engineProject(looping)["audioClips"] as? [[String: Any]])?.first
        try require(abs((loopClip?["durationBeats"] as? Double ?? 0) - 8) < 1e-6, "looping song stops the print at the musical end")
        names.append("Pure: print tail follows loop mode")
        let merged = AudioFreezeTrack.mergingDormant([], canonical: frozen)
        try require(merged.count == 1 && merged[0]["instanceID"] as? String == "freeze-test-delay", "dormant descriptor must be merged")
        names.append("Pure: snapshots keep dormant plug-ins")
        var edited = frozen
        var patterns = edited["patterns"] as! [[String: Any]]
        var rows = patterns[0]["steps"] as! [[Any]]; rows[0][0] = ["note": 30, "instrument": 0] as [String: Any]; patterns[0]["steps"] = rows
        edited["patterns"] = patterns
        try require(try AudioFreezeTrack.fingerprint(edited, index: 0) != AudioFreezeTrack.fingerprint(frozen, index: 0), "note edit must make the print stale")
        var mixOnly = frozen
        var mixTracks = mixOnly["tracks"] as! [[String: Any]]; mixTracks[0]["volume"] = 0.3; mixTracks[0]["name"] = "Renamed"; mixOnly["tracks"] = mixTracks
        try require(try AudioFreezeTrack.fingerprint(mixOnly, index: 0) == AudioFreezeTrack.fingerprint(frozen, index: 0), "mix edits must not make the print stale")
        names.append("Pure: staleness fingerprint ignores mix, catches notes")
        var routed = project(routed: true, delay: "x")
        routed["routing"] = ["version": 1, "mixModel": "standard", "buses": [["id": "fx"]],
                             "sends": [["id": "s", "source": "track:lead", "destination": "bus:fx", "tap": "preInserts"]], "sidechains": []]
        do { try AudioFreezeTrack.validateFreezable(routed, index: 0); throw TrackerError.message("pre-insert send must be refused") }
        catch is AudioFreezeTrack.Failure {}
        names.append("Pure: pre-insert sends are refused")
        var liveInput = base
        var inputTracks = liveInput["tracks"] as! [[String: Any]]
        inputTracks[0]["audioInput"] = ["enabled": true, "deviceUID": "offline-interface", "channels": [2, 3], "gain": 1] as [String: Any]
        liveInput["tracks"] = inputTracks
        do { try AudioFreezeTrack.validateFreezable(liveInput, index: 0); throw TrackerError.message("live input must not silently freeze to silence") }
        catch is AudioFreezeTrack.Failure {}
        inputTracks[0]["audioInput"] = ["enabled": false, "deviceUID": "offline-interface", "channels": [2, 3], "gain": 1] as [String: Any]
        liveInput["tracks"] = inputTracks
        try AudioFreezeTrack.validateFreezable(liveInput, index: 0)
        var unrelated = inputTracks[0]; unrelated["routingID"] = "other-input"; unrelated["audioInput"] = ["enabled": true] as [String: Any]
        inputTracks.append(unrelated); liveInput["tracks"] = inputTracks
        try AudioFreezeTrack.validateFreezable(liveInput, index: 0)
        names.append("Pure: freeze refuses target live input and permits disabled or unrelated input routes")
        // Hardware: offline freeze explains Freeze by recording; recording needs the return
        // as a line input and the flat mixer; a frozen hardware track closes its input and
        // its editor plug-in (no MIDI to the synth).
        var hardware = base
        var hardwareTracks = hardware["tracks"] as! [[String: Any]]
        var hardwareInstruments = hardware["instruments"] as! [[String: Any]]
        hardwareInstruments[0]["type"] = "midi"; hardware["instruments"] = hardwareInstruments
        hardware["audioPlugins"] = [["track": 0, "kind": "instrument", "slot": 0, "component": "vst3:HARDWARE-EDITOR",
                                     "name": "Hardware Editor", "instanceID": "editor", "state": "x"]]
        do { try AudioFreezeTrack.validateFreezable(hardware, index: 0); throw TrackerError.message("a hardware editor track must not freeze offline") }
        catch let error as AudioFreezeTrack.Failure { try require(error.message.contains("Freeze by recording"), "unexpected: \(error.message)") }
        do { try AudioFreezeTrack.validateFreezable(hardware, index: 0, recorded: true); throw TrackerError.message("recording needs the line input") }
        catch let error as AudioFreezeTrack.Failure { try require(error.message.contains("line input"), "unexpected: \(error.message)") }
        hardwareTracks[0]["audioInput"] = ["enabled": true, "deviceUID": "interface", "channels": [4, 5], "gain": 1] as [String: Any]
        hardware["tracks"] = hardwareTracks
        try AudioFreezeTrack.validateFreezable(hardware, index: 0, recorded: true)
        var routedHardware = hardware; routedHardware["routing"] = ["version": 1, "mixModel": "standard", "buses": [], "sends": [], "sidechains": []]
        try AudioFreezeTrack.validateFreezable(routedHardware, index: 0, recorded: true)
        hardwareTracks[0]["frozen"] = (frozen["tracks"] as! [[String: Any]])[0]["frozen"]; hardware["tracks"] = hardwareTracks
        let hardwareRuntime = try AudioFreezeTrack.runtimeProject(hardware)
        let closedInput = ((hardwareRuntime["tracks"] as? [[String: Any]])?[0]["audioInput"] as? [String: Any])?["enabled"] as? Bool
        try require(closedInput == false && (hardwareRuntime["audioPlugins"] as? [Any])?.isEmpty == true, "frozen hardware track must close its input and editor")
        try require(((hardware["tracks"] as? [[String: Any]])?[0]["audioInput"] as? [String: Any])?["enabled"] as? Bool == true, "canonical input stays for Unfreeze")
        names.append("Pure: hardware tracks freeze by recording only; frozen they release input and editor")
        // Flat pre-insert gain: printed at track volume x master, divided out on playback.
        try require(abs(AudioFreezeTrack.printGain(base, index: 0) - 0.48) < 1e-12 && AudioFreezeTrack.printGain(base, index: 1) == 1
                    && AudioFreezeTrack.printGain(project(routed: true, delay: "x"), index: 0) == 1, "print gain")
        var gained = try AudioFreezeTrack.Record.parse((frozen["tracks"] as! [[String: Any]])[0]["frozen"]); gained.printGain = 0.48
        var gainedProject = frozen, gainedTracks = frozen["tracks"] as! [[String: Any]]
        gainedTracks[0]["frozen"] = gained.dictionary; gainedProject["tracks"] = gainedTracks
        let gainedClip = (try AudioFreezeTrack.runtimeProject(gainedProject)["audioClips"] as? [[String: Any]])?.first
        try require(abs((gainedClip?["gain"] as? Double ?? 0) - 1 / 0.48) < 1e-12, "print clip must divide out the print gain")
        names.append("Pure: flat inserts print at the live pre-insert gain")


        // Pattern prints: projection, pattern export and selection.
        try require(AudioFreezeTrack.patternsToPrint(base, index: 0).print == [0, 1], "both patterns use the lead")
        try require(abs(AudioFreezeTrack.passFrames(base, pattern: 0, sampleRate: 48_000) - 96_000) < 1e-9, "16 rows at 120 BPM = 96000 frames")
        var printed = frozen
        var printedTracks = printed["tracks"] as! [[String: Any]]
        var printedRecord = try AudioFreezeTrack.Record.parse(printedTracks[0]["frozen"])
        printedRecord.patternPrints = [AudioFreezeTrack.PatternPrint(pattern: 1, path: "/tmp/p1.wav", sha256: "", loopPath: "/tmp/p1-loop.wav", loopSha256: "",
                                                                     frames: 150_000, passFrames: 96_000, loopFrames: 150_000, passes: 2)]
        printedTracks[0]["frozen"] = printedRecord.dictionary; printed["tracks"] = printedTracks
        try require(try AudioFreezeTrack.Record.parse(printedRecord.dictionary).patternPrints.first?.loopPath == "/tmp/p1-loop.wav", "pattern prints round-trip")
        let printedRuntime = try AudioFreezeTrack.runtimeProject(printed)
        let patternClip = (printedRuntime["patternClips"] as? [[String: Any]])?.first
        try require(patternClip?["path"] as? String == "/tmp/p1.wav" && patternClip?["loopPath"] as? String == "/tmp/p1-loop.wav"
                    && patternClip?["pattern"] as? Int == 1 && patternClip?["track"] as? Int == 0, "pattern clip missing: \(String(describing: patternClip))")
        try require(AudioProject.clipAssetEntries(printedRuntime).compactMap { $0["path"] as? String }.filter { $0.hasPrefix("/tmp/p1") }.count == 2, "pattern print assets must load")
        var exportProject = printedRuntime; exportProject["audioClips"] = [[String: Any]](); exportProject["patternExport"] = true; exportProject["activePattern"] = 1
        let exportClips = AudioFreezeTrack.engineProject(exportProject)["audioClips"] as? [[String: Any]] ?? []
        try require(exportClips.count == 1 && exportClips[0]["path"] as? String == "/tmp/p1.wav" && exportClips[0]["durationBeats"] as? Double == 0, "pattern export plays the head print")
        exportProject["activePattern"] = 0
        try require((AudioFreezeTrack.engineProject(exportProject)["audioClips"] as? [Any])?.isEmpty == true, "other patterns stay silent in pattern export")
        names.append("Pure: pattern prints project to pattern clips and pattern export")
        return names
    }

    /// Direct engine render of a project (no host graph), as the freeze prints are made.
    static func engineRender(_ value: [String: Any], assets: [String: ([Float], [Float])] = [:], song: Bool, row: Int, frames: Int, stopAt: Int? = nil) throws -> ([Float], [Float]) {
        let rate = 48_000.0
        guard let engine = st_engine_create(rate) else { throw TrackerError.message("FREEZE TEST: engine allocation failed") }
        defer { st_engine_destroy(engine) }
        var slots: [String: Int] = [:]
        for (slot, item) in assets.sorted(by: { $0.key < $1.key }).enumerated() {
            let ok = item.value.0.withUnsafeBufferPointer { l in item.value.1.withUnsafeBufferPointer { r in
                st_engine_set_audio_asset(engine, Int32(slot), l.baseAddress, r.baseAddress, UInt64(l.count), rate) } }
            try require(ok != 0, "asset load failed"); slots[item.key] = slot
        }
        // The fixture's retired synths migrate to built-in single-cycle samplers; load
        // their samples as AudioHost does.
        let migrated = ProjectMigration.migrated(value)
        try AudioProject.apply(migrated, to: engine, clipAssets: slots, sampleRate: rate)
        for (index, item) in (migrated["instruments"] as? [[String: Any]] ?? []).enumerated() {
            guard item["type"] as? String == "sample", let path = item["samplePath"] as? String, BuiltinSamples.isBuiltin(path) else { continue }
            let sample = try AudioSamples.decode(try BuiltinSamples.resolve(path))
            let ok = sample.left.withUnsafeBufferPointer { l in sample.right.withUnsafeBufferPointer { r in
                st_engine_set_sample(engine, Int32(index), l.baseAddress, r.baseAddress, UInt64(l.count), sample.sampleRate) } }
            try require(ok != 0, "built-in sample load failed")
            _ = st_engine_set_sample_cycle(engine, Int32(index), Int32(sample.cycleFrames ?? 0))
        }
        st_engine_seek(engine, 0, Int32(row), 0); st_engine_play(engine, song ? 1 : 0)
        var left = [Float](repeating: 0, count: frames), right = [Float](repeating: 0, count: frames)
        var done = 0, stopped = false
        while done < frames {
            if let stopAt, done >= stopAt, !stopped { st_engine_stop(engine); stopped = true }
            var count = min(512, frames - done)
            if let stopAt, done < stopAt { count = min(count, stopAt - done) }
            left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
                st_engine_render(engine, l.baseAddress! + done, r.baseAddress! + done, UInt32(count)) } }
            done += count
        }
        return (left, right)
    }

    /// Engine-level Pattern mode: the live native track vs its pattern prints
    /// (rendered by the same engine exactly as the host prints them), played
    /// through st_engine_add_pattern_clip. Covers the head pass, loop wrap with a
    /// release tail and a note held across the wrap, and Play from a cursor row.
    static func enginePatternChecks() throws -> [(name: String, details: [String: Any])] {
        let rate = 48_000.0, pass = 96_000, tail = 72_000
        var live = project(routed: false, delay: "x")
        live["audioPlugins"] = [[String: Any]]()
        var instruments = live["instruments"] as! [[String: Any]]; instruments[0]["release"] = 0.8; live["instruments"] = instruments
        var tracks = live["tracks"] as! [[String: Any]]; tracks[0]["delaySend"] = 0.3; tracks[0]["pan"] = 0.25; live["tracks"] = tracks
        var patterns = live["patterns"] as! [[String: Any]]
        var steps = patterns[0]["steps"] as! [[Any]]
        steps[0][15] = ["note": 71, "instrument": 0, "velocity": 110, "gate": 3.0] as [String: Any] // held across the wrap
        patterns[0]["steps"] = steps; live["patterns"] = patterns

        func render(_ value: [String: Any], assets: [String: ([Float], [Float])] = [:], song: Bool, row: Int, frames: Int, stopAt: Int? = nil) throws -> ([Float], [Float]) {
            try engineRender(value, assets: assets, song: song, row: row, frames: frames, stopAt: stopAt)
        }
        // Print exactly as freeze does: the track isolated at unity, song transport,
        // stop at the musical end, then the tail.
        func printProject(passes: Int) -> [String: Any] {
            var value = live
            var t = value["tracks"] as! [[String: Any]]
            t[0]["volume"] = 1.0; t[0]["pan"] = 0.0; t[0]["delaySend"] = 0.0; t[0]["reverbSend"] = 0.0; t[1]["mute"] = true
            value["tracks"] = t; value["masterVolume"] = 1.0
            value["sequence"] = Array(repeating: 0, count: passes); value["loopSong"] = false
            return value
        }
        let head = try render(printProject(passes: 1), song: true, row: 0, frames: pass + tail, stopAt: pass)
        let looped = try render(printProject(passes: 2), song: true, row: 0, frames: 2 * pass + tail, stopAt: 2 * pass)
        let loop = (Array(looped.0[pass...]), Array(looped.1[pass...]))
        var frozen = live
        var frozenPatterns = frozen["patterns"] as! [[String: Any]]
        for p in frozenPatterns.indices { var st = frozenPatterns[p]["steps"] as! [[Any]]; st[0] = [Any](repeating: NSNull(), count: 16); frozenPatterns[p]["steps"] = st }
        frozen["patterns"] = frozenPatterns
        frozen["patternClips"] = [["path": "head", "loopPath": "loop", "track": 0, "pattern": 0, "gain": 1.0]]
        let assets = ["head": head, "loop": loop]
        var results: [(String, [String: Any])] = []
        do {
            let frames = Int(Double(pass) * 3.5)
            let reference = try render(live, song: false, row: 0, frames: frames)
            let candidate = try render(frozen, assets: assets, song: false, row: 0, frames: frames)
            let diff = compare(reference, candidate, frames: frames)
            let wrap = compare(reference, candidate, frames: 9_600, from: pass - 4_800)
            try require(diff.reference > 0.01 && diff.maxAbs < 1e-4, "engine pattern mode: max diff \(diff.maxAbs) (rms \(diff.rms), ref \(diff.reference))")
            results.append(("Engine: frozen pattern mode == live (3.5 passes: head, loop wraps with release tail and held note)",
                            ["frames": frames, "maxAbs": diff.maxAbs, "rms": diff.rms, "referenceRMS": diff.reference,
                             "wrapWindowMaxAbs": wrap.maxAbs, "wrapWindowReferenceRMS": wrap.reference]))
        }
        do {
            // Play from cursor row 5. After the first wrap the frozen track equals the
            // live one; during the first partial pass the print also carries the
            // tails of notes before row 5 (a live restart does not trigger them).
            // (The lead's delay send is off here: echoes of those extra first-pass tails
            // would otherwise legitimately continue past the wrap.)
            let row = 5, offset = 30_000, frames = Int(Double(pass) * 2.5)
            func dry(_ value: [String: Any]) -> [String: Any] {
                var copy = value, t = copy["tracks"] as! [[String: Any]]; t[0]["delaySend"] = 0.0; copy["tracks"] = t; return copy
            }
            let reference = try render(dry(live), song: false, row: row, frames: frames)
            let candidate = try render(dry(frozen), assets: assets, song: false, row: row, frames: frames)
            let afterWrap = compare(reference, candidate, frames: frames - (pass - offset), from: pass - offset)
            let firstPass = compare(reference, candidate, frames: pass - offset)
            try require(afterWrap.reference > 0.01 && afterWrap.maxAbs < 1e-4, "engine pattern mode from row 5 after the wrap: max diff \(afterWrap.maxAbs)")
            // Alignment: with only the frozen track sounding (no sends), Play from row 5
            // is exactly the from-row-0 performance shifted by row 5's start frame.
            func isolated(_ value: [String: Any]) -> [String: Any] {
                var copy = dry(value), t = copy["tracks"] as! [[String: Any]]; t[1]["mute"] = true; copy["tracks"] = t; return copy
            }
            let fromStart = try render(isolated(live), song: false, row: 0, frames: frames + offset)
            let fromRow = try render(isolated(frozen), assets: assets, song: false, row: row, frames: frames)
            let shifted = compare((Array(fromStart.0[offset...]), Array(fromStart.1[offset...])), fromRow, frames: frames)
            try require(shifted.reference > 0.01 && shifted.maxAbs < 1e-4, "engine pattern mode from row 5 is misaligned: max diff \(shifted.maxAbs)")
            results.append(("Engine: Play from cursor row 5 is row-aligned and equals live after the wrap",
                            ["frames": frames, "afterWrapMaxAbs": afterWrap.maxAbs, "afterWrapReferenceRMS": afterWrap.reference,
                             "shiftedAlignmentMaxAbs": shifted.maxAbs, "firstPartialPassMaxAbs": firstPass.maxAbs,
                             "firstPartialPassNote": "print includes tails of notes before the start row"]))
        }
        // Song mode is unchanged by pattern clips (they never play there).
        let songLive = try render(live, song: true, row: 0, frames: pass / 2)
        let songFrozen = try render(frozen, assets: assets, song: true, row: 0, frames: pass / 2)
        let leadless = compare(songLive, songFrozen, frames: pass / 2)
        try require(leadless.maxAbs > 1e-3, "song mode must not play pattern clips")
        results.append(("Engine: pattern clips are silent in Song mode", ["songDiffFromLive": leadless.maxAbs]))
        // Song loop: the print's tail must ring over the loop point (song tail clip), as the
        // live release + delay send do, with no doubled or cut tail over 2.5 passes.
        do {
            let song = 2 * pass
            var printValue = printProject(passes: 1); printValue["sequence"] = [0, 1]
            let print = try render(printValue, song: true, row: 0, frames: song + tail, stopAt: song)
            var looping = live; looping["loopSong"] = true
            var frozenLoop = frozen; frozenLoop["loopSong"] = true; frozenLoop.removeValue(forKey: "patternClips")
            frozenLoop["freezeRuntime"] = true
            frozenLoop["audioClips"] = [["id": "freeze-print-x", "path": "song", "track": 0, "startBeat": 0.0, "durationBeats": 8.0, "sourceOffset": 0.0,
                "gain": 1.0, "fadeIn": 0.0, "fadeOut": 0.0, "muted": false, "freezePrint": true, "freezeMusicalEndBeat": 8.0,
                "freezeMusicalFrames": song, "freezeTail": "auto"] as [String: Any]]
            let engineValue = AudioFreezeTrack.engineProject(frozenLoop)
            try require((engineValue["songTailClips"] as? [[String: Any]])?.first?["startFrame"] as? Int64 == Int64(song), "looping frozen song must add a song tail clip")
            let frames = Int(Double(song) * 2.5)
            let reference = try render(looping, song: true, row: 0, frames: frames)
            let candidate = try render(engineValue, assets: ["song": print], song: true, row: 0, frames: frames)
            let diff = compare(reference, candidate, frames: frames)
            let wrap = compare(reference, candidate, frames: 9_600, from: song - 4_800)
            var withoutTail = engineValue; withoutTail.removeValue(forKey: "songTailClips")
            let cut = compare(reference, try render(withoutTail, assets: ["song": print], song: true, row: 0, frames: frames), frames: 9_600, from: song)
            try require(diff.reference > 0.01 && diff.maxAbs < 1e-4, "engine song loop with printed tail: max diff \(diff.maxAbs) (rms \(diff.rms), ref \(diff.reference))")
            try require(cut.maxAbs > 1e-3, "the fixture must have a tail across the loop point (cut diff \(cut.maxAbs))")
            results.append(("Engine: looping song plays the printed tail over the loop point (== live over 2.5 passes)",
                            ["frames": frames, "maxAbs": diff.maxAbs, "rms": diff.rms, "referenceRMS": diff.reference,
                             "wrapWindowMaxAbs": wrap.maxAbs, "withoutSongTailWrapMaxAbs": cut.maxAbs]))
        }
        return results
    }
}

private func + (lhs: [String: Any], rhs: [String: Any]) -> [String: Any] { lhs.merging(rhs) { _, new in new } }

#endif

import Foundation

/// Pure "Render selection to sample" policy: request parsing, the render project (only the
/// selected tracks' notes in the range, other tracks muted and without plug-ins, plug-ins on
/// the selection's path kept), refusals, exact musical length with swing, the tail decision
/// and the output post-processing. No audio graph, devices, plug-ins or files.
@main enum AudioRenderSelectionTests {
    static var passed = 0
    static func check(_ condition: Bool, _ message: String) {
        if !condition { fputs("FAIL \(message)\n", stderr); exit(1) }
        passed += 1
    }
    static func refuses(_ text: String, _ body: () throws -> Void) {
        do { try body(); check(false, "expected a refusal containing “\(text)”") }
        catch { check(error.localizedDescription.contains(text), "refusal “\(error.localizedDescription)” lacks “\(text)”") }
    }

    static func project() -> [String: Any] {
        let note: [String: Any] = ["note": 60, "velocity": 100]
        var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: 16), count: 4)
        steps[0][0] = note; steps[0][4] = note; steps[0][9] = note
        steps[1][6] = ["note": 62, "instrument": 2] as [String: Any]
        steps[2][4] = note; steps[3][5] = note
        return [
            "bpm": 120.0, "linesPerBeat": 4, "swing": 0.0, "masterVolume": 0.5,
            "tracks": [["name": "Kick", "routingID": "kick", "instrument": 0, "groupID": "kit", "mute": true, "solo": false],
                       ["name": "Snare", "routingID": "snare", "instrument": 1, "groupID": "kit"],
                       ["name": "Bass", "routingID": "bass", "instrument": 3, "solo": true, "audioInput": ["enabled": false]],
                       ["name": "Pad", "routingID": "pad", "instrument": 3]],
            "instruments": [["type": "sample"], ["type": "sample"], ["type": "sample"], ["type": "au"], ["type": "midi"]],
            "patterns": [["id": 0, "length": 16, "steps": [[Any]](repeating: [Any](repeating: NSNull(), count: 16), count: 4), "automation": []],
                         ["id": 7, "length": 16, "steps": steps, "automation": [
                            ["owner": "track:kick", "target": "volume", "points": []],
                            ["owner": "track:bass", "target": "volume", "points": []],
                            ["owner": "master", "target": "volume", "points": []],
                            ["owner": "bus:kitbus", "target": "volume", "points": []]]]],
            "sequence": [0, 7, 0], "activePattern": 0, "loopSong": true, "audioClips": [["track": 0, "asset": "a"]],
            "audioPlugins": [
                ["owner": "track:kick", "kind": "effect", "slot": 0, "component": "stfx:eq", "instanceID": "kick-eq"],
                ["owner": "track:bass", "kind": "instrument", "slot": 0, "component": "vst3:SYNTH", "instanceID": "bass-synth"],
                ["owner": "track:pad", "kind": "instrument", "slot": 0, "component": "vst3:EDITOR", "instanceID": "pad-editor", "name": "Hardware Editor"],
                ["owner": "bus:kitbus", "kind": "effect", "slot": 0, "component": "stfx:dynamics", "instanceID": "kit-dyn"],
                ["owner": "bus:verb", "kind": "effect", "slot": 0, "component": "stfx:reverb", "instanceID": "verb"],
                ["owner": "bus:other", "kind": "effect", "slot": 0, "component": "stfx:delay", "instanceID": "other-delay"],
                ["owner": "master", "kind": "effect", "slot": 0, "component": "stfx:saturator", "instanceID": "master-sat"]],
            "routing": ["version": 1, "mixModel": "standard",
                        "buses": [["id": "kitbus", "type": "group", "solo": true], ["id": "verb", "type": "return"], ["id": "other", "type": "return"]],
                        "outputs": ["track:kick": "bus:kitbus", "track:snare": "bus:kitbus", "bus:kitbus": "master", "track:bass": "bus:other"],
                        "sends": [["source": "track:snare", "destination": "bus:verb"]],
                        "sidechains": [["source": "track:bass", "destination": ["instanceID": "kit-dyn"]],
                                       ["source": "track:kick", "destination": ["instanceID": "other-delay"]]]] as [String: Any]]
    }

    static func main() throws {
        // Request parsing.
        let request = try AudioRenderSelection.request(["pattern": 7, "tracks": [1, 0], "firstRow": 4, "lastRow": 7])
        check(request.tracks == [0, 1] && request.throughMaster && request.maximumTail == 10 && request.channels == 2
              && request.sampleRate == 48_000 && !request.normalize && request.thresholdDB == -70 && request.holdSeconds == 0.2, "request defaults")
        let custom = try AudioRenderSelection.request(["pattern": 7, "tracks": [3], "firstRow": 0, "lastRow": 0, "mode": "track", "tail": 0,
            "channels": 1, "sampleRate": 44_100, "normalize": true, "name": "  Pad 00–00 "])
        check(!custom.throughMaster && custom.maximumTail == 0 && custom.channels == 1 && custom.sampleRate == 44_100 && custom.normalize && custom.name == "Pad 00–00", "request options")
        refuses("tracks") { _ = try AudioRenderSelection.request(["pattern": 0, "tracks": [], "firstRow": 0, "lastRow": 1]) }
        refuses("tracks") { _ = try AudioRenderSelection.request(["pattern": 0, "tracks": [1, 1], "firstRow": 0, "lastRow": 1]) }
        refuses("row range") { _ = try AudioRenderSelection.request(["pattern": 0, "tracks": [0], "firstRow": 5, "lastRow": 4]) }
        refuses("maximum tail") { _ = try AudioRenderSelection.request(["pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 4, "tail": 61]) }
        refuses("44.1") { _ = try AudioRenderSelection.request(["pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 4, "sampleRate": 22_050]) }
        refuses("mono or stereo") { _ = try AudioRenderSelection.request(["pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 4, "channels": 3]) }
        refuses("Through master") { _ = try AudioRenderSelection.request(["pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 4, "mode": "pre"]) }

        // The render project: Kick + Snare rows 4–7 of pattern 7, through master.
        let base = project()
        let plan = try AudioRenderSelection.plan(base, request: request, hardwareComponents: ["vst3:EDITOR"])
        let p = plan.project
        check(p["sequence"] as? [Int] == [7] && p["activePattern"] as? Int == 7 && p["loopSong"] as? Bool == false
              && p["patternExport"] as? Bool == true && (p["audioClips"] as? [Any])?.isEmpty == true, "one non-looping pattern pass without clips")
        check(plan.pattern == 7 && plan.musicalFrames == 24_000, "pattern 7, four rows of 6000 frames")
        let pattern = (p["patterns"] as! [[String: Any]])[1], steps = pattern["steps"] as! [[Any]]
        check(pattern["length"] as? Int == 8 && steps.allSatisfy { $0.count == 8 }, "the pattern ends after the last selected row")
        check(steps[0][0] is NSNull && steps[0][4] is [String: Any] && steps[1][6] is [String: Any], "selected notes in the range only (clean start)")
        check(steps[2].allSatisfy { $0 is NSNull } && steps[3].allSatisfy { $0 is NSNull }, "unselected tracks have no notes")
        let tracks = p["tracks"] as! [[String: Any]]
        check(tracks[0]["mute"] as? Bool == false && tracks[1]["mute"] as? Bool == false && tracks[2]["mute"] as? Bool == true
              && tracks[3]["mute"] as? Bool == true && tracks.allSatisfy { $0["solo"] as? Bool == false && $0["audioInput"] == nil }, "mute/solo/input")
        let ids = Set((p["audioPlugins"] as! [[String: Any]]).compactMap { $0["instanceID"] as? String })
        check(ids == ["kick-eq", "kit-dyn", "verb", "master-sat"], "kept plug-ins: selected track, group bus, send return, master — got \(ids.sorted())")
        check(plan.keptPlugins == 4, "kept plug-in count")
        let routing = p["routing"] as! [String: Any]
        let sidechains = routing["sidechains"] as! [[String: Any]]
        check(sidechains.count == 1 && (sidechains[0]["destination"] as? [String: Any])?["instanceID"] as? String == "kit-dyn", "sidechains into dropped plug-ins are removed")
        check((routing["buses"] as! [[String: Any]]).allSatisfy { $0["solo"] as? Bool == false }, "bus solo cleared")
        let lanes = (pattern["automation"] as! [[String: Any]]).compactMap { $0["owner"] as? String }
        check(lanes == ["track:kick", "master", "bus:kitbus"], "automation of selected tracks, buses and master: \(lanes)")
        check(p["masterVolume"] as? Double == 0.5, "through master keeps the master fader")
        check(plan.warnings.contains { $0.contains("Audio clips") }, "clip warning")
        check((base["tracks"] as! [[String: Any]])[0]["mute"] as? Bool == true, "the canonical project is not modified")

        // Track only (pre-master): master unity, no master inserts or master automation.
        var preMaster = request; preMaster.throughMaster = false
        let pre = try AudioRenderSelection.plan(base, request: preMaster).project
        check(pre["masterVolume"] as? Double == 1.0, "pre-master unity")
        check(!(pre["audioPlugins"] as! [[String: Any]]).contains { $0["owner"] as? String == "master" }, "pre-master drops master inserts")
        check(!((pre["patterns"] as! [[String: Any]])[1]["automation"] as! [[String: Any]]).contains { $0["owner"] as? String == "master" }, "pre-master drops master automation")

        // Refusals.
        func planOf(_ project: [String: Any], _ tracks: [Int], _ first: Int = 4, _ last: Int = 7) throws {
            _ = try AudioRenderSelection.plan(project, request: try AudioRenderSelection.request(["pattern": 7, "tracks": tracks, "firstRow": first, "lastRow": last]),
                                               hardwareComponents: ["vst3:EDITOR"])
        }
        refuses("no notes") { try planOf(base, [1], 0, 3) }
        refuses("past the end") { try planOf(base, [0], 4, 16) }
        refuses("hardware") { try planOf(base, [3]) }
        var frozen = base, list = base["tracks"] as! [[String: Any]]
        list[0]["frozen"] = ["path": "/print.wav"]; frozen["tracks"] = list
        refuses("frozen") { try planOf(frozen, [0]) }
        var live = base; list = base["tracks"] as! [[String: Any]]
        list[1]["audioInput"] = ["enabled": true]; live["tracks"] = list
        refuses("line input") { try planOf(live, [1]) }
        var midi = base; list = base["tracks"] as! [[String: Any]]
        list[1]["instrument"] = 4; midi["tracks"] = list
        refuses("external MIDI") { try planOf(midi, [1]) }
        refuses("no longer exists") { _ = try AudioRenderSelection.plan(base, request: try AudioRenderSelection.request(["pattern": 3, "tracks": [0], "firstRow": 0, "lastRow": 1])) }

        // Legacy (no routing) plug-ins on instruments and tracks: only the selected track's.
        var legacy = base
        legacy.removeValue(forKey: "routing"); legacy.removeValue(forKey: "audioPlugins")
        var legacyTracks = base["tracks"] as! [[String: Any]]
        legacyTracks[0]["plugins"] = [["component": "stfx:eq"]]; legacyTracks[2]["plugins"] = [["component": "stfx:delay"]]
        legacy["tracks"] = legacyTracks
        var legacyInstruments = base["instruments"] as! [[String: Any]]
        legacyInstruments[3]["plugin"] = ["component": "vst3:SYNTH"]; legacy["instruments"] = legacyInstruments
        let legacyPlan = try AudioRenderSelection.plan(legacy, request: request)
        let legacyPlugins = legacyPlan.project["audioPlugins"] as! [[String: Any]]
        check(legacyPlugins.count == 1 && legacyPlugins[0]["track"] as? Int == 0 && legacyPlugins[0]["component"] as? String == "stfx:eq", "legacy plug-ins: \(legacyPlugins)")
        check(((legacyPlan.project["tracks"] as! [[String: Any]])[2]["plugins"] as? [Any])?.isEmpty == true, "legacy unselected track inserts removed")

        // Musical length: swing makes even rows longer, odd rows shorter.
        var swung = base; swung["swing"] = 0.25
        check(AudioRenderSelection.musicalFrames(swung, firstRow: 0, lastRow: 0, sampleRate: 48_000) == 7_500, "even row with swing")
        check(AudioRenderSelection.musicalFrames(swung, firstRow: 1, lastRow: 1, sampleRate: 48_000) == 4_500, "odd row with swing")
        check(AudioRenderSelection.musicalFrames(swung, firstRow: 1, lastRow: 4, sampleRate: 48_000) == 24_000, "two pairs")
        var mapped = base; mapped["timing"] = ["version": 1]
        check(AudioRenderSelection.musicalFrames(mapped, firstRow: 0, lastRow: 3, sampleRate: 48_000) == nil, "tempo maps use the engine's end")

        // Tail decision.
        typealias D = AudioRenderSelection.TailDecision
        func decide(_ rendered: Int64, _ loud: Int64, ended: Bool = true) -> D {
            AudioRenderSelection.tailDecision(rendered: rendered, lastLoud: loud, musicalFrames: 1_000, musicalEnded: ended, holdFrames: 100, maximumTailFrames: 500)
        }
        check(decide(900, 10) == .render, "inside the selection")
        check(decide(1_200, 10, ended: false) == .render, "the engine has not ended yet")
        check(decide(1_099, 10) == .render && decide(1_100, 10) == .silent(frames: 1_000), "quiet end: keep exactly the selection")
        check(decide(1_350, 1_249) == .silent(frames: 1_250) && decide(1_349, 1_249) == .render, "tail stops 100 quiet frames after the last loud frame")
        check(decide(1_501, 1_500) == .maximum(frames: 1_500) && decide(1_499, 1_450) == .render, "cap at selection + maximum tail")
        check(AudioRenderSelection.tailDecision(rendered: 1_001, lastLoud: 1_000, musicalFrames: 1_000, musicalEnded: true, holdFrames: 100, maximumTailFrames: 0) == .maximum(frames: 1_000), "zero tail cuts at the selection")

        // Output.
        let ramp = (0..<100).map { Float($0) / 100 }
        let stereo = AudioRenderSelection.finish(left: ramp, right: ramp.map { -$0 }, frames: 50, fadeFrames: 10, channels: 2, normalize: false)
        check(stereo.channels.count == 2 && stereo.channels[0].count == 50 && stereo.channels[0][49] == 0 && abs(stereo.channels[0][39] - 0.39) < 1e-6, "trim + fade")
        let mono = AudioRenderSelection.finish(left: [0.2, 0.4], right: [0.4, 0.0], frames: 2, fadeFrames: 0, channels: 1, normalize: true)
        check(mono.channels.count == 1 && abs(mono.channels[0][0] - AudioRenderSelection.linear(dB: -0.3)) < 1e-6 && abs(mono.channels[0][1] - mono.channels[0][0] * 2 / 3) < 1e-6, "mono + normalize")
        let quiet = AudioRenderSelection.finish(left: [0, 0], right: [0, 0], frames: 2, fadeFrames: 0, channels: 2, normalize: true)
        check(quiet.peak == 0 && quiet.gain == 1, "silence is not amplified")
        let values: [Float] = [0.5, 0.0, 0.001, 0.0001, 0]
        values.withUnsafeBufferPointer { l in
            [Float](repeating: 0, count: 5).withUnsafeBufferPointer { r in
                check(AudioRenderSelection.lastLoud(l, r, threshold: AudioRenderSelection.linear(dB: -70)) == 2, "last frame above −70 dBFS")
            }
        }
        print("PASS \(passed) render selection policy checks")
    }
}

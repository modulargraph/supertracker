import Foundation
import TrackerEngine

@main struct PluginStepFXTests {
    struct Failure: Error { let message: String }
    static var checks = 0
    static let address = "18446744073709551615"
    static func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try condition() else { throw Failure(message: message) }; checks += 1
    }
    static func project(routing: String? = nil) -> [String: Any] {
        var project: [String: Any] = ["tracks": [["routingID": "a", "instrument": 0], ["routingID": "b", "instrument": 0]],
            "instruments": [["type": "au"]], "bpm": 300, "linesPerBeat": 16,
            "audioPlugins": [["track": 0, "kind": "effect", "slot": 0, "instanceID": "stable-effect", "component": "test-gain"]],
            "patterns": [["id": 0, "length": 4, "steps": [[
                ["note": -3, "fx1": ["type": "plugin", "instanceID": "stable-effect", "address": address, "value": 0.7123456789012345]],
                NSNull(), NSNull(), NSNull()], [NSNull(), NSNull(), NSNull(), NSNull()]]]], "sequence": [0]]
        if let routing { project["routing"] = ["version": 1, "mixModel": routing, "buses": [], "outputs": [:], "sends": [], "sidechains": []] }
        return project
    }
    static func editFX(_ project: inout [String: Any], _ mutate: (inout [String: Any]) -> Void) {
        var patterns = project["patterns"] as! [[String: Any]], steps = patterns[0]["steps"] as! [[Any]], step = steps[0][0] as! [String: Any]
        var fx = step["fx1"] as! [String: Any]; mutate(&fx); step["fx1"] = fx; steps[0][0] = step; patterns[0]["steps"] = steps; project["patterns"] = patterns
    }
    static func map(_ project: [String: Any]) throws -> AudioPluginStepFX { try AudioProject.pluginStepFX(project) }
    static func rejects(_ project: [String: Any], _ message: String) throws {
        do { _ = try AudioProject.routingAutomation(project) }
        catch is AudioRouting.RoutingError { checks += 1; return }
        throw Failure(message: message)
    }
    /// Note columns through the real publisher (AudioProject.apply): step.columns[i] plays as note column i + 1 up to the
    /// track's noteColumns, each its own lane; an OFF in column 1 ends only column 1's note.
    static func noteColumns() throws {
        let engine = st_engine_create(8000)!
        defer { st_engine_destroy(engine) }
        let project: [String: Any] = ["tracks": [["instrument": 0, "noteColumns": 3], ["instrument": 0]], "instruments": [["type": "au"]],
            "bpm": 300, "linesPerBeat": 16, "sequence": [0], "patterns": [["id": 0, "length": 4, "steps": [
                [["note": 60, "velocity": 100, "gate": 0, "columns": [["note": 64, "velocity": 90], ["note": 67, "velocity": 80, "gate": 1]]],
                 ["note": -3, "columns": [["note": -1]]], NSNull(), NSNull()],
                [["note": -3, "columns": [["note": 71]]], NSNull(), NSNull(), NSNull()]]]]]
        try AudioProject.apply(project, to: engine); st_engine_play(engine, 0)
        var on: [Int32: Int] = [:], off: [Int32: Int] = [:], frame = 0
        for _ in 0..<3 {
            st_engine_render(engine, nil, nil, 100)
            var event = STEvent()
            while st_engine_pop_event(engine, &event) != 0 {
                if event.type == ST_EVENT_NOTE_ON { on[event.note] = frame + Int(event.frameOffset) }
                if event.type == ST_EVENT_NOTE_OFF { off[event.note] = frame + Int(event.frameOffset) }
            }
            frame += 100
        }
        try require(on[60] == 0 && on[64] == 0 && on[67] == 0, "Every note column of a row starts together")
        try require(on[71] == nil, "A one-column track ignores extra note columns")
        try require(off[67] == 100 && off[64] == 100 && off[60] == nil, "Column 2's gate and column 1's OFF end only their own notes")
    }
    static func main() throws {
        for mode in [nil, "legacy", "standard"] as [String?] {
            var data = project(routing: mode)
            let initial = try map(data)
            try require(initial.bindings.count == 1 && initial.unavailableFXs.isEmpty, "Valid lock missing")
            let binding = initial.bindings[0]
            try require(binding.address == UInt64.max && binding.value == 0.7123456789012345, "Exact parameter data changed")
            try require(binding.slot == 0 && binding.destination >= 0, "Wrong dispatch mode")
            if mode != nil {
                let mapping = try AudioProject.routingAutomation(data)
                try require(mapping.entries[binding.destination].requiresExactIdentity, "Explicit identity lost")
            }
            // Reordering moves identity, not the lock. Replacement/removal cannot
            // retarget the parameter to whichever effect now occupies old slot0.
            data["audioPlugins"] = [["track": 0, "kind": "effect", "slot": 1, "instanceID": "stable-effect", "component": "test-gain"],
                                    ["track": 0, "kind": "effect", "slot": 0, "instanceID": "replacement", "component": "test-gain"]]
            try require(try map(data).bindings[0].slot == 1, "Effect reorder silently retargeted")
            data["audioPlugins"] = [["track": 0, "kind": "effect", "slot": 0, "instanceID": "replacement", "component": "test-gain"]]
            try require(try map(data).bindings.isEmpty && map(data).unavailableFXs.count == 1, "Missing identity retargeted slot")
            data = project(routing: mode)
            var patterns = data["patterns"] as! [[String: Any]], steps = patterns[0]["steps"] as! [[Any]]
            steps[1][0] = steps[0][0]; steps[0][0] = NSNull(); patterns[0]["steps"] = steps; data["patterns"] = patterns
            try require(try map(data).bindings.isEmpty && map(data).unavailableFXs[0].reason.contains("different track"), "Cross-track pasted lock drove original")
            // Missing/quarantined processors still have desired descriptors. Dense
            // host IDs are assigned from intent and are independent of live units.
            data = project(routing: mode)
            var plugins = data["audioPlugins"] as! [[String: Any]]; plugins[0]["missing"] = true; data["audioPlugins"] = plugins
            try require(try map(data).bindings.count == 1, "Desired missing slot lost identity")
            let encoded = try JSONSerialization.data(withJSONObject: data)
            let reopened = try JSONSerialization.jsonObject(with: encoded) as! [String: Any]
            try require(try map(reopened).bindings[0].address == UInt64.max, "JSON roundtrip lost address")

            // Exercise actual AudioProject -> C scheduler in every routing mode.
            let engine = st_engine_create(8000)!
            try AudioProject.apply(reopened, to: engine); st_engine_play(engine, 0); st_engine_render(engine, nil, nil, 1)
            var event = STEvent(), captured: [STEvent] = []
            while st_engine_pop_event(engine, &event) != 0 {
                if event.address == UInt64.max { captured.append(event) }
            }
            st_engine_destroy(engine)
            try require(captured.count == 1 && captured[0].value == binding.value && captured[0].frameOffset == 0, "AudioProject failed exact lock delivery")
            try require(captured[0].type == ST_EVENT_HOST_PARAMETER, "AudioProject wrong event dispatch")
        }
        for bad in [NSNumber(value: 9007199254740993 as UInt64), "-1", "+1", "01", "18446744073709551616", "1.0", "", " 1"] as [Any] {
            var data = project(); editFX(&data) { $0["address"] = bad }; try rejects(data, "Invalid address accepted: \(bad)")
        }
        for bad in [NSNumber(value: true), -0.1, 1.1, Double.nan, Double.infinity, "0.5"] as [Any] {
            var data = project(); editFX(&data) { $0["value"] = bad }; try rejects(data, "Invalid normalized value accepted")
        }
        var data = project(); editFX(&data) { $0.removeValue(forKey: "instanceID") }
        try require(try map(data).bindings.isEmpty && map(data).unavailableFXs.count == 1, "Missing identity was inferred")
        data = project(); var plugins = data["audioPlugins"] as! [[String: Any]]; plugins.append(plugins[0]); data["audioPlugins"] = plugins
        try require(try map(data).bindings.isEmpty && map(data).unavailableFXs.count == 1, "Ambiguous identity resolved")
        data = project(); data["audioPlugins"] = []
        data["instruments"] = [["type": "au", "plugin": ["instanceID": "stable-effect", "component": "test-gain"]]]
        try require(try map(data).bindings.isEmpty, "Authoritative empty descriptors fell back to stale instrument")
        data = project(); data.removeValue(forKey: "audioPlugins")
        data["instruments"] = [["type": "au", "plugin": ["instanceID": "stable-effect", "component": "test-gain"]], ["type": "synth"]]
        data["tracks"] = [["instrument": 0], ["instrument": 1]]
        try require(try map(data).bindings[0].slot == -1, "Legacy instrument descriptor did not resolve")
        // Old built-in steps remain untouched and unknown FX are inert.
        data = project(); editFX(&data) { $0 = ["type": "volume", "value": 0.3] }
        try require(try map(data).bindings.isEmpty && map(data).unavailableFXs.isEmpty, "Built-in effect changed")
        try require(AudioProject.effect(["type": "volume", "value": 0.3]).type == ST_FX_VOLUME, "Built-in FX ABI changed")
        // Ownerless lanes with unknown targets are skipped, not turned into track volume.
        try require(AudioProject.nativeAutomationTarget("volume")! == (0, 0, 0) && AudioProject.nativeAutomationTarget("cutoff")!.target == 2, "Native lane target changed")
        try require(AudioProject.nativeAutomationTarget("send:fx1") == nil && AudioProject.nativeAutomationTarget("futureTarget") == nil, "Unknown lane fell back to track volume")
        try require(AudioProject.nativeAutomationTarget("plugin:instrument:0:12")! == (7, -1, 12) && AudioProject.nativeAutomationTarget("plugin:2:99")! == (7, 2, 99), "Plug-in lane target changed")
        try noteColumns()
        print("PASS \(checks) plug-in step FX adapter checks: exact identity/address, routing modes, reorder, missing/paste safety, malformed input and real engine dispatch")
    }
}

import Foundation

@main struct PluginSnapshotCaptureTests {
    static func main() throws {
        func check(_ condition: Bool) { precondition(condition) }
        func rejected(_ snapshot: Any, containing text: String? = nil) throws {
            do {
                _ = try PluginSnapshotCapture.states(from: snapshot)
            } catch {
                if let text { precondition(error.localizedDescription.contains(text), error.localizedDescription) }
                return
            }
            throw TrackerError.message("Malformed/failed capture was accepted: \(snapshot)")
        }
        for snapshot: Any in [NSNull(), [], "bad", [:], ["audioPlugins": NSNull()], ["audioPlugins": "bad"], ["audioPlugins": ["bad"]], ["audioPlugins": [["state": 123]]]] {
            try rejected(snapshot)
        }
        try rejected(["audioPlugins": [["name": "Saved Synth", "state": "previous opaque patch", "stateError": "Render still busy"]]], containing: "Saved Synth: Render still busy")
        try rejected(["audioPlugins": [["component": "missing.component", "stateError": NSNull()]]], containing: "missing.component")
        try rejected(["audioPlugins": [["stateError": ""]]], containing: "Plug-in 1")
        try rejected(["audioPlugins": [["name": "Bad metadata", "latency": Double.nan]]])
        check(try PluginSnapshotCapture.states(from: ["audioPlugins": []]).isEmpty)
        let desired: [[String: Any]] = [["track": 0, "kind": "instrument", "slot": 0, "component": "unavailable.component", "name": "Unavailable Synth", "state": "opaque recovery bytes, deliberately not decoded", "recoveryBlocked": true], ["track": 1, "kind": "effect", "slot": 0, "component": "unavailable.effect"]]
        let result = try PluginSnapshotCapture.states(from: ["audioPlugins": desired])
        let encode: (Any) throws -> Data = { try JSONSerialization.data(withJSONObject: $0, options: .sortedKeys) }
        check(try encode(result) == encode(desired))

        let root = FileManager.default.temporaryDirectory.appendingPathComponent("PluginCapture-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let store = try ProjectStore(directory: root.appendingPathComponent("Recovery"))
        var project: [String: Any] = ["version": 1, "name": "Capture fixture", "bpm": 120.0, "tracks": [["id": 0]], "instruments": [], "patterns": [["id": 0, "length": 1, "steps": [[]]]], "sequence": [0], "audioPlugins": desired]
        let destination = root.appendingPathComponent("Original.supertracker")
        _ = try store.save(to: destination, snapshot: project, collectSamples: false)
        let before = try Data(contentsOf: destination), recoveryBefore = try Data(contentsOf: store.recoveryURL)
        do {
            project["audioPlugins"] = try PluginSnapshotCapture.states(from: ["audioPlugins": [["name": "Failed Synth", "state": "old patch", "stateError": "Cannot capture current patch"]]])
            _ = try store.save(to: destination, snapshot: project, collectSamples: false)
            throw TrackerError.message("Failed capture reached save")
        } catch {
            precondition(error.localizedDescription.contains("Could not capture Failed Synth"), error.localizedDescription)
        }
        check(try Data(contentsOf: destination) == before)
        check(try Data(contentsOf: store.recoveryURL) == recoveryBefore)
        print("PASS Plug-in snapshot capture: malformed responses/state rejected; named live errors reject fallback patches; desired unavailable opaque state preserved; existing project and recovery bytes unchanged")
    }
}

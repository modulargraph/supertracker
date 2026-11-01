import Foundation

private struct TestFailure: Error { let message: String }
private final class SearchProbe: @unchecked Sendable {
    let gate = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var count = 0
    var calls: Int { lock.lock(); defer { lock.unlock() }; return count }
    func enter() throws {
        guard !Thread.isMainThread else { throw TestFailure(message: "Filesystem search ran on the main thread") }
        lock.lock(); count += 1; lock.unlock()
        guard gate.wait(timeout: .now() + 5) == .success else { throw TestFailure(message: "Worker test gate timed out") }
    }
}

@main struct MediaRelinkServiceTests {
    static func check(_ condition: Bool, _ message: String) throws {
        if !condition { throw TestFailure(message: message) }
    }
    @MainActor static func waitFor(_ condition: () -> Bool) async throws {
        for _ in 0..<500 {
            if condition() { return }
            try await Task.sleep(nanoseconds: 2_000_000)
        }
        throw TestFailure(message: "Async operation did not reach the expected checkpoint")
    }
    static func expectCancellation(_ task: Task<[String: Any], Error>) async throws {
        do { _ = try await task.value; throw TestFailure(message: "Cancelled search returned results") }
        catch MediaRelinker.Failure.cancelled { }
    }
    static func kitFixture(_ project: [String: Any], missing: String) -> [String: Any] {
        var result = project
        var sounds: [[String: Any]] = (0..<16).map { index in
            ["id": "slot-\(index)", "name": "Sound \(index + 1)", "instrument": ["type": "synth"]]
        }
        sounds[0]["instrument"] = ["type": "sample", "samplePath": missing,
                                    "zones": [["path": missing, "samplePath": missing, "rootNote": 48]]]
        result["kits"] = [["id": "worker-kit", "name": "Worker kit", "busID": "kit-group", "sounds": sounds]]
        result["tracks"] = [["id": 0, "routingID": "lane"]]
        result["routing"] = ["version": 1, "mixModel": "standard",
                             "buses": [["id": "kit-group", "name": "Kit group", "type": "group"]],
                             "outputs": ["track:lane": "master", "bus:kit-group": "master"],
                             "sends": [[String: Any]](), "sidechains": [[String: Any]]()]
        result["audioPlugins"] = [["owner": "sound:worker-kit:slot-0", "kind": "effect", "slot": 0,
                                   "state": ["path": "/vendor/opaque.wav", "bytes": "AAECAw=="]]]
        return result
    }

    @MainActor static func main() async throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-media-service-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let replacement = folder.appendingPathComponent("Kick.wav"), missing = folder.appendingPathComponent("Offline/Kick.wav")
        let bytes = Data(repeating: 19, count: 123)
        try bytes.write(to: replacement)
        let project: [String: Any] = ["version": 1, "name": "Unsaved session", "bpm": 120.0,
            "tracks": [["id": 0]], "instruments": [["id": 0, "samplePath": missing.path]],
            "patterns": [["id": 0, "length": 64, "steps": [[]]]], "sequence": [0]]

        var operations = MediaRelinkService.Operations.filesystem
        let filesystem = operations
        operations.inspect = { project, url in
            try check(!Thread.isMainThread, "Inspect ran filesystem work on the main thread")
            return try filesystem.inspect(project, url)
        }
        operations.apply = { project, url, selections, cancelled in
            try check(!Thread.isMainThread, "Apply ran filesystem work on the main thread")
            return try filesystem.apply(project, url, selections, cancelled)
        }
        let service = MediaRelinkService(operations: operations)
        let report = try await service.inspect(project, projectURL: nil)
        try check(report["missingCount"] as? Int == 1, "Unsaved project media was not inspected")
        let found = try await service.search(project, projectURL: nil, roots: [folder.path])
        let reference = (found["missing"] as! [[String: Any]])[0]
        let candidates = reference["candidates"] as! [[String: Any]]
        try check(candidates.contains { $0["path"] as? String == replacement.path }, "Worker search did not locate replacement")
        let result = try await service.apply(project, selections: [["id": reference["id"]!, "from": missing.path, "to": replacement.path]], projectURL: nil)
        let proposed = result["project"] as! [String: Any]
        let store = try ProjectStore(directory: folder.appendingPathComponent("Support"))
        try store.validate(proposed)
        try check((project["instruments"] as! [[String: Any]])[0]["samplePath"] as? String == missing.path, "Apply mutated its input snapshot")
        try check((proposed["instruments"] as! [[String: Any]])[0]["samplePath"] as? String == replacement.path, "Apply did not return proposed replacements")
        try check(store.project == nil, "Proposal committed project store state")
        try check(try Data(contentsOf: replacement) == bytes, "Replacement source was modified")

        let kitProject = kitFixture(project, missing: missing.path)
        try store.validate(kitProject)
        let kitReport = try await service.inspect(kitProject, projectURL: nil)
        try check(kitReport["missingCount"] as? Int == 4, "Worker inventory omitted kit sample/zone usages")
        let kitFound = try await service.search(kitProject, projectURL: nil, roots: [folder.path])
        let kitReferences = (kitFound["missing"] as! [[String: Any]]).filter { $0["kitID"] as? String == "worker-kit" }
        try check(kitReferences.count == 3 && kitReferences.allSatisfy { ($0["candidates"] as? [[String: Any]])?.contains { $0["path"] as? String == replacement.path } == true }, "Worker search did not locate kit media replacements")
        let kitResult = try await service.apply(kitProject, selections: kitReferences.map { ["id": $0["id"]!, "from": $0["path"]!, "to": replacement.path] }, projectURL: nil)
        let proposedKit = kitResult["project"] as! [String: Any]
        try store.validate(proposedKit)
        try check(kitResult["changed"] as? Int == 3 && (kitResult["remaining"] as? [Any])?.count == 1, "Worker proposal must repair only the selected kit references")
        try check(try JSONSerialization.data(withJSONObject: proposedKit["audioPlugins"]!, options: .sortedKeys) == JSONSerialization.data(withJSONObject: kitProject["audioPlugins"]!, options: .sortedKeys), "Worker relink changed opaque plug-in state")
        try check(MediaRelinker.inspect(kitProject)["missingCount"] as? Int == 4 && store.project == nil, "Kit proposal mutated the original snapshot or store")

        // Gate a real filesystem search to verify simultaneous requests share one walk.
        let probe = SearchProbe()
        operations.search = { project, url, roots, relocations, limits, cancelled, progress in
            try probe.enter()
            return try filesystem.search(project, url, roots, relocations, limits, cancelled, progress)
        }
        let coalesced = MediaRelinkService(operations: operations)
        var events: [[String: Any]] = []
        coalesced.onProgress = { events.append($0) }
        let first = Task { try await coalesced.search(kitProject, projectURL: nil, roots: [folder.path], requestID: "first") }
        try await waitFor { probe.calls == 1 }
        var duplicateEntered = false
        let duplicate = Task { duplicateEntered = true; return try await coalesced.search(kitProject, projectURL: nil, roots: [folder.path], requestID: "duplicate") }
        try await waitFor { duplicateEntered }
        probe.gate.signal()
        let firstResult = try await first.value, duplicateResult = try await duplicate.value
        try check(probe.calls == 1 && firstResult["operationId"] as? String == duplicateResult["operationId"] as? String, "Identical searches did not coalesce")
        try check(duplicateResult["requestId"] as? String == "duplicate", "Coalesced caller lost its request identifier")
        try check(events.contains { $0["requestId"] as? String == "duplicate" && $0["phase"] as? String == "complete" }, "Progress did not reach both coalesced callers")

        let old = Task { try await coalesced.search(kitProject, projectURL: nil, roots: [folder.path], requestID: "old") }
        try await waitFor { probe.calls == 2 }
        var changed = kitProject
        var changedKits = changed["kits"] as! [[String: Any]]
        var changedSounds = changedKits[0]["sounds"] as! [[String: Any]]
        var changedInstrument = changedSounds[0]["instrument"] as! [String: Any]
        let latestPath = folder.appendingPathComponent("Offline/Latest.wav").path
        changedInstrument["samplePath"] = latestPath; changedSounds[0]["instrument"] = changedInstrument
        changedKits[0]["sounds"] = changedSounds; changed["kits"] = changedKits
        var newerEntered = false
        let newer = Task { newerEntered = true; return try await coalesced.search(changed, projectURL: nil, roots: [folder.path], requestID: "new") }
        try await waitFor { newerEntered }
        probe.gate.signal()
        try await expectCancellation(old)
        try await waitFor { probe.calls == 3 }
        probe.gate.signal()
        let latest = try await newer.value
        try check((latest["missing"] as? [[String: Any]])?.contains { $0["path"] as? String == latestPath } == true, "A nested kit edit must search the latest snapshot rather than coalesce into stale results")
        try check(events.contains { $0["requestId"] as? String == "old" && $0["phase"] as? String == "cancelled" }, "Superseded search was not cancelled")

        let cancelled = Task { try await coalesced.search(project, projectURL: nil, roots: [folder.path], requestID: "cancel") }
        try await waitFor { probe.calls == 4 }
        _ = coalesced.cancel(); probe.gate.signal()
        try await expectCancellation(cancelled)
        _ = try await coalesced.inspect(project, projectURL: nil)

        var limits = MediaRelinker.Limits(); limits.maxFiles = 1
        try Data([1]).write(to: folder.appendingPathComponent("Other.wav"))
        let bounded = MediaRelinkService(limits: limits)
        let limited = try await bounded.search(project, projectURL: nil, roots: [folder.path])
        try check(limited["truncated"] as? Bool == true, "Search ignored its file bound")
        do { _ = try await bounded.search(project, projectURL: nil, roots: ["relative/path"]); throw TestFailure(message: "Relative root was accepted") }
        catch MediaRelinker.Failure.invalid { }
        print("PASS media service: utility workers, unsaved and kit snapshots, read-only proposals, opaque state preservation, coalescing, nested Sound edit cancellation, explicit cancellation, progress IDs, bounded search")
    }
}

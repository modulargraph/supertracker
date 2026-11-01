import Foundation

// swiftc -module-cache-path /private/tmp/supertracker-takes-module-cache Sources/SuperTracker/ProjectStore.swift Sources/SuperTracker/MediaRelinker.swift tests/TakesMediaTests.swift -o /private/tmp/supertracker-takes-media-tests
@main struct TakesMediaTests {
    struct Failure: Error, CustomStringConvertible { let description: String }
    static func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !condition() { throw Failure(description: message) }
    }
    static func canonical(_ value: Any) throws -> Data {
        try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    }
    static func rejects(_ message: String, _ body: () throws -> Void) throws {
        do { try body() } catch is MediaRelinker.Failure { return }
        throw Failure(description: message)
    }
    static func clips(_ project: [String: Any]) -> [[String: Any]] { project["audioClips"] as! [[String: Any]] }
    static func takes(_ project: [String: Any]) -> [[String: Any]] { (clips(project)[0]["takeSet"] as! [String: Any])["takes"] as! [[String: Any]] }
    static func takeMetadata(_ project: [String: Any]) -> [[String: Any]] {
        takes(project).map { item in var value = item; value.removeValue(forKey: "path"); return value }
    }
    static func wav(_ value: Int16) -> Data {
        var result = Data()
        func text(_ value: String) { result.append(contentsOf: value.utf8) }
        func u16(_ value: UInt16) { result.append(UInt8(value & 255)); result.append(UInt8(value >> 8)) }
        func u32(_ value: UInt32) { for shift in stride(from: 0, to: 32, by: 8) { result.append(UInt8((value >> shift) & 255)) } }
        let frames: UInt32 = 160
        text("RIFF"); u32(36 + frames * 2); text("WAVEfmt "); u32(16); u16(1); u16(1)
        u32(8_000); u32(16_000); u16(2); u16(16); text("data"); u32(frames * 2)
        for _ in 0..<frames { u16(UInt16(bitPattern: value)) }
        return result
    }
    static func fixture(_ root: URL) throws -> ([String: Any], [URL], [Data]) {
        let folder = root.appendingPathComponent("Original library")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let files = ["Active.wav", "Alternate one.wav", "Alternate two.wav"].map { folder.appendingPathComponent($0) }
        let contents = [wav(12), wav(-34), wav(56)]
        for (file, data) in zip(files, contents) { try data.write(to: file) }
        let takeSet: [String: Any] = ["version": 1, "activeID": "take-active", "takes": [
            ["id": "take-active", "name": "First performance", "path": files[0].path, "sourceOffset": 0.002, "sourceDuration": 0.020],
            ["id": "take-alt-one", "name": "Second performance", "path": files[1].path, "sourceOffset": 0.004, "sourceDuration": 0.020],
            ["id": "take-alt-two", "name": "Third performance", "path": files[2].path, "sourceOffset": 0.006],
            ["id": "take-shared", "name": "Second performance trimmed", "path": files[1].path, "sourceOffset": 0.008, "sourceDuration": 0.020]
        ]]
        let project: [String: Any] = ["version": 1, "name": "Take portability", "bpm": 120.0,
            "tracks": [["id": 0]], "instruments": [["id": 0, "samplePath": files[0].path]],
            "patterns": [["id": 0, "length": 64, "steps": [[]]]], "sequence": [0],
            "audioClips": [["id": "clip", "track": 0, "name": "Performance", "path": files[0].path,
                "startBeat": 4.0, "durationBeats": 0.02, "sourceOffset": 0.002, "gain": 0.7,
                "fadeIn": 0.001, "fadeOut": 0.002, "takeSet": takeSet]]]
        return (project, files, contents)
    }
    static func portable(_ root: URL) throws {
        let (project, originals, contents) = try fixture(root)
        let store = try ProjectStore(directory: root.appendingPathComponent("Private support"))
        let folder = root.appendingPathComponent("Before")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let file = folder.appendingPathComponent("Takes.supertracker")
        let fingerprinted = try MediaRelinker.fingerprint(project)["project"] as! [String: Any]
        let saved = try store.save(to: file, snapshot: fingerprinted)
        let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as! [String: Any]
        let relative = takes(raw).map { $0["path"] as! String }
        try require(relative.allSatisfy { $0.hasPrefix("Takes.media/") }, "Inactive takes must be collected into relative portable paths")
        try require(Set(relative).count == 3 && relative[1] == relative[3], "Shared take audio must use one collected path")
        let manifest = raw["mediaManifest"] as! [[String: Any]]
        try require(Set(manifest.compactMap { $0["path"] as? String }) == Set(relative), "All alternate fingerprints must follow their collected relative media paths")
        try require(clips(raw)[0]["path"] as? String == relative[0], "Active outer path and embedded take must remain aligned")
        try require(try canonical(takeMetadata(raw)) == canonical(takeMetadata(project)), "Saving must retain take IDs, names, offsets, durations and order")
        let media = folder.appendingPathComponent("Takes.media")
        try require(try FileManager.default.contentsOfDirectory(atPath: media.path).count == 3, "One active plus two unused alternate WAVs must be copied exactly once")
        let again = try store.save(to: file)
        try require(try canonical(again) == canonical(saved), "Repeated save must preserve collected take paths")
        try require(try FileManager.default.contentsOfDirectory(atPath: media.path).count == 3, "Repeated save must not copy shared audio again")
        let relocated = root.appendingPathComponent("After")
        try FileManager.default.moveItem(at: folder, to: relocated)
        let loaded = try store.read(relocated.appendingPathComponent(file.lastPathComponent))
        let restored = takes(loaded)
        for (index, take) in restored.enumerated() {
            let path = take["path"] as! String
            try require(path == relocated.appendingPathComponent(relative[index]).path, "Every take must resolve relative to the relocated project")
            try require(try Data(contentsOf: URL(fileURLWithPath: path)) == contents[index == 3 ? 1 : index], "Collected WAV bytes must be exact")
        }
        try require(try canonical(takeMetadata(loaded)) == canonical(takeMetadata(project)), "Read must retain all take metadata")
        try require((clips(loaded)[0]["takeSet"] as! [String: Any])["activeID"] as? String == "take-active", "Portable round trip must retain selected take")
        try require(clips(loaded)[0]["sourceOffset"] as? Double == 0.002, "Portable round trip must retain selected source offset")
        for (file, data) in zip(originals, contents) { try require(try Data(contentsOf: file) == data, "Portable save must preserve original audio") }
        print("PASS takes portable: two inactive WAVs, shared audio deduplication, repeated save, relocated read, metadata and original bytes")
    }
    static func relink(_ root: URL) throws {
        var (initial, originals, _) = try fixture(root)
        var aliases = clips(initial); aliases[0]["samplePath"] = originals[0].path; initial["audioClips"] = aliases
        let fingerprinted = try MediaRelinker.fingerprint(initial)
        try require(fingerprinted["fingerprinted"] as? Int == 3, "Fingerprinting must include both inactive take audio files and deduplicate shared audio")
        let project = fingerprinted["project"] as! [String: Any]
        let moved = root.appendingPathComponent("Moved library")
        try FileManager.default.moveItem(at: originals[0].deletingLastPathComponent(), to: moved)
        let report = try MediaRelinker.findCandidates(project, searchRoots: [moved])
        let missing = report["missing"] as! [[String: Any]]
        let inactive = missing.filter { $0["path"] as? String == originals[2].path }
        try require(inactive.count == 1, "Media inspector must expose a missing inactive take")
        func selection(_ row: [String: Any]) throws -> [String: Any] {
            let candidates = row["candidates"] as? [[String: Any]] ?? []
            guard let candidate = candidates.first(where: { $0["verified"] as? Bool == true }) else { throw Failure(description: "Every take must have a fingerprint-verified relocated candidate") }
            return ["id": row["id"]!, "from": row["path"]!, "to": candidate["path"]!]
        }
        let inactiveResult = try MediaRelinker.apply(project, selections: [selection(inactive[0])])["project"] as! [String: Any]
        try require(takes(inactiveResult)[2]["path"] as? String == moved.appendingPathComponent(originals[2].lastPathComponent).path, "Relinking an inactive take must replace its embedded path")
        try require(clips(inactiveResult)[0]["path"] as? String == originals[0].path, "Relinking an inactive take must preserve the selected audio")
        var wrong = try selection(inactive[0]); wrong["to"] = moved.appendingPathComponent(originals[1].lastPathComponent).path
        try rejects("An inactive take must reject different-content audio") { _ = try MediaRelinker.apply(project, selections: [wrong]) }
        var stale = try selection(inactive[0]); stale["from"] = originals[1].path
        try rejects("A stale embedded take reference must be rejected") { _ = try MediaRelinker.apply(project, selections: [stale]) }
        let shared = missing.filter { $0["path"] as? String == originals[1].path }
        try require(shared.count == 2, "Both independent take entries sharing one file must be inventoried")
        let sharedResult = try MediaRelinker.apply(project, selections: [selection(shared[0])])["project"] as! [String: Any]
        try require(takes(sharedResult).filter { $0["path"] as? String == originals[1].path }.count == 1, "Selecting one inactive take must not silently replace another shared reference")
        let activeRows = missing.filter { $0["path"] as? String == originals[0].path && ($0["id"] as? String ?? "").hasPrefix("/audioClips/") }
        try require(activeRows.count == 3, "Active take, clip path, and legacy samplePath alias must be inventoried")
        for row in activeRows {
            let result = try MediaRelinker.apply(project, selections: [selection(row)])["project"] as! [String: Any]
            let expected = moved.appendingPathComponent(originals[0].lastPathComponent).path
            try require(clips(result)[0]["path"] as? String == expected && takes(result)[0]["path"] as? String == expected, "Relinking either active clip reference must synchronize the selected take and playback path")
            try require(clips(result)[0]["samplePath"] as? String == expected, "Relinking an active reference must synchronize its legacy samplePath alias")
        }
        let firstActive = try selection(activeRows[0])
        var conflicting = try selection(activeRows[1]); conflicting["to"] = moved.appendingPathComponent(originals[1].lastPathComponent).path; conflicting["allowDifferentContent"] = true
        try rejects("Linked active references must reject conflicting replacement paths atomically") { _ = try MediaRelinker.apply(project, selections: [firstActive, conflicting]) }
        var inconsistent = project, inconsistentClips = clips(project), inconsistentSet = clips(project)[0]["takeSet"] as! [String: Any], inconsistentTakes = takes(project)
        inconsistentTakes[0]["path"] = originals[2].path; inconsistentSet["takes"] = inconsistentTakes; inconsistentClips[0]["takeSet"] = inconsistentSet; inconsistent["audioClips"] = inconsistentClips
        let outer = try selection(activeRows.first { ($0["id"] as? String) == "/audioClips/0/path" }!)
        try rejects("Relinking must not overwrite a disagreeing active take using the outer clip fingerprint") { _ = try MediaRelinker.apply(inconsistent, selections: [outer]) }
        let repaired = try MediaRelinker.apply(project, selections: missing.map(selection))["project"] as! [String: Any]
        try require(MediaRelinker.inspect(repaired)["missingCount"] as? Int == 0, "All embedded take references must be repairable")
        try require(try canonical(takeMetadata(repaired)) == canonical(takeMetadata(project)), "Relinking must preserve names, offsets, source lengths, IDs and order")
        try require(takes(repaired)[1]["path"] as? String == takes(repaired)[3]["path"] as? String, "Shared alternate references must remain aligned when all are relinked")
        try ProjectStore(directory: root.appendingPathComponent("Validation support")).validate(repaired)
        print("PASS takes relink: inactive inventory, three distinct fingerprints, verified recovery, active-path synchronization and metadata retention")
    }
    static func main() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-takes-media-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        try portable(root.appendingPathComponent("Portable"))
        if !CommandLine.arguments.contains("--portable-only") { try relink(root.appendingPathComponent("Relink")) }
    }
}

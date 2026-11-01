import Foundation

@main struct MediaPortableTests {
    static func main() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-media-portable-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let source = root.appendingPathComponent("Library/Kick.wav")
        try FileManager.default.createDirectory(at: source.deletingLastPathComponent(), withIntermediateDirectories: true)
        let original = Data(repeating: 7, count: 123)
        try original.write(to: source)
        let initial: [String: Any] = ["version": 1, "name": "Portable", "bpm": 120.0, "tracks": [["id": 0]],
            "instruments": [["id": 0, "samplePath": source.path]], "patterns": [["id": 0, "length": 64, "steps": [[]]]], "sequence": [0]]
        let fingerprinted = try MediaRelinker.fingerprint(initial)["project"] as! [String: Any]
        let folder = root.appendingPathComponent("Before")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let file = folder.appendingPathComponent("Session.supertracker")
        let store = try ProjectStore(directory: root.appendingPathComponent("Support"))
        let saved = try store.save(to: file, snapshot: fingerprinted)
        let stored = try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as! [String: Any]
        let samplePath = (stored["instruments"] as! [[String: Any]])[0]["samplePath"] as! String
        let manifestPath = (stored["mediaManifest"] as! [[String: Any]])[0]["path"] as! String
        guard samplePath == manifestPath, samplePath.hasPrefix("Session.media/"),
              MediaRelinker.inspect(saved)["missingCount"] as? Int == 0 else { throw TrackerError.message("Portable save did not keep media fingerprints aligned with sample paths.") }
        let relocated = root.appendingPathComponent("After")
        try FileManager.default.moveItem(at: folder, to: relocated)
        let loaded = try store.read(relocated.appendingPathComponent("Session.supertracker"))
        guard MediaRelinker.inspect(loaded)["missingCount"] as? Int == 0 else { throw TrackerError.message("Moving a portable project should keep its relative media available.") }
        let collected = URL(fileURLWithPath: (loaded["instruments"] as! [[String: Any]])[0]["samplePath"] as! String)
        try FileManager.default.moveItem(at: collected, to: root.appendingPathComponent("Temporarily unavailable.wav"))
        let report = try MediaRelinker.findCandidates(loaded, searchRoots: [source.deletingLastPathComponent()])
        let reference = (report["missing"] as! [[String: Any]])[0]
        let candidate = (reference["candidates"] as! [[String: Any]]).first { $0["verified"] as? Bool == true }
        guard candidate?["match"] as? String == "sha256" else { throw TrackerError.message("Collected project must recover its original library file by full hash.") }
        let repaired = try MediaRelinker.apply(loaded, selections: [["id": reference["id"]!, "from": reference["path"]!, "to": candidate!["path"]!]])
        try store.validate(repaired["project"] as! [String: Any])
        guard try Data(contentsOf: source) == original else { throw TrackerError.message("Original library media was modified.") }
        print("PASS media portable integration: collected hashes, relative manifest paths, moved project folder, full-hash recovery, schema validation, original library preserved")
    }
}

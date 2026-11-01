import Foundation
import CryptoKit

private struct RelinkTestFailure: Error { let message: String }
private func check(_ value: @autoclosure () throws -> Bool, _ message: String) throws {
    if try !value() { throw RelinkTestFailure(message: message) }
}
private func encoded(_ value: [String: Any]) throws -> Data { try JSONSerialization.data(withJSONObject: value, options: .sortedKeys) }

@main struct MediaRelinkerTests {
    static func main() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-relink-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try movedRootAndTransactionalApply(root)
        try ambiguityAndDifferentContent(root)
        try portablePrefixAndRelativePaths(root)
        try renamedByContentAndSafety(root)
        print("PASS media relinking: samples/zones/clips, root relocation, SHA256, collected prefix, relative paths, ambiguity, explicit mismatch override, stale selection, atomic failure, cancellation, bounds, symlink exclusion, originals preserved")
    }
    private static func put(_ data: Data, _ url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url)
    }
    private static func fixture(_ sample: String, zone: String? = nil, clip: String? = nil) -> [String: Any] {
        var instrument: [String: Any] = ["id": 0, "name": "Drums", "type": "sample", "samplePath": sample, "cutoff": 1234]
        if let zone { instrument["zones"] = [["samplePath": zone, "rootNote": 42, "keyLow": 20, "keyHigh": 80, "volume": 0.7], ["path": sample, "rootNote": 64]] }
        var result: [String: Any] = ["version": 1, "name": "Relink fixture", "bpm": 120.0, "linesPerBeat": 4, "tracks": [["id": 0]],
            "instruments": [instrument], "patterns": [["id": 0, "length": 64, "steps": [[]]]], "sequence": [0],
            "audioPlugins": [["opaqueVendorState": ["path": "/keep/opaque/plugin.wav"]]]]
        if let clip { result["audioClips"] = [["id": "clip-1", "name": "Field recording", "track": 0, "path": clip,
                                               "startBeat": 4.0, "durationBeats": 8.0, "sourceOffset": 1.25, "fadeIn": 0.2, "gain": 0.6]] }
        return result
    }
    private static func missing(_ report: [String: Any]) -> [[String: Any]] { report["missing"] as? [[String: Any]] ?? [] }
    private static func candidates(_ row: [String: Any]) -> [[String: Any]] { row["candidates"] as? [[String: Any]] ?? [] }
    private static func selections(_ report: [String: Any]) throws -> [[String: Any]] {
        try missing(report).map { row in
            guard let candidate = candidates(row).first(where: { $0["verified"] as? Bool == true }) else { throw RelinkTestFailure(message: "Expected a verified candidate") }
            return ["id": row["id"]!, "from": row["path"]!, "to": candidate["path"]!]
        }
    }
    private static func movedRootAndTransactionalApply(_ root: URL) throws {
        let original = root.appendingPathComponent("Original/Pack"), relocated = root.appendingPathComponent("Relocated/Pack")
        let kick = original.appendingPathComponent("Kick.wav"), zone = original.appendingPathComponent("Zone.aif"), clip = original.appendingPathComponent("Recording.caf")
        let kickData = Data((0..<73).map(UInt8.init)), zoneData = Data(repeating: 3, count: 81), clipData = Data(repeating: 5, count: 90)
        try put(kickData, kick); try put(zoneData, zone); try put(clipData, clip)
        let source = fixture(kick.path, zone: zone.path, clip: clip.path), sourceBytes = try encoded(source)
        let fingerprinted = try MediaRelinker.fingerprint(source)
        try check(fingerprinted["fingerprinted"] as? Int == 3, "Duplicate references must be hashed once")
        let project = fingerprinted["project"] as! [String: Any]
        try check(try encoded(source) == sourceBytes, "Fingerprinting must not mutate the caller's project")
        try check(MediaRelinker.inspect(project)["presentCount"] as? Int == 4, "Sample, two zone aliases, and clip must be inventoried")
        try FileManager.default.createDirectory(at: relocated.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.moveItem(at: original, to: relocated)
        let before = try encoded(project)
        let report = try MediaRelinker.findCandidates(project, searchRoots: [relocated], relocations: [.init(from: original.path, to: relocated.path)])
        try check(missing(report).count == 4, "Moved media must include every usage")
        try check(missing(report).allSatisfy { candidates($0).contains { $0["verified"] as? Bool == true && $0["match"] as? String == "sha256" } }, "Root mappings must verify actual content")
        let applied = try MediaRelinker.apply(project, selections: selections(report))
        let updated = applied["project"] as! [String: Any]
        try check(applied["changed"] as? Int == 4 && (applied["remaining"] as? [Any])?.isEmpty == true, "Apply must repair all selected usages")
        try check(try encoded(project) == before, "Apply must return a copied project")
        let instrument = (updated["instruments"] as! [[String: Any]])[0]
        let zones = instrument["zones"] as! [[String: Any]]
        let updatedClip = (updated["audioClips"] as! [[String: Any]])[0]
        try check(instrument["cutoff"] as? Int == 1234 && zones[0]["rootNote"] as? Int == 42 && updatedClip["sourceOffset"] as? Double == 1.25, "Relinking must preserve sound and clip edits")
        try check(try Data(contentsOf: relocated.appendingPathComponent("Kick.wav")) == kickData, "Relinking must not modify sample content")
        try check(try Data(contentsOf: relocated.appendingPathComponent("Zone.aif")) == zoneData, "Relinking must not modify zone content")
        try check(try Data(contentsOf: relocated.appendingPathComponent("Recording.caf")) == clipData, "Relinking must not modify clip content")
        try check(try encoded(["plugins": updated["audioPlugins"]!]) == encoded(["plugins": source["audioPlugins"]!]), "Opaque plugin paths must not be rewritten")
        try check((updated["mediaManifest"] as? [[String: Any]])?.count == 3, "Manifest must follow active paths without duplicate copies")
    }
    private static func ambiguityAndDifferentContent(_ root: URL) throws {
        let source = root.appendingPathComponent("AmbiguousOld/Hit.wav"), data = Data(repeating: 9, count: 64)
        try put(data, source)
        let project = try MediaRelinker.fingerprint(fixture(source.path))["project"] as! [String: Any]
        let first = root.appendingPathComponent("AmbiguousA/Hit.wav"), second = root.appendingPathComponent("AmbiguousB/Hit.wav"), wrong = root.appendingPathComponent("AmbiguousWrong/Hit.wav")
        try put(data, first); try put(data, second); try put(Data(repeating: 8, count: 64), wrong)
        try FileManager.default.removeItem(at: source)
        let roots = [first.deletingLastPathComponent(), second.deletingLastPathComponent(), wrong.deletingLastPathComponent()]
        let report = try MediaRelinker.findCandidates(project, searchRoots: roots)
        let reverse = try MediaRelinker.findCandidates(project, searchRoots: roots.reversed())
        let row = missing(report)[0], options = candidates(row)
        try check(row["ambiguous"] as? Bool == true && row["resolution"] as? String == "ambiguous", "Duplicate content paths require explicit disambiguation")
        try check(options.filter { $0["verified"] as? Bool == true }.count == 2, "Both identical files must remain visible")
        try check(options.map { $0["path"] as! String } == candidates(missing(reverse)[0]).map { $0["path"] as! String }, "Candidate ordering must not depend on chosen-root order")
        try check(options.first(where: { ($0["path"] as? String)?.hasSuffix("AmbiguousWrong/Hit.wav") == true })?["contentMismatch"] as? Bool == true, "Same-name different content must be flagged")
        let selection: [String: Any] = ["id": row["id"]!, "from": row["path"]!, "to": wrong.path]
        try rejects("Fingerprint mismatch must fail") { _ = try MediaRelinker.apply(project, selections: [selection]) }
        var deliberate = selection; deliberate["allowDifferentContent"] = true
        let replaced = try MediaRelinker.apply(project, selections: [deliberate])
        try check(!(replaced["warnings"] as? [String] ?? []).isEmpty, "Deliberate replacement must be reported")
        var stale = selection; stale["from"] = "/old/stale.wav"
        try rejects("Stale selections must fail") { _ = try MediaRelinker.apply(project, selections: [stale]) }
        var invalid = selection; invalid["id"] = "/audioPlugins/0/opaqueVendorState/path"
        let valid: [String: Any] = ["id": row["id"]!, "from": row["path"]!, "to": first.path]
        let original = try encoded(project)
        try rejects("A bad second selection must reject the entire transaction") { _ = try MediaRelinker.apply(project, selections: [valid, invalid]) }
        try check(try encoded(project) == original, "Failed batch must leave the caller project unchanged")
    }
    private static func portablePrefixAndRelativePaths(_ root: URL) throws {
        let data = Data(repeating: 4, count: 87)
        let prefix = SHA256.hash(data: data).prefix(8).map { String(format: "%02x", $0) }.joined()
        let directory = root.appendingPathComponent("Portable"), projectURL = directory.appendingPathComponent("Song.supertracker")
        let relative = "Song.media/\(prefix)-Original.wav"
        let restored = root.appendingPathComponent("Recovered/Original.wav")
        try put(data, restored)
        let project = fixture(relative)
        let inventory = MediaRelinker.inspect(project, projectURL: projectURL)
        try check(missing(inventory)[0]["expectedHashPrefix"] as? String == prefix, "Portable collected name must supply its SHA256 prefix")
        try check((missing(inventory)[0]["resolvedPath"] as? String)?.hasSuffix(relative) == true, "Relative paths must resolve beside the project")
        let report = try MediaRelinker.findCandidates(project, projectURL: projectURL, searchRoots: [restored.deletingLastPathComponent()])
        try check(candidates(missing(report)[0]).first?["match"] as? String == "hash-prefix", "Original unprefixed filename must be found and content-verified")
        let applied = try MediaRelinker.apply(project, selections: selections(report), projectURL: projectURL)
        try check((applied["remaining"] as? [Any])?.isEmpty == true, "Portable reference must relink to verified absolute media")
        let unknown = MediaRelinker.inspect(project)
        try check(missing(unknown)[0]["status"] as? String == "unresolvedRelativePath", "Relative media without a project location must not resolve against the process cwd")
    }
    private static func renamedByContentAndSafety(_ root: URL) throws {
        let old = root.appendingPathComponent("RenameOld/Original.wav"), data = Data((0..<97).map(UInt8.init))
        try put(data, old)
        let project = try MediaRelinker.fingerprint(fixture(old.path))["project"] as! [String: Any]
        let new = root.appendingPathComponent("RenameNew/Changed name.wav")
        try put(data, new); try FileManager.default.removeItem(at: old)
        try put(Data(repeating: 2, count: 97), new.deletingLastPathComponent().appendingPathComponent("Wrong renamed.wav"))
        try put(data, new.deletingLastPathComponent().appendingPathComponent("._Original.wav"))
        try FileManager.default.createSymbolicLink(at: new.deletingLastPathComponent().appendingPathComponent("cycle"), withDestinationURL: new.deletingLastPathComponent())
        let report = try MediaRelinker.findCandidates(project, searchRoots: [new.deletingLastPathComponent()])
        try check(candidates(missing(report)[0]).contains { ($0["path"] as? String)?.hasSuffix("Changed name.wav") == true && $0["verified"] as? Bool == true }, "Same-size renamed content must be discovered using its hash")
        try check(!candidates(missing(report)[0]).contains { ($0["path"] as? String)?.contains("._") == true }, "AppleDouble files must not be candidates")
        try rejects("Cancellation must be honored") { _ = try MediaRelinker.findCandidates(project, searchRoots: [root], cancelled: { true }) }
        var limits = MediaRelinker.Limits(); limits.maxHashes = 0
        let limited = try MediaRelinker.findCandidates(project, searchRoots: [new.deletingLastPathComponent()], limits: limits)
        try check(limited["truncated"] as? Bool == true && !candidates(missing(limited)[0]).contains { $0["verified"] as? Bool == true }, "Verification exhaustion must remain unverified and be disclosed")
        try rejects("Apply must not silently bypass a content-check budget") { _ = try MediaRelinker.apply(project, selections: selections(report), maxHashBytes: 1) }
        let weak = fixture(root.appendingPathComponent("Missing/Changed name.wav").path)
        let weakReport = try MediaRelinker.findCandidates(weak, searchRoots: [new.deletingLastPathComponent()])
        let weakRow = missing(weakReport)[0]
        try check(candidates(weakRow).allSatisfy { $0["verified"] as? Bool == false }, "Matching a name alone must never claim content verification")
        let manual = try MediaRelinker.apply(weak, selections: [["id": weakRow["id"]!, "from": weakRow["path"]!, "to": new.path]])
        try check(manual["changed"] as? Int == 1, "Explicit manual file selection must work without an old fingerprint")
        try check(try Data(contentsOf: new) == data, "Search and apply must preserve replacement originals")
    }
    private static func rejects(_ message: String, _ operation: () throws -> Void) throws {
        var rejected = false
        do { try operation() } catch { rejected = true }
        try check(rejected, message)
    }
}

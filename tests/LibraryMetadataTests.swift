import Foundation

/// Public-command regression tests. All samples and caches are temporary fixtures.
@main
struct LibraryMetadataTests {
    struct Failure: Error, CustomStringConvertible {
        let description: String
        init(_ description: String) { self.description = description }
    }

    struct Fixture {
        let directory: URL
        let root: URL
        let cache: URL
        let kick: URL
        let snare: URL
        let pad: URL

        init() throws {
            directory = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-library-metadata-\(UUID().uuidString)")
            root = directory.appendingPathComponent("Samples")
            cache = directory.appendingPathComponent("Cache")
            kick = root.appendingPathComponent("Test Pack/One Shots/Kick_120bpm_Cmin.wav")
            snare = root.appendingPathComponent("Test Pack/Loops/Snare_100bpm_Dmaj.wav")
            pad = root.appendingPathComponent("Test Pack/Pad.wav")
            try Self.wav(kick, seconds: 0.25)
            try Self.wav(snare, seconds: 0.5)
            try Self.wav(pad, seconds: 0.75)
            try FileManager.default.createDirectory(at: cache, withIntermediateDirectories: true)
        }

        func library() -> LibraryService {
            LibraryService(cacheDirectory: cache, defaultRoots: [root], loadCacheAsynchronously: false)
        }
        var metadataFile: URL { cache.appendingPathComponent("metadata-v1.json") }
        var indexFile: URL { cache.appendingPathComponent("index-v1.json") }
        func remove() { try? FileManager.default.removeItem(at: directory) }

        static func wav(_ path: URL, seconds: Double) throws {
            try FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
            func word(_ value: UInt32) -> Data { Data((0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) }) }
            let bytes = Int(seconds * 8000) * 2
            var data = Data("RIFF".utf8) + word(UInt32(36 + bytes)) + Data("WAVEfmt ".utf8) + word(16)
            data += Data([1, 0, 1, 0]) + word(8000) + word(16000) + Data([2, 0, 16, 0])
            data += Data("data".utf8) + word(UInt32(bytes)) + Data(repeating: 0, count: bytes)
            try data.write(to: path)
        }
    }

    static func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !condition() { throw Failure(message) }
    }

    static func command(_ library: LibraryService, _ name: String, _ payload: [String: Any] = [:]) async throws -> [String: Any] {
        guard let result = try await library.handle(name, payload: payload) as? [String: Any] else {
            throw Failure("\(name) did not return a dictionary")
        }
        return result
    }

    static func scan(_ library: LibraryService) async throws {
        _ = try await command(library, "library.scan")
        for _ in 0..<1000 {
            let result = try await command(library, "library.status")
            if result["scanning"] as? Bool == false {
                try require(result["count"] as? Int == 3, "fixture scan must index all three WAV files")
                try require((result["errors"] as? [String] ?? []).isEmpty, "fixture scan must succeed")
                return
            }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        throw Failure("fixture scan timed out")
    }

    static func search(_ library: LibraryService, _ payload: [String: Any] = [:]) async throws -> [String: Any] {
        try await command(library, "library.search", payload)
    }

    static func rows(_ result: [String: Any]) -> [[String: Any]] { result["items"] as? [[String: Any]] ?? [] }
    static func row(_ result: [String: Any], _ path: URL) throws -> [String: Any] {
        guard let value = rows(result).first(where: { $0["path"] as? String == path.path }) else {
            throw Failure("missing result for \(path.lastPathComponent)")
        }
        return value
    }
    static func metadata(_ row: [String: Any]) -> [String: Any] { row["userMetadata"] as? [String: Any] ?? [:] }
    static func inferred(_ row: [String: Any]) -> [String: Any] { row["inferredMetadata"] as? [String: Any] ?? [:] }
    static func canonical(_ value: Any) throws -> Data { try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) }
    static func edit(_ library: LibraryService, _ paths: [URL], _ changes: [String: Any]) async throws -> [String: Any] {
        try await command(library, "library.metadata.edit", ["paths": paths.map(\.path), "changes": changes])
    }
    static func reject(_ library: LibraryService, _ payload: [String: Any], _ message: String) async throws {
        do { _ = try await command(library, "library.metadata.edit", payload) }
        catch { return }
        throw Failure("invalid edit accepted: \(message)")
    }
    static func facetCount(_ result: [String: Any], _ field: String, _ value: String) -> Int {
        let facets = result["facets"] as? [String: [[String: Any]]] ?? [:]
        return facets[field]?.first { $0["value"] as? String == value }?["count"] as? Int ?? 0
    }
    static func awaitDuration(_ library: LibraryService, _ path: URL, _ seconds: Double) async throws -> [String: Any] {
        for _ in 0..<1000 {
            let item = try row(await search(library), path)
            if let duration = item["duration"] as? Double, abs(duration - seconds) < 0.001 { return item }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        throw Failure("header duration did not become \(seconds)")
    }
    static func awaitIndex(_ fixture: Fixture, matching predicate: ([String: Any]) -> Bool = { _ in true }) async throws -> [String: Any] {
        for _ in 0..<1000 {
            if let data = try? Data(contentsOf: fixture.indexFile),
               let result = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               rows(result).count == 3, predicate(result) { return result }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        throw Failure("inferred cache was not persisted")
    }

    static func batchSearchAndSourcePreservation() async throws {
        let fixture = try Fixture(); defer { fixture.remove() }
        let paths = [fixture.kick, fixture.snare, fixture.pad]
        let bytes = try paths.map { try Data(contentsOf: $0) }
        let dates = try paths.map { try FileManager.default.attributesOfItem(atPath: $0.path)[.modificationDate] as? Date }
        let library = fixture.library(); try await scan(library)
        let before = try await search(library)
        let response = try await edit(library, [fixture.kick, fixture.snare, fixture.kick], ["category": "  Texture  ", "tags": [" Custom ", "custom", "", "Wide"]])
        try require(response["updated"] as? Int == 2, "duplicate paths must count once")
        for path in [fixture.kick, fixture.snare] {
            let item = try row(response, path), original = try row(before, path)
            try require(item["category"] as? String == "Texture", "batch category must be trimmed and applied")
            try require(item["tags"] as? [String] == ["Custom", "Wide"], "tags must be trimmed and deduplicated")
            try require(item["bpm"] as? Double == original["bpm"] as? Double && item["key"] as? String == original["key"] as? String, "omitted tempo and key must be unchanged")
            try require(item["sampleType"] as? String == original["sampleType"] as? String, "omitted sample type must be unchanged")
            try require(Set(metadata(item).keys) == ["category", "tags"], "partial edits must not create unrelated overrides")
            try require(try canonical(inferred(item)) == canonical(inferred(original)), "inferred metadata must stay available")
        }
        let after = try await search(library)
        try require(try canonical(metadata(row(after, fixture.pad))) == canonical([String: String]()), "unselected item must remain unedited")
        try require(facetCount(after, "categories", "Texture") == 2 && facetCount(after, "categories", "Kick") == 0, "category facets must reflect overrides")
        try require(facetCount(after, "tags", "Custom") == 2 && facetCount(after, "tags", "Kick") == 0, "tag facets must reflect replacement")
        let filtered = try await search(library, ["query": "tag:custom", "filters": ["categories": ["Texture"]]])
        try require(filtered["total"] as? Int == 2, "query and category filters must find overrides immediately")
        _ = try await edit(library, [fixture.kick], ["bpm": 133.5, "key": "f♯MIN", "sampleType": "loop"])
        let musical = try await search(library, ["filters": ["bpm": ["min": 133, "max": 134], "keys": ["F# minor"], "sampleTypes": ["loop"]]])
        try require(musical["total"] as? Int == 1, "tempo/key/type filters must use edited values")
        try require(try row(musical, fixture.kick)["key"] as? String == "F# minor", "key aliases must be canonicalized")
        for (input, expected) in [("DB minor", "Db minor"), ("BB MAJOR", "Bb major")] {
            let changed = try await edit(library, [fixture.kick], ["key": input])
            try require(try row(changed, fixture.kick)["key"] as? String == expected, "uppercase flat accidentals must retain their accidental and mode")
        }
        for (path, seconds) in [(fixture.kick, 0.25), (fixture.snare, 0.5), (fixture.pad, 0.75)] {
            _ = try await awaitDuration(library, path, seconds)
        }
        for index in paths.indices {
            try require(try Data(contentsOf: paths[index]) == bytes[index], "editing and header enrichment must leave sample bytes unchanged")
            let current = try FileManager.default.attributesOfItem(atPath: paths[index].path)[.modificationDate] as? Date
            try require(current == dates[index], "editing and header enrichment must leave source mtime unchanged")
        }
    }

    static func unknownAndReset() async throws {
        let fixture = try Fixture(); defer { fixture.remove() }
        let library = fixture.library(); try await scan(library)
        let original = try row(await search(library), fixture.kick)
        let unknown = try row(await edit(library, [fixture.kick], ["category": "Texture", "tags": [], "bpm": 0, "key": "", "sampleType": "unknown"]), fixture.kick)
        try require(unknown["bpm"] == nil && unknown["key"] == nil && unknown["sampleType"] as? String == "unknown", "explicit unknown must hide inferred values")
        try require(metadata(unknown)["bpm"] as? Double == 0 && metadata(unknown)["key"] as? String == "", "explicit unknown must remain a stored override")
        try require(unknown["tags"] as? [String] == [], "empty tags must hide inferred tags")
        let unknownResults = try await search(library, ["filters": ["keys": ["unknown"], "sampleTypes": ["unknown"], "bpm": ["includeUnknown": true]]])
        try require(unknownResults["total"] as? Int == 2, "unknown filters must include explicit and naturally unknown items")
        let partial = try row(await edit(library, [fixture.kick], ["bpm": NSNull(), "key": NSNull(), "sampleType": NSNull()]), fixture.kick)
        try require(partial["bpm"] as? Double == original["bpm"] as? Double && partial["key"] as? String == original["key"] as? String, "null must restore inferred tempo and key")
        try require(partial["sampleType"] as? String == "oneshot" && partial["category"] as? String == "Texture" && partial["tags"] as? [String] == [], "partial reset must preserve omitted overrides")
        let reset = try row(await edit(library, [fixture.kick], ["category": NSNull(), "tags": NSNull()]), fixture.kick)
        try require(metadata(reset).isEmpty, "resetting final fields must clear all overrides")
        try require(reset["category"] as? String == original["category"] as? String && reset["tags"] as? [String] == original["tags"] as? [String], "reset must restore inferred category and tags")
        let persisted = try JSONSerialization.jsonObject(with: Data(contentsOf: fixture.metadataFile)) as? [String: Any]
        try require((persisted?["items"] as? [String: Any])?.isEmpty == true, "empty override records must be removed from durable storage")
    }

    static func persistenceAndChangedSource() async throws {
        let fixture = try Fixture(); defer { fixture.remove() }
        let library = fixture.library(); try await scan(library)
        _ = try await awaitIndex(fixture)
        let changes: [String: Any] = ["category": "Texture", "tags": ["Persistent"], "bpm": 0, "key": "", "sampleType": "loop"]
        let edited = try row(await edit(library, [fixture.kick], changes), fixture.kick)
        let saved = try Data(contentsOf: fixture.metadataFile)
        let index = try await awaitIndex(fixture)
        let cachedKick = try row(index, fixture.kick)
        try require(cachedKick["category"] as? String == "Kick" && cachedKick["bpm"] as? Double == 120, "inferred cache must not contain effective overrides")
        let restored = fixture.library()
        let cached = try row(await search(restored), fixture.kick)
        try require(try canonical(metadata(cached)) == canonical(metadata(edited)), "recreated service must restore every override")
        try require(cached["bpm"] == nil && cached["key"] == nil, "unknown overrides must survive recreation")
        _ = try await awaitDuration(restored, fixture.kick, 0.25)
        try Fixture.wav(fixture.kick, seconds: 1)
        try FileManager.default.setAttributes([.modificationDate: Date(timeIntervalSince1970: 2_000_000_000)], ofItemAtPath: fixture.kick.path)
        let changedBytes = try Data(contentsOf: fixture.kick)
        try await scan(restored)
        let rescanned = try row(await search(restored), fixture.kick)
        try require((rescanned["size"] as? NSNumber)?.intValue == changedBytes.count, "reindex must detect changed source bytes")
        try require(try canonical(metadata(rescanned)) == canonical(metadata(edited)), "source replacement must preserve overrides by path")
        let enriched = try await awaitDuration(restored, fixture.kick, 1)
        try require(try canonical(metadata(enriched)) == canonical(metadata(edited)), "new duration metadata must preserve persisted overrides")
        let updatedIndex = try await awaitIndex(fixture) { result in
            (try? row(result, fixture.kick)["size"] as? Int) == changedBytes.count
        }
        let updatedCachedKick = try row(updatedIndex, fixture.kick)
        try require(updatedCachedKick["category"] as? String == "Kick" && updatedCachedKick["bpm"] as? Double == 120 && updatedCachedKick["key"] as? String == "C minor", "post-edit cache write must persist inference rather than effective overrides")
        try require(updatedCachedKick["tags"] as? [String] != ["Persistent"] && updatedCachedKick["userMetadata"] == nil, "inferred cache must remain independent of override storage")
        try require(try Data(contentsOf: fixture.metadataFile) == saved, "scanning and header enrichment must not rewrite authoritative overrides")
        let reset = try row(await edit(restored, [fixture.kick], ["category": NSNull(), "tags": NSNull(), "bpm": NSNull(), "key": NSNull(), "sampleType": NSNull()]), fixture.kick)
        try require(reset["category"] as? String == "Kick" && reset["bpm"] as? Double == 120 && reset["key"] as? String == "C minor", "reset after reindex must reveal refreshed inference")
        try require(abs((reset["duration"] as? Double ?? -1) - 1) < 0.001, "metadata reset must retain refreshed duration")
    }

    static func durationPreservesEdits() async throws {
        let fixture = try Fixture(); defer { fixture.remove() }
        let library = fixture.library(); try await scan(library)
        let edited = try row(await edit(library, [fixture.kick], ["category": "Texture", "tags": ["Before header"], "bpm": 142, "key": "Cm", "sampleType": "loop"]), fixture.kick)
        try require(edited["duration"] == nil, "fixture must begin without enriched duration")
        let enriched = try await awaitDuration(library, fixture.kick, 0.25)
        try require(try canonical(metadata(enriched)) == canonical(metadata(edited)), "asynchronous header publication must preserve all overrides")
        try require(enriched["category"] as? String == "Texture" && enriched["bpm"] as? Double == 142 && enriched["key"] as? String == "C minor", "effective fields must survive asynchronous header publication")
    }

    static func validationAndAddTags() async throws {
        let fixture = try Fixture(); defer { fixture.remove() }
        let library = fixture.library(); try await scan(library)
        let before = try await search(library)
        let added = try await edit(library, [fixture.kick, fixture.snare], ["addTags": [" Shared ", "shared"]])
        for path in [fixture.kick, fixture.snare] {
            let original = try row(before, path)["tags"] as? [String] ?? []
            let tags = try row(added, path)["tags"] as? [String] ?? []
            try require(tags == original + ["Shared"], "batch addTags must retain each item's own tags and deduplicate")
        }
        let invalidChanges: [[String: Any]] = [
            ["category": ""], ["category": String(repeating: "x", count: 81)],
            ["tags": [1]], ["tags": Array(repeating: "x", count: 65)],
            ["tags": [String(repeating: "x", count: 81)]], ["bpm": true], ["bpm": -1], ["bpm": 1000],
            ["bpm": Double.nan], ["bpm": Double.infinity], ["bpm": "120"], ["key": "H minor"],
            ["sampleType": "other"], ["unrecognized": "value"], ["addTags": NSNull()],
            ["addTags": ["Added"], "tags": ["Replacement"]], ["category": "Changed", "bpm": -1]
        ]
        let baselineBytes = try Data(contentsOf: fixture.metadataFile)
        let baseline = try await search(library)
        for (index, changes) in invalidChanges.enumerated() {
            try await reject(library, ["paths": [fixture.kick.path, fixture.snare.path], "changes": changes], "case \(index)")
        }
        for payload: [String: Any] in [
            ["paths": [], "changes": ["category": "Changed"]],
            ["paths": [fixture.kick.path, fixture.root.appendingPathComponent("Missing.wav").path], "changes": ["category": "Changed"]],
            ["paths": Array(repeating: fixture.kick.path, count: 501), "changes": ["category": "Changed"]],
            ["paths": [fixture.kick.path]], ["paths": [""], "changes": ["category": "Changed"]]
        ] { try await reject(library, payload, "invalid paths or shape") }
        try require(try Data(contentsOf: fixture.metadataFile) == baselineBytes, "invalid edits must not alter saved bytes")
        let rejected = try await search(library)
        for path in [fixture.kick, fixture.snare] {
            try require(try canonical(metadata(row(rejected, path))) == canonical(metadata(row(baseline, path))), "invalid edits must not partially mutate the batch")
        }
        // Loops/Snare sorts before One Shots/Kick in the backend's normalized batch.
        _ = try await edit(library, [fixture.kick], ["tags": (0..<64).map { "Tag\($0)" }])
        let fullBytes = try Data(contentsOf: fixture.metadataFile)
        let fullBefore = try await search(library)
        try await reject(library, ["paths": [fixture.kick.path, fixture.snare.path], "changes": ["category": "Changed", "addTags": ["Overflow"]]], "second item exceeds resulting tag limit")
        try require(try Data(contentsOf: fixture.metadataFile) == fullBytes, "later batch validation failure must not save earlier changes")
        let fullAfter = try await search(library)
        for path in [fixture.kick, fixture.snare] {
            try require(try canonical(metadata(row(fullBefore, path))) == canonical(metadata(row(fullAfter, path))), "later batch failure must roll back every item")
        }
    }

    static func failedWritePreservesMemory() async throws {
        let fixture = try Fixture(); defer { fixture.remove() }
        let library = fixture.library(); try await scan(library)
        let savedResult = try await edit(library, [fixture.kick, fixture.snare], ["category": "Saved"])
        let savedBytes = try Data(contentsOf: fixture.metadataFile)
        try FileManager.default.removeItem(at: fixture.metadataFile)
        try FileManager.default.createDirectory(at: fixture.metadataFile, withIntermediateDirectories: false)
        let sentinel = fixture.metadataFile.appendingPathComponent("keep")
        try Data("preserve".utf8).write(to: sentinel)
        try await reject(library, ["paths": [fixture.kick.path, fixture.snare.path], "changes": ["category": "Unsaved", "bpm": 150]], "unwritable metadata destination")
        let after = try await search(library)
        for path in [fixture.kick, fixture.snare] {
            try require(try canonical(metadata(row(after, path))) == canonical(metadata(row(savedResult, path))), "failed disk write must not publish in-memory overrides")
        }
        try require(try Data(contentsOf: sentinel) == Data("preserve".utf8), "failed atomic write must preserve destination contents")
        try FileManager.default.removeItem(at: fixture.metadataFile)
        try savedBytes.write(to: fixture.metadataFile, options: .atomic)
        let retried = try await edit(library, [fixture.kick], ["bpm": 135])
        try require(try row(retried, fixture.kick)["category"] as? String == "Saved", "retry must start from the last successful commit")
    }

    static func malformedSavedMetadata() async throws {
        for malformed in [Data("{broken json".utf8), Data("{\"version\":9,\"items\":{}}".utf8)] {
            let fixture = try Fixture(); defer { fixture.remove() }
            try malformed.write(to: fixture.metadataFile)
            let library = fixture.library(); try await scan(library)
            let before = try await search(library)
            try await reject(library, ["paths": [fixture.kick.path], "changes": ["category": "Overwrite"]], "malformed saved metadata")
            try require(try Data(contentsOf: fixture.metadataFile) == malformed, "malformed metadata must be preserved byte-for-byte")
            let after = try await search(library)
            try require(try canonical(metadata(row(before, fixture.kick))) == canonical(metadata(row(after, fixture.kick))), "blocked edit must not change memory")
        }
        let fixture = try Fixture(); defer { fixture.remove() }
        let invalid = try canonical(["version": 1, "items": [
            fixture.kick.path: ["category": "Must not load"],
            fixture.snare.path: ["tags": ["Must not load"]],
            fixture.pad.path: ["bpm": 150],
            fixture.root.appendingPathComponent("Invalid.wav").path: ["bpm": -8]
        ]])
        try invalid.write(to: fixture.metadataFile)
        let library = fixture.library(); try await scan(library)
        let rejectedFile = try await search(library)
        try require(rows(rejectedFile).allSatisfy { metadata($0).isEmpty }, "a semantically invalid saved file must not partially apply its valid records")
        try await reject(library, ["paths": [fixture.kick.path], "changes": ["tags": ["Overwrite"]]], "invalid stored override value")
        try require(try Data(contentsOf: fixture.metadataFile) == invalid, "invalid stored override values must be preserved")
    }

    static func main() async {
        setbuf(stdout, nil)
        let tests: [(String, () async throws -> Void)] = [
            ("batch partial overrides, search/facets and untouched source bytes/mtime", batchSearchAndSourcePreservation),
            ("explicit unknown versus null reset", unknownAndReset),
            ("durable overrides, separate inferred cache and changed-source reindex", persistenceAndChangedSource),
            ("duration enrichment preserves edits", durationPreservesEdits),
            ("duration completion across concurrent edits and source replacement", { _ = try LibraryService.metadataCompletionSelfTest() }),
            ("validation, additive tags and all-or-nothing batch rollback", validationAndAddTags),
            ("failed atomic write preserves memory and destination", failedWritePreservesMemory),
            ("malformed saved metadata is preserved and blocks edits", malformedSavedMetadata)
        ]
        var failures: [String] = []
        for (name, test) in tests {
            do { try await test(); print("PASS library metadata: \(name)") }
            catch { failures.append("\(name): \(error)"); print("FAIL library metadata: \(name): \(error)") }
        }
        if !failures.isEmpty { exit(1) }
        print("PASS library metadata: all \(tests.count) groups")
    }
}

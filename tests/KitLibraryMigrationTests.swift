import Foundation

/// Private generated libraries only. No scan command, plug-in execution or audio decoding.
@main struct KitLibraryMigrationTests {
    static var checks = 0
    static let fm = FileManager.default
    static func require(_ condition: Bool, _ message: String) throws {
        checks += 1
        if !condition { throw NSError(domain: "KitLibraryMigrationTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    }
    static func directory(_ url: URL) throws { try fm.createDirectory(at: url, withIntermediateDirectories: true) }
    static func json(_ value: Any) throws -> Data { try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) }
    static func object(_ url: URL) throws -> [String: Any] { try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any] }
    static func same(_ lhs: Any, _ rhs: Any) throws -> Bool { try json(lhs) == json(rhs) }
    static func call(_ service: LibraryService, _ command: String, _ payload: [String: Any] = [:]) async throws -> [String: Any] {
        guard let result = try await service.handle(command, payload: payload) as? [String: Any] else {
            throw NSError(domain: "KitLibraryMigrationTests", code: 2, userInfo: [NSLocalizedDescriptionKey: "Expected dictionary: " + command])
        }
        return result
    }
    static func waitFor(_ message: String, _ predicate: () async throws -> Bool) async throws {
        let deadline = Date().addingTimeInterval(12)
        repeat {
            if try await predicate() { return }
            try await Task.sleep(nanoseconds: 5_000_000)
        } while Date() < deadline
        try require(false, "Timed out: " + message)
    }
    static func settled(_ service: LibraryService) async throws {
        try await waitFor("automatic kit migration completes") {
            let result = try await call(service, "library.status")
            return result["kitIndexUpgradeScanning"] as? Bool == false && result["scanning"] as? Bool == false
        }
    }
    static func markers(_ cache: URL) throws -> Set<String> {
        Set(try object(cache.appendingPathComponent("index-v1.json"))["kitIndexedRoots"] as? [String] ?? [])
    }
    static func seed(_ cache: URL, roots: [URL], items: [[String: Any]], markers: [String]? = nil) throws {
        try directory(cache)
        var value: [String: Any] = ["version": 1, "inferenceVersion": 2,
            "roots": roots.map { ["path": $0.path, "name": $0.lastPathComponent, "source": "configured"] },
            "items": items, "excludedRoots": [], "scanErrors": [], "scanReports": [], "verifiedExpansionRoots": []]
        if let markers { value["kitIndexedRoots"] = markers }
        try json(value).write(to: cache.appendingPathComponent("index-v1.json"))
    }
    static func cachedItem(_ path: URL, root: URL, kind: String, name: String) -> [String: Any] {
        ["path": path.path, "root": root.path, "name": name, "kind": kind,
         "category": kind == "nks" ? "Cached Synth" : "Cached Percussion", "tags": ["Cached tag", "Keep this"],
         "pack": "Cached Pack", "bpm": 137.25, "key": "F# minor", "size": 98765, "modified": 123456.75,
         "metadataSource": "cached authoritative fixture", "previewPath": root.appendingPathComponent("MissingPreview.ogg").path,
         "duration": 2.375, "durationChecked": true]
    }
    static func group(_ path: URL) throws {
        try directory(path.deletingLastPathComponent())
        try Data("opaque generated group; not a vendor archive".utf8).write(to: path)
    }
    static func preserveCachedRows(_ service: LibraryService, originals: [[String: Any]], sample: URL, metadata: [String: Any]) async throws {
        let all = try await call(service, "library.search", ["kind": "all"])
        let rows = all["items"] as? [[String: Any]] ?? []
        try require(all["total"] as? Int == originals.count, "Kit-only upgrade preserves sample/NKS count and ignores uncached sounds")
        for original in originals {
            let path = original["path"] as! String
            guard let row = rows.first(where: { $0["path"] as? String == path }) else { try require(false, "Cached row remains indexed"); continue }
            for key in ["name", "kind", "pack", "size", "bpm", "key", "previewPath", "duration", "metadataSource"] {
                let expected = path == sample.path ? metadata[key] ?? original[key]! : original[key]!
                try require(NSDictionary(dictionary: ["value": row[key] ?? NSNull()]).isEqual(to: ["value": expected]), "Cached or explicitly edited \(key) survives for \(path)")
            }
            try require(row["favorite"] as? Bool == true, "Cached favorites remain selected")
            if path == sample.path {
                try require(try same(row["userMetadata"] ?? [:], metadata), "User metadata override remains exact")
                try require(row["category"] as? String == metadata["category"] as? String && row["tags"] as? [String] == metadata["tags"] as? [String], "Edited category/tags remain applied")
            } else {
                try require(row["category"] as? String == original["category"] as? String && row["tags"] as? [String] == original["tags"] as? [String], "NKS cached category/tags are not re-inferred from malformed payload")
            }
        }
        let samples = try await call(service, "library.search", ["kind": "sample"])
        let nks = try await call(service, "library.search", ["kind": "nks"])
        try require(samples["total"] as? Int == 1 && nks["total"] as? Int == 1, "Sample and NKS kind counts stay exact")
    }
    static func main() async {
        do { try await run() }
        catch {
            FileHandle.standardError.write(Data(("FAIL kit library migration: " + error.localizedDescription + "\n").utf8))
            exit(1)
        }
    }
    static func run() async throws {
        let temp = fm.temporaryDirectory.appendingPathComponent("SuperTracker-kit-migration-" + UUID().uuidString)
        try directory(temp)
        defer { try? fm.removeItem(at: temp) }
        let root = temp.appendingPathComponent("Native Access Content")
        let expansion = root.appendingPathComponent("Orbit Library")
        let offline = temp.appendingPathComponent("Disconnected Content")
        for folder in ["Groups/.previews", "Sounds", "Samples", "User Kits"] { try directory(expansion.appendingPathComponent(folder)) }
        let sample = expansion.appendingPathComponent("Samples/Unhelpful file.wav")
        let nks = expansion.appendingPathComponent("Sounds/Unhelpful preset.nksf")
        let sourceBytes = Data("intentionally malformed; metadata must come from the old cache".utf8)
        for path in [sample, nks, expansion.appendingPathComponent("Samples/Uncached.wav"), expansion.appendingPathComponent("Sounds/Uncached.nksf")] { try sourceBytes.write(to: path) }
        let niKit = expansion.appendingPathComponent("Groups/Orbit Kit.mxgrp")
        try group(niKit)
        let preview = expansion.appendingPathComponent("Groups/.previews/Orbit Kit.mxgrp.ogg")
        try sourceBytes.write(to: preview)
        let userKit = expansion.appendingPathComponent("User Kits/Warm.stkit")
        try directory(userKit.appendingPathComponent("Samples"))
        let header: [String: Any] = ["format": "supertracker-kit", "version": 1, "projectFile": "project.json", "name": "Warm saved kit", "soundCount": 16, "tags": ["Warm"], "type": "Drum kit", "expansion": "Personal"]
        try json(header).write(to: userKit.appendingPathComponent("kit.json"))
        try sourceBytes.write(to: userKit.appendingPathComponent("project.json"))
        try sourceBytes.write(to: userKit.appendingPathComponent("Samples/Do not index.wav"))
        let originals = [cachedItem(sample, root: root, kind: "sample", name: "Cached sample title"), cachedItem(nks, root: root, kind: "nks", name: "Cached preset title")]
        let cache = temp.appendingPathComponent("Cache")
        try seed(cache, roots: [root, offline], items: originals)
        let metadata: [String: Any] = ["category": "User category", "tags": ["My label", "Favorite texture"], "bpm": 94.5, "key": "Bb minor", "sampleType": "loop"]
        let metadataData = try json(["version": 1, "items": [sample.path: metadata]])
        let favoriteData = try json([sample.path, nks.path, offline.appendingPathComponent("Old favorite.wav").path])
        let browserData = try json(["kind": "kit", "query": "remember me", "filters": ["tags": ["My label"]]])
        try metadataData.write(to: cache.appendingPathComponent("metadata-v1.json"))
        try favoriteData.write(to: cache.appendingPathComponent("favorites.json"))
        try browserData.write(to: cache.appendingPathComponent("browser-state.json"))
        let service = LibraryService(cacheDirectory: cache, defaultRoots: [], loadCacheAsynchronously: false)
        try await waitFor("both kit formats become visible without library.scan") {
            let result = try await call(service, "library.search", ["kind": "kit"])
            return result["total"] as? Int == 2 && result["kitIndexUpgradeScanning"] as? Bool == false
        }
        try await settled(service)
        let status = try await call(service, "library.status")
        try require(status["kitIndexUpgradePending"] as? Bool == true, "Disconnected unindexed root stays pending")
        try require(status["kitIndexUpgradeOfflineRoots"] as? [String] == [offline.path], "Public status names the disconnected root awaiting upgrade")
        let kits = try await call(service, "library.search", ["kind": "kit"])
        try require(kits["kitIndexUpgradePending"] as? Bool == true && kits["kitIndexUpgradeScanning"] as? Bool == false, "Search exposes idle but pending upgrade status")
        let kitRows = kits["items"] as? [[String: Any]] ?? []
        try require(Set(kitRows.compactMap { $0["path"] as? String }) == Set([niKit.path, userKit.path]), "Upgrade discovers only kit roots, excluding package samples and hidden previews")
        try require(kitRows.first { $0["path"] as? String == niKit.path }?["previewPath"] as? String == preview.path, "Exact adjacent NI kit preview survives migration")
        try require(kitRows.first { $0["path"] as? String == userKit.path }?["name"] as? String == "Warm saved kit", "Small kit header indexed even with malformed project state")
        try await preserveCachedRows(service, originals: originals, sample: sample, metadata: metadata)
        try await waitFor("completed online marker persists; offline marker does not") { try markers(cache) == [root.path] }
        let persisted = try object(cache.appendingPathComponent("index-v1.json"))
        let storedRows = persisted["items"] as? [[String: Any]] ?? []
        for original in originals {
            let stored = storedRows.first { $0["path"] as? String == original["path"] as? String }
            try require(try same(stored ?? [:], original), "Persisted original cache fields remain exact")
        }
        try require(persisted["version"] as? Int == 1 && persisted["inferenceVersion"] as? Int == 2, "Kit completion marker upgrades the compatible cache without changing inference")
        print("PASS old cache: automatic kit-only upgrade, both formats, original sample/NKS metadata and offline pending marker")

        let laterKit = expansion.appendingPathComponent("Groups/Added After Upgrade.mxgrp")
        try group(laterKit)
        let reopened = LibraryService(cacheDirectory: cache, defaultRoots: [], loadCacheAsynchronously: false)
        try await settled(reopened)
        let reopenedKits = try await call(reopened, "library.search", ["kind": "kit"])
        try require(reopenedKits["total"] as? Int == 2, "Completed root is not scanned again on launch")
        try await preserveCachedRows(reopened, originals: originals, sample: sample, metadata: metadata)
        try require(try markers(cache) == [root.path], "Retrying offline roots cannot mark them complete")

        let recoveredKit = offline.appendingPathComponent("Groups/Returned Drive.mxgrp")
        try group(recoveredKit)
        // Public search observes the returned directory in this same service; no restart or scan command.
        let availableAgain = reopened
        try await waitFor("pending root indexes after becoming available") {
            let result = try await call(availableAgain, "library.search", ["kind": "kit"])
            return result["total"] as? Int == 3 && result["kitIndexUpgradePending"] as? Bool == false && result["kitIndexUpgradeScanning"] as? Bool == false
        }
        try await waitFor("both root completion markers persist") { try markers(cache) == [root.path, offline.path] }
        let completeStatus = try await call(availableAgain, "library.status")
        try require(completeStatus["kitIndexUpgradePending"] as? Bool == false && completeStatus["kitIndexUpgradeScanning"] as? Bool == false, "All completed roots clear public pending/scanning flags")
        try require((completeStatus["kitIndexUpgradeOfflineRoots"] as? [String])?.isEmpty == true, "Returned root clears the offline upgrade list")
        let finalKits = try await call(availableAgain, "library.search", ["kind": "kit"])
        try require(!(finalKits["items"] as? [[String: Any]] ?? []).contains { $0["path"] as? String == laterKit.path }, "Offline retry still skips already-completed online root")
        for (filename, expected) in [("metadata-v1.json", metadataData), ("favorites.json", favoriteData), ("browser-state.json", browserData)] {
            try require(try Data(contentsOf: cache.appendingPathComponent(filename)) == expected, "Migration does not rewrite user file: " + filename)
        }
        for path in [sample, nks, preview, userKit.appendingPathComponent("project.json"), userKit.appendingPathComponent("Samples/Do not index.wav")] {
            try require(try Data(contentsOf: path) == sourceBytes, "Source bytes stay unchanged: " + path.lastPathComponent)
        }
        print("PASS completion markers: completed roots skipped on relaunch; offline root resumes later; user files and source bytes unchanged")

        // Enough cheap irrelevant entries to exercise cancellation before a full root pass.
        // The scan's normal 256-entry scheduling budget keeps this fixture bounded and observable.
        let cancelRoot = temp.appendingPathComponent("Cancellation Library")
        let cancelCache = temp.appendingPathComponent("Cancellation Cache")
        try directory(cancelRoot)
        for index in 0..<2048 { try Data([1]).write(to: cancelRoot.appendingPathComponent("Ignored \(index).txt")) }
        for index in 0..<8 { try group(cancelRoot.appendingPathComponent("Kit \(index).mxgrp")) }
        let cancelSample = cancelRoot.appendingPathComponent("Cached only.wav")
        try sourceBytes.write(to: cancelSample)
        let cancelOriginal = cachedItem(cancelSample, root: cancelRoot, kind: "sample", name: "Retained on cancellation")
        try seed(cancelCache, roots: [cancelRoot], items: [cancelOriginal])
        let cancelled = LibraryService(cacheDirectory: cancelCache, defaultRoots: [], loadCacheAsynchronously: false)
        let started = try await call(cancelled, "library.status")
        try require(started["kitIndexUpgradeScanning"] as? Bool == true, "Bounded fixture starts automatic upgrade before cancellation")
        _ = try await call(cancelled, "library.cancel")
        let cancelledStatus = try await call(cancelled, "library.status")
        try require(cancelledStatus["kitIndexUpgradePending"] as? Bool == true && cancelledStatus["kitIndexUpgradeScanning"] as? Bool == false, "Cancelled upgrade remains pending with idle status")
        try await waitFor("cancelled root remains unmarked in saved cache") {
            let saved = try object(cancelCache.appendingPathComponent("index-v1.json"))
            let completed = try markers(cancelCache)
            return saved["kitIndexedRoots"] is [String] && completed.isEmpty
        }
        let cancelledSamples = try await call(cancelled, "library.search", ["kind": "sample"])
        try require(cancelledSamples["total"] as? Int == 1 && cancelledSamples["kitIndexUpgradeScanning"] as? Bool == false, "Search after cancellation preserves cached sample and does not restart upgrade")
        let resumed = LibraryService(cacheDirectory: cancelCache, defaultRoots: [], loadCacheAsynchronously: false)
        try await waitFor("cancelled root retries on next launch and completes all kits") {
            let result = try await call(resumed, "library.search", ["kind": "kit"])
            return result["total"] as? Int == 8 && result["kitIndexUpgradePending"] as? Bool == false && result["kitIndexUpgradeScanning"] as? Bool == false
        }
        try await waitFor("resumed root marker persists only after completion") { try markers(cancelCache) == [cancelRoot.path] }
        let resumedSamples = try await call(resumed, "library.search", ["kind": "sample"])
        try require(resumedSamples["total"] as? Int == 1, "Resume never performs a destructive full-library scan")
        let resumedCache = try object(cancelCache.appendingPathComponent("index-v1.json"))
        let resumedOriginal = (resumedCache["items"] as? [[String: Any]])?.first { $0["path"] as? String == cancelSample.path }
        try require(try same(resumedOriginal ?? [:], cancelOriginal), "Cancellation and retry preserve every cached sample field")
        print("PASS cancellation: no partial completion marker, no automatic restart during session, complete retry on relaunch")
        for check in try await LibraryService.kitUpgradeRegistrationRaceSelfTest() {
            checks += 1
            print("PASS registration race: " + check)
        }
        print("PASS kit library migration: \(checks) checks; generated private files only")
    }
}

import Foundation

/// Generated fixtures only; no plug-ins, audio host, user profile or installed content.
@main struct KitLibraryIndexTests {
    static func main() async throws {
        let fm = FileManager.default
        let temp = fm.temporaryDirectory.appendingPathComponent("SuperTracker-kit-index-" + UUID().uuidString)
        try fm.createDirectory(at: temp, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: temp) }
        func require(_ value: Bool, _ message: String) throws {
            if !value { throw NSError(domain: "KitLibraryIndexTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
        }
        let nativeRoot = temp.appendingPathComponent("Native Access Content"), expansion = nativeRoot.appendingPathComponent("Orbit Library")
        for folder in ["Groups/.previews", "Samples", "Sounds"] { try fm.createDirectory(at: expansion.appendingPathComponent(folder), withIntermediateDirectories: true) }
        let userRoot = temp.appendingPathComponent("User Kits")
        try fm.createDirectory(at: userRoot, withIntermediateDirectories: true)
        let group = expansion.appendingPathComponent("Groups/Orbit Kit.mxgrp")
        try Data("opaque NI group fixture".utf8).write(to: group)
        let preview = expansion.appendingPathComponent("Groups/.previews/Orbit Kit.mxgrp.ogg")
        try Data("generated preview header".utf8).write(to: preview)
        let sample = temp.appendingPathComponent("Source.wav")
        try Data([1, 2, 3, 4]).write(to: sample)
        let project: [String: Any] = ["kits": [["id": "kit", "sounds": [["id": "sound", "name": "Kick", "instrument": ["type": "sample", "samplePath": sample.path]]]]], "audioPlugins": [["owner": "sound:kit:sound", "state": "opaque-state"]]]
        let kit = userRoot.appendingPathComponent("Warm.stkit")
        _ = try KitLibrary.save(project: project, metadata: ["name": "Warm homemade kit", "type": "Drum kit", "tags": ["Warm", "Analog"], "expansion": "Personal"], to: kit)
        // Indexing must not parse state: the header remains indexable even if state is corrupt.
        try Data("not project JSON".utf8).write(to: kit.appendingPathComponent("project.json"))
        let broken = userRoot.appendingPathComponent("Broken.stkit")
        try fm.createDirectory(at: broken, withIntermediateDirectories: true)
        try Data("bad header".utf8).write(to: broken.appendingPathComponent("kit.json"))
        try Data([1]).write(to: userRoot.appendingPathComponent("._ghost.mxgrp"))
        try fm.createSymbolicLink(at: userRoot.appendingPathComponent("loop"), withDestinationURL: userRoot)
        let cache = temp.appendingPathComponent("Cache")
        let service = LibraryService(cacheDirectory: cache, defaultRoots: [nativeRoot, userRoot], loadCacheAsynchronously: false)
        func search(_ payload: [String: Any] = [:]) async throws -> [String: Any] {
            var payload = payload; payload["kind"] = payload["kind"] ?? "kit"
            return try await service.handle("library.search", payload: payload) as! [String: Any]
        }
        _ = try await service.handle("library.scan", payload: [:])
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            let status = try await service.handle("library.status", payload: [:]) as! [String: Any]
            if status["scanning"] as? Bool == false { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        let result = try await search(), items = result["items"] as! [[String: Any]]
        try require(result["total"] as? Int == 3, "Index both kit formats and malformed kit metadata without indexing package media or hidden previews")
        let homemade = items.first { $0["path"] as? String == kit.path }!
        try require(homemade["name"] as? String == "Warm homemade kit" && homemade["user"] as? Bool == true, "User kit metadata")
        try require(homemade["soundCount"] as? Int == 1 && homemade["type"] as? String == "Drum kit" && homemade["expansion"] as? String == "Personal", "Small header fields without reading corrupt state")
        let ni = items.first { $0["path"] as? String == group.path }!
        try require(ni["kitFormat"] as? String == "maschine-group" && ni["previewPath"] as? String == preview.path, "Native group preview and format")
        try require(items.first { $0["path"] as? String == broken.path }?["metadataError"] is String, "Malformed kit remains visible with readable metadata error")
        try require((try await search(["kind": "all"]))["total"] as? Int == 0, "Legacy All sounds must not offer kits as samples")
        print("PASS kit index: both formats, small headers, exact preview, malformed metadata and package/hidden exclusions")

        try require((try await search(["query": "warm analog"]))["total"] as? Int == 1, "Text finds kit name and tags")
        try require((try await search(["filters": ["extensions": ["stkit"]]]))["total"] as? Int == 2, "User-only format filter")
        try require((try await search(["filters": ["packs": ["Personal"], "categories": ["Drum kit"], "tags": ["Warm", "Absent"]]]))["total"] as? Int == 1, "AND across kit fields and OR within tags")
        let sourceDeadline = Date().addingTimeInterval(3)
        var sourceMatches = 0
        repeat {
            sourceMatches = (try await search(["filters": ["collections": ["Maschine expansions"], "packs": ["Orbit"]]]))["total"] as? Int ?? 0
            if sourceMatches == 1 { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        } while Date() < sourceDeadline
        try require(sourceMatches == 1, "Verified Groups/Sounds/Samples layout supports expansion-source filtering")
        try require((try await search(["filters": ["collections": ["User libraries"], "tags": ["Warm"]]]))["total"] as? Int == 1, "User kit source and tags combine")
        let facets = result["facets"] as! [String: [[String: Any]]]
        try require(facets["packs"]?.contains { $0["value"] as? String == "Orbit" } == true, "Expansion is a pack facet")
        _ = try await service.handle("library.favorite", payload: ["path": group.path, "favorite": true])
        try require((try await search(["favorites": true]))["total"] as? Int == 1, "Kit favorite uses shared durable favorite service")
        let page = try await search(["limit": 1, "offset": 1])
        try require((page["items"] as? [[String: Any]])?.count == 1 && page["total"] as? Int == 3, "Kit paging total")
        print("PASS kit search: text, source/expansion/type/tag facets, user-only, favorites and paging")

        // Index persistence is asynchronous; wait only for the private cache's small publication.
        let index = cache.appendingPathComponent("index-v1.json")
        while !fm.fileExists(atPath: index.path) && Date() < deadline { try await Task.sleep(nanoseconds: 10_000_000) }
        try require(fm.fileExists(atPath: index.path), "Kit index persisted")
        try fm.moveItem(at: nativeRoot, to: temp.appendingPathComponent("Disconnected"))
        let offline = try await search(["query": "Orbit"])
        try require((offline["items"] as? [[String: Any]])?.first?["available"] as? Bool == false, "Disconnected kit remains visible as offline")
        _ = try await service.handle("library.scan", payload: ["roots": [nativeRoot.path]])
        _ = try await service.handle("library.cancel", payload: [:])
        try require((try await search(["query": "Orbit"]))["total"] as? Int == 1, "Cancelled/offline scan preserves cached kit")
        let restored = LibraryService(cacheDirectory: cache, defaultRoots: [], loadCacheAsynchronously: false)
        let cached = try await restored.handle("library.search", payload: ["kind": "kit"]) as! [String: Any]
        try require(cached["total"] as? Int == 3, "Both kit formats survive cache reload")
        print("PASS kit persistence: offline roots, cancellation and cache restoration")
        try await browse(temp.appendingPathComponent("Browse"))
    }

    static func header(name: String, chain: [String], types: [String]) -> Data {
        func u32(_ value: UInt32) -> [UInt8] { (0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) } }
        func text(_ value: String) -> [UInt8] { let units = Array(value.utf16); return u32(UInt32(units.count)) + units.flatMap { [UInt8($0 & 0xff), UInt8($0 >> 8)] } }
        var bytes = [UInt8](repeating: 0, count: 64) + Array("DSIN".utf8) + [0x6c, 0, 0, 0, 1, 0, 0, 0, 0x18, 0, 0, 0] + [UInt8](repeating: 0, count: 20)
        bytes += u32(1) + u32(2) + u32(1) + u32(0) + text(name) + text("Native Instruments") + text("Native Instruments") + text("")
        bytes += u32(0) + u32(.max) + u32(.max) + [UInt8](repeating: 0, count: 20) + u32(1)
        bytes += u32(UInt32(chain.count)) + chain.flatMap(text) + u32(UInt32(types.count)) + types.flatMap(text) + [UInt8](repeating: 0, count: 64)
        return Data(bytes)
    }

    /// library.kits.browse: Library / My kits, expansion › bank drill-down, Maschine 3 sub-types, lazily read and cached headers.
    static func browse(_ temp: URL) async throws {
        let fm = FileManager.default
        func require(_ value: Bool, _ message: String) throws {
            if !value { throw NSError(domain: "KitLibraryIndexTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
        }
        let native = temp.appendingPathComponent("Native Access Content"), user = temp.appendingPathComponent("User Kits")
        let orbit = native.appendingPathComponent("Orbit Library/Groups/Kits"), factory = native.appendingPathComponent("Maschine 2 Factory Library Library/Groups/Kits")
        for folder in [orbit, factory, user.appendingPathComponent("Breaks")] { try fm.createDirectory(at: folder, withIntermediateDirectories: true) }
        try header(name: "Acid Kit", chain: ["Orbit"], types: ["\\:Kits", "\\:Kits\\:Urban Kit"]).write(to: orbit.appendingPathComponent("Acid Kit.mxgrp"))
        try header(name: "Box Kit", chain: ["Orbit"], types: ["\\:Kits\\:Analog Kit"]).write(to: orbit.appendingPathComponent("Box Kit.mxgrp"))
        try Data("opaque NI group".utf8).write(to: orbit.appendingPathComponent("Opaque Kit.mxgrp"))
        try header(name: "Abbey Kit", chain: ["Maschine", "Maschine 1.8 Library"], types: ["\\:Kits\\:Acoustic Kit"]).write(to: factory.appendingPathComponent("Abbey Kit.mxgrp"))
        let sample = temp.appendingPathComponent("Kick.wav"); try Data([1, 2, 3, 4]).write(to: sample)
        let project: [String: Any] = ["kits": [["id": "kit", "sounds": [["id": "sound", "name": "Kick", "instrument": ["type": "sample", "samplePath": sample.path]]]]]]
        _ = try KitLibrary.save(project: project, metadata: ["name": "Warm", "tags": ["Warm"]], to: user.appendingPathComponent("Breaks/Warm.stkit"), copySamples: false)
        _ = try KitLibrary.save(project: project, metadata: ["name": "Loose"], to: user.appendingPathComponent("Loose.stkit"), copySamples: false)
        let cache = temp.appendingPathComponent("Cache")
        let service = LibraryService(cacheDirectory: cache, defaultRoots: [native, user], loadCacheAsynchronously: false)
        _ = try await service.handle("library.scan", payload: [:])
        func browse(_ payload: [String: Any] = [:], on library: LibraryService? = nil) async throws -> [String: Any] { try await (library ?? service).handle("library.kits.browse", payload: payload) as! [String: Any] }
        func names(_ result: [String: Any], _ key: String = "items") -> [String] { (result[key] as? [[String: Any]] ?? []).compactMap { $0["name"] as? String } }
        var all: [String: Any] = [:]
        let deadline = Date().addingTimeInterval(10)
        repeat {
            all = try await browse()
            let status = all["status"] as? [String: Any] ?? [:]
            if all["scanning"] as? Bool == false, status["reading"] as? Bool == false, (all["counts"] as? [String: Int])?["library"] == 4, names(all, "products") == ["Maschine", "Orbit"] { break }
            try await Task.sleep(nanoseconds: 20_000_000)
        } while Date() < deadline
        try require((all["counts"] as? [String: Int]) == ["library": 4, "user": 2], "Library groups and saved kits are counted separately")
        try require(names(all, "products") == ["Maschine", "Orbit"], "Header bank chains and the Library folder name the expansions")
        try require(names(all) == ["Abbey Kit", "Acid Kit", "Box Kit", "Opaque Kit"], "All expansions lists groups by expansion, then name")
        let orbitResult = try await browse(["product": "Orbit"])
        try require(orbitResult["total"] as? Int == 3, "Choosing an expansion narrows the groups")
        let subtypes = orbitResult["subtypes"] as? [[String: Any]] ?? []
        try require(subtypes.contains { $0["name"] as? String == "Kits\u{1f}Hip Hop" && $0["label"] as? String == "Hip Hop" && $0["count"] as? Int == 1 }, "Urban Kit appears under Maschine 3's Hip Hop sub-type")
        let hipHop = try await browse(["product": "Orbit", "subtypes": ["Kits\u{1f}Hip Hop"]])
        try require(names(hipHop) == ["Acid Kit"], "Sub-type filter")
        let maschine = try await browse(["product": "Maschine"])
        let factoryBank = try await browse(["product": "Maschine", "bank": "Maschine 1.8 Library"])
        try require(names(maschine, "banks") == ["Maschine 1.8 Library"] && factoryBank["total"] as? Int == 1, "Factory banks drill down")
        let acoustic = try await browse(["query": "acoustic"])
        try require(names(acoustic) == ["Abbey Kit"], "Search covers types")
        let mine = try await browse(["user": true])
        try require(names(mine, "products") == ["Breaks", "Saved kits"] && names(mine, "characters") == ["Warm"], "Kit folders and kit tags for My kits")
        try require((mine["items"] as? [[String: Any]])?.allSatisfy { $0["art"] == nil } == true, "User kits never borrow NI artwork")
        _ = try await service.handle("library.favorite", payload: ["path": orbit.appendingPathComponent("Box Kit.mxgrp").path, "favorite": true])
        let favorites = try await browse(["favorites": true])
        try require(names(favorites) == ["Box Kit"], "Favorites")
        let paged = try await browse(["offset": 1, "limit": 2])
        try require(names(paged) == ["Acid Kit", "Box Kit"] && paged["total"] as? Int == 4, "Paging keeps the total")
        // Headers are remembered by size and date: a new session browses expansions without re-reading them.
        let restored = LibraryService(cacheDirectory: cache, defaultRoots: [native, user], loadCacheAsynchronously: false)
        let again = try await browse(on: restored)
        try require(names(again, "products") == ["Maschine", "Orbit"] && (again["status"] as? [String: Any])?["reading"] as? Bool == false, "Header cache restores expansions without reading groups again")
        print("PASS kit browser catalogue: expansions from group headers, bank drill-down, Maschine 3 sub-types, My kits folders, favorites, paging and header cache")
    }
}

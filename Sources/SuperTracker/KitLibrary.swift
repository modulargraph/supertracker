import Foundation
import CryptoKit
import Darwin
import CoreFoundation

/// File-only kit storage. This never loads a plug-in, mutates a project or plays audio.
/// A .stkit directory separates its small indexable header from opaque project state.
enum KitLibrary {
    static let extensions: Set<String> = ["stkit", "mxgrp"]
    static let maximumProjectBytes = 64 * 1024 * 1024
    static let maximumMetadataBytes = 256 * 1024
    static let maximumMediaBytes: Int64 = 4 * 1024 * 1024 * 1024
    private static let previewExtensions = ["ogg", "wav", "aif", "aiff", "mp3", "m4a", "caf"]
    static var userDirectory: URL {
        if let override = ProcessInfo.processInfo.environment["SUPERTRACKER_SUPPORT_DIR"], override.hasPrefix("/") {
            return URL(fileURLWithPath: override, isDirectory: true).appendingPathComponent("Kits", isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Documents/SuperTracker/Kits", isDirectory: true)
    }

    static func metadata(at url: URL, root: URL? = nil) throws -> [String: Any] {
        let url = url.standardizedFileURL, ext = url.pathExtension.lowercased()
        guard extensions.contains(ext) else { throw failure("Choose a .stkit kit or Maschine .mxgrp group.") }
        let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey])
        guard values.isSymbolicLink != true else { throw failure("Kit aliases are not followed while indexing.") }
        var result: [String: Any] = ["path": url.path, "name": url.deletingPathExtension().lastPathComponent,
            "kind": "kit", "kitFormat": ext == "stkit" ? "supertracker-kit" : "maschine-group",
            "user": ext == "stkit", "type": "Kit", "tags": [], "available": true]
        if let root {
            let prefix = root.standardizedFileURL.path + "/"
            if url.path.hasPrefix(prefix) {
                let folders = String(url.path.dropFirst(prefix.count)).split(separator: "/").dropLast().map(String.init)
                result["tags"] = folders.filter { !["groups", "kits"].contains($0.lowercased()) }
                result["expansion"] = folders.first ?? root.lastPathComponent
            }
        }
        // A root inside an expansion (".../Anima Ascent Library/Groups") must not name the expansion "Kits" or
        // "Groups": the expansion is the folder that holds Groups, or the nearest "… Library" folder.
        if ext == "mxgrp" {
            let parents = Array(url.deletingLastPathComponent().pathComponents.dropFirst())
            if let library = parents.last(where: { $0.hasSuffix(" Library") }) { result["expansion"] = library }
            else if let index = parents.lastIndex(where: { $0.lowercased() == "groups" }), index > 0 { result["expansion"] = parents[index - 1] }
        }
        if ext == "stkit" {
            guard values.isDirectory == true else { throw failure("A SuperTracker kit must be a .stkit package directory.") }
            let header = try readObject(inside(url, "kit.json"), limit: maximumMetadataBytes)
            try validateHeader(header)
            for key in ["id", "name", "type", "tags", "author", "description", "expansion", "soundCount", "createdAt", "source"] {
                if let value = header[key] { result[key] = value }
            }
            result["metadataSource"] = "SuperTracker kit metadata"
            if let relative = header["previewFile"] as? String {
                let preview = try inside(url, relative)
                if readableAudio(preview) { result["previewPath"] = preview.path }
            }
        } else {
            guard values.isRegularFile == true else { throw failure("A Maschine group must be a regular .mxgrp file.") }
            result["metadataSource"] = "filename and folders"
            if let header = groupHeader(at: url) {
                result["metadataSource"] = "Maschine group header"
                if !header.author.isEmpty { result["author"] = header.author }
                if !header.vendor.isEmpty { result["vendor"] = header.vendor }
                for (index, key) in ["product", "bank", "subBank"].enumerated() where index < header.chain.count { result[key] = header.chain[index] }
                if let product = header.chain.first { result["expansion"] = product }
                result["types"] = Array(Set(header.types.map { $0[0] })).sorted()
                result["subtypes"] = header.types.filter { $0.count > 1 }.map { $0.prefix(2).joined(separator: "\u{1f}") }
            }
            result["compatibility"] = ["status": "uninspected", "summary": "Inspect the group before loading; installed plug-ins and unsupported settings are checked separately."]
        }
        if result["previewPath"] == nil, let preview = previewURL(for: url) { result["previewPath"] = preview.path }
        return result
    }

    /// What Maschine's own browser shows for a group, read from the .mxgrp header (NI SoundInfo, "DSIN" item 0x6c):
    /// name, author, vendor, bank chain (Product › Bank › Sub-bank) and type paths ("\:Kits\:Urban Kit").
    struct GroupHeader: Codable, Equatable {
        var name = "", author = "", vendor = "", comment = ""
        var chain: [String] = []
        var types: [[String]] = []
    }
    /// Maschine 3 shows some legacy sub-types under their current names.
    static let groupSubtypeAliases = ["Urban Kit": "Hip Hop"]
    private static let soundInfoMarker: [UInt8] = Array("DSIN".utf8) + [0x6c, 0, 0, 0, 1, 0, 0, 0, 0x18, 0, 0, 0]

    /// Bounded: reads at most 64 KiB and accepts only well-formed counts and UTF-16 strings. Never reads the archive.
    static func groupHeader(at url: URL) -> GroupHeader? {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? handle.close() }
        guard let data = try? handle.read(upToCount: 65_536) else { return nil }
        return groupHeader(data)
    }
    static func groupHeader(_ data: Data) -> GroupHeader? {
        let b = [UInt8](data.prefix(65_536)), marker = soundInfoMarker
        guard b.count > marker.count, let start = (0...(b.count - marker.count)).first(where: { b[$0] == 0x44 && Array(b[$0..<$0 + marker.count]) == marker }) else { return nil }
        // Item header (20 bytes) and its DSIN terminator (16 bytes) precede the SoundInfo fields.
        var p = start + 36
        func u32() -> Int? {
            guard p + 4 <= b.count else { return nil }
            defer { p += 4 }
            return Int(b[p]) | Int(b[p + 1]) << 8 | Int(b[p + 2]) << 16 | Int(b[p + 3]) << 24
        }
        func text() -> String? {
            guard let count = u32(), count <= 1024, p + count * 2 <= b.count else { return nil }
            let units = stride(from: p, to: p + count * 2, by: 2).map { UInt16(b[$0]) | UInt16(b[$0 + 1]) << 8 }
            p += count * 2
            return String(decoding: units, as: UTF16.self)
        }
        func list() -> [String]? {
            guard let count = u32(), count <= 64 else { return nil }
            var values: [String] = []
            for _ in 0..<count { guard let value = text() else { return nil }; values.append(value) }
            return values
        }
        var header = GroupHeader()
        guard u32() == 1, u32() != nil, u32() != nil, u32() != nil,
              let name = text(), let author = text(), let vendor = text(), let comment = text() else { return nil }
        header.name = name; header.author = author; header.vendor = vendor; header.comment = comment
        for _ in 0..<8 { guard u32() != nil else { return nil } }
        guard u32() != nil, let chain = list(), let types = list() else { return nil }
        let clean = { (value: String) in value.trimmingCharacters(in: .whitespacesAndNewlines) }
        header.chain = chain.map(clean).filter { !$0.isEmpty }
        header.types = types.compactMap { path in
            let parts = path.components(separatedBy: "\\:").map(clean).filter { !$0.isEmpty }
            return parts.isEmpty ? nil : parts.enumerated().map { $0.offset == 1 ? groupSubtypeAliases[$0.element] ?? $0.element : $0.element }
        }
        // "\:Kits" and "\:Kits\:Hip Hop" both arrive; keep only the most specific paths.
        header.types = header.types.filter { path in !header.types.contains { $0.count > path.count && Array($0.prefix(path.count)) == path } }
        var seen: Set<[String]> = []
        header.types = header.types.filter { seen.insert($0).inserted }
        return header
    }

    /// Exact candidate names only. Never enumerates large preview folders or loads project data.
    static func previewURL(for url: URL) -> URL? {
        let parent = url.deletingLastPathComponent(), stem = url.deletingPathExtension().lastPathComponent
        var candidates: [URL] = []
        if url.pathExtension.lowercased() == "stkit" {
            for ext in previewExtensions { candidates.append(url.appendingPathComponent("Preview." + ext)) }
        }
        for folder in [parent.appendingPathComponent(".previews", isDirectory: true), parent.appendingPathComponent("Previews", isDirectory: true), parent] {
            for ext in previewExtensions {
                candidates.append(folder.appendingPathComponent(url.lastPathComponent + "." + ext))
                candidates.append(folder.appendingPathComponent(stem + "." + ext))
            }
        }
        return candidates.first(where: readableAudio)
    }

    static func read(at url: URL) throws -> [String: Any] {
        guard url.pathExtension.lowercased() == "stkit" else { throw failure("Maschine groups require the Maschine group importer.") }
        let details = try metadata(at: url)
        var project = try readObject(inside(url, "project.json"), limit: maximumProjectBytes)
        try validateProjectShape(project)
        var warnings: [String] = [], checked: Set<String> = []
        project = try mapMedia(project) { path in
            guard !path.isEmpty else { return path }
            let source = path.hasPrefix("/") ? URL(fileURLWithPath: path).standardizedFileURL : try inside(url, path)
            if checked.insert(source.path).inserted && !FileManager.default.isReadableFile(atPath: source.path), warnings.count < 128 {
                warnings.append("Missing or unreadable sample: " + source.path)
            }
            return source.path
        }
        return ["path": url.standardizedFileURL.path, "metadata": details, "project": project,
                "sounds": soundSummaries(project), "warnings": warnings]
    }

    /// The caller captures and validates the kit's project subset before invoking this method.
    /// Publication is atomic; cancellation or copy failure leaves the destination untouched.
    static func save(project: [String: Any], metadata: [String: Any], to destination: URL,
                     copySamples: Bool = true, relativeTo projectURL: URL? = nil, overwrite: Bool = false,
                     cancelled: () -> Bool = { false }) throws -> [String: Any] {
        try validateProjectShape(project)
        let destination = destination.standardizedFileURL
        guard destination.pathExtension.lowercased() == "stkit" else { throw failure("Use the .stkit extension for a SuperTracker kit.") }
        let fm = FileManager.default, parent = destination.deletingLastPathComponent()
        guard try parent.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else { throw failure("Choose an existing kit destination folder.") }
        let existing = fm.fileExists(atPath: destination.path)
        if existing {
            guard overwrite else { throw failure("A kit already exists at this destination. Choose another name or explicitly replace it.") }
            let values = try destination.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            guard values.isDirectory == true, values.isSymbolicLink != true else { throw failure("Only an existing regular .stkit package can be replaced.") }
            _ = try Self.metadata(at: destination) // Never replace an unrelated directory with a .stkit suffix.
        }
        func checkCancellation() throws { if cancelled() { throw failure("Kit save cancelled.", code: NSUserCancelledError) } }
        try checkCancellation()
        let stage = parent.appendingPathComponent(".supertracker-kit-" + UUID().uuidString, isDirectory: true)
        try fm.createDirectory(at: stage, withIntermediateDirectories: false)
        defer { try? fm.removeItem(at: stage) }
        var header = metadata
        header["format"] = "supertracker-kit"; header["version"] = 1; header["projectFile"] = "project.json"
        if (header["id"] as? String)?.isEmpty != false { header["id"] = UUID().uuidString }
        if (header["name"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty != false { header["name"] = destination.deletingPathExtension().lastPathComponent }
        if header["type"] == nil { header["type"] = "Kit" }
        if header["tags"] == nil { header["tags"] = [String]() }
        header["soundCount"] = loadedSoundCount(project)
        if header["createdAt"] == nil { header["createdAt"] = ISO8601DateFormatter().string(from: Date()) }
        header["collectedSamples"] = copySamples
        let suppliedPreview = header.removeValue(forKey: "previewPath") as? String
        header.removeValue(forKey: "previewFile")
        try validateHeader(header)
        var remaining = maximumMediaBytes, copied: [String: String] = [:]
        let stored = try mapMedia(project) { path in
            guard !path.isEmpty else { return path }
            try checkCancellation()
            let source: URL
            if path.hasPrefix("/") { source = URL(fileURLWithPath: path).standardizedFileURL }
            else if let projectURL { source = projectURL.deletingLastPathComponent().appendingPathComponent(path).standardizedFileURL }
            else { throw failure("A relative sample reference needs its original project location: " + path) }
            guard copySamples else {
                // Exchanging this package would delete the old members referenced by its new
                // snapshot. Reference-only replacement cannot safely keep those paths alive.
                if source.resolvingSymlinksInPath().path.hasPrefix(destination.resolvingSymlinksInPath().path + "/") {
                    throw failure("This sample belongs to the kit being replaced. Collect samples or save to a different kit: " + source.path)
                }
                return source.path
            }
            if let prior = copied[source.path] { return prior }
            let folder = stage.appendingPathComponent("Samples", isDirectory: true)
            try fm.createDirectory(at: folder, withIntermediateDirectories: true)
            let target = try copyFile(source, into: folder, remaining: &remaining, cancelled: cancelled)
            let relative = "Samples/" + target.lastPathComponent
            copied[source.path] = relative
            return relative
        }
        // Keep the caller's project and opaque state immutable. JSON conversion below is bounded.
        try validateProjectShape(stored)
        if let suppliedPreview, !suppliedPreview.isEmpty {
            let source = URL(fileURLWithPath: suppliedPreview)
            guard readableAudio(source) else { throw failure("The requested kit preview is unavailable or has an unsupported audio extension.") }
            let target = try copyFile(source, into: stage, remaining: &remaining, cancelled: cancelled)
            header["previewFile"] = target.lastPathComponent
        }
        let payload = try JSONSerialization.data(withJSONObject: stored, options: [.sortedKeys])
        guard payload.count <= maximumProjectBytes else { throw failure("Kit project state exceeds the 64 MiB limit.") }
        let headerData = try JSONSerialization.data(withJSONObject: header, options: [.prettyPrinted, .sortedKeys])
        guard headerData.count <= maximumMetadataBytes else { throw failure("Kit metadata exceeds the 256 KiB limit.") }
        try payload.write(to: stage.appendingPathComponent("project.json"), options: .atomic)
        try headerData.write(to: stage.appendingPathComponent("kit.json"), options: .atomic)
        try checkCancellation()
        let flags = existing ? UInt32(RENAME_SWAP) : UInt32(RENAME_EXCL)
        guard renamex_np(stage.path, destination.path, flags) == 0 else {
            throw failure("Could not publish the kit; the prior destination was preserved: " + String(cString: strerror(errno)))
        }
        // For replacement, stage now contains the old package and the defer removes only that package.
        var published = header
        published["path"] = destination.path; published["kind"] = "kit"; published["kitFormat"] = "supertracker-kit"; published["user"] = true
        if let preview = header["previewFile"] as? String { published["previewPath"] = destination.appendingPathComponent(preview).path }
        return ["saved": true, "path": destination.path, "metadata": published,
                "copiedSamples": copied.count, "mediaBytes": maximumMediaBytes - remaining]
    }

    private static func validateHeader(_ header: [String: Any]) throws {
        guard header["format"] as? String == "supertracker-kit", let version = header["version"] as? NSNumber,
              CFGetTypeID(version) != CFBooleanGetTypeID(), version.doubleValue == 1,
              header["projectFile"] as? String == "project.json" else { throw failure("Unsupported or malformed SuperTracker kit format.") }
        for key in ["name", "type", "id", "author", "expansion", "source"] {
            if let value = header[key] { guard let text = value as? String, text.count <= 256 else { throw failure("Invalid kit metadata field: " + key) } }
        }
        if let tags = header["tags"] {
            guard let values = tags as? [String], values.count <= 64, values.allSatisfy({ $0.count <= 80 }) else { throw failure("Kit tags exceed the supported limit.") }
        }
        guard JSONSerialization.isValidJSONObject(header) else { throw failure("Kit metadata must contain JSON values.") }
    }

    private static func validateProjectShape(_ project: [String: Any]) throws {
        guard JSONSerialization.isValidJSONObject(project) else { throw failure("Kit project state must contain JSON values.") }
        if let kits = project["kits"] {
            guard let values = kits as? [[String: Any]], values.count <= 4 else { throw failure("A kit document supports up to four kits.") }
            for kit in values {
                guard let sounds = kit["sounds"] as? [[String: Any]], sounds.count <= 16 else { throw failure("A kit supports up to 16 sounds.") }
                for sound in sounds { guard sound["instrument"] is [String: Any] else { throw failure("A kit sound needs its instrument settings.") } }
            }
        }
        if let instruments = project["instruments"] { guard let values = instruments as? [[String: Any]], values.count <= 1024 else { throw failure("Too many instruments in this kit document.") } } // ProjectLayout.instrumentSanityLimit
        if let plugins = project["audioPlugins"] { guard let values = plugins as? [[String: Any]], values.count <= 1024 else { throw failure("Too many plug-in states in this kit document.") } }
    }

    /// Sounds that make a sound: an empty Sampler slot (no sample and no zones) is not counted,
    /// so the browser shows how many of a kit's sixteen pads are actually loaded.
    static func loadedSoundCount(_ project: [String: Any]) -> Int {
        let kits = project["kits"] as? [[String: Any]] ?? []
        guard !kits.isEmpty else { return soundSummaries(project).count }
        return kits.flatMap { $0["sounds"] as? [[String: Any]] ?? [] }.filter { sound in
            let instrument = sound["instrument"] as? [String: Any] ?? [:]
            guard (instrument["type"] as? String ?? "sample") == "sample" else { return true }
            return !(instrument["samplePath"] as? String ?? "").isEmpty || !(instrument["zones"] as? [Any] ?? []).isEmpty
        }.count
    }

    private static func soundSummaries(_ project: [String: Any]) -> [[String: Any]] {
        let sounds = (project["kits"] as? [[String: Any]] ?? []).flatMap { $0["sounds"] as? [[String: Any]] ?? [] }
        if !sounds.isEmpty {
            return sounds.enumerated().map { slot, sound in
                let instrument = sound["instrument"] as? [String: Any] ?? [:]
                return ["slot": slot, "id": sound["id"] ?? "", "name": sound["name"] ?? instrument["name"] ?? "Sound \(slot + 1)", "type": instrument["type"] ?? "unknown"]
            }
        }
        return (project["instruments"] as? [[String: Any]] ?? []).enumerated().map { slot, instrument in
            ["slot": slot, "name": instrument["name"] ?? "Sound \(slot + 1)", "type": instrument["type"] ?? "unknown"]
        }
    }

    /// Visit explicit media fields only; strings inside plug-in state are never interpreted as paths.
    private static func mapMedia(_ project: [String: Any], transform: (String) throws -> String) throws -> [String: Any] {
        func fields(_ source: [String: Any], keys: [String]) throws -> [String: Any] {
            var value = source
            // "builtin:" sample references (BuiltinSamples) are app content, never kit media.
            for key in keys { if let path = source[key] as? String, !path.hasPrefix("builtin:") { value[key] = try transform(path) } }
            return value
        }
        func instrument(_ source: [String: Any]) throws -> [String: Any] {
            var value = try fields(source, keys: ["samplePath"])
            if let zones = source["zones"] as? [[String: Any]] {
                guard zones.count <= 128 else { throw failure("A sound supports up to 128 sample zones.") }
                value["zones"] = try zones.map { try fields($0, keys: ["path", "samplePath"]) }
            }
            return value
        }
        var value = project
        if let instruments = project["instruments"] as? [[String: Any]] { value["instruments"] = try instruments.map(instrument) }
        if let kits = project["kits"] as? [[String: Any]] {
            value["kits"] = try kits.map { kit -> [String: Any] in
                var result = kit
                result["sounds"] = try (kit["sounds"] as? [[String: Any]] ?? []).map { sound -> [String: Any] in
                    var result = sound
                    if let source = sound["instrument"] as? [String: Any] { result["instrument"] = try instrument(source) }
                    return result
                }
                return result
            }
        }
        if let clips = project["audioClips"] as? [[String: Any]] {
            guard clips.count <= 256 else { throw failure("Too many audio clips in the kit document.") }
            value["audioClips"] = try clips.map { clip -> [String: Any] in
                var result = try fields(clip, keys: ["path", "samplePath"])
                if let takes = clip["takes"] as? [[String: Any]] { result["takes"] = try takes.map { try fields($0, keys: ["path", "samplePath"]) } }
                return result
            }
        }
        return value
    }

    private static func readObject(_ url: URL, limit: Int) throws -> [String: Any] {
        let values = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
        guard values.isRegularFile == true, let size = values.fileSize, size > 0, size <= limit else { throw failure("Kit data is missing or exceeds its supported size: " + url.lastPathComponent) }
        let handle = try FileHandle(forReadingFrom: url); defer { try? handle.close() }
        guard let data = try handle.read(upToCount: limit + 1), data.count <= limit,
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw failure("Malformed kit JSON: " + url.lastPathComponent) }
        return object
    }

    private static func inside(_ folder: URL, _ relative: String) throws -> URL {
        guard !relative.isEmpty, !relative.hasPrefix("/"), !relative.contains("\0") else { throw failure("Invalid package-relative kit path.") }
        let folder = folder.standardizedFileURL.resolvingSymlinksInPath()
        let target = folder.appendingPathComponent(relative).standardizedFileURL.resolvingSymlinksInPath()
        guard target.path.hasPrefix(folder.path + "/") else { throw failure("A kit path escapes its package: " + relative) }
        return target
    }

    private static func readableAudio(_ url: URL) -> Bool {
        guard previewExtensions.contains(url.pathExtension.lowercased()), let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]),
              values.isRegularFile == true, (values.fileSize ?? 0) > 0 else { return false }
        return FileManager.default.isReadableFile(atPath: url.path)
    }

    private static func copyFile(_ source: URL, into folder: URL, remaining: inout Int64, cancelled: () -> Bool) throws -> URL {
        let before = try source.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey, .contentModificationDateKey])
        guard before.isRegularFile == true, let size = before.fileSize, size > 0, Int64(size) <= remaining else { throw failure("Sample is missing, empty or exceeds the kit's 4 GiB media budget: " + source.path) }
        let temporary = folder.appendingPathComponent(".copy-" + UUID().uuidString)
        guard FileManager.default.createFile(atPath: temporary.path, contents: nil) else { throw failure("Could not create collected kit media.") }
        defer { try? FileManager.default.removeItem(at: temporary) }
        let input = try FileHandle(forReadingFrom: source), output = try FileHandle(forWritingTo: temporary)
        defer { try? input.close(); try? output.close() }
        var hash = SHA256(), count: Int64 = 0
        while let bytes = try input.read(upToCount: 1024 * 1024), !bytes.isEmpty {
            if cancelled() { throw failure("Kit save cancelled.", code: NSUserCancelledError) }
            count += Int64(bytes.count)
            guard count <= remaining else { throw failure("Kit media exceeds the 4 GiB collection budget.") }
            hash.update(data: bytes); try output.write(contentsOf: bytes)
        }
        try output.synchronize()
        let after = try source.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
        guard count == Int64(size), after.fileSize == size, after.contentModificationDate == before.contentModificationDate else { throw failure("A source sample changed while saving the kit: " + source.path) }
        let digest = hash.finalize().map { String(format: "%02x", $0) }.joined()
        // A sample already collected under this content hash keeps its name, so saving a kit over itself leaves
        // the paths an open session still uses valid (they used to gain a second prefix and dangle).
        let hashPrefix = String(digest.prefix(24)) + "-"
        var stem = source.deletingPathExtension().lastPathComponent
        while stem.hasPrefix(hashPrefix) { stem = String(stem.dropFirst(hashPrefix.count)) }
        let name = String(stem.prefix(100)) + "." + source.pathExtension.lowercased()
        let target = folder.appendingPathComponent(hashPrefix + name)
        if !FileManager.default.fileExists(atPath: target.path) { try FileManager.default.moveItem(at: temporary, to: target); remaining -= count }
        return target
    }

    private static func failure(_ message: String, code: Int = 1) -> NSError {
        NSError(domain: "SuperTracker.KitLibrary", code: code, userInfo: [NSLocalizedDescriptionKey: message])
    }
}

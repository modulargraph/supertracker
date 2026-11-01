import Foundation
import CryptoKit

enum TrackerError: LocalizedError {
    case message(String)
    var errorDescription: String? { switch self { case .message(let text): return text } }
}

final class ProjectStore {
    private(set) var project: [String: Any]?
    private(set) var url: URL?
    private(set) var dirty = false
    private(set) var lastRecoveryWarning: String?
    /// The last `read` brought an older document to the current model (ProjectMigration);
    /// accepting it leaves the session dirty, so the next save writes the migrated form.
    private(set) var lastReadWasMigrated = false
    /// What the last migrating `read` changed (kit lanes converted to groups, …).
    private(set) var lastMigrationNotes: [String] = []
    /// SHA-256 of `url` as this session last read or wrote it; recovery reattaches the file only while it still matches.
    private(set) var fileFingerprint: String?
    let support: URL
    var recoveryURL: URL { support.appendingPathComponent("Recovery.supertracker") }
    /// Which file the recovered session belongs to and whether it had unsaved changes (written with every autosave).
    var recoverySessionURL: URL { support.appendingPathComponent("Recovery-session.json") }

    init(directory: URL? = nil) throws {
        // Development builds can run against a private session store without touching the user's recovery file.
        let override = ProcessInfo.processInfo.environment["SUPERTRACKER_SUPPORT_DIR"].flatMap { $0.hasPrefix("/") ? URL(fileURLWithPath: $0, isDirectory: true) : nil }
        support = directory ?? override ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("SuperTracker", isDirectory: true)
        try FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
    }

    func validate(_ candidate: [String: Any]) throws {
        guard (candidate["version"] as? Int ?? 1) == 1 else { throw TrackerError.message("This project uses an unsupported format version.") }
        // The track bound is a sanity check against corrupt files, not a design size.
        guard let tracks = candidate["tracks"] as? [[String: Any]], (1...ProjectLayout.trackSanityLimit).contains(tracks.count),
              let instruments = candidate["instruments"] as? [[String: Any]], instruments.count <= ProjectLayout.instrumentSanityLimit,
              let patterns = candidate["patterns"] as? [[String: Any]], !patterns.isEmpty, patterns.count <= 256 else {
            throw TrackerError.message("The project must contain 1–\(ProjectLayout.trackSanityLimit) tracks, up to \(ProjectLayout.instrumentSanityLimit) instruments, and 1–256 patterns.")
        }
        guard let bpm = candidate["bpm"] as? Double, bpm.isFinite, (20...400).contains(bpm) else { throw TrackerError.message("Project tempo must be between 20 and 400 BPM.") }
        var patternIDs = Set<Int>()
        for (index, pattern) in patterns.enumerated() {
            guard let length = pattern["length"] as? Int, (1...256).contains(length), let steps = pattern["steps"] as? [Any], steps.count <= ProjectLayout.trackSanityLimit else { throw TrackerError.message("A pattern has invalid dimensions (maximum 256 rows and \(ProjectLayout.trackSanityLimit) tracks).") }
            let id = pattern["id"] as? Int ?? index
            guard (0..<256).contains(id), patternIDs.insert(id).inserted else { throw TrackerError.message("Pattern identifiers must be unique numbers from 0 to 255.") }
        }
        if let sequence = candidate["sequence"] as? [Int] {
            guard !sequence.isEmpty, sequence.count <= 1024, sequence.allSatisfy({ patternIDs.contains($0) }) else { throw TrackerError.message("The song sequence must reference existing patterns (up to 1024 entries).") }
        }
        for instrument in instruments {
            guard (instrument["zones"] as? [Any] ?? []).count <= 128 else { throw TrackerError.message("An instrument can contain up to 128 sample zones.") }
        }
        let clips = candidate["audioClips"] as? [[String: Any]] ?? []
        guard clips.count <= 256 else { throw TrackerError.message("A song can contain up to 256 audio clips.") }
        for clip in clips {
            let track = clip["track"] as? Int ?? 0
            let start = (clip["startBeat"] as? NSNumber)?.doubleValue ?? 0
            let duration = (clip["durationBeats"] as? NSNumber)?.doubleValue ?? 0
            guard tracks.indices.contains(track), start.isFinite, start >= 0, duration.isFinite, duration > 0 else { throw TrackerError.message("An audio clip has an invalid track, position, or duration.") }
        }
        guard JSONSerialization.isValidJSONObject(candidate) else { throw TrackerError.message("Project contains values that cannot be saved.") }
        _ = try AudioFreezeProject.validate(project: candidate)
    }

    func update(_ candidate: [String: Any], dirty: Bool = true) throws {
        try validate(candidate)
        let changed = project == nil || (try? JSONSerialization.data(withJSONObject: project!, options: [.sortedKeys])) != (try? JSONSerialization.data(withJSONObject: candidate, options: [.sortedKeys]))
        project = candidate
        self.dirty = dirty && (self.dirty || changed)
    }

    func newProject() {
        project = nil
        url = nil; fileFingerprint = nil
        dirty = false
    }

    var templatesURL: URL { support.appendingPathComponent("Templates", isDirectory: true) }
    private var templateSettingsURL: URL { support.appendingPathComponent("ProjectTemplates.json") }

    private func templateDirectory(_ id: String) throws -> URL {
        guard !id.isEmpty, id.count <= 80, id.range(of: "^[A-Za-z0-9_-]+$", options: .regularExpression) != nil else {
            throw TrackerError.message("Invalid project template identifier.")
        }
        return templatesURL.appendingPathComponent(id, isDirectory: true)
    }

    func defaultTemplateID() throws -> String? {
        guard FileManager.default.fileExists(atPath: templateSettingsURL.path) else { return nil }
        guard let settings = try JSONSerialization.jsonObject(with: Data(contentsOf: templateSettingsURL)) as? [String: Any] else {
            throw TrackerError.message("Project template preferences are unreadable. Choose a default template again.")
        }
        guard settings["defaultID"] == nil || settings["defaultID"] is String else {
            throw TrackerError.message("The default project template setting is invalid. Choose a default template again.")
        }
        return settings["defaultID"] as? String
    }

    func templateProject(id: String) throws -> [String: Any] {
        let file = try templateDirectory(id).appendingPathComponent("Template.supertracker")
        do { return try read(file) }
        catch { throw TrackerError.message("Project template \(id) could not be opened: \(error.localizedDescription). Choose another default or save this template again.") }
    }

    func listTemplates() throws -> [[String: Any]] {
        guard FileManager.default.fileExists(atPath: templatesURL.path) else { return [] }
        return try FileManager.default.contentsOfDirectory(at: templatesURL, includingPropertiesForKeys: nil).compactMap { folder in
            guard let data = try? Data(contentsOf: folder.appendingPathComponent("Metadata.json")),
                  var metadata = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let id = metadata["id"] as? String, (try? templateDirectory(id).standardizedFileURL.path) == folder.standardizedFileURL.path else { return nil }
            metadata["path"] = folder.appendingPathComponent("Template.supertracker").path
            return metadata
        }.sorted { ($0["name"] as? String ?? "") < ($1["name"] as? String ?? "") }
    }

    @discardableResult
    func saveTemplate(project candidate: [String: Any], name: String, id: String? = nil) throws -> [String: Any] {
        try validate(candidate)
        let title = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, title.count <= 160 else { throw TrackerError.message("Give the project template a name of 1–160 characters.") }
        let identity = id ?? UUID().uuidString.lowercased(), folder = try templateDirectory(identity)
        // A separate store collects media without rebinding the open document,
        // changing its dirty state, or writing its recovery file.
        let writer = try ProjectStore(directory: folder)
        let file = folder.appendingPathComponent("Template.supertracker")
        _ = try writer.save(to: file, snapshot: candidate, collectSamples: true)
        try? FileManager.default.removeItem(at: writer.recoveryURL)
        try? FileManager.default.removeItem(at: writer.recoverySessionURL)
        let metadata: [String: Any] = ["id": identity, "name": title, "path": file.path]
        try Self.write(metadata, to: folder.appendingPathComponent("Metadata.json"))
        return metadata
    }

    func setDefaultTemplate(id: String?) throws {
        if let id { _ = try templateProject(id: id) }
        try Self.write(id.map { ["defaultID": $0] } ?? [:], to: templateSettingsURL)
    }

    /// Resolve before replacing a session, so invalid template preferences never
    /// discard an existing project. A nil result requests the web blank factory.
    func proposedNewProject(templateID: String? = nil, blank: Bool = false) throws -> [String: Any]? {
        if blank { return nil }
        guard let id = try templateID ?? defaultTemplateID() else { return nil }
        return try templateProject(id: id)
    }

    func startupProject() throws -> [String: Any]? {
        // Recovery is rewritten within seconds of launch (autosave, New from the default template, a
        // session that failed to come up). Keep the session this launch found before anything can.
        try? keepLaunchRecoveryCopy()
        let recovered: [String: Any]?
        do { recovered = try restore() }
        catch {
            // The caller continues with a blank session whose first autosave would
            // replace this file. Keep the unreadable session for manual recovery.
            let kept = try? preserveUnreadableRecovery()
            throw TrackerError.message(error.localizedDescription + (kept.map { " The previous session was kept as \($0.path)." } ?? ""))
        }
        if let recovered { return recovered }
        let candidate = try proposedNewProject()
        try acceptNewProject(candidate)
        return candidate
    }

    func acceptNewProject(_ candidate: [String: Any]?) throws {
        if let candidate { try validate(candidate) }
        project = candidate; url = nil; fileFingerprint = nil; dirty = false
    }

    func read(_ file: URL) throws -> [String: Any] {
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        guard (attributes[.size] as? NSNumber)?.intValue ?? 0 < 100_000_000 else { throw TrackerError.message("Project is too large to safely open (100 MB limit).") }
        guard var result = try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any] else { throw TrackerError.message("The file is not a SuperTracker project.") }
        lastReadWasMigrated = ProjectMigration.needsMigration(result)
        let migration = ProjectMigration.migratedWithNotes(result)
        result = migration.project; lastMigrationNotes = migration.notes
        try validate(result)
        let stored = result
        var resolvedAliases: [String: String] = [:]
        result = Self.mapMedia(result) { path in
            if path.isEmpty || path.hasPrefix("/") { return path }
            let resolved = file.deletingLastPathComponent().appendingPathComponent(path).standardizedFileURL.path
            resolvedAliases[path] = resolved
            return resolved
        } as! [String: Any]
        // Resolving a package-relative alias changes its spelling, not its
        // content identity. Opaque/unsupported proof remains byte-equivalent.
        result = try Self.relocateFreezeEvidence(from: stored, to: result) { path, _ in
            if path.isEmpty || path.hasPrefix("/") { return path }
            return file.deletingLastPathComponent().appendingPathComponent(path).standardizedFileURL.path
        }
        result = try AudioFreezeTrackFingerprint.preservingRelocatedSources(from: stored, to: result, verifiedAliases: resolvedAliases)
        try validate(result)
        return result
    }

    func acceptLoaded(_ result: [String: Any], from file: URL) {
        project = result
        url = file; fileFingerprint = try? Self.contentHash(file)
        dirty = lastReadWasMigrated
    }

    func load(_ file: URL) throws -> [String: Any] {
        let result = try read(file)
        acceptLoaded(result, from: file)
        return result
    }

    /// The recovered session keeps its file (and is dirty only if it had unsaved changes) when the session record matches
    /// this recovery file and the file on disk is byte-identical to what the session last read or saved. Otherwise
    /// (file changed or gone, record missing or stale) it reopens untitled and dirty. Autosave only ever writes recovery files.
    func restore() throws -> [String: Any]? {
        guard FileManager.default.fileExists(atPath: recoveryURL.path) else { return nil }
        let restored = try read(recoveryURL)
        project = restored; url = nil; fileFingerprint = nil; dirty = true
        if let record = try? Data(contentsOf: recoverySessionURL),
           let session = try? JSONSerialization.jsonObject(with: record) as? [String: Any],
           let recovery = try? Data(contentsOf: recoveryURL), session["recoverySHA256"] as? String == Self.sha256(recovery),
           let path = session["path"] as? String, path.hasPrefix("/"), let expected = session["sha256"] as? String,
           (try? Self.contentHash(URL(fileURLWithPath: path))) == expected {
            url = URL(fileURLWithPath: path); fileFingerprint = expected
            dirty = session["dirty"] as? Bool ?? true
        }
        return restored
    }

    /// The recovery file as each of the last few launches found it (distinct sessions only, newest kept),
    /// openable with File > Open when a relaunch replaced an unsaved session.
    var recoveryBackupsURL: URL { support.appendingPathComponent("Recovery Backups", isDirectory: true) }
    static let recoveryBackupLimit = 5

    func keepLaunchRecoveryCopy() throws {
        guard let data = try? Data(contentsOf: recoveryURL), !data.isEmpty else { return }
        let manager = FileManager.default
        try manager.createDirectory(at: recoveryBackupsURL, withIntermediateDirectories: true)
        let existing = try manager.contentsOfDirectory(at: recoveryBackupsURL, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "supertracker" }.sorted { $0.lastPathComponent > $1.lastPathComponent }
        if let newest = existing.first, (try? Data(contentsOf: newest)) == data { return }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX"); formatter.dateFormat = "yyyy-MM-dd HH.mm.ss.SSS"
        // Names sort by time, so the newest copy is always the first name.
        var date = Date(), target: URL
        repeat {
            target = recoveryBackupsURL.appendingPathComponent("Session \(formatter.string(from: date)).supertracker")
            date += 0.001
        } while manager.fileExists(atPath: target.path)
        try data.write(to: target, options: .atomic)
        let kept = ([target] + existing).sorted { $0.lastPathComponent > $1.lastPathComponent }
        for old in kept.dropFirst(Self.recoveryBackupLimit) { try? manager.removeItem(at: old) }
    }

    private func preserveUnreadableRecovery() throws -> URL {
        let stamp = ISO8601DateFormatter.string(from: Date(), timeZone: .current, formatOptions: [.withFullDate, .withTime])
        var target = support.appendingPathComponent("Recovery (unreadable \(stamp)).supertracker")
        if FileManager.default.fileExists(atPath: target.path) {
            target = support.appendingPathComponent("Recovery (unreadable \(stamp) \(UUID().uuidString.prefix(8))).supertracker")
        }
        try FileManager.default.moveItem(at: recoveryURL, to: target)
        return target
    }

    func autosave() throws {
        guard let project else { return }
        try validate(project)
        let data = try Self.encoded(project)
        try data.write(to: recoveryURL, options: .atomic)
        var session: [String: Any] = ["recoverySHA256": Self.sha256(data), "dirty": dirty]
        if let url, let fileFingerprint { session["path"] = url.path; session["sha256"] = fileFingerprint }
        try Self.write(session, to: recoverySessionURL)
    }

    func save(to destination: URL, snapshot: [String: Any]? = nil, collectSamples: Bool = true) throws -> [String: Any] {
        lastRecoveryWarning = nil
        guard var current = snapshot ?? project else { throw TrackerError.message("There is no project to save.") }
        let original = current
        try validate(current)
        let freezeHashes = try Self.freezeAssetHashes(current)
        try Self.verifyFreezeAssets(freezeHashes, relativeTo: url ?? destination)
        var stored = current
        if collectSamples {
            let media = destination.deletingPathExtension().appendingPathExtension("media")
            var replacements: [String: (stored: String, live: String, sha256: String)] = [:]
            _ = try Self.mapMedia(current) { path in
                guard !path.isEmpty, replacements[path] == nil else { return path }
                let source = Self.mediaURL(path, relativeTo: url ?? destination)
                guard FileManager.default.fileExists(atPath: source.path) else { return path }
                // Missing/empty/unreadable printed media remains repairable. Do
                // not collect it as a valid print or rewrite its recorded hash.
                if freezeHashes[path] != nil, !Self.readableNonemptyFile(source) { return path }
                try FileManager.default.createDirectory(at: media, withIntermediateDirectories: true)
                let digest = try Self.contentHash(source)
                let target: URL
                if source.deletingLastPathComponent().resolvingSymlinksInPath().standardizedFileURL.path == media.resolvingSymlinksInPath().standardizedFileURL.path {
                    target = media.appendingPathComponent(source.lastPathComponent)
                } else {
                    target = media.appendingPathComponent("\(digest.prefix(16))-\(source.lastPathComponent)")
                    if !FileManager.default.fileExists(atPath: target.path) { try FileManager.default.copyItem(at: source, to: target) }
                    guard try Self.contentHash(target) == digest else {
                        throw TrackerError.message("Collected media has different content: \(target.lastPathComponent). The existing file was not overwritten.")
                    }
                }
                replacements[path] = (media.lastPathComponent + "/" + target.lastPathComponent, target.path, digest)
                return path
            }
            stored = Self.mapMedia(current) { replacements[$0]?.stored ?? $0 } as! [String: Any]
            let live = Self.mapMedia(current) { replacements[$0]?.live ?? $0 } as! [String: Any]
            // Path rewriting must not carry a captured hash onto changed bytes.
            // Leave unmatched evidence at its original path so validation rejects
            // changed protected sources and marks upstream dependencies stale.
            stored = try Self.relocateFreezeEvidence(from: current, to: stored) { path, hash in
                guard let replacement = replacements[path], replacement.sha256 == hash else { return path }
                return replacement.stored
            }
            current = try Self.relocateFreezeEvidence(from: current, to: live) { path, hash in
                guard let replacement = replacements[path], replacement.sha256 == hash else { return path }
                return replacement.live
            }
            // Copying already verifies both ends against the same full hash.
            // Preserve only previously fresh source fingerprints across those
            // proven aliases; a stale capture is never certified by Save As.
            stored = try AudioFreezeTrackFingerprint.preservingRelocatedSources(from: original, to: stored,
                verifiedAliases: replacements.mapValues(\.stored))
            current = try AudioFreezeTrackFingerprint.preservingRelocatedSources(from: original, to: current,
                verifiedAliases: replacements.mapValues(\.live))
        }
        // Both the live and portable forms must preserve all captured musical
        // fingerprints. The media manifest paths move together with source paths.
        try validate(stored)
        try validate(current)
        try Self.verifyFreezeAssets(try Self.freezeAssetHashes(current), relativeTo: destination)
        try Self.write(stored, to: destination)
        project = current
        url = destination; fileFingerprint = try? Self.contentHash(destination)
        dirty = false
        // The primary atomic write has committed. A secondary recovery copy
        // cannot turn that success into a failure or hide the new destination.
        do { try autosave() }
        catch { lastRecoveryWarning = error.localizedDescription }
        return current
    }

    private static func relocateFreezeEvidence(from original: [String: Any], to rewritten: [String: Any],
                                               transform: (String, String) -> String) throws -> [String: Any] {
        var result = rewritten, tracks = rewritten["tracks"] as? [[String: Any]] ?? []
        let originalTracks = original["tracks"] as? [[String: Any]] ?? []
        for index in tracks.indices where originalTracks.indices.contains(index) {
            guard let originalFreeze = originalTracks[index]["freeze"] as? [String: Any],
                  var freeze = tracks[index]["freeze"] as? [String: Any],
                  let media = originalFreeze["media"] as? [[String: Any]] else { continue }
            freeze["media"] = media.map { entry -> [String: Any] in
                var moved = entry
                if let path = entry["path"] as? String, let hash = entry["sha256"] as? String { moved["path"] = transform(path, hash) }
                return moved
            }
            if case .supported(let proof) = try AudioFreezeDurationProof.assess(originalFreeze["durationProof"]) {
                let relocations = proof.sources.flatMap { source in
                    source.paths.compactMap { path -> AudioFreezeDurationProof.Relocation? in
                        let moved = transform(path, source.decoded.sha256)
                        guard moved != path else { return nil }
                        return AudioFreezeDurationProof.Relocation(from: path, to: moved, sha256: source.decoded.sha256)
                    }
                }
                if let relocated = try AudioFreezeDurationProof.relocating(originalFreeze["durationProof"],
                    relocations: relocations, replacePaths: true) { freeze["durationProof"] = relocated }
            }
            tracks[index]["freeze"] = freeze
        }
        result["tracks"] = tracks
        return result
    }
    private static func mapMedia(_ value: Any, transform: (String) throws -> String) rethrows -> Any {
        // File ownership is defined by the project schema, not a key name or
        // filename extension. Plug-in state, future extensions and historical
        // records can contain path-like strings with completely different meaning.
        guard var project = value as? [String: Any] else { return value }
        func fields(_ object: [String: Any], _ keys: [String]) throws -> [String: Any] {
            var result = object
            // Built-in sample references are app content, never project media.
            for key in keys { if let path = object[key] as? String, !BuiltinSamples.isBuiltin(path) { result[key] = try transform(path) } }
            return result
        }
        if let instruments = project["instruments"] as? [[String: Any]] {
            project["instruments"] = try instruments.map { instrument in
                var result = try fields(instrument, ["samplePath"])
                if let zones = instrument["zones"] as? [[String: Any]] {
                    result["zones"] = try zones.map { try fields($0, ["path", "samplePath"]) }
                }
                return result
            }
        }
        if let kits = project["kits"] as? [[String: Any]] {
            project["kits"] = try kits.map { kit in
                var result = kit
                if let sounds = kit["sounds"] as? [[String: Any]] {
                    result["sounds"] = try sounds.map { sound in
                        var value = sound
                        if let instrument = sound["instrument"] as? [String: Any] {
                            var mapped = try fields(instrument, ["samplePath"])
                            if let zones = instrument["zones"] as? [[String: Any]] { mapped["zones"] = try zones.map { try fields($0, ["path", "samplePath"]) } }
                            value["instrument"] = mapped
                        }
                        return value
                    }
                }
                return result
            }
        }
        if let clips = project["audioClips"] as? [[String: Any]] {
            project["audioClips"] = try clips.map { clip in
                var result = try fields(clip, ["path", "samplePath"])
                if var set = clip["takeSet"] as? [String: Any], let takes = set["takes"] as? [[String: Any]] {
                    set["takes"] = try takes.map { try fields($0, ["path"]) }; result["takeSet"] = set
                }
                // Preserve portability for earlier projects that retained a
                // plain alternatives array before versioned takeSet metadata.
                if let takes = clip["takes"] as? [[String: Any]] {
                    result["takes"] = try takes.map { try fields($0, ["path", "samplePath"]) }
                }
                return result
            }
        }
        if let tracks = project["tracks"] as? [[String: Any]] {
            project["tracks"] = try tracks.map { track in
                var result = track
                if var freeze = track["freeze"] as? [String: Any], let assets = freeze["assets"] as? [[String: Any]] {
                    freeze["assets"] = try assets.map { try fields($0, ["path"]) }; result["freeze"] = freeze
                }
                if let captured = track["frozen"] as? [String: Any] {
                    var frozen = try fields(captured, ["path"])
                    if let prints = captured["patternPrints"] as? [[String: Any]] {
                        frozen["patternPrints"] = try prints.map { print in
                            var moved = try fields(print, ["path"])
                            if let loop = print["loop"] as? [String: Any] { moved["loop"] = try fields(loop, ["path"]) }
                            return moved
                        }
                    }
                    result["frozen"] = frozen
                }
                // Captured source evidence and duration proof aliases are moved
                // only by relocateFreezeEvidence after exact hash agreement.
                return result
            }
        }
        if let manifest = project["mediaManifest"] as? [[String: Any]] {
            project["mediaManifest"] = try manifest.map { try fields($0, ["path"]) }
        }
        return project
    }

    private static func encoded(_ object: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
    }
    private static func write(_ object: [String: Any], to url: URL) throws { try encoded(object).write(to: url, options: .atomic) }
    private static func sha256(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }

    private static func mediaURL(_ path: String, relativeTo projectURL: URL) -> URL {
        path.hasPrefix("/") ? URL(fileURLWithPath: path) : projectURL.deletingLastPathComponent().appendingPathComponent(path)
    }
    private static func readableNonemptyFile(_ url: URL) -> Bool {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              attributes[.type] as? FileAttributeType == .typeRegular,
              (attributes[.size] as? NSNumber)?.int64Value ?? 0 > 0 else { return false }
        return FileManager.default.isReadableFile(atPath: url.path)
    }
    private static func freezeAssetHashes(_ project: [String: Any]) throws -> [String: String] {
        var result: [String: String] = [:]
        func include(_ path: String, _ hash: String) throws {
            guard result[path] == nil || result[path] == hash else { throw TrackerError.message("Frozen stages disagree about the content of \(path).") }
            result[path] = hash
        }
        for status in try AudioFreezeProject.validate(project: project) {
            for asset in status.record.assets {
                try include(asset.path, asset.sha256)
            }
        }
        // Current Track Freeze stores its song and pattern prints separately
        // from staged freeze assets. Their captured identities are just as
        // immutable; collecting changed bytes must never carry forward old SHA.
        for track in project["tracks"] as? [[String: Any]] ?? [] {
            guard let frozen = track["frozen"] as? [String: Any] else { continue }
            let prints = frozen["patternPrints"] as? [[String: Any]] ?? []
            for entry in [frozen] + prints + prints.compactMap({ $0["loop"] as? [String: Any] }) {
                guard let path = entry["path"] as? String, !path.isEmpty,
                      let hash = entry["sha256"] as? String, !hash.isEmpty else { continue }
                guard hash.count == 64, hash.allSatisfy({ $0.isASCII && $0.isHexDigit }) else {
                    throw TrackerError.message("Frozen audio has an invalid captured SHA-256. Restore its metadata or Unfreeze before collecting it.")
                }
                try include(path, hash.lowercased())
            }
        }
        return result
    }
    private static func verifyFreezeAssets(_ hashes: [String: String], relativeTo projectURL: URL) throws {
        var remaining: Int64 = 4_294_967_296
        for (path, expected) in hashes.sorted(by: { $0.key < $1.key }) {
            let url = mediaURL(path, relativeTo: projectURL)
            guard readableNonemptyFile(url) else { continue }
            let size = (try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? 0
            guard size <= remaining else { throw TrackerError.message("Frozen media exceeds the 4 GiB save-verification budget.") }
            remaining -= size
            guard try contentHash(url, maximumBytes: min(size, 1_073_741_824)) == expected else {
                throw TrackerError.message("Frozen audio content changed: \(url.lastPathComponent). Relink the verified original print or Unfreeze; its recorded hash cannot be replaced.")
            }
        }
    }
    private static func contentHash(_ url: URL, maximumBytes: Int64 = 4_294_967_296) throws -> String {
        let before = try FileManager.default.attributesOfItem(atPath: url.path)
        let expectedBytes = (before[.size] as? NSNumber)?.int64Value ?? 0
        guard before[.type] as? FileAttributeType == .typeRegular, expectedBytes <= maximumBytes else {
            throw TrackerError.message("Media cannot be verified within the bounded save budget: \(url.lastPathComponent).")
        }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hash = SHA256(), bytes: Int64 = 0
        while let data = try handle.read(upToCount: 1024 * 1024), !data.isEmpty {
            bytes += Int64(data.count)
            guard bytes <= maximumBytes else { throw TrackerError.message("Media grew while saving: \(url.lastPathComponent).") }
            hash.update(data: data)
        }
        let after = try FileManager.default.attributesOfItem(atPath: url.path)
        guard bytes == expectedBytes, (after[.size] as? NSNumber)?.int64Value == expectedBytes,
              before[.modificationDate] as? Date == after[.modificationDate] as? Date,
              before[.systemFileNumber] as? NSNumber == after[.systemFileNumber] as? NSNumber else {
            throw TrackerError.message("Media changed while saving: \(url.lastPathComponent).")
        }
        return hash.finalize().map { String(format: "%02x", $0) }.joined()
    }
}

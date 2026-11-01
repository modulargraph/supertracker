import Foundation
import AudioToolbox
import CoreFoundation

/// Read-only discovery of sample, NKS and kit libraries. Audio is never decoded while indexing.
/// The mutable index is protected by one short-held lock; filesystem work stays off the UI thread.
final class LibraryService: @unchecked Sendable {
    private struct LibraryRoot: Codable {
        var path: String
        var name: String
        var source: String
        var dictionary: [String: Any] {
            var directory: ObjCBool = false
            let exists = FileManager.default.fileExists(atPath: path, isDirectory: &directory)
            return ["path": path, "name": name, "source": source, "available": exists && directory.boolValue]
        }
    }

    /// Immutable after insertion. Snapshots retain references instead of copying large value records.
    private final class Item: Codable {
        var path: String
        var root: String
        var name: String
        var kind: String
        var category: String
        var tags: [String]
        var pack: String
        var bpm: Double?
        var key: String?
        var size: Int64
        var modified: Double
        var metadataSource: String
        var previewPath: String?
        var duration: Double?
        var durationChecked: Bool?
        var kitSoundCount: Int?
        var kitMetadataError: String?
        // Only the inferred record is encoded in the index; overrides have their own durable file.
        var inferred: Item?
        var userMetadata: MetadataOverride?
        /// Folded UTF-8 search text; byte search keeps 160k-entry queries interactive.
        var searchBytes: [UInt8] = []
        var foldedNameBytes: [UInt8] = []
        var foldedName = ""
        var foldedPack = ""
        var foldedCategory = ""
        var foldedTags = ""
        var foldedKey = ""
        var fileExtension = ""
        var foldedRoot = ""
        var tagValues: Set<String> = []
        /// Interned facet values (see facetInterners), indexed like facetNames; collections (1) are derived.
        var facetIDs: [Int32] = []
        var tagIDs: [Int32] = []
        var collectionRootID: Int32 = -1
        var sampleType = "unknown"
        var baseCollection = "User libraries"
        var collectionRoot: String?
        private enum CodingKeys: String, CodingKey {
            case path, root, name, kind, category, tags, pack, bpm, key, size, modified, metadataSource, previewPath, duration, durationChecked, kitSoundCount, kitMetadataError
        }
        init(path: String, root: String, name: String, kind: String, category: String, tags: [String], pack: String,
             bpm: Double?, key: String?, size: Int64, modified: Double, metadataSource: String, previewPath: String?) {
            self.path = path; self.root = root; self.name = name; self.kind = kind; self.category = category
            self.tags = tags; self.pack = pack; self.bpm = bpm; self.key = key; self.size = size
            self.modified = modified; self.metadataSource = metadataSource; self.previewPath = previewPath
        }
        func prepareSearchFields() {
            foldedName = LibraryService.fold(name)
            foldedPack = LibraryService.fold(pack)
            foldedCategory = LibraryService.fold(category)
            foldedTags = LibraryService.fold(tags.joined(separator: " "))
            foldedKey = key.map(LibraryService.keySlot) ?? ""
            fileExtension = (path as NSString).pathExtension.lowercased()
            foldedRoot = LibraryService.fold(root)
            tagValues = Set(tags.map(LibraryService.fold))
            sampleType = userMetadata?.sampleType ?? LibraryService.inferSampleType(name: name, path: path, root: root, kind: kind, filenameTempo: (inferred ?? self).bpm != nil)
            baseCollection = LibraryService.baseCollection(path: path)
            collectionRoot = LibraryService.collectionRoot(path: path, root: root, collection: baseCollection)
            let interners = LibraryService.facetInterners
            facetIDs = [interners[0].id(category), -1, interners[2].id(root), interners[3].id(pack), interners[4].id(key ?? "unknown"),
                        interners[5].id(sampleType), interners[6].id(fileExtension)]
            tagIDs = tags.map { interners[7].id($0) }
            collectionRootID = collectionRoot.map { LibraryService.collectionRootInterner.id($0) } ?? -1
            searchBytes = Array([foldedName, foldedCategory, foldedTags, foldedPack, LibraryService.fold(path), foldedKey].joined(separator: " ").utf8)
            foldedNameBytes = Array(foldedName.utf8)
        }
        func applying(_ metadata: MetadataOverride?) -> Item {
            let base = inferred ?? self
            guard let metadata, !metadata.isEmpty else { return base }
            let item = Item(path: base.path, root: base.root, name: base.name, kind: base.kind,
                            category: metadata.category ?? base.category, tags: metadata.tags ?? base.tags, pack: base.pack,
                            bpm: metadata.bpm ?? base.bpm, key: metadata.key ?? base.key,
                            size: base.size, modified: base.modified, metadataSource: base.metadataSource, previewPath: base.previewPath)
            // Zero/empty are explicit unknowns, distinct from absent overrides.
            if metadata.bpm == 0 { item.bpm = nil }
            if metadata.key == "" { item.key = nil }
            item.duration = base.duration; item.durationChecked = base.durationChecked
            item.kitSoundCount = base.kitSoundCount; item.kitMetadataError = base.kitMetadataError
            item.inferred = base; item.userMetadata = metadata; item.prepareSearchFields()
            return item
        }
        func replacingDuration(_ duration: Double?) -> Item {
            let base = inferred ?? self
            let item = Item(path: base.path, root: base.root, name: base.name, kind: base.kind, category: base.category, tags: base.tags, pack: base.pack,
                            bpm: base.bpm, key: base.key, size: base.size, modified: base.modified, metadataSource: base.metadataSource, previewPath: base.previewPath)
            item.duration = duration; item.durationChecked = true
            item.kitSoundCount = base.kitSoundCount; item.kitMetadataError = base.kitMetadataError
            item.prepareSearchFields()
            return item.applying(userMetadata)
        }
        var editableDictionary: [String: Any] {
            ["category": category, "tags": tags, "bpm": bpm ?? 0, "key": key ?? "", "sampleType": sampleType]
        }
        func collection(verified: Set<String>) -> String { collectionRoot.map { verified.contains($0) } == true ? "Maschine expansions" : baseCollection }
        func collectionIndex(verified: Set<Int32>) -> Int {
            if collectionRootID >= 0 && verified.contains(collectionRootID) { return 1 }
            return baseCollection == "Loopcloud" ? 0 : baseCollection == "Native Instruments" ? 2 : 3
        }
        func dictionary(favorites: Set<String>, verifiedCollections: Set<String> = [], pendingDurations: Set<String> = []) -> [String: Any] {
            var result: [String: Any] = [
                "id": path, "path": path, "name": name, "kind": kind,
                "category": category, "tags": tags, "pack": pack,
                "favorite": favorites.contains(path), "size": size,
                "folder": URL(fileURLWithPath: path).deletingLastPathComponent().path,
                "extension": URL(fileURLWithPath: path).pathExtension.lowercased(),
                "metadataSource": metadataSource, "root": root, "collection": collection(verified: verifiedCollections),
                "sampleType": sampleType, "durationStatus": duration != nil ? "known" : pendingDurations.contains(path) ? "pending" : "unknown"
            ]
            result["userMetadata"] = userMetadata?.dictionary ?? [:]
            result["inferredMetadata"] = (inferred ?? self).editableDictionary
            if let bpm = bpm { result["bpm"] = bpm }
            if let key = key { result["key"] = key }
            if let previewPath = previewPath { result["previewPath"] = previewPath }
            if let duration { result["duration"] = duration; result["durationSource"] = "audio header" }
            if kind == "kit" {
                result["kitFormat"] = fileExtension == "stkit" ? "supertracker-kit" : "maschine-group"
                result["source"] = collection(verified: verifiedCollections)
                result["expansion"] = pack; result["type"] = category
                result["user"] = fileExtension == "stkit"
                if let kitSoundCount { result["soundCount"] = kitSoundCount }
                if let kitMetadataError { result["metadataError"] = kitMetadataError }
            }
            return result
        }
    }

    private struct MetadataOverride: Codable {
        var category: String?
        var tags: [String]?
        var bpm: Double? // Zero is an explicit unknown; nil restores inference.
        var key: String? // Empty is an explicit unknown.
        var sampleType: String?
        var isEmpty: Bool { category == nil && tags == nil && bpm == nil && key == nil && sampleType == nil }
        var dictionary: [String: Any] {
            var result: [String: Any] = [:]
            if let category { result["category"] = category }
            if let tags { result["tags"] = tags }
            if let bpm { result["bpm"] = bpm }
            if let key { result["key"] = key }
            if let sampleType { result["sampleType"] = sampleType }
            return result
        }
        mutating func merge(_ changes: [String: Any]) throws {
            guard Set(changes.keys).isSubset(of: ["category", "tags", "bpm", "key", "sampleType"]) else { throw LibraryService.error("Unknown library metadata field.") }
            for (field, value) in changes {
                let reset = value is NSNull
                switch field {
                case "category":
                    if reset { category = nil; continue }
                    guard let text = value as? String, text.count <= 80, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { throw LibraryService.error("Choose a category up to 80 characters, or restore its inferred value.") }
                    category = text.trimmingCharacters(in: .whitespacesAndNewlines)
                case "tags":
                    if reset { tags = nil; continue }
                    guard let values = value as? [String], values.count <= 64 else { throw LibraryService.error("Use up to 64 tags.") }
                    var found: Set<String> = []; var cleaned: [String] = []
                    for value in values {
                        let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard text.count <= 80 else { throw LibraryService.error("Each tag can contain up to 80 characters.") }
                        if !text.isEmpty, found.insert(LibraryService.fold(text)).inserted { cleaned.append(text) }
                    }
                    tags = cleaned
                case "bpm":
                    if reset { bpm = nil; continue }
                    guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite,
                          number.doubleValue == 0 || (1...999).contains(number.doubleValue) else { throw LibraryService.error("Tempo must be 1–999 BPM, or blank for unknown.") }
                    bpm = number.doubleValue
                case "key":
                    if reset { key = nil; continue }
                    guard let text = value as? String, text.count <= 24 else { throw LibraryService.error("Choose a musical key, or leave it blank for unknown.") }
                    let normalized = text.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: "♯", with: "#").replacingOccurrences(of: "♭", with: "b")
                    guard normalized.isEmpty || normalized.range(of: "^[A-Ga-g][#b]?(?:\\s*(?:major|minor|maj|min|m))?$", options: [.regularExpression, .caseInsensitive]) != nil else { throw LibraryService.error("Use a key such as C, F# minor or Bb major.") }
                    if normalized.isEmpty { key = "" }
                    else {
                        var suffix = String(normalized.dropFirst())
                        let accidental = suffix.first == "#" || suffix.first?.lowercased() == "b" ? String(suffix.removeFirst()).lowercased() : ""
                        let mode = suffix.trimmingCharacters(in: .whitespaces).lowercased()
                        key = String(normalized.prefix(1)).uppercased() + accidental + (["m", "min", "minor"].contains(mode) ? " minor" : ["maj", "major"].contains(mode) ? " major" : "")
                    }
                case "sampleType":
                    if reset { sampleType = nil; continue }
                    guard let text = value as? String, ["oneshot", "loop", "unknown"].contains(text) else { throw LibraryService.error("Sound type must be one-shot, loop or unknown.") }
                    sampleType = text
                default: break
                }
            }
        }
    }
    private struct MetadataFile: Codable {
        var version = 1
        var items: [String: MetadataOverride]
    }

    private struct Cache: Codable {
        var version: Int = 1
        var roots: [LibraryRoot]
        var items: [Item]
        var excludedRoots: [String]?
        var scanErrors: [String]?
        var scanReports: [RootReport]?
        var verifiedExpansionRoots: [String]?
        var inferenceVersion: Int?
        /// Roots completely enumerated by a kit-capable indexer. Absent in older v1 caches.
        var kitIndexedRoots: [String]?
    }
    private struct RootReport: Codable {
        var path: String
        var status: String
        var visited: Int = 0
        var matched: Int = 0
        var skipped: Int = 0
        var errors: [String] = []
        var dictionary: [String: Any] {
            ["path": path, "status": status, "visited": visited, "matched": matched, "skipped": skipped, "errors": errors]
        }
    }

    private final class ScanToken {
        let id = UUID().uuidString
        let kitsOnly: Bool
        init(kitsOnly: Bool = false) { self.kitsOnly = kitsOnly }
        private let lock = NSLock()
        private var stopped = false
        var cancelled: Bool { lock.lock(); defer { lock.unlock() }; return stopped }
        func cancel() { lock.lock(); stopped = true; lock.unlock() }
    }

    private let lock = NSLock()
    // Utility avoids macOS background-I/O starvation on large external-volume directories.
    // Explicit pauses below provide a predictable scan budget while audio runs independently.
    private let scanQueue = DispatchQueue(label: "SuperTracker.library.scan", qos: .utility)
    private let searchQueue = DispatchQueue(label: "SuperTracker.library.search", qos: .utility)
    private let diskQueue = DispatchQueue(label: "SuperTracker.library.cache", qos: .utility)
    // Small user files (favorites, browser state) must never wait behind a multi-second index write.
    private let userFileQueue = DispatchQueue(label: "SuperTracker.library.userFiles", qos: .userInitiated)
    private let metadataEditQueue = DispatchQueue(label: "SuperTracker.library.edits", qos: .utility)
    private var metadataOverrides: [String: MetadataOverride] = [:]
    private var metadataReadError: String?
    private let metadataQueue = DispatchQueue(label: "SuperTracker.library.metadata", qos: .utility)
    private let cacheDirectory: URL
    /// Maschine-style NKS product/bank/type browser over installed and library presets.
    let nksCatalog: NKSCatalog
    /// Maschine-style expansion / bank / type browser over indexed kits and groups.
    let kitCatalog: KitCatalog
    private var roots: [LibraryRoot]
    private var items: [String: Item] = [:]
    private var favorites: Set<String> = []
    private var excludedRoots: Set<String> = []
    private var activeScan: ScanToken?
    private var kitIndexedRoots: Set<String> = []
    private var attemptedKitUpgradeRoots: Set<String> = []
    private var kitUpgradeEnabled = false
    private var eventHandler: ((String, [String: Any]) -> Void)?
    private var lastErrors: [String] = []
    private var rootReports: [String: RootReport] = [:]
    private var verifiedExpansionRoots: Set<String> = []
    /// Library roots whose folder is currently missing (for example a disconnected external drive).
    private var unavailableRoots: Set<String> = []
    private var volumeWatcher: DispatchSourceFileSystemObject?
    private var checkedCollectionRoots: Set<String> = []
    private var pendingCollections: [String] = []
    private var queuedCollections: Set<String> = []
    private var pendingDurations: [String] = []
    private var queuedDurations: Set<String> = []
    private var metadataWorkerScheduled = false
    private var metadataSaveWork: DispatchWorkItem?
    private var indexRevision: UInt64 = 0
    private var searchGeneration: UInt64 = 0
    private var searchWorkerScheduled = false
    private var pendingSearch: SearchRequest?
    private var cacheWriteScheduled = false
    private var cacheWriteRequested = false
    private var runningSearchComputations = 0
    private var peakSearchComputations = 0
    // Accessed only by the serial search worker.
    private var lastSearchKey = ""
    private var lastSearchRevision: UInt64 = .max
    private var lastSearchResult: [String: Any]?
    private struct SearchPopulation {
        let key: String
        let revision: UInt64
        let matching: [RankedItem]
        let facetCounts: [[String: Int]]
        let durationKnown: Int
        let durationUnknown: Int
        var eligiblePending: Set<String>
        let durationCandidates: [String]
        let offlineMatches: [String: Int]
    }
    // One bounded population cache shares filtering/facet work across result pages.
    private var lastPopulation: SearchPopulation?
    private struct SearchRequest {
        let generation: UInt64
        let payload: [String: Any]
        let completion: ([String: Any]) -> Void
    }
    private static let sampleExtensions: Set<String> = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "caf"]
    private static let nksExtensions: Set<String> = ["nksf", "nksfx"]
    private static let artworkQueue = DispatchQueue(label: "SuperTracker.nks.artwork", qos: .userInitiated)
    private static let ignoredDirectories: Set<String> = ["node_modules", ".git", ".svn", "__macosx", "$recycle.bin", "system volume information", ".trashes", ".spotlight-v100", ".fseventsd"]
    private static let softwarePackages: Set<String> = ["app", "component", "vst", "vst3", "framework", "bundle", "xcodeproj", "photoslibrary", "musiclibrary"]

    var onEvent: ((String, [String: Any]) -> Void)? {
        get { locked { eventHandler } }
        set { locked { eventHandler = newValue } }
    }

    init(cacheDirectory: URL? = nil, defaultRoots: [URL]? = nil, loadCacheAsynchronously: Bool = true) {
        // Development runs with a private support store must not share the user's index, favorites or labels.
        let supportOverride = ProcessInfo.processInfo.environment["SUPERTRACKER_SUPPORT_DIR"].flatMap { $0.hasPrefix("/") ? URL(fileURLWithPath: $0, isDirectory: true) : nil }
        self.cacheDirectory = cacheDirectory ?? supportOverride?.appendingPathComponent("Library", isDirectory: true) ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("SuperTracker/Library", isDirectory: true)
        self.roots = defaultRoots.map { $0.map { Self.root(for: $0.path, source: "configured") } } ?? Self.discoverRoots()
        // Fixture libraries never discover the system's installed NKS content.
        self.nksCatalog = NKSCatalog(cacheDirectory: self.cacheDirectory, discoverSystemContent: defaultRoots == nil)
        self.kitCatalog = KitCatalog(cacheDirectory: self.cacheDirectory, artwork: nksCatalog.artwork)
        // Load the small authoritative override file before any cache/scan publication.
        let overrideURL = self.cacheDirectory.appendingPathComponent("metadata-v1.json")
        if FileManager.default.fileExists(atPath: overrideURL.path) {
            do {
                let data = try Data(contentsOf: overrideURL)
                guard data.count <= 32 * 1024 * 1024 else { throw Self.error("Library metadata exceeds the supported size.") }
                let file = try JSONDecoder().decode(MetadataFile.self, from: data)
                guard file.version == 1 else { throw Self.error("This library metadata version is unsupported.") }
                var validatedItems: [String: MetadataOverride] = [:]
                for (path, value) in file.items {
                    var validated = MetadataOverride(); try validated.merge(value.dictionary)
                    validatedItems[Self.normalized(path)] = validated
                }
                metadataOverrides = validatedItems
            } catch { metadataReadError = "Could not read saved library metadata. The file has been preserved: \(error.localizedDescription)" }
        }
        nksCatalog.extraPresets = { [weak self] in
            guard let self else { return [] }
            return self.locked { self.items.values.filter { $0.kind == "nks" }.map(\.path) }
        }
        nksCatalog.extraRoots = { [weak self] in
            guard let self else { return [] }
            return self.locked { self.roots.map(\.path) }
        }
        nksCatalog.onEvent = { [weak self] type, payload in self?.emit(type, payload) }
        kitCatalog.onEvent = { [weak self] type, payload in self?.emit(type, payload) }
        if defaultRoots == nil { watchVolumes() }
        if loadCacheAsynchronously {
            scanQueue.async { [weak self] in self?.loadCache() }
        } else {
            loadCache()
        }
    }

    deinit { activeScan?.cancel(); volumeWatcher?.cancel() }

    /// Mounting or ejecting a drive changes /Volumes; refresh results so offline sounds are marked at once.
    private func watchVolumes() {
        let descriptor = open("/Volumes", O_EVTONLY)
        guard descriptor >= 0 else { return }
        let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: descriptor, eventMask: [.write, .link], queue: searchQueue)
        source.setEventHandler { [weak self] in
            guard let self else { return }
            let changed = self.refreshAvailability()
            self.scheduleKitIndexUpgrade()
            if changed { self.emit("library.results", self.locked { ["scanning": self.activeScan != nil, "count": self.items.count, "volumes": true, "errors": self.lastErrors] }) }
        }
        source.setCancelHandler { close(descriptor) }
        source.resume()
        volumeWatcher = source
    }

    /// Returns true when a root became available or unavailable. Cheap: one stat per root.
    @discardableResult private func refreshAvailability() -> Bool {
        let paths = locked { roots.map(\.path) }
        let offline = Set(paths.filter { path in
            var directory: ObjCBool = false
            return !(FileManager.default.fileExists(atPath: path, isDirectory: &directory) && directory.boolValue)
        })
        return locked {
            guard offline != unavailableRoots else { return false }
            attemptedKitUpgradeRoots.subtract(unavailableRoots.subtracting(offline))
            unavailableRoots = offline; indexRevision &+= 1
            return true
        }
    }

    private static func volumeName(_ path: String) -> String? {
        let parts = path.split(separator: "/")
        return parts.count >= 2 && parts[0] == "Volumes" ? String(parts[1]) : nil
    }

    func handle(_ command: String, payload: [String: Any]) async throws -> Any {
        switch command {
        case "library.roots":
            return rootDictionaries()
        case "library.diagnostics":
            let paths = payload["roots"] as? [String] ?? locked { roots.map(\.path) }
            guard paths.count <= 128 else { throw Self.error("Inspect up to 128 library roots at a time.") }
            return await withCheckedContinuation { continuation in
                searchQueue.async { continuation.resume(returning: paths.map { Self.probeRoot(Self.normalized($0)) }) }
            }
        case "library.status":
            return withKitUpgradeStatus(locked { ["scanning": activeScan != nil, "count": items.count, "errors": lastErrors, "rootReports": rootReports.values.map(\.dictionary).sorted { ($0["path"] as? String ?? "") < ($1["path"] as? String ?? "") }, "diagnosticsPath": cacheDirectory.appendingPathComponent("diagnostics-v1.json").path] })
        case "library.scan":
            return try beginScan(paths: payload["roots"] as? [String])
        case "library.cancel":
            let result = locked { () -> [String: Any] in
                activeScan?.cancel()
                activeScan = nil
                for path in rootReports.keys where ["scanning", "pending"].contains(rootReports[path]!.status) { rootReports[path]?.status = "cancelled" }
                return ["scanning": false, "count": items.count, "cancelled": true, "errors": lastErrors, "rootReports": rootReports.values.map(\.dictionary)]
            }
            persistCache()
            persistDiagnostics()
            emit("library.progress", result)
            return result
        case "library.search":
            return await withCheckedContinuation { continuation in
                enqueueSearch(payload) { continuation.resume(returning: $0) }
            }
        case "library.metadata.edit":
            return try await withCheckedThrowingContinuation { continuation in
                metadataEditQueue.async {
                    do { continuation.resume(returning: try self.editMetadata(payload)) }
                    catch { continuation.resume(throwing: error) }
                }
            }
        case "library.favorite":
            guard let path = payload["path"] as? String, !path.isEmpty else { throw Self.error("A file path is required to change a favorite.") }
            let normalized = Self.normalized(path)
            let favorite = payload["favorite"] as? Bool ?? true
            locked { if favorite { favorites.insert(normalized) } else { favorites.remove(normalized) }; indexRevision &+= 1 }
            persistFavorites()
            emit("library.favorite", ["path": normalized, "favorite": favorite])
            return ["path": normalized, "favorite": favorite]
        case "library.state.get":
            // Remembered browser query, filters and recent searches (UI-owned JSON, bounded).
            return await withCheckedContinuation { continuation in
                userFileQueue.async {
                    let url = self.cacheDirectory.appendingPathComponent("browser-state.json")
                    let value = (try? Data(contentsOf: url)).flatMap { $0.count <= 256 * 1024 ? try? JSONSerialization.jsonObject(with: $0) : nil }
                    continuation.resume(returning: value as? [String: Any] ?? [:])
                }
            }
        case "library.state.set":
            guard let state = payload["state"] as? [String: Any], JSONSerialization.isValidJSONObject(state) else { throw Self.error("Browser state must be a JSON object.") }
            let data = try JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
            guard data.count <= 256 * 1024 else { throw Self.error("Browser state is too large to remember.") }
            userFileQueue.async {
                do {
                    try FileManager.default.createDirectory(at: self.cacheDirectory, withIntermediateDirectories: true)
                    try data.write(to: self.cacheDirectory.appendingPathComponent("browser-state.json"), options: .atomic)
                } catch { self.emit("library.error", ["error": "Could not remember browser state: \(error.localizedDescription)"]) }
            }
            return ["saved": true]
        case "library.removeRoot":
            guard let path = payload["path"] as? String else { throw Self.error("A library root path is required.") }
            let normalized = Self.normalized(path)
            locked {
                activeScan?.cancel()
                activeScan = nil
                roots.removeAll { $0.path == normalized }
                items = items.filter { !Self.isInside($0.key, root: normalized) }
                excludedRoots.insert(normalized)
                kitIndexedRoots = kitIndexedRoots.filter { !Self.isInside($0, root: normalized) }
                attemptedKitUpgradeRoots = attemptedKitUpgradeRoots.filter { !Self.isInside($0, root: normalized) }
                rootReports.removeValue(forKey: normalized)
                indexRevision &+= 1
            }
            persistCache()
            return rootDictionaries()
        case "library.nks.browse":
            let snapshot = locked { self.favorites }
            return await nksCatalog.handleBrowse(payload, favorites: snapshot)
        case "library.nks.rescan":
            nksCatalog.refresh()
            return nksCatalog.status
        case "library.nks.status":
            return nksCatalog.status
        case "library.kits.browse":
            // Kits and Maschine groups by expansion, bank and type; headers are read lazily in the background.
            return await withCheckedContinuation { continuation in
                searchQueue.async { continuation.resume(returning: self.browseKits(payload)) }
            }
        case "library.nks.artwork":
            // Bounded thumbnails for artwork folders the index found; decoded off the main thread.
            let keys = (payload["keys"] as? [String] ?? []).prefix(400).map { String($0.prefix(512)) }
            let variant = payload["variant"] as? String ?? "tile"
            let artwork = nksCatalog.artwork
            return await withCheckedContinuation { continuation in
                Self.artworkQueue.async { continuation.resume(returning: ["images": artwork.thumbnails(keys: Array(keys), variant: variant), "variant": variant]) }
            }
        case "library.nks":
            guard let path = payload["path"] as? String else { throw Self.error("Choose an NKS preset first.") }
            guard Self.nksExtensions.contains(URL(fileURLWithPath: path).pathExtension.lowercased()) else {
                throw Self.error("This file is not an .nksf or .nksfx preset.")
            }
            return try await withCheckedThrowingContinuation { continuation in
                searchQueue.async {
                    do { continuation.resume(returning: try NKSMetadata(url: URL(fileURLWithPath: Self.normalized(path))).dictionary) }
                    catch { continuation.resume(throwing: error) }
                }
            }
        case "library.kit":
            guard let path = payload["path"] as? String else { throw Self.error("Choose a kit first.") }
            return try await withCheckedThrowingContinuation { continuation in
                userFileQueue.async {
                    do { continuation.resume(returning: try KitLibrary.metadata(at: URL(fileURLWithPath: Self.normalized(path)))) }
                    catch { continuation.resume(throwing: error) }
                }
            }
        default:
            throw Self.error("Unknown library command: \(command)")
        }
    }

    /// Upgrade discovery adds only newly supported kit records. Existing samples,
    /// NKS, favorites and labels remain immediately usable throughout the pass.
    private func scheduleKitIndexUpgrade() {
        let pending: [String] = locked {
            guard kitUpgradeEnabled, activeScan == nil else { return [] }
            return roots.map(\.path).filter { !kitIndexedRoots.contains($0) && !attemptedKitUpgradeRoots.contains($0) && !excludedRoots.contains($0) }
        }
        let online = pending.filter { path in
            var directory: ObjCBool = false
            return FileManager.default.fileExists(atPath: path, isDirectory: &directory) && directory.boolValue
        }
        guard !online.isEmpty else { return }
        _ = try? beginScan(paths: Array(online.prefix(128)), kitsOnly: true)
    }

    private func browseKits(_ payload: [String: Any]) -> [String: Any] {
        refreshAvailability()
        scheduleKitIndexUpgrade()
        let snapshot = locked { (items.values.filter { $0.kind == "kit" }, favorites, unavailableRoots, Dictionary(roots.map { ($0.path, $0.name) }, uniquingKeysWith: { first, _ in first }), activeScan != nil, lastErrors) }
        let sources = snapshot.0.map { item in
            KitCatalog.Source(path: item.path, name: item.name, pack: item.pack, rootName: snapshot.3[item.root] ?? (item.root as NSString).lastPathComponent, category: item.category,
                              size: item.size, modified: item.modified, tags: item.tags, previewPath: item.previewPath, soundCount: item.kitSoundCount,
                              metadataError: item.kitMetadataError, available: !snapshot.2.contains(item.root))
        }
        var result = kitCatalog.browse(payload, sources: sources, favorites: snapshot.1)
        result["scanning"] = snapshot.4; result["errors"] = snapshot.5
        return withKitUpgradeStatus(result)
    }

    private func withKitUpgradeStatus(_ value: [String: Any]) -> [String: Any] {
        var result = value
        locked {
            let pending = roots.map(\.path).filter { kitUpgradeEnabled && !kitIndexedRoots.contains($0) && !excludedRoots.contains($0) }
            result["kitIndexUpgradePending"] = !pending.isEmpty
            result["kitIndexUpgradeScanning"] = activeScan?.kitsOnly == true
            result["kitIndexUpgradeOfflineRoots"] = pending.filter { unavailableRoots.contains($0) }.sorted()
        }
        return result
    }

    private func beginScan(paths: [String]?, kitsOnly: Bool = false) throws -> [String: Any] {
        let requested: [LibraryRoot]
        if let paths = paths {
            guard paths.count <= 128 else { throw Self.error("Scan up to 128 library folders at a time.") }
            requested = paths.filter { !$0.isEmpty }.map { Self.root(for: $0, source: "user") }
            guard !requested.isEmpty else { throw Self.error("Choose at least one library folder to scan.") }
        } else {
            requested = locked { roots }
        }
        let token = ScanToken(kitsOnly: kitsOnly)
        var scanRequested = requested
        let result: [String: Any]? = locked {
            // Automatic discovery must never cancel a scan the user just started.
            if kitsOnly && activeScan != nil { return nil }
            if kitsOnly {
                // Filesystem checks ran outside this lock. A root may since have
                // been removed, completed or claimed by another upgrade attempt.
                // Automatic work cannot register a folder or undo its exclusion.
                let requestedPaths = Set(requested.map(\.path))
                scanRequested = roots.filter { requestedPaths.contains($0.path) && !excludedRoots.contains($0.path) && !kitIndexedRoots.contains($0.path) && !attemptedKitUpgradeRoots.contains($0.path) }
                guard !scanRequested.isEmpty else { return nil }
            }
            activeScan?.cancel()
            activeScan = token
            if kitsOnly { attemptedKitUpgradeRoots.formUnion(scanRequested.map(\.path)) }
            lastErrors = []
            if !kitsOnly { for root in requested { rootReports[root.path] = RootReport(path: root.path, status: "pending") } }
            if !kitsOnly {
                for root in requested where !roots.contains(where: { $0.path == root.path }) { roots.append(root) }
                for root in requested { excludedRoots.remove(root.path) }
            }
            checkedCollectionRoots = checkedCollectionRoots.filter { path in !scanRequested.contains { Self.isInside(path, root: $0.path) } }
            return ["scanning": true, "count": items.count, "roots": scanRequested.map(\.dictionary), "scanID": token.id]
        }
        guard let result else { return withKitUpgradeStatus(locked { ["scanning": activeScan != nil, "count": items.count] }) }
        emit("library.progress", result)
        let accepted = scanRequested
        scanQueue.async { [weak self] in self?.scan(accepted, token: token) }
        return withKitUpgradeStatus(result)
    }

    private func scan(_ requested: [LibraryRoot], token: ScanToken) {
        var scanned = 0
        var skipped = 0
        var errors: [String] = []
        var lastUpdate = Date.distantPast
        var lastFlush = Date()
        var visited = 0
        let resourceKeys: [URLResourceKey] = [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey, .contentModificationDateKey, .isHiddenKey]
        // Scan nested roots only once. Keep the parent so pack inference remains stable.
        let sorted = requested.sorted { $0.path.count < $1.path.count }
        var scanRoots: [LibraryRoot] = []
        for root in sorted where !scanRoots.contains(where: { Self.isInside(root.path, root: $0.path) }) { scanRoots.append(root) }

        for root in scanRoots {
            if token.cancelled { break }
            var rootReport = RootReport(path: root.path, status: "scanning")
            locked { if activeScan === token && !token.kitsOnly { rootReports[root.path] = rootReport } }
            progress(token: token, root: root.path, scanned: scanned, skipped: skipped, errors: errors)
            func rootFailure(_ message: String) {
                if rootReport.errors.count < 6 { rootReport.errors.append(message) }
                if errors.count < 30 { errors.append(message) }
            }
            func publishRoot(_ status: String) {
                rootReport.status = status
                locked { if activeScan === token { if !token.kitsOnly { rootReports[root.path] = rootReport }; lastErrors = errors } }
                progress(token: token, root: root.path, scanned: scanned, skipped: skipped, errors: errors)
            }
            var directory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: root.path, isDirectory: &directory), directory.boolValue else {
                rootFailure("Library is offline or unavailable: \(root.path)")
                publishRoot("error")
                continue // Preserve cached entries for disconnected external disks.
            }
            let topLevelCount: Int
            do { topLevelCount = try FileManager.default.contentsOfDirectory(atPath: root.path).count }
            catch {
                rootFailure("Cannot read \(root.path): \(Self.describe(error)). Choose this folder again to grant access.")
                publishRoot("error")
                continue
            }
            var seen = Set<String>()
            var batch: [Item] = []
            // Some sample expansions carry package metadata. Descend them, excluding only software bundles below.
            guard let enumerator = FileManager.default.enumerator(at: URL(fileURLWithPath: root.path, isDirectory: true), includingPropertiesForKeys: resourceKeys, options: [.skipsHiddenFiles], errorHandler: { url, error in
                rootFailure("Cannot enumerate \(url.path): \(Self.describe(error))")
                return !token.cancelled
            }) else {
                rootFailure("Cannot enumerate library folder: \(root.path)")
                publishRoot("error")
                continue
            }
            while let url = enumerator.nextObject() as? URL {
                if token.cancelled { break }
                visited += 1
                rootReport.visited += 1
                // A background bulk scan must leave CPU and filesystem bandwidth for playback.
                if visited % 256 == 0 { Thread.sleep(forTimeInterval: 0.008) }
                autoreleasepool {
                    let filename = url.lastPathComponent
                    if filename.hasPrefix(".") || filename.hasPrefix("._") || Self.ignoredDirectories.contains(filename.lowercased()) {
                        enumerator.skipDescendants(); skipped += 1; rootReport.skipped += 1; return
                    }
                    let values: URLResourceValues
                    do { values = try url.resourceValues(forKeys: Set(resourceKeys)) }
                    catch { rootFailure("Cannot inspect \(url.path): \(Self.describe(error))"); skipped += 1; rootReport.skipped += 1; return }
                    if values.isSymbolicLink == true { enumerator.skipDescendants(); skipped += 1; rootReport.skipped += 1; return }
                    let ext = url.pathExtension.lowercased()
                    let kitPackage = values.isDirectory == true && ext == "stkit"
                    if kitPackage { enumerator.skipDescendants() }
                    else if values.isDirectory == true {
                        if Self.softwarePackages.contains(url.pathExtension.lowercased()) { enumerator.skipDescendants(); skipped += 1; rootReport.skipped += 1 }
                        return
                    }
                    guard kitPackage || values.isRegularFile == true else { skipped += 1; rootReport.skipped += 1; return }
                    if token.kitsOnly && !KitLibrary.extensions.contains(ext) { return }
                    guard Self.sampleExtensions.contains(ext) || Self.nksExtensions.contains(ext) || KitLibrary.extensions.contains(ext) else { return }
                    // The enumerator already supplies absolute normalized URLs; avoid resolving file symlinks.
                    let path = url.standardizedFileURL.path
                    // Only the small package header affects the index. Never index/decode collected
                    // media or read opaque project state while scanning a .stkit package.
                    let headerValues = kitPackage ? try? url.appendingPathComponent("kit.json").resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey]) : nil
                    let size = Int64(kitPackage ? max(1, headerValues?.fileSize ?? 1) : values.fileSize ?? 0)
                    let modified = (headerValues?.contentModificationDate ?? values.contentModificationDate)?.timeIntervalSince1970 ?? 0
                    guard size > 0 else { skipped += 1; rootReport.skipped += 1; return }
                    seen.insert(path)
                    if let previous = locked({ items[path] }), previous.size == size, previous.modified == modified {
                        batch.append(previous)
                    } else {
                        batch.append(Self.makeItem(url: url, root: root, size: size, modified: modified))
                    }
                    scanned += 1
                    rootReport.matched += 1
                }
                if batch.count >= 512 || Date().timeIntervalSince(lastFlush) > 0.25 {
                    flush(batch, token: token)
                    batch.removeAll(keepingCapacity: true)
                    lastFlush = Date()
                }
                if Date().timeIntervalSince(lastUpdate) >= 1.0 {
                    locked { if activeScan === token && !token.kitsOnly { rootReports[root.path] = rootReport } }
                    progress(token: token, root: root.path, scanned: scanned, skipped: skipped, errors: errors)
                    lastUpdate = Date()
                }
            }
            flush(batch, token: token)
            if !token.cancelled {
                locked {
                    guard activeScan === token else { return }
                    // Only remove stale entries when this root was read successfully in full.
                    // Permission-denied subfolders keep their cached entries until a clean rescan.
                    if rootReport.errors.isEmpty {
                        let stale = items.keys.filter { Self.isInside($0, root: root.path) && !seen.contains($0) && (!token.kitsOnly || items[$0]?.kind == "kit") }
                        for path in stale { items.removeValue(forKey: path) }
                        if !stale.isEmpty { indexRevision &+= 1 }
                        // Nested roots were traversed by the parent too. Never mark
                        // cancelled, inaccessible or only partially enumerated roots.
                        kitIndexedRoots.formUnion(roots.map(\.path).filter { Self.isInside($0, root: root.path) })
                    }
                }
            }
            if !token.cancelled {
                if rootReport.matched == 0 && rootReport.errors.isEmpty {
                    if !token.kitsOnly { rootReport.errors.append("No supported audio, NKS or kit files were found in \(root.path). Top-level entries: \(topLevelCount); entries inspected: \(rootReport.visited).") }
                    publishRoot("empty")
                } else { publishRoot(rootReport.errors.isEmpty ? "complete" : "error") }
            } else { publishRoot("cancelled") }
        }
        let result: [String: Any]? = locked {
            guard activeScan === token else { return nil }
            activeScan = nil
            lastErrors = errors
            return ["scanning": false, "count": items.count, "scanned": scanned, "skipped": skipped,
                    "errors": errors, "rootReports": rootReports.values.map(\.dictionary), "cancelled": token.cancelled, "scanID": token.id, "root": ""]
        }
        if let result = result {
            persistCache()
            persistDiagnostics()
            emit("library.progress", result)
            emit("library.results", result)
            // Library folders can contain NKS presets; refresh the catalogue once it is in use.
            if !token.kitsOnly && nksCatalog.isActive { nksCatalog.refresh() }
            if !token.cancelled { scheduleKitIndexUpgrade() }
        }
    }

    private func flush(_ batch: [Item], token: ScanToken) {
        guard !batch.isEmpty else { return }
        locked {
            guard activeScan === token, !token.cancelled else { return }
            var changed = false
            for item in batch {
                if let previous = items[item.path], previous === item { continue }
                items[item.path] = item.applying(metadataOverrides[item.path])
                changed = true
            }
            if changed { indexRevision &+= 1 }
        }
        queueMetadata(collections: batch.compactMap(\.collectionRoot))
    }

    private func progress(token: ScanToken, root: String, scanned: Int, skipped: Int, errors: [String]) {
        let result: [String: Any]? = locked {
            guard activeScan === token else { return nil }
            lastErrors = errors
            return ["scanning": true, "count": items.count, "root": root, "scanned": scanned,
                    "skipped": skipped, "errors": errors, "rootReports": rootReports.values.map(\.dictionary), "scanID": token.id]
        }
        if let result = result { emit("library.progress", result); emit("library.results", result) }
    }

    private static func bytesContain(_ haystack: [UInt8], _ needle: [UInt8]) -> Bool {
        guard !needle.isEmpty else { return true }
        guard haystack.count >= needle.count else { return false }
        return haystack.withUnsafeBufferPointer { h in needle.withUnsafeBufferPointer { n in
            // memchr finds candidate starts quickly; memcmp confirms them.
            let first = Int32(n[0]), length = n.count, last = h.count - length
            var start = 0
            while start <= last {
                guard let hit = memchr(h.baseAddress! + start, first, last - start + 1) else { return false }
                let offset = h.baseAddress!.distance(to: hit.assumingMemoryBound(to: UInt8.self))
                if length == 1 || memcmp(h.baseAddress! + offset + 1, n.baseAddress! + 1, length - 1) == 0 { return true }
                start = offset + 1
            }
            return false
        } }
    }
    private static func bytesHavePrefix(_ value: [UInt8], _ prefix: [UInt8]) -> Bool {
        value.count >= prefix.count && value.prefix(prefix.count).elementsEqual(prefix)
    }

    private struct Query {
        var words: [String] = []
        private(set) var wordBytes: [[UInt8]] = []
        var filters: [(String, String)] = []
        init(_ query: String) {
            let expression = try! NSRegularExpression(pattern: "(?:[^\\s\"]+|\"[^\"]*\")+")
            let range = NSRange(query.startIndex..., in: query)
            for match in expression.matches(in: query, range: range) {
                guard let range = Range(match.range, in: query) else { continue }
                let value = LibraryService.fold(String(query[range]).replacingOccurrences(of: "\"", with: ""))
                if let colon = value.firstIndex(of: ":"), ["tag", "pack", "bpm", "key", "ext", "kind", "category"].contains(String(value[..<colon])) {
                    let field = String(value[..<colon]), argument = String(value[value.index(after: colon)...])
                    filters.append((field, field == "key" ? LibraryService.keySlot(argument) : argument))
                } else if !value.isEmpty { words.append(value) }
            }
            wordBytes = words.map { Array($0.utf8) }
        }
        func matches(_ item: Item) -> Bool {
            guard wordBytes.allSatisfy({ LibraryService.bytesContain(item.searchBytes, $0) }) else { return false }
            return filters.allSatisfy { field, value in
                switch field {
                case "pack": return item.foldedPack.contains(value)
                case "tag": return item.foldedTags.contains(value)
                case "category": return item.foldedCategory.contains(value)
                case "kind": return item.kind == value || (value == "preset" && item.kind == "nks")
                case "key": return item.foldedKey == value
                case "ext": return item.fileExtension == value.trimmingCharacters(in: CharacterSet(charactersIn: "."))
                case "bpm":
                    guard let bpm = item.bpm else { return false }
                    let bounds = value.split(separator: "-").compactMap { Double($0) }
                    if bounds.count == 2 { return bpm >= bounds[0] && bpm <= bounds[1] }
                    return bounds.first.map { abs(bpm - $0) < 0.5 } ?? false
                default: return true
                }
            }
        }
    }

    private func cancelledSearch() -> [String: Any] {
        locked { ["items": [], "total": 0, "scanning": activeScan != nil, "cancelled": true, "categories": [], "packs": []] }
    }

    /// At most one active computation and one pending request. Superseded callers are always resumed.
    private func enqueueSearch(_ payload: [String: Any], completion: @escaping ([String: Any]) -> Void) {
        var displaced: SearchRequest?
        let startWorker = locked { () -> Bool in
            searchGeneration &+= 1
            displaced = pendingSearch
            pendingSearch = SearchRequest(generation: searchGeneration, payload: payload, completion: completion)
            if searchWorkerScheduled { return false }
            searchWorkerScheduled = true
            return true
        }
        displaced?.completion(cancelledSearch())
        if startWorker {
            searchQueue.async { [self] in
                while true {
                    let request = locked { () -> SearchRequest? in
                        guard let request = pendingSearch else { searchWorkerScheduled = false; return nil }
                        pendingSearch = nil
                        return request
                    }
                    guard let request else { break }
                    locked { runningSearchComputations += 1; peakSearchComputations = max(peakSearchComputations, runningSearchComputations) }
                    let result = autoreleasepool { search(request.payload, generation: request.generation) }
                    locked { runningSearchComputations -= 1 }
                    request.completion(result)
                }
            }
        }
    }

    private struct RankedItem { let item: Item; let score: Int }
    private static func ranksBefore(_ left: RankedItem, _ right: RankedItem) -> Bool {
        if left.score != right.score { return left.score > right.score }
        let order = compareBytes(left.item.foldedNameBytes, right.item.foldedNameBytes)
        if order != 0 { return order < 0 }
        return left.item.path < right.item.path
    }

    private static func compareBytes(_ left: [UInt8], _ right: [UInt8]) -> Int32 {
        left.withUnsafeBufferPointer { l in right.withUnsafeBufferPointer { r in
            let common = min(l.count, r.count)
            let order = common == 0 ? 0 : memcmp(l.baseAddress!, r.baseAddress!, common)
            return order != 0 ? order : Int32(l.count) - Int32(r.count)
        } }
    }

    /// Selects a page in linear average time; only the visible page is sorted.
    private static func partition(_ values: inout [RankedItem], at target: Int, upper: Int? = nil, cancelled: () -> Bool) -> Bool {
        guard target >= 0 && target < values.count else { return true }
        var low = 0, high = upper ?? (values.count - 1), comparisons = 0
        while low < high {
            if cancelled() { return false }
            let pivot = values[(low + high) / 2]
            var left = low, right = high
            while left <= right {
                while left <= high && ranksBefore(values[left], pivot) { left += 1 }
                while right >= low && ranksBefore(pivot, values[right]) { right -= 1 }
                if left <= right { values.swapAt(left, right); left += 1; right -= 1 }
                comparisons += 1
                if comparisons % 256 == 0 && cancelled() { return false }
            }
            if target <= right { high = right }
            else if target >= left { low = left }
            else { break }
        }
        return true
    }

    /// Append-only string table shared by all index snapshots; facet counting uses integer slots.
    private final class Interner: @unchecked Sendable {
        private let lock = NSLock()
        private var ids: [String: Int32] = [:]
        private var values: [String] = []
        private var folded: [String] = []
        private let normalize: (String) -> String
        init(normalize: @escaping (String) -> String = LibraryService.fold) { self.normalize = normalize }
        func id(_ value: String) -> Int32 {
            lock.lock(); defer { lock.unlock() }
            if let id = ids[value] { return id }
            let id = Int32(values.count)
            ids[value] = id; values.append(value); folded.append(normalize(value))
            return id
        }
        func snapshot() -> (values: [String], folded: [String]) { lock.lock(); defer { lock.unlock() }; return (values, folded) }
    }
    private static let facetInterners: [Interner] = (0..<8).map { index in
        index == 4 ? Interner(normalize: { $0 == "unknown" ? $0 : LibraryService.keySlot($0) }) : Interner()
    }
    private static let collectionRootInterner = Interner()
    private static let collectionNames = ["Loopcloud", "Maschine expansions", "Native Instruments", "User libraries"]

    private static let facetNames = ["categories", "collections", "roots", "packs", "keys", "sampleTypes", "extensions", "tags", "bpm", "duration"]
    private struct NumericFilter {
        let min: Double?
        let max: Double?
        let includeUnknown: Bool
        var active: Bool { min != nil || max != nil || includeUnknown }
        init(_ value: Any?) {
            let data = value as? [String: Any] ?? [:]
            func number(_ key: String) -> Double? {
                guard let value = (data[key] as? NSNumber)?.doubleValue, value.isFinite else { return nil }
                return Swift.max(0, value)
            }
            min = number("min"); max = number("max"); includeUnknown = data["includeUnknown"] as? Bool ?? false
        }
        func matches(_ value: Double?) -> Bool {
            guard active else { return true }
            guard let value else { return includeUnknown }
            if min == nil && max == nil { return false } // An unknown-only selection.
            return (min == nil || value >= min!) && (max == nil || value <= max!)
        }
        var cacheKey: String { "\(min.map(String.init(describing:)) ?? "")/\(max.map(String.init(describing:)) ?? "")/\(includeUnknown)" }
    }
    private struct Filters {
        let values: [Set<String>]
        let bpm: NumericFilter
        let duration: NumericFilter
        let active: Bool
        let cacheKey: String
        init(_ payload: [String: Any]) {
            let data = payload["filters"] as? [String: Any] ?? [:]
            var arrays = LibraryService.facetNames.prefix(8).map { key -> Set<String> in
                let raw = data[key] as? [String] ?? []
                return Set(raw.prefix(128).map { LibraryService.fold(String($0.prefix(2048))) }.filter { !$0.isEmpty })
            }
            if let sources = data["sources"] as? [String] { arrays[1].formUnion(sources.prefix(128).map(LibraryService.fold)) }
            if arrays[0].isEmpty, let category = payload["category"] as? String, !category.isEmpty, category != "all" { arrays[0].insert(LibraryService.fold(category)) }
            if arrays[3].isEmpty, let pack = payload["pack"] as? String, !pack.isEmpty, pack != "all" { arrays[3].insert(LibraryService.fold(pack)) }
            arrays[4] = Set(arrays[4].map { $0 == "unknown" ? $0 : LibraryService.keySlot($0) })
            values = arrays
            bpm = NumericFilter(data["bpm"]); duration = NumericFilter(data["duration"])
            active = arrays.contains { !$0.isEmpty } || bpm.active || duration.active
            let encoded = (try? JSONSerialization.data(withJSONObject: arrays.map { $0.sorted() })) ?? Data()
            cacheKey = String(decoding: encoded, as: UTF8.self) + bpm.cacheKey + ";" + duration.cacheKey
        }
        /// Resolves folded selections to interned slots once per search instead of hashing strings per item.
        func slotSets(_ tables: [(values: [String], folded: [String])]) -> [Set<Int32>?] {
            (0..<8).map { field in
                guard field != 1, !values[field].isEmpty else { return nil }
                let folded = tables[field].folded
                return Set(folded.indices.filter { values[field].contains(folded[$0]) }.map { Int32($0) })
            }
        }
        func failureMask(_ item: Item, collection: Int, slots: [Set<Int32>?]) -> Int {
            guard active else { return 0 }
            var result = 0
            for field in [0, 2, 3, 4, 5, 6] { if let set = slots[field], !set.contains(item.facetIDs[field]) { result |= 1 << field } }
            if !values[1].isEmpty && !values[1].contains(LibraryService.foldedCollectionNames[collection]) { result |= 1 << 1 }
            if let set = slots[7], !item.tagIDs.contains(where: set.contains) { result |= 1 << 7 }
            if !bpm.matches(item.bpm) { result |= 1 << 8 }
            if !duration.matches(item.duration) { result |= 1 << 9 }
            return result
        }
    }
    private static let foldedCollectionNames = collectionNames.map(fold)

    private func search(_ payload: [String: Any], generation: UInt64? = nil) -> [String: Any] {
        func cancelled() -> Bool { generation.map { expected in locked { searchGeneration != expected } } ?? false }
        if cancelled() { return cancelledSearch() }
        refreshAvailability()
        scheduleKitIndexUpgrade()
        let snapshot = locked { (Array(items.values), favorites, activeScan != nil, lastErrors, indexRevision, rootReports.values.map(\.dictionary), verifiedExpansionRoots, queuedDurations, roots) }
        let offlineRoots = locked { unavailableRoots }
        let hideOffline = payload["hideOffline"] as? Bool ?? false
        let query = Query(String((payload["query"] as? String ?? "").prefix(1024)))
        let kind = payload["kind"] as? String ?? "all"
        let filters = Filters(payload)
        let onlyFavorites = payload["favorites"] as? Bool ?? false
        let offset = max(0, (payload["offset"] as? NSNumber)?.intValue ?? 0)
        let limit = min(1000, max(1, (payload["limit"] as? NSNumber)?.intValue ?? 200))
        let populationKey = "\(payload["query"] as? String ?? "")\u{1f}\(kind)\u{1f}\(filters.cacheKey)\u{1f}\(onlyFavorites)\u{1f}\(hideOffline)"
        let cacheKey = populationKey + "\u{1f}\(offset)\u{1f}\(limit)"
        if cacheKey == lastSearchKey, snapshot.4 == lastSearchRevision, var cached = lastSearchResult {
            cached["scanning"] = snapshot.2; cached["errors"] = snapshot.3; cached["rootReports"] = snapshot.5
            return cancelled() ? cancelledSearch() : withKitUpgradeStatus(cached)
        }
        var facetCounts = Array(repeating: [String: Int](), count: Self.facetNames.count)
        var durationKnown = 0, durationUnknown = 0, durationPending = 0
        var eligiblePending: Set<String> = []
        var durationCandidates: [String] = []
        var offlineMatches: [String: Int] = [:]
        var matching: [RankedItem] = []
        matching.reserveCapacity(min(4096, snapshot.0.count))
        if let cached = lastPopulation, cached.key == populationKey, cached.revision == snapshot.4 {
            matching = cached.matching; facetCounts = cached.facetCounts
            durationKnown = cached.durationKnown; durationUnknown = cached.durationUnknown
            eligiblePending = cached.eligiblePending.intersection(snapshot.7)
            durationPending = eligiblePending.count
            durationCandidates = cached.durationCandidates.filter { !snapshot.7.contains($0) }
            offlineMatches = cached.offlineMatches
        } else {
          let tables = Self.facetInterners.map { $0.snapshot() }
          let slots = filters.slotSets(tables)
          let verified = Set(snapshot.6.map { Self.collectionRootInterner.id($0) })
          var slotCounts = tables.map { [Int](repeating: 0, count: $0.values.count) }
          var collectionCounts = [0, 0, 0, 0], bpmCounts = [0, 0], durationCounts = [0, 0]
          let hasQueuedDurations = !snapshot.7.isEmpty
          let offlineRootIDs = Set(offlineRoots.map { Self.facetInterners[2].id($0) })
          func bump(_ field: Int, _ id: Int32) {
              let index = Int(id)
              if index >= slotCounts[field].count { slotCounts[field] += [Int](repeating: 0, count: index - slotCounts[field].count + 1) }
              slotCounts[field][index] += 1
          }
          for (index, item) in snapshot.0.enumerated() {
            if index % 256 == 0 && cancelled() { return cancelledSearch() }
            // The existing All sounds view assumes sample/NKS rows. Kits have their own
            // explicit view and activation path, so never offer them as playable samples.
            guard (((kind == "all" || kind.isEmpty) && item.kind != "kit") || item.kind == kind),
                  (!onlyFavorites || snapshot.1.contains(item.path)), query.matches(item) else { continue }
            let collection = item.collectionIndex(verified: verified)
            let failures = filters.failureMask(item, collection: collection, slots: slots)
            if !offlineRootIDs.isEmpty && offlineRootIDs.contains(item.facetIDs[2]) {
                if failures == 0 { offlineMatches[item.root, default: 0] += 1 }
                if hideOffline { continue }
            }
            // Disjunctive facets: count an option when all filters except its own match.
            if failures == 0 || failures.nonzeroBitCount == 1 {
                for field in [0, 2, 3, 4, 5, 6] where failures & ~(1 << field) == 0 { bump(field, item.facetIDs[field]) }
                if failures & ~(1 << 1) == 0 { collectionCounts[collection] += 1 }
                if failures & ~(1 << 7) == 0 { for tag in item.tagIDs { bump(7, tag) } }
                if failures & ~(1 << 8) == 0 { bpmCounts[item.bpm == nil ? 1 : 0] += 1 }
                if failures & ~(1 << 9) == 0 { durationCounts[item.duration == nil ? 1 : 0] += 1 }
            }
            if failures & ~(1 << 9) == 0 {
                if item.duration != nil { durationKnown += 1 } else { durationUnknown += 1 }
                let queued = hasQueuedDurations && snapshot.7.contains(item.path)
                if queued { durationPending += 1; eligiblePending.insert(item.path) }
                if filters.duration.active, item.kind == "sample", item.durationChecked != true, !queued, durationCandidates.count < 32, !offlineRootIDs.contains(item.facetIDs[2]) { durationCandidates.append(item.path) }
            }
            guard failures == 0 else { continue }
            let score = query.wordBytes.reduce(0) { $0 + (Self.bytesHavePrefix(item.foldedNameBytes, $1) ? 2 : Self.bytesContain(item.foldedNameBytes, $1) ? 1 : 0) }
            matching.append(RankedItem(item: item, score: score))
          }
          let names = Self.facetInterners.map { $0.snapshot().values }
          for field in [0, 2, 3, 4, 5, 6, 7] {
              for (slot, count) in slotCounts[field].enumerated() where count > 0 && slot < names[field].count { facetCounts[field][names[field][slot], default: 0] += count }
          }
          for (index, count) in collectionCounts.enumerated() where count > 0 { facetCounts[1][Self.collectionNames[index]] = count }
          for (index, name) in ["known", "unknown"].enumerated() {
              if bpmCounts[index] > 0 { facetCounts[8][name] = bpmCounts[index] }
              if durationCounts[index] > 0 { facetCounts[9][name] = durationCounts[index] }
          }
          if cancelled() { return cancelledSearch() }
          lastPopulation = SearchPopulation(key: populationKey, revision: snapshot.4, matching: matching,
              facetCounts: facetCounts, durationKnown: durationKnown, durationUnknown: durationUnknown,
              eligiblePending: eligiblePending, durationCandidates: durationCandidates, offlineMatches: offlineMatches)
        }
        let start = min(offset, matching.count), end = min(start + limit, matching.count)
        if end < matching.count && !Self.partition(&matching, at: end, cancelled: cancelled) { return cancelledSearch() }
        if start > 0 && !Self.partition(&matching, at: start, upper: end - 1, cancelled: cancelled) { return cancelledSearch() }
        let visible = matching[start..<end].sorted(by: Self.ranksBefore)
        if !filters.duration.active {
            durationCandidates = visible.map(\.item).filter { $0.kind == "sample" && $0.durationChecked != true && !snapshot.7.contains($0.path) && !offlineRoots.contains($0.root) }.prefix(32).map(\.path)
        }
        let pending = snapshot.7.union(durationCandidates)
        let page = visible.map { ranked -> [String: Any] in
            var value = ranked.item.dictionary(favorites: snapshot.1, verifiedCollections: snapshot.6, pendingDurations: pending)
            if offlineRoots.contains(ranked.item.root) { value["available"] = false; value["volume"] = Self.volumeName(ranked.item.root) ?? "" }
            return value
        }
        let rootLabels = Dictionary(snapshot.8.map { ($0.path, $0.name) }, uniquingKeysWith: { first, _ in first })
        var facets: [String: [[String: Any]]] = [:]
        for field in 0..<Self.facetNames.count {
            let counts = facetCounts[field]
            // Bound bridge/UI payload even when user tags contain thousands of distinct values.
            let selected = field < 8 ? filters.values[field] : []
            // Fold each value once; comparators must not fold strings O(n log n) times.
            let foldForField: (String) -> String = field == 4 ? { $0 == "unknown" ? $0 : Self.keySlot($0) } : Self.fold
            let keyed = counts.map { (value: $0.key, count: $0.value, selected: !selected.isEmpty && selected.contains(foldForField($0.key))) }
            let large = counts.count > 1000
            let values = keyed.sorted {
                if $0.selected != $1.selected { return $0.selected }
                if large, $0.count != $1.count { return $0.count > $1.count }
                return $0.value < $1.value
            }.prefix(1000).map(\.value)
            facets[Self.facetNames[field]] = values.map { value in
                ["value": value, "name": value, "label": field == 2 ? rootLabels[value] ?? value : value == "unknown" ? "Unknown" : value, "count": counts[value]!]
            }
        }
        facets["sources"] = facets["collections"]
        let result: [String: Any] = ["items": page, "total": matching.count, "scanning": snapshot.2,
                "categories": facets["categories"] ?? [], "packs": facets["packs"] ?? [], "facets": facets,
                "facetsTruncated": Self.facetNames.enumerated().filter { facetCounts[$0.offset].count > 1000 }.map(\.element),
                "durationCoverage": ["known": durationKnown, "unknown": durationUnknown, "pending": durationPending + durationCandidates.count],
                "offset": offset, "limit": limit, "errors": snapshot.3, "rootReports": snapshot.5,
                "offline": offlineRoots.sorted().map { path -> [String: Any] in
                    ["path": path, "name": rootLabels[path] ?? (path as NSString).lastPathComponent, "volume": Self.volumeName(path) ?? "", "matching": offlineMatches[path] ?? 0]
                }, "hideOffline": hideOffline]
        if cancelled() { return cancelledSearch() }
        lastPopulation?.eligiblePending.formUnion(durationCandidates)
        queueMetadata(durations: durationCandidates)
        lastSearchKey = cacheKey; lastSearchRevision = snapshot.4; lastSearchResult = result
        return withKitUpgradeStatus(result)
    }

    private static func makeItem(url: URL, root: LibraryRoot, size: Int64, modified: Double) -> Item {
        let item = inferredItem(path: url.standardizedFileURL.path, root: root, size: size, modified: modified)
        if item.kind == "nks", let parsed = try? NKSMetadata(url: url).dictionary {
            if let name = parsed["name"] as? String, !name.isEmpty { item.name = name }
            let categories = parsed["categories"] as? [String] ?? []
            let tags = parsed["tags"] as? [String] ?? []
            // The full NKS type path stays searchable as a tag; the category joins the sample categories.
            item.tags = Array(Set(item.tags + categories + tags)).sorted()
            if let first = categories.first, !first.isEmpty {
                item.category = nksCategory(first)
                item.tags.removeAll { $0 == "Other" }
            }
            item.previewPath = parsed["previewPath"] as? String
            item.metadataSource = "NKS metadata and folders"
        }
        if item.kind == "kit" {
            do {
                let parsed = try KitLibrary.metadata(at: url, root: URL(fileURLWithPath: root.path))
                if let name = parsed["name"] as? String, !name.isEmpty { item.name = name }
                if let type = parsed["type"] as? String, !type.isEmpty { item.category = type }
                // A SuperTracker kit's tags are its own; words guessed from its folders ("Drums" from "Breaks") are not.
                let tags = parsed["tags"] as? [String] ?? []
                item.tags = url.pathExtension.lowercased() == "stkit" ? tags : Array(Set(item.tags + tags)).sorted()
                if let expansion = parsed["expansion"] as? String, !expansion.isEmpty { item.pack = cleanPack(expansion) }
                item.previewPath = parsed["previewPath"] as? String
                item.kitSoundCount = parsed["soundCount"] as? Int
                item.metadataSource = parsed["metadataSource"] as? String ?? item.metadataSource
            } catch { item.kitMetadataError = error.localizedDescription }
        }
        item.prepareSearchFields()
        return item
    }

    /// Pure path inference: no file IO. Also used to upgrade cached records when inference improves.
    private static func inferredItem(path: String, root: LibraryRoot, size: Int64, modified: Double) -> Item {
        let relative = String(path.dropFirst(min(path.count, root.path.count))).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let hierarchy = relative.split(separator: "/").map(String.init)
        let fileName = (path as NSString).lastPathComponent
        let name = (fileName as NSString).deletingPathExtension
        let nks = nksExtensions.contains((fileName as NSString).pathExtension.lowercased())
        let kit = KitLibrary.extensions.contains((fileName as NSString).pathExtension.lowercased())
        let pack = cleanPack(hierarchy.count > 1 ? hierarchy[0] : root.name)
        let metadata = inferMetadata(name: name, hierarchy: hierarchy)
        // Preset names are not musical keys ("A Fading Dream").
        return Item(path: path, root: root.path, name: name, kind: kit ? "kit" : nks ? "nks" : "sample", category: kit ? "Kit" : metadata.category,
                    tags: metadata.tags, pack: pack, bpm: metadata.bpm, key: nks || kit ? nil : metadata.key,
                    size: size, modified: modified, metadataSource: "filename and folders", previewPath: nil)
    }

    /// Display pack names: drop NI's " Library" suffix and Loopcloud's numeric "[12345]" pack IDs.
    private static func cleanPack(_ value: String) -> String {
        var result = value.replacingOccurrences(of: " Library", with: "")
        result = result.replacingOccurrences(of: "\\s*\\[\\d{2,7}\\]\\s*$", with: "", options: .regularExpression)
        result = result.replacingOccurrences(of: "\\s{2,}", with: " ", options: .regularExpression).trimmingCharacters(in: .whitespaces)
        return result.isEmpty ? value : result
    }

    /// Maps NKS "Type / Subtype" paths onto the sample categories so one category list covers both.
    private static func nksCategory(_ value: String) -> String {
        let top = value.components(separatedBy: " / ").first?.trimmingCharacters(in: .whitespaces) ?? value
        let map: [String: String] = ["Synth Lead": "Synth", "Synth Misc": "Synth", "Synth Pad": "Pad & Atmosphere", "Soundscapes": "Pad & Atmosphere",
                                     "Piano / Keys": "Keys", "Piano": "Keys", "Organ": "Keys", "Bowed Strings": "Strings", "Brass": "Brass & Winds",
                                     "Flute": "Brass & Winds", "Reed Instruments": "Brass & Winds", "Sound Effects": "FX", "Mallet Instruments": "Mallets & Bells",
                                     "Plucked Strings": "Guitar & Plucked", "Guitar": "Guitar & Plucked", "Vocal": "Vocals", "Vocals": "Vocals", "Drums": "Drums",
                                     "Percussion": "Percussion", "Bass": "Bass", "Ethnic World": "World", "Multitrack": "Combinations"]
        if value.hasPrefix("Piano / Keys") { return "Keys" }
        return map[top] ?? (categoryRules.contains { $0.0 == top } ? top : top.isEmpty ? "Other" : top)
    }

    /// Canonical sharp-based key names ("A# minor", "C major"); bare note names are major, as in NI and Loopcloud filenames.
    static func canonicalKey(_ text: String) -> String? {
        let value = text.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "♯", with: "#").replacingOccurrences(of: "♭", with: "b")
        guard let match = keyTextPattern.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)),
              let letterRange = Range(match.range(at: 1), in: value) else { return nil }
        let accidental = Range(match.range(at: 2), in: value).map { String(value[$0]).lowercased() } ?? ""
        let quality = Range(match.range(at: 3), in: value).map { String(value[$0]).lowercased() } ?? ""
        let base = ["c": 0, "d": 2, "e": 4, "f": 5, "g": 7, "a": 9, "b": 11][value[letterRange].lowercased()]!
        let pitch = (base + (accidental == "#" ? 1 : accidental == "b" ? 11 : 0)) % 12
        let names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
        return names[pitch] + (quality == "m" || quality.hasPrefix("min") ? " minor" : " major")
    }
    private static let keyTextPattern = try! NSRegularExpression(pattern: "^([A-Ga-g])([#b]?)\\s*(major|minor|maj|min|m)?$", options: [.caseInsensitive])
    /// Comparable key slot for filters: enharmonic spellings share one value.
    private static func keySlot(_ text: String) -> String {
        fold(canonicalKey(text) ?? text).replacingOccurrences(of: " ", with: "")
    }

    private static func inferSampleType(name: String, path: String, root: String, kind: String, filenameTempo: Bool = false) -> String {
        guard kind == "sample" else { return "unknown" }
        let parent = (path as NSString).deletingLastPathComponent
        let relative = String(parent.dropFirst(min(root.count, parent.count)))
        let folders = relative.split(separator: "/").reversed().map(String.init) + [(root as NSString).lastPathComponent]
        for part in [name] + folders {
            let words = fold(part).components(separatedBy: CharacterSet.alphanumerics.inverted).filter { !$0.isEmpty }
            let tokens = Set(words)
            if tokens.contains("oneshot") || tokens.contains("oneshots") || (tokens.contains("one") && (tokens.contains("shot") || tokens.contains("shots"))) { return "oneshot" }
            if tokens.contains("loop") || tokens.contains("loops") { return "loop" }
        }
        // Maschine expansion layout: Samples/Drums/<type> holds drum hits, Samples/Instruments holds playable notes.
        let components = path.split(separator: "/")
        if let samples = components.lastIndex(of: "Samples"), samples + 2 < components.count,
           ["Drums", "Instruments", "Insruments"].contains(String(components[samples + 1])) { return "oneshot" }
        // Library filenames carry a tempo for loops (e.g. Loopcloud "_120_"), not for one-shots.
        if filenameTempo { return "loop" }
        return "unknown" // A kick or short file is not sufficient evidence of a one-shot.
    }

    private static func baseCollection(path: String) -> String {
        let folded = path.lowercased()
        if folded.split(separator: "/").contains("loopcloud") { return "Loopcloud" }
        if folded.contains("/native access content/") || folded.contains("/native instruments/") || folded.contains("/native instruments user content/") { return "Native Instruments" }
        return "User libraries"
    }

    private static func collectionRoot(path: String, root: String, collection: String) -> String? {
        guard collection != "Loopcloud" else { return nil }
        // A folder containing Groups, Sounds and Samples is verified asynchronously as a Maschine expansion.
        // These explicit components make collection detection useful for user-added expansion folders too.
        let components = path.split(separator: "/").map(String.init)
        if let marker = components.lastIndex(where: { ["Samples", "Sounds", "Groups"].contains($0) }), marker > 1 {
            let candidate = "/" + components[..<marker].joined(separator: "/")
            if isInside(candidate, root: root) { return candidate }
        }
        let relative = String(path.dropFirst(min(root.count, path.count))).split(separator: "/")
        if collection == "Native Instruments", relative.count > 1, let first = relative.first { return root + "/" + first }
        return root
    }

    /// Work is serialized, bounded and lower priority than realtime audio. Duration is requested only
    /// for visible results or a small batch eligible for the active duration filter, never the full index.
    private func queueMetadata(durations: [String] = [], collections: [String] = []) {
        let schedule = locked { () -> Bool in
            for path in durations.prefix(32) where queuedDurations.count < 128 {
                guard let item = items[path], item.kind == "sample", item.durationChecked != true else { continue }
                if queuedDurations.insert(path).inserted { pendingDurations.append(path) }
            }
            for path in collections where queuedCollections.count < 4096 && !checkedCollectionRoots.contains(path) {
                if queuedCollections.insert(path).inserted { pendingCollections.append(path) }
            }
            guard !metadataWorkerScheduled, !pendingDurations.isEmpty || !pendingCollections.isEmpty else { return false }
            metadataWorkerScheduled = true
            return true
        }
        guard schedule else { return }
        metadataQueue.async { [weak self] in self?.processMetadata() }
    }

    /// A user edit replaces the immutable indexed item while its header may be in flight.
    /// Publish onto the latest labels only when the source fingerprint still matches.
    private func completeDuration(path: String, source: Item, duration: Double?) -> Int {
        locked {
            queuedDurations.remove(path)
            guard let latest = items[path] else { return 0 }
            if latest.size == source.size && latest.modified == source.modified && latest.kind == source.kind {
                items[path] = latest.replacingDuration(duration)
            } else if latest.kind == "sample", latest.durationChecked != true {
                queuedDurations.insert(path)
                pendingDurations.append(path)
            }
            // Queue changes invalidate cached pending status even without a new duration.
            indexRevision &+= 1
            return 1
        }
    }

    private func processMetadata() {
        var changed = 0
        var lastEvent = Date()
        while true {
            let job = locked { () -> (String, Item?)? in
                while !pendingDurations.isEmpty {
                    let path = pendingDurations.removeFirst()
                    if let item = items[path] { return (path, item) }
                    queuedDurations.remove(path)
                }
                if !pendingCollections.isEmpty { return (pendingCollections.removeFirst(), nil) }
                metadataWorkerScheduled = false
                return nil
            }
            guard let (path, item) = job else { break }
            autoreleasepool {
                if let item {
                    // A missing file (ejected drive) is not evidence of an unreadable header; retry later.
                    if !FileManager.default.fileExists(atPath: item.path) {
                        locked { queuedDurations.remove(path); indexRevision &+= 1 }
                        changed += 1
                        return
                    }
                    let duration = Self.headerDuration(item)
                    changed += completeDuration(path: path, source: item, duration: duration)
                } else {
                    let fm = FileManager.default
                    var directory: ObjCBool = false
                    let available = fm.fileExists(atPath: path, isDirectory: &directory) && directory.boolValue
                    let expansion = available && ["Groups", "Sounds", "Samples"].allSatisfy { component in
                        var childDirectory: ObjCBool = false
                        return fm.fileExists(atPath: path + "/" + component, isDirectory: &childDirectory) && childDirectory.boolValue
                    }
                    locked {
                        queuedCollections.remove(path); checkedCollectionRoots.insert(path)
                        if expansion {
                            if verifiedExpansionRoots.insert(path).inserted { indexRevision &+= 1; changed += 1 }
                        } else if available, verifiedExpansionRoots.remove(path) != nil { indexRevision &+= 1; changed += 1 }
                    }
                }
            }
            if changed > 0, Date().timeIntervalSince(lastEvent) >= 0.5 {
                emit("library.metadata", locked { ["changed": changed, "pending": queuedDurations.count, "collectionsPending": queuedCollections.count] })
                changed = 0; lastEvent = Date()
                scheduleMetadataPersistence()
            }
            Thread.sleep(forTimeInterval: 0.004)
        }
        if changed > 0 {
            emit("library.metadata", locked { ["changed": changed, "pending": queuedDurations.count, "collectionsPending": queuedCollections.count] })
            scheduleMetadataPersistence()
        }
    }

    private static func headerDuration(_ item: Item) -> Double? {
        let url = URL(fileURLWithPath: item.path)
        func unchanged() -> Bool {
            guard let values = try? FileManager.default.attributesOfItem(atPath: item.path),
                  let size = values[.size] as? NSNumber, let modified = values[.modificationDate] as? Date else { return false }
            return size.int64Value == item.size && modified.timeIntervalSince1970 == item.modified
        }
        guard unchanged() else { return nil }
        var audioFile: AudioFileID?
        guard AudioFileOpenURL(url as CFURL, .readPermission, 0, &audioFile) == noErr, let file = audioFile else { return nil }
        defer { AudioFileClose(file) }
        var duration: Float64 = 0
        var size = UInt32(MemoryLayout<Float64>.size)
        guard AudioFileGetProperty(file, kAudioFilePropertyEstimatedDuration, &size, &duration) == noErr,
              duration.isFinite, duration >= 0, unchanged() else { return nil }
        return duration
    }

    private func scheduleMetadataPersistence() {
        // Debounce the large (tens of MB) index write until browsing settles. Header lengths are cheap to
        // re-read, so losing a few on a crash is preferable to re-encoding the index every few seconds.
        let work = DispatchWorkItem { [weak self] in self?.persistCache() }
        locked { metadataSaveWork?.cancel(); metadataSaveWork = work }
        diskQueue.asyncAfter(deadline: .now() + 45, execute: work)
    }

    /// Inference version stored in the index; bump it when categories/tempo/key/pack rules change so
    /// cached records are re-inferred from their paths on the next launch (no file IO).
    private static let inferenceVersion = 2

    private static let categoryRules: [(String, [String])] = [
        ("Kick", ["kick", "kicks", "kickdrum", "bassdrum", "bd"]),
        ("Snare", ["snare", "snares", "snr", "sd", "rim", "rimshot"]),
        ("Clap", ["clap", "claps", "snap", "snaps", "fingersnap"]),
        ("Hi-hat", ["hihat", "hihats", "closedhat", "openhat", "hat", "hats", "hh", "chh", "ohh", "closedhh", "openhh", "hhclosed", "hhopen"]),
        ("Cymbal", ["cymbal", "cymbals", "crash", "ride", "splash", "china"]),
        ("Tom", ["tom", "toms"]),
        ("Percussion", ["percussion", "perc", "percs", "conga", "congas", "bongo", "bongos", "shaker", "shakers", "tambourine", "tamb", "cowbell",
                        "triangle", "guiro", "cabasa", "woodblock", "clave", "claves", "timbale", "timbales", "djembe", "tabla", "cajon", "metallic", "wooden"]),
        ("Drums", ["drum", "drums", "break", "breaks", "breakbeat", "kit", "top", "tops", "drumloop", "groove", "grooves"]),
        ("Bass", ["bass", "sub", "reese", "basses"]),
        ("Vocals", ["vocal", "vocals", "voice", "voices", "vox", "voc", "choir", "spoken", "chant", "chants", "adlib", "adlibs", "acapella", "phrase", "phrases"]),
        ("Mallets & Bells", ["mallet", "mallets", "marimba", "vibes", "vibraphone", "glockenspiel", "glock", "xylophone", "bell", "bells", "chime", "chimes",
                             "kalimba", "celesta", "musicbox", "gamelan", "steelpan", "steeldrum"]),
        ("Keys", ["keys", "piano", "pianos", "organ", "rhodes", "clav", "clavinet", "wurly", "wurlitzer", "epiano", "harpsichord", "accordion", "melodica"]),
        ("Guitar & Plucked", ["guitar", "guitars", "gtr", "pluck", "plucks", "plucked", "harp", "lute", "koto", "sitar", "oud", "banjo", "mandolin", "ukulele", "bouzouki"]),
        ("Strings", ["strings", "string", "violin", "violins", "viola", "cello", "cellos", "orchestra", "orchestral", "ensemble"]),
        ("Brass & Winds", ["brass", "trumpet", "trumpets", "sax", "saxophone", "flute", "flutes", "winds", "woodwinds", "horn", "horns", "trombone",
                           "tuba", "clarinet", "oboe", "bassoon", "bagpipe", "whistle", "ocarina", "duduk", "shakuhachi"]),
        ("Pad & Atmosphere", ["pad", "pads", "atmosphere", "atmospheres", "atmos", "texture", "textures", "drone", "drones", "ambience", "ambient", "soundscape", "soundscapes"]),
        ("Synth", ["synth", "synths", "lead", "leads", "arp", "arps", "chord", "chords", "acid", "lick", "licks", "riff", "riffs", "seq", "sequence"]),
        ("Stabs & Hits", ["stab", "stabs", "hit", "hits", "strike", "strikes", "metal"]),
        ("FX", ["fx", "sfx", "effect", "effects", "riser", "risers", "impact", "impacts", "sweep", "sweeps", "noise", "noises", "glitch", "glitches",
                "buzz", "zap", "zaps", "crackle", "click", "clicks", "distortion", "drill", "scratch", "scratches", "blip", "blips", "blop", "laser",
                "foley", "transition", "transitions", "uplifter", "downlifter", "whoosh", "swoosh", "boom", "braam"]),
        ("Full Mix", ["fullmix", "trackstarter", "trackstarters"])
    ]
    private static let camelCaseBoundary = try! NSRegularExpression(pattern: "([a-z])([A-Z])")
    private static let drumHitCategories: Set<String> = ["Kick", "Snare", "Clap", "Hi-hat", "Cymbal", "Tom", "Percussion", "Drums"]

    // An explicit tempo: "120bpm", "120 BPM" or Maschine's "[120]".
    private static let explicitBPMPattern = try! NSRegularExpression(pattern: "(?i)(?<![0-9])([4-9][0-9]|1[0-9]{2}|2[0-5][0-9])(?:[._ ]?bpm)(?![a-z0-9])|\\[([4-9][0-9]|1[0-9]{2}|2[0-5][0-9])\\]")
    // A delimited bare number such as Loopcloud's "PFX_120_Cm_..." (validated by context below).
    private static let bareBPMPattern = try! NSRegularExpression(pattern: "(?:^|[ _-])([4-9][0-9]|1[0-9]{2}|2[0-5][0-9])(?=[ _-]|$)")
    private static let keyPattern = try! NSRegularExpression(pattern: "(?:^|[ _\\]-])([A-Ga-g])([#b]?)(?:[ _-]?(maj(?:or)?|min(?:or)?|m))(?:[ _-]|$)|(?:^|[ _\\]-])([A-G])([#b]?)(?:[ _-]|$)")
    private static let fullMixPattern = try! NSRegularExpression(pattern: "(?i)^(?:full|combo|fullmix|full[ _]mix)\\s*\\[")

    private static func inferMetadata(name: String, hierarchy: [String]) -> (category: String, tags: [String], bpm: Double?, key: String?) {
        let text = fold(([name] + hierarchy.dropLast()).joined(separator: " ")).replacingOccurrences(of: "hi-hat", with: "hihat").replacingOccurrences(of: "hi hat", with: "hihat")
        let words = Set(text.components(separatedBy: CharacterSet.alphanumerics.inverted).filter { !$0.isEmpty })
        // Filename and nearest folder are stronger signals than the expansion's name.
        // A snare in "808 Kick Collection" must still be categorized as a snare.
        var category = "Other"
        if fullMixPattern.firstMatch(in: name, range: NSRange(name.startIndex..., in: name)) != nil { category = "Full Mix" }
        else {
            for component in [name] + hierarchy.dropFirst().dropLast().reversed() {
                // Split camelCase ("BoomSnare", "ClosedHH", "HouseChords") before matching words.
                let spaced = camelCaseBoundary.stringByReplacingMatches(in: component, range: NSRange(location: 0, length: (component as NSString).length), withTemplate: "$1 $2")
                let componentText = fold(spaced).replacingOccurrences(of: "hi-hat", with: "hihat").replacingOccurrences(of: "hi hat", with: "hihat")
                let componentWords = Set(componentText.components(separatedBy: CharacterSet.letters.inverted))
                if let rule = categoryRules.first(where: { $0.1.contains(where: { componentWords.contains($0) }) }) {
                    category = rule.0
                    break
                }
            }
        }
        var tags: Set<String> = [category]
        let folderTags = hierarchy.dropFirst().dropLast().filter { !["samples", "sample", "sounds", "presets", "instruments", "wav", "aiff", "loops"].contains(fold($0)) }
        for folder in folderTags.prefix(8) { tags.insert(folder) }
        let loopWord = words.contains("loop") || words.contains("loops")
        if loopWord { tags.insert("Loop") }
        if words.contains("oneshot") || words.contains("one") && words.contains("shots") || text.contains("one shot") { tags.insert("One-shot") }
        if words.contains("stereo") { tags.insert("Stereo") }
        if words.contains("mono") { tags.insert("Mono") }
        // A tempo in the filename is stronger evidence than a number in an expansion name.
        let bpmText = name + " " + hierarchy.dropFirst().dropLast().joined(separator: " ")
        let bpmRange = NSRange(bpmText.startIndex..., in: bpmText)
        func number(_ match: NSTextCheckingResult) -> Double? {
            for index in 1..<match.numberOfRanges where match.range(at: index).location != NSNotFound {
                if let range = Range(match.range(at: index), in: bpmText) { return Double(bpmText[range]) }
            }
            return nil
        }
        var bpm = explicitBPMPattern.firstMatch(in: bpmText, range: bpmRange).flatMap(number)
        // Bare numbers are often take/variation/velocity numbers ("Snare 64", "Kick_46", "Pizzi 69"):
        // accept them only inside the filename, never as its final token.
        if bpm == nil {
            let nameLength = (name as NSString).length
            for match in bareBPMPattern.matches(in: bpmText, range: bpmRange) {
                let end = match.range(at: 1).location + match.range(at: 1).length
                guard end < nameLength else { break }
                bpm = number(match); break
            }
        }
        var key: String?
        let nameText = name as NSString
        for match in keyPattern.matches(in: name, range: NSRange(location: 0, length: nameText.length)) {
            func group(_ number: Int) -> String {
                guard let range = Range(match.range(at: number), in: name) else { return "" }
                return String(name[range])
            }
            if !group(1).isEmpty { key = canonicalKey(group(1) + group(2) + " " + group(3)); break }
            // Bare letters in drum names are usually variations ("Kick A 2"), not keys,
            // and a leading "A " is usually an article ("A Fading Dream").
            if drumHitCategories.contains(category) || category == "FX" { continue }
            let letter = match.range(at: 4)
            if letter.location == 0, group(5).isEmpty, nameText.length > 2,
               let next = nameText.substring(with: NSRange(location: 2, length: 1)).unicodeScalars.first, CharacterSet.letters.contains(next) { continue }
            key = canonicalKey(group(4) + group(5)); break
        }
        return (category, tags.sorted(), bpm, key)
    }

    private func loadCache() {
        var restored: Cache?
        let cacheURL = cacheDirectory.appendingPathComponent("index-v1.json")
        if let attributes = try? FileManager.default.attributesOfItem(atPath: cacheURL.path),
           let size = attributes[.size] as? NSNumber, size.int64Value < 256 * 1024 * 1024,
           let data = try? Data(contentsOf: cacheURL), let cache = try? JSONDecoder().decode(Cache.self, from: data), cache.version == 1 {
            restored = cache
        }
        let savedFavorites = (try? Data(contentsOf: cacheDirectory.appendingPathComponent("favorites.json")))
            .flatMap { try? JSONDecoder().decode([String].self, from: $0) } ?? []
        // Older caches are re-inferred from their paths so improved categories/tempo/keys apply without a rescan.
        let upgrade = restored.map { $0.inferenceVersion != Self.inferenceVersion } ?? false
        if upgrade, var cache = restored { cache.items = cache.items.map(Self.reinferred); restored = cache }
        else if let restored { for item in restored.items { item.prepareSearchFields() } }
        locked {
            if let restored = restored {
                excludedRoots.formUnion(restored.excludedRoots ?? [])
                roots.removeAll { excludedRoots.contains($0.path) }
                for root in restored.roots where !excludedRoots.contains(root.path) && !roots.contains(where: { $0.path == root.path }) { roots.append(root) }
                for item in restored.items where items[item.path] == nil { items[item.path] = item.applying(metadataOverrides[item.path]) }
                kitIndexedRoots.formUnion(restored.kitIndexedRoots ?? [])
                kitUpgradeEnabled = true
                verifiedExpansionRoots.formUnion(restored.verifiedExpansionRoots ?? [])
                if activeScan == nil {
                    lastErrors = restored.scanErrors ?? []
                    for var report in restored.scanReports ?? [] {
                        if ["pending", "scanning"].contains(report.status) { report.status = "cancelled" }
                        rootReports[report.path] = report
                    }
                }
            }
            favorites.formUnion(savedFavorites)
            indexRevision &+= 1
        }
        refreshAvailability()
        emit("library.results", locked { ["scanning": activeScan != nil, "count": items.count, "cached": true, "errors": lastErrors, "rootReports": rootReports.values.map(\.dictionary)] })
        if let restored { queueMetadata(collections: Array(Set(restored.items.compactMap(\.collectionRoot)))) }
        if upgrade { persistCache() }
        scheduleKitIndexUpgrade()
    }

    private static func reinferred(_ old: Item) -> Item {
        let root = LibraryRoot(path: old.root, name: URL(fileURLWithPath: old.root).lastPathComponent, source: "")
        let item: Item
        if old.kind == "nks" || old.kind == "kit" {
            // NKS metadata came from the preset file; only normalize presentation without reading it again.
            item = Item(path: old.path, root: old.root, name: old.name, kind: old.kind, category: old.kind == "kit" ? old.category : nksCategory(old.category), tags: old.tags,
                        pack: cleanPack(old.pack), bpm: old.bpm, key: nil, size: old.size, modified: old.modified,
                        metadataSource: old.metadataSource, previewPath: old.previewPath)
        } else {
            item = inferredItem(path: old.path, root: root, size: old.size, modified: old.modified)
        }
        item.duration = old.duration; item.durationChecked = old.durationChecked
        item.kitSoundCount = old.kitSoundCount; item.kitMetadataError = old.kitMetadataError
        item.prepareSearchFields()
        return item
    }

    private func persistCache() {
        let schedule = locked { () -> Bool in
            cacheWriteRequested = true
            if cacheWriteScheduled { return false }
            cacheWriteScheduled = true
            return true
        }
        guard schedule else { return }
        diskQueue.async { [weak self] in
            guard let self = self else { return }
            while true {
                let cache = self.locked { () -> Cache? in
                    guard self.cacheWriteRequested else { self.cacheWriteScheduled = false; return nil }
                    self.cacheWriteRequested = false
                    return Cache(roots: self.roots, items: self.items.values.map { $0.inferred ?? $0 }, excludedRoots: self.excludedRoots.sorted(), scanErrors: self.lastErrors, scanReports: Array(self.rootReports.values), verifiedExpansionRoots: self.verifiedExpansionRoots.sorted(), inferenceVersion: Self.inferenceVersion, kitIndexedRoots: self.kitIndexedRoots.sorted())
                }
                guard let cache else { break }
                autoreleasepool {
                    do {
                        try FileManager.default.createDirectory(at: self.cacheDirectory, withIntermediateDirectories: true)
                        let data = try JSONEncoder().encode(cache)
                        try data.write(to: self.cacheDirectory.appendingPathComponent("index-v1.json"), options: .atomic)
                    } catch { self.emit("library.error", ["error": "Could not save the library index: \(error.localizedDescription)"]) }
                }
            }
        }
    }

    /// Serial, write-before-publish batch edits. A failed write cannot change search results.
    /// Source files are never opened or modified by this operation.
    private func editMetadata(_ payload: [String: Any]) throws -> [String: Any] {
        if let metadataReadError { throw Self.error(metadataReadError) }
        guard let requested = payload["paths"] as? [String], !requested.isEmpty, requested.count <= 500,
              requested.allSatisfy({ !$0.isEmpty }), let changes = payload["changes"] as? [String: Any] else {
            throw Self.error("Select between 1 and 500 indexed sounds and their metadata changes.")
        }
        let paths = Array(Set(requested.map(Self.normalized))).sorted()
        var next = try locked { () throws -> [String: MetadataOverride] in
            guard paths.allSatisfy({ items[$0] != nil }) else { throw Self.error("A selected sound is no longer indexed. Refresh the library and try again.") }
            return metadataOverrides
        }
        var edits = changes
        let addedTags = edits.removeValue(forKey: "addTags")
        if addedTags != nil && edits["tags"] != nil { throw Self.error("Choose either Add tags or Replace tags.") }
        var validatedTags = MetadataOverride()
        if let addedTags { try validatedTags.merge(["tags": addedTags]); guard validatedTags.tags != nil else { throw Self.error("Provide the tags to add.") } }
        let currentTags = locked { Dictionary(uniqueKeysWithValues: paths.map { ($0, items[$0]?.tags ?? []) }) }
        for path in paths {
            var value = next[path] ?? MetadataOverride()
            try value.merge(edits)
            if let tags = validatedTags.tags {
                // Merge case-insensitively before enforcing the resulting tag limit.
                var seen: Set<String> = []
                let combined = ((currentTags[path] ?? []) + tags).filter { seen.insert(Self.fold($0)).inserted }
                try value.merge(["tags": combined])
            }
            if value.isEmpty { next.removeValue(forKey: path) } else { next[path] = value }
        }
        let data = try JSONEncoder().encode(MetadataFile(items: next))
        guard data.count <= 32 * 1024 * 1024 else { throw Self.error("Library metadata exceeds the supported size.") }
        try FileManager.default.createDirectory(at: cacheDirectory, withIntermediateDirectories: true)
        try data.write(to: cacheDirectory.appendingPathComponent("metadata-v1.json"), options: .atomic)
        let updated = locked { () -> [[String: Any]] in
            metadataOverrides = next
            for path in paths { if let item = items[path] { items[path] = item.applying(next[path]) } }
            indexRevision &+= 1
            return paths.compactMap { items[$0]?.dictionary(favorites: favorites, verifiedCollections: verifiedExpansionRoots, pendingDurations: queuedDurations) }
        }
        emit("library.metadata", ["paths": paths, "edited": true])
        return ["items": updated, "updated": paths.count]
    }

    private func persistFavorites() {
        let values = locked { favorites.sorted() }
        userFileQueue.async { [weak self] in
            guard let self = self else { return }
            do {
                try FileManager.default.createDirectory(at: self.cacheDirectory, withIntermediateDirectories: true)
                try JSONEncoder().encode(values).write(to: self.cacheDirectory.appendingPathComponent("favorites.json"), options: .atomic)
            } catch { self.emit("library.error", ["error": "Could not save favorites: \(error.localizedDescription)"]) }
        }
    }

    /// Read-only, shallow diagnostics from the app process. Retain only the latest scan report.
    /// This is separate from index-v1 so support can inspect access errors without loading a huge index.
    private func persistDiagnostics() {
        let snapshot = locked { (roots.map(\.path), rootReports.values.map(\.dictionary), lastErrors) }
        diskQueue.async { [weak self] in
            guard let self else { return }
            let result: [String: Any] = ["version": 1, "createdAt": ISO8601DateFormatter().string(from: Date()),
                                         "rootReports": snapshot.1, "errors": snapshot.2,
                                         "rootProbes": snapshot.0.prefix(128).map { Self.probeRoot($0) }]
            do {
                try FileManager.default.createDirectory(at: self.cacheDirectory, withIntermediateDirectories: true)
                try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
                    .write(to: self.cacheDirectory.appendingPathComponent("diagnostics-v1.json"), options: .atomic)
            } catch { self.emit("library.error", ["error": "Could not save library diagnostics: \(error.localizedDescription)"]) }
        }
    }

    private func rootDictionaries() -> [[String: Any]] {
        let snapshot = locked { () -> ([LibraryRoot], [String: RootReport], [String: Int]) in
            var counts: [String: Int] = [:]
            for item in items.values { counts[item.root, default: 0] += 1 }
            return (roots, rootReports, counts)
        }
        return snapshot.0.map { root in
            var result = root.dictionary
            result["scanStatus"] = snapshot.1[root.path]?.status ?? "unscanned"
            result["scanErrors"] = snapshot.1[root.path]?.errors ?? []
            result["indexedCount"] = snapshot.2[root.path] ?? 0
            return result
        }
    }

    /// Shallow filesystem probe in this process, so app-specific privacy failures remain visible.
    /// Reading from a CLI can succeed when the bundled app lacks access to the same folder.
    private static func probeRoot(_ path: String) -> [String: Any] {
        let fm = FileManager.default
        let url = URL(fileURLWithPath: path, isDirectory: true)
        var directory: ObjCBool = false
        var result: [String: Any] = ["path": path, "available": fm.fileExists(atPath: path, isDirectory: &directory) && directory.boolValue,
                                     "canReadDirectory": false, "entryCount": 0, "sampleEntries": []]
        let keys: Set<URLResourceKey> = [.isDirectoryKey, .isRegularFileKey, .isPackageKey, .isSymbolicLinkKey, .isHiddenKey, .isReadableKey]
        func attributes(_ values: URLResourceValues) -> [String: Any] {
            ["isDirectory": values.isDirectory ?? false, "isRegularFile": values.isRegularFile ?? false,
             "isPackage": values.isPackage ?? false, "isSymbolicLink": values.isSymbolicLink ?? false,
             "isHidden": values.isHidden ?? false, "isReadable": values.isReadable ?? false]
        }
        do { result["attributes"] = attributes(try url.resourceValues(forKeys: keys)) }
        catch { result["metadataError"] = describe(error) }
        do {
            let names = try fm.contentsOfDirectory(atPath: path)
            result["canReadDirectory"] = true
            result["entryCount"] = names.count
            result["sampleEntries"] = names.filter { !$0.hasPrefix(".") }.sorted().prefix(8).map { name -> [String: Any] in
                var entry: [String: Any] = ["name": name]
                let child = URL(fileURLWithPath: path + "/" + name)
                do { entry.merge(attributes(try child.resourceValues(forKeys: keys)), uniquingKeysWith: { _, new in new }) }
                catch { entry["error"] = describe(error) }
                return entry
            }
        } catch { result["error"] = describe(error) }
        return result
    }

    private static func describe(_ error: Error) -> String {
        let ns = error as NSError
        var result = "\(ns.localizedDescription) [\(ns.domain) \(ns.code)]"
        if let underlying = ns.userInfo[NSUnderlyingErrorKey] as? NSError {
            result += " (\(underlying.domain) \(underlying.code): \(underlying.localizedDescription))"
        }
        return result
    }

    /// Default roots: the user kit folder plus standard locations that exist on this Mac (Apple Loops,
    /// Loopcloud, Native Instruments user content and the products Native Access registered). Other folders,
    /// such as sample libraries on an external drive, are added with Add folder and stay listed while offline.
    private static func discoverRoots() -> [LibraryRoot] {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        // The user kit folder is created on first use so an empty library is not reported as offline.
        try? FileManager.default.createDirectory(at: KitLibrary.userDirectory, withIntermediateDirectories: true)
        var result = [root(for: KitLibrary.userDirectory.path, source: "User kits")]
        var candidates: [(path: String, source: String)] = [("/Library/Audio/Apple Loops", "Apple Loops"), ("\(home)/Music/Loopcloud", "Loopcloud")]
        for suffix in ["Maschine 3/Samples", "Maschine 3/Recordings", "Maschine 3/Sounds", "Maschine 3/Groups", "Maschine 2/Samples", "User Content"] {
            candidates.append(("\(home)/Documents/Native Instruments/\(suffix)", "Native Instruments"))
        }
        candidates += nativeInstrumentsContentRoots().map { ($0, "Native Instruments") }
        for candidate in candidates {
            let path = normalized(candidate.path)
            var directory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: path, isDirectory: &directory), directory.boolValue,
                  !result.contains(where: { isInside(path, root: $0.path) || isInside($0.path, root: path) }) else { continue }
            result.append(root(for: path, source: candidate.source))
        }
        return result
    }

    /// Content folders of installed Native Instruments products and expansions that hold samples or kits.
    /// Products that Native Access installed side by side are indexed through their shared parent folder
    /// (for example ".../Native Access Content"), unless that parent is a volume, home or other broad folder.
    static func nativeInstrumentsContentRoots(_ registrations: [NKSCatalog.Registration] = NKSCatalog.discoverRegistrations()) -> [String] {
        let home = FileManager.default.homeDirectoryForCurrentUser.standardizedFileURL.path
        let broad = Set(["/Users/Shared", "/Applications", "/Library", "/Library/Application Support", home] + ["Documents", "Music", "Desktop", "Downloads"].map { "\(home)/\($0)" })
        let content = registrations.map(\.contentDir).filter { directory in
            ["Samples", "Groups", "Kits", "Sounds"].contains { FileManager.default.fileExists(atPath: directory + "/" + $0) }
        }
        var roots: [String] = []
        for (parent, children) in Dictionary(grouping: content, by: { URL(fileURLWithPath: $0).deletingLastPathComponent().standardizedFileURL.path }) {
            let volumeRoot = parent.hasPrefix("/Volumes/") && URL(fileURLWithPath: parent).pathComponents.count <= 3
            if children.count >= 2, !broad.contains(parent), !volumeRoot, URL(fileURLWithPath: parent).pathComponents.count >= 3 { roots.append(parent) }
            else { roots += children }
        }
        return roots.sorted()
    }

    private static func root(for path: String, source: String) -> LibraryRoot {
        let path = normalized(path)
        return LibraryRoot(path: path, name: URL(fileURLWithPath: path).lastPathComponent, source: source)
    }

    private static func normalized(_ path: String) -> String {
        URL(fileURLWithPath: (path as NSString).expandingTildeInPath).standardizedFileURL.path
    }

    private static func isInside(_ path: String, root: String) -> Bool { path == root || path.hasPrefix(root == "/" ? "/" : root + "/") }
    static func foldText(_ string: String) -> String { fold(string) }
    private static func fold(_ string: String) -> String { string.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: Locale(identifier: "en_US_POSIX")) }
    private static func error(_ message: String) -> NSError { NSError(domain: "SuperTracker.Library", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }

    private func locked<T>(_ body: () throws -> T) rethrows -> T { lock.lock(); defer { lock.unlock() }; return try body() }
    private func emit(_ name: String, _ payload: [String: Any]) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let value = ["library.progress", "library.results"].contains(name) ? self.withKitUpgradeStatus(payload) : payload
            self.onEvent?(name, value)
        }
    }

#if SELFTESTS
    // Self-tests, the stress test and the manual benchmark: debug and test builds only.
    /// Exercise the stale-candidate boundary directly, without timing filesystem
    /// calls or adding a production hook. Fixtures never access installed roots.
    static func kitUpgradeRegistrationRaceSelfTest() async throws -> [String] {
        let fm = FileManager.default
        let temp = fm.temporaryDirectory.appendingPathComponent("SuperTracker-kit-registration-" + UUID().uuidString)
        try fm.createDirectory(at: temp, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: temp) }
        func require(_ condition: Bool, _ message: String) throws { if !condition { throw error("Kit upgrade registration: " + message) } }
        let removed = temp.appendingPathComponent("Removed"), valid = temp.appendingPathComponent("Valid")
        let complete = temp.appendingPathComponent("Complete"), attempted = temp.appendingPathComponent("Attempted"), unknown = temp.appendingPathComponent("Unknown")
        for root in [removed, valid, complete, attempted, unknown] {
            try fm.createDirectory(at: root, withIntermediateDirectories: true)
            try Data("generated kit metadata fixture".utf8).write(to: root.appendingPathComponent("Fixture.mxgrp"))
        }
        let library = LibraryService(cacheDirectory: temp.appendingPathComponent("Cache"), defaultRoots: [removed, valid, complete, attempted], loadCacheAsynchronously: false)
        // These are candidates captured before the filesystem phase. Publish the
        // intervening removal/completion before passing that stale list onward.
        let candidates = library.locked { library.roots.map(\.path) } + [unknown.path]
        _ = try await library.handle("library.removeRoot", payload: ["path": removed.path])
        library.locked { library.kitIndexedRoots.insert(complete.path); library.attemptedKitUpgradeRoots.insert(attempted.path) }
        let rejected = try library.beginScan(paths: candidates.filter { $0 != valid.path }, kitsOnly: true)
        try require(rejected["scanning"] as? Bool == false, "a fully stale candidate list cannot start a scan")
        try require(library.locked { library.activeScan == nil && library.excludedRoots.contains(removed.path) && !library.roots.contains { $0.path == removed.path || $0.path == unknown.path } }, "automatic work must not re-register or unexclude removed/unknown roots")
        let accepted = try library.beginScan(paths: candidates, kitsOnly: true)
        let acceptedRoots = (accepted["roots"] as? [[String: Any]] ?? []).compactMap { $0["path"] as? String }
        try require(acceptedRoots == [valid.path], "revalidate every root in a mixed stale/valid batch")
        library.scanQueue.sync { }
        try require(library.locked { Set(library.items.values.map(\.root)) == [valid.path] && !library.attemptedKitUpgradeRoots.contains(removed.path) && !library.attemptedKitUpgradeRoots.contains(unknown.path) }, "only the remaining registered root may be enumerated")
        _ = try library.beginScan(paths: [removed.path])
        library.scanQueue.sync { }; library.metadataQueue.sync { }; library.diskQueue.sync { }
        try require(library.locked { !library.excludedRoots.contains(removed.path) && library.roots.contains { $0.path == removed.path } && library.items.values.contains { $0.root == removed.path } }, "an explicit user scan may still re-add the removed root")
        return ["stale automatic candidate cannot re-register a removed or unknown root",
                "completed and previously attempted roots are revalidated under the lock",
                "mixed candidate batch scans only its remaining registered root",
                "explicit user scan retains intentional re-add behavior"]
    }

    /// Deterministic worker-completion interleavings; no file audio or user library access.
    static func metadataCompletionSelfTest() throws -> [String] {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-metadata-completion-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let library = LibraryService(cacheDirectory: directory, defaultRoots: [], loadCacheAsynchronously: false)
        // Hold the actual reader while driving completion of captured immutable records.
        library.metadataQueue.suspend()
        defer {
            library.locked { library.pendingDurations.removeAll(); library.queuedDurations.removeAll() }
            library.metadataQueue.resume()
        }
        func require(_ condition: Bool, _ message: String) throws { if !condition { throw Self.error(message) } }
        let path = directory.appendingPathComponent("Fixture.wav").path
        func fixture(size: Int64, modified: Double) -> Item {
            let value = Item(path: path, root: directory.path, name: "Fixture", kind: "sample", category: "Other", tags: ["Original"], pack: "Fixture", bpm: nil, key: nil, size: size, modified: modified, metadataSource: "fixture", previewPath: nil)
            value.prepareSearchFields(); return value
        }
        func result() -> [String: Any] { (library.search(["query": ""]) ["items"] as? [[String: Any]])?.first ?? [:] }
        let captured = fixture(size: 10, modified: 1)
        library.locked { library.items[path] = captured; library.indexRevision &+= 1 }
        _ = result() // Queue and cache a pending header.
        _ = try library.editMetadata(["paths": [path], "changes": ["tags": ["Edited"]]])
        try require(result()["durationStatus"] as? String == "pending", "header must be pending after an interleaved edit")
        _ = library.completeDuration(path: path, source: captured, duration: 0.5)
        let finished = result()
        try require(finished["duration"] as? Double == 0.5 && finished["durationStatus"] as? String == "known", "old-item completion must replace cached pending status")
        try require(finished["tags"] as? [String] == ["Edited"], "old-item completion must preserve newer user labels")
        let newer = fixture(size: 20, modified: 2)
        library.locked { library.items[path] = newer.applying(library.metadataOverrides[path]); library.indexRevision &+= 1 }
        _ = result()
        _ = library.completeDuration(path: path, source: captured, duration: 99)
        try require(library.locked { library.items[path]?.duration == nil && library.queuedDurations.contains(path) }, "changed-source completion must requeue rather than publish obsolete length")
        _ = library.completeDuration(path: path, source: newer, duration: 0.75)
        let updated = result()
        try require(updated["duration"] as? Double == 0.75 && updated["tags"] as? [String] == ["Edited"], "new-source completion must preserve labels and publish its own length")
        return ["edited immutable item completion preserves labels and invalidates pending cache", "changed source header is requeued and cannot publish obsolete duration"]
    }

    /// Isolated, deterministic integration tests: never touch the user's cache or sample libraries.
    static func selfTest() throws -> [String] {
        let fm = FileManager.default
        let temp = fm.temporaryDirectory.appendingPathComponent("SuperTracker-library-test-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: temp, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: temp) }
        let root = temp.appendingPathComponent("Samples", isDirectory: true)
        let drums = root.appendingPathComponent("Orbit Pack/Loops/Drums", isDirectory: true)
        try fm.createDirectory(at: drums, withIntermediateDirectories: true)
        let sample = drums.appendingPathComponent("Kick_120bpm_Cmin.wav")
        try Data([0x52, 0x49, 0x46, 0x46]).write(to: sample)
        try Data([1]).write(to: drums.appendingPathComponent("._Ghost.wav"))
        try Data([1]).write(to: drums.appendingPathComponent(".Hidden.wav"))
        try Data([1]).write(to: drums.appendingPathComponent("Readme.txt"))
        try fm.createSymbolicLink(at: drums.appendingPathComponent("cycle"), withDestinationURL: root)
        let library = LibraryService(cacheDirectory: temp.appendingPathComponent("Cache"), defaultRoots: [root], loadCacheAsynchronously: false)
        _ = try library.beginScan(paths: nil)
        library.scanQueue.sync { }
        func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
            if !condition() { throw error("Library self-test failed: \(message)") }
        }
        let results = library.search(["query": "kick bpm:120 key:Cminor pack:\"Orbit Pack\"", "kind": "sample"])
        let found = results["items"] as? [[String: Any]] ?? []
        try require(found.count == 1, "search and inferred metadata")
        try require(found.first?["category"] as? String == "Kick", "specific drum category")
        try require(library.locked { library.items.count } == 1, "hidden files and symlink loop ignored")
        _ = library.locked { library.favorites.insert(sample.path) }
        library.persistFavorites()
        library.diskQueue.sync { }; library.userFileQueue.sync { }
        let cached = LibraryService(cacheDirectory: temp.appendingPathComponent("Cache"), defaultRoots: [root], loadCacheAsynchronously: false)
        try require(cached.search(["favorites": true])["total"] as? Int == 1, "index and favorites persisted")
        try fm.removeItem(at: sample)
        _ = try library.beginScan(paths: nil)
        library.scanQueue.sync { }
        library.diskQueue.sync { }
        try require(library.search([:])["total"] as? Int == 0, "deleted sample removed on complete scan")
        let phrase = Query("pack:\"Orbit Pack\" tag:loop")
        try require(phrase.filters.count == 2 && phrase.words.isEmpty, "quoted search filters")
        try require(inferMetadata(name: "snare02", hierarchy: ["Kick Pack", "Drums", "snare02.wav"]).category == "Snare", "filename outranks pack category")
        // Cancel a queued scan before it can enumerate and verify it cannot publish stale results.
        let blocker = DispatchSemaphore(value: 0)
        library.scanQueue.async { blocker.wait() }
        _ = try library.beginScan(paths: nil)
        library.locked { library.activeScan?.cancel(); library.activeScan = nil }
        blocker.signal()
        library.scanQueue.sync { }
        try require(library.locked { library.activeScan == nil }, "cancelled scan stays cancelled")
        let accessible = probeRoot(root.path)
        try require(accessible["canReadDirectory"] as? Bool == true && (accessible["entryCount"] as? Int ?? 0) > 0, "same-process readable folder diagnostics")
        let missing = temp.appendingPathComponent("DisconnectedLibrary", isDirectory: true)
        let inaccessible = probeRoot(missing.path)
        try require(inaccessible["available"] as? Bool == false && inaccessible["canReadDirectory"] as? Bool == false && inaccessible["error"] is String, "missing folder diagnostics")
        _ = try library.beginScan(paths: [missing.path])
        library.scanQueue.sync { }
        library.diskQueue.sync { }
        let restoredErrors = LibraryService(cacheDirectory: temp.appendingPathComponent("Cache"), defaultRoots: [root], loadCacheAsynchronously: false)
        let missingReport = restoredErrors.rootDictionaries().first { $0["path"] as? String == missing.path }
        try require(missingReport?["scanStatus"] as? String == "error" && !(missingReport?["scanErrors"] as? [String] ?? []).isEmpty, "folder failures persist through restart")
        try require(!(restoredErrors.search([:])["errors"] as? [String] ?? []).isEmpty, "search exposes persisted failures")
        let diagnosticsData = try Data(contentsOf: temp.appendingPathComponent("Cache/diagnostics-v1.json"))
        let diagnostics = try JSONSerialization.jsonObject(with: diagnosticsData) as? [String: Any]
        let probes = diagnostics?["rootProbes"] as? [[String: Any]] ?? []
        try require(probes.contains { $0["path"] as? String == missing.path && $0["canReadDirectory"] as? Bool == false }, "latest scan saves same-process diagnostics separately")
        let packageRoot = temp.appendingPathComponent("PackageSamples", isDirectory: true)
        try fm.createDirectory(at: packageRoot.appendingPathComponent("Expansion.band/Audio"), withIntermediateDirectories: true)
        try Data([1]).write(to: packageRoot.appendingPathComponent("Expansion.band/Audio/Pad.wav"))
        try fm.createDirectory(at: packageRoot.appendingPathComponent("Plugin.component/Resources"), withIntermediateDirectories: true)
        try Data([1]).write(to: packageRoot.appendingPathComponent("Plugin.component/Resources/Ignore.wav"))
        let packages = LibraryService(cacheDirectory: temp.appendingPathComponent("PackageCache"), defaultRoots: [packageRoot], loadCacheAsynchronously: false)
        _ = try packages.beginScan(paths: nil)
        packages.scanQueue.sync { }
        packages.diskQueue.sync { }
        try require(packages.search([:])["total"] as? Int == 1, "sample packages descended, software packages skipped")
        let disconnected = temp.appendingPathComponent("MovedPackageSamples", isDirectory: true)
        try fm.moveItem(at: packageRoot, to: disconnected)
        _ = try packages.beginScan(paths: nil)
        packages.scanQueue.sync { }
        packages.diskQueue.sync { }
        try require(packages.search([:])["total"] as? Int == 1 && packages.rootDictionaries().first?["scanStatus"] as? String == "error", "offline folder preserves cached records with error status")
        return ["library scan/search/categories/tempo/key", "hidden and symlink exclusion", "cache and favorites", "stale item cleanup", "quoted search filters", "category specificity and cancellation", "same-process folder diagnostics", "persisted folder errors and offline cache preservation", "sample package traversal and software bundle exclusion"]
    }

    /// Facet and header-enrichment regressions using small real WAV headers in an isolated directory.
    static func filterSelfTest() throws -> [String] {
        let fm = FileManager.default
        let temp = fm.temporaryDirectory.appendingPathComponent("SuperTracker-library-filters-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: temp, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: temp) }
        let ni = temp.appendingPathComponent("Native Access Content", isDirectory: true)
        let loopcloud = temp.appendingPathComponent("Loopcloud", isDirectory: true)
        let user = temp.appendingPathComponent("User", isDirectory: true)
        func wav(_ path: URL, seconds: Double) throws {
            try fm.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
            func word(_ value: UInt32) -> Data { Data((0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) }) }
            let bytes = Int(seconds * 8000) * 2
            var data = Data("RIFF".utf8) + word(UInt32(36 + bytes)) + Data("WAVEfmt ".utf8) + word(16)
            data += Data([1, 0, 1, 0]) + word(8000) + word(16000) + Data([2, 0, 16, 0])
            data += Data("data".utf8) + word(UInt32(bytes)) + Data(repeating: 0, count: bytes)
            try data.write(to: path)
        }
        let kick = ni.appendingPathComponent("Anima Ascent Library/Samples/One Shots/Kick_120bpm_Cmin.wav")
        try wav(kick, seconds: 0.25)
        try wav(ni.appendingPathComponent("Anima Ascent Library/Samples/One Shots/Snare_100bpm_Dmaj.wav"), seconds: 0.5)
        try wav(ni.appendingPathComponent("Anima Ascent Library/Samples/Loops/Drum_Loop_130bpm_Cmin.wav"), seconds: 2)
        try fm.createDirectory(at: ni.appendingPathComponent("Anima Ascent Library/Groups"), withIntermediateDirectories: true)
        try fm.createDirectory(at: ni.appendingPathComponent("Anima Ascent Library/Sounds"), withIntermediateDirectories: true)
        try wav(ni.appendingPathComponent("Piano Library/Samples/Keys/Piano.wav"), seconds: 3)
        try wav(loopcloud.appendingPathComponent("Pack Two/One Shots/Kick_128bpm_Cmin.wav"), seconds: 1)
        try fm.createDirectory(at: user, withIntermediateDirectories: true)
        try Data([1, 2, 3]).write(to: user.appendingPathComponent("Unclassified.wav"))
        let original = try Data(contentsOf: kick)
        let cache = temp.appendingPathComponent("Cache")
        let library = LibraryService(cacheDirectory: cache, defaultRoots: [ni, loopcloud, user], loadCacheAsynchronously: false)
        _ = try library.beginScan(paths: nil)
        library.scanQueue.sync { }
        library.metadataQueue.sync { }
        func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
            if !condition() { throw error("Library filter self-test failed: \(message)") }
        }
        func result(_ filters: [String: Any]) -> [String: Any] { library.search(["filters": filters]) }
        func total(_ filters: [String: Any]) -> Int { result(filters)["total"] as? Int ?? -1 }
        let first = library.search(["limit": 1])
        try require(first["total"] as? Int == 6, "all fixture files indexed")
        try require((first["durationCoverage"] as? [String: Int])?["pending"] == 1, "only visible page queues header reads")
        try require(total(["categories": ["Kick", "Snare"]]) == 3, "categories OR together")
        try require(total(["collections": ["Maschine expansions"]]) == 3, "verified expansion provenance")
        try require(total(["collections": ["Native Instruments"]]) == 1, "ordinary NI libraries are not expansions")
        try require(total(["categories": ["Kick", "Snare"], "collections": ["Maschine expansions"]]) == 2, "different filters intersect")
        try require(total(["sources": ["Loopcloud"]]) == 1, "source alias")
        try require(total(["collections": ["Loopcloud", "Native Instruments"]]) == 2, "source selections OR together")
        try require(total(["roots": [loopcloud.path, user.path]]) == 2, "root selections OR together")
        try require(total(["packs": ["Anima Ascent", "Pack Two"]]) == 4, "pack selections OR together")
        try require(total(["keys": ["C minor"]]) == 3 && total(["keys": ["unknown"]]) == 2, "key and unknown filters")
        try require(total(["keys": ["C minor", "D major"], "sampleTypes": ["loop", "oneshot"]]) == 4, "key and type selections OR within each facet and AND across facets")
        try require(total(["sampleTypes": ["loop"]]) == 1 && total(["sampleTypes": ["oneshot"]]) == 3 && total(["sampleTypes": ["unknown"]]) == 2, "sample type requires explicit evidence")
        try require(total(["extensions": ["wav"], "tags": ["Kick", "Snare"]]) == 3, "format and tag filters")
        let facets = result(["categories": ["Kick"], "collections": ["Maschine expansions"]])["facets"] as? [String: [[String: Any]]] ?? [:]
        try require(facets["categories"]?.contains { $0["value"] as? String == "Snare" && $0["count"] as? Int == 1 } == true, "disjunctive facet retains alternatives")
        try require(total(["bpm": ["min": 110, "max": 129]]) == 2, "BPM range")
        try require(total(["bpm": ["min": 110, "max": 129, "includeUnknown": true]]) == 4, "BPM range plus unknown")
        try require(total(["bpm": ["includeUnknown": true]]) == 2, "unknown-only BPM")
        _ = result(["duration": ["min": 0.45, "max": 0.6]])
        library.metadataQueue.sync { }
        try require(total(["duration": ["min": 0.45, "max": 0.6]]) == 1, "range filter enriches candidates before duration exclusion")
        try require(total(["duration": ["includeUnknown": true]]) == 1, "unreadable audio remains explicitly unknown")
        try require(total(["duration": ["min": 0.45, "max": 0.6, "includeUnknown": true]]) == 2, "duration range plus unknown")
        library.metadataQueue.sync { }
        let known = library.locked { library.items[kick.path]?.duration }
        try require(known.map { abs($0 - 0.25) < 0.001 } == true, "WAV header duration read without decoding")
        library.persistCache(); library.diskQueue.sync { }
        let restored = LibraryService(cacheDirectory: cache, defaultRoots: [ni, loopcloud, user], loadCacheAsynchronously: false)
        try require(restored.locked { restored.items[kick.path]?.duration } == known, "header duration persists")
        try require(restored.search(["filters": ["collections": ["Maschine expansions"]]])["total"] as? Int == 3, "verified provenance persists")
        restored.metadataQueue.sync { }
        let cacheFile = cache.appendingPathComponent("index-v1.json")
        var legacy = try JSONSerialization.jsonObject(with: Data(contentsOf: cacheFile)) as! [String: Any]
        legacy.removeValue(forKey: "verifiedExpansionRoots")
        legacy["items"] = (legacy["items"] as! [[String: Any]]).map { item -> [String: Any] in
            var old = item; old.removeValue(forKey: "duration"); old.removeValue(forKey: "durationChecked"); return old
        }
        try JSONSerialization.data(withJSONObject: legacy).write(to: cacheFile, options: .atomic)
        let oldCache = LibraryService(cacheDirectory: cache, defaultRoots: [], loadCacheAsynchronously: false)
        try require(oldCache.locked { oldCache.items.count == 6 && oldCache.items.values.allSatisfy { $0.duration == nil } }, "legacy cache loads with unknown metadata")
        oldCache.metadataQueue.sync { }
        let finalSample = try Data(contentsOf: kick)
        try require(finalSample == original, "source sample unchanged")
        let preset = Item(path: user.appendingPathComponent("Preset.nksf").path, root: user.path, name: "Preset", kind: "nks",
                          category: "Pad", tags: ["Synth"], pack: "Preset Pack", bpm: nil, key: nil, size: 32, modified: 1,
                          metadataSource: "fixture", previewPath: nil)
        preset.prepareSearchFields()
        library.locked { library.items[preset.path] = preset; library.indexRevision &+= 1 }
        let onlyNKS = library.search(["kind": "nks", "limit": 1])
        let onlyNKSFacets = onlyNKS["facets"] as? [String: [[String: Any]]] ?? [:]
        try require(onlyNKS["total"] as? Int == 1 && onlyNKSFacets["extensions"]?.first?["value"] as? String == "nksf", "kind filter separates preset facets")
        let nextNKSPage = library.search(["kind": "nks", "offset": 1, "limit": 1])
        try require(nextNKSPage["total"] as? Int == 1 && (nextNKSPage["items"] as? [[String: Any]])?.isEmpty == true, "population cache preserves pagination counts")
        let onlySamples = library.search(["kind": "sample"])
        let sampleFacets = onlySamples["facets"] as? [String: [[String: Any]]] ?? [:]
        try require(onlySamples["total"] as? Int == 6 && sampleFacets["extensions"]?.first?["value"] as? String == "wav", "kind changes cannot reuse stale source/format facets")
        return ["multi-select OR and cross-filter AND", "disjunctive facet counts", "verified Maschine provenance", "key/BPM/type/format/tag filters", "bounded header duration enrichment and unknown values", "metadata cache compatibility and source preservation"]
    }

    /// Maschine expansion / Loopcloud naming, cache upgrade, remembered browser state and offline roots.
    static func browsingSelfTest() async throws -> [String] {
        func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
            if !condition() { throw error("Library browsing self-test failed: \(message)") }
        }
        func infer(_ path: String, root: String) -> Item {
            let item = inferredItem(path: root + "/" + path, root: LibraryRoot(path: root, name: (root as NSString).lastPathComponent, source: ""), size: 1, modified: 1)
            item.prepareSearchFields(); return item
        }
        let ni = "/Volumes/Fixture/Native Access Content", lc = "/Volumes/Fixture/Samples/Loopcloud"
        let full = infer("Bumpin Flava Library/Samples/Loops/Construction/Target Kit/Full[135] G# Target 1.wav", root: ni)
        try require(full.bpm == 135 && full.key == "G# major" && full.category == "Full Mix" && full.sampleType == "loop" && full.pack == "Bumpin Flava", "Maschine construction kit tempo/key/category/pack")
        let hat = infer("District Xeo Library/Samples/Loops/Construction/Radium/ClosedHH[127] Radium.wav", root: ni)
        try require(hat.category == "Hi-hat" && hat.bpm == 127, "camelCase hi-hat stem")
        let combo = infer("Infamous Flow Library/Samples/Loops/Construction/The Vault/Combo[90] C#m TheVault 1.wav", root: ni)
        try require(combo.key == "C# minor" && combo.bpm == 90, "minor key suffix")
        let kick = infer("Maschine 2 Factory Library Library/Samples/Drums/Kick/Kick RnB A 2.wav", root: ni)
        try require(kick.key == nil && kick.bpm == nil && kick.sampleType == "oneshot" && kick.pack == "Maschine 2 Factory", "drum variation letters/numbers are not key/tempo; Samples/Drums are one-shots")
        let velocity = infer("Maschine 2 Factory Library Library/Samples/Instruments/Strings/Strings Pizzicato Samples/Violn Pizzi 69.wav", root: ni)
        try require(velocity.bpm == nil && velocity.sampleType == "oneshot" && velocity.category == "Strings", "trailing note numbers are not tempo")
        let bells = infer("Pure Drip Library/Samples/Loops/Bells/Bells[139] Am Animosity.wav", root: ni)
        try require(bells.category == "Mallets & Bells" && bells.key == "A minor", "mallet category")
        let stab = infer("Street Swarm Library/Samples/One Shots/Stab & Hit/Stab A BlackMamba 2.wav", root: ni)
        try require(stab.category == "Stabs & Hits" && stab.sampleType == "oneshot", "stab category")
        let loop = infer("Chilled Guitars [14011]/31L_120_Dmin_Elec_Guitar.wav", root: lc)
        try require(loop.bpm == 120 && loop.key == "D minor" && loop.pack == "Chilled Guitars" && loop.sampleType == "loop" && loop.category == "Guitar & Plucked", "Loopcloud tempo/key/pack ID")
        let numbered = infer("Loopcloud Drum - Intro II Drums [12983]/LCD2_Snare_92.wav", root: lc)
        try require(numbered.bpm == nil && numbered.category == "Snare", "Loopcloud take number is not tempo")
        try require(canonicalKey("Bb min") == "A# minor" && canonicalKey("f#") == "F# major" && keySlot("Bb minor") == keySlot("A# minor"), "enharmonic key slots")
        try require(nksCategory("Synth Pad / Warm") == "Pad & Atmosphere" && nksCategory("Bass / Synth") == "Bass", "NKS types join sample categories")
        try require(Query("key:Bbm").filters.first?.1 == keySlot("A# minor"), "key query normalized")

        let fm = FileManager.default
        let temp = fm.temporaryDirectory.appendingPathComponent("SuperTracker-library-browsing-\(UUID().uuidString)", isDirectory: true)
        defer { try? fm.removeItem(at: temp) }
        let online = temp.appendingPathComponent("Online", isDirectory: true), offline = temp.appendingPathComponent("Offline", isDirectory: true)
        try fm.createDirectory(at: online.appendingPathComponent("Pack/Loops"), withIntermediateDirectories: true)
        try fm.createDirectory(at: offline.appendingPathComponent("Pack/Loops"), withIntermediateDirectories: true)
        try Data([1, 2]).write(to: online.appendingPathComponent("Pack/Loops/Bass[120] Am Up.wav"))
        try Data([1, 2]).write(to: offline.appendingPathComponent("Pack/Loops/Synth[128] C Away.wav"))
        let cache = temp.appendingPathComponent("Cache")
        let library = LibraryService(cacheDirectory: cache, defaultRoots: [online, offline], loadCacheAsynchronously: false)
        _ = try library.beginScan(paths: nil); library.scanQueue.sync { }; library.metadataQueue.sync { }
        library.persistCache(); library.diskQueue.sync { }
        // A version-1 cache with stale inferred fields is upgraded on load without a rescan.
        let cacheFile = cache.appendingPathComponent("index-v1.json")
        var legacy = try JSONSerialization.jsonObject(with: Data(contentsOf: cacheFile)) as! [String: Any]
        legacy.removeValue(forKey: "inferenceVersion")
        legacy["items"] = (legacy["items"] as! [[String: Any]]).map { var old = $0; old["category"] = "Other"; old.removeValue(forKey: "bpm"); return old }
        try JSONSerialization.data(withJSONObject: legacy).write(to: cacheFile, options: .atomic)
        try fm.moveItem(at: offline, to: temp.appendingPathComponent("Ejected"))
        let restored = LibraryService(cacheDirectory: cache, defaultRoots: [], loadCacheAsynchronously: false)
        restored.diskQueue.sync { }
        let all = restored.search(["limit": 10])
        let items = all["items"] as? [[String: Any]] ?? []
        try require(items.count == 2 && items.allSatisfy { $0["bpm"] != nil && $0["category"] as? String != "Other" }, "cached records re-inferred after upgrade")
        let upgraded = try JSONSerialization.jsonObject(with: Data(contentsOf: cacheFile)) as? [String: Any]
        try require(upgraded?["inferenceVersion"] as? Int == inferenceVersion, "upgraded cache persisted")
        try require(items.first { ($0["path"] as? String ?? "").hasPrefix(offline.path) }?["available"] as? Bool == false, "offline root marks its sounds unavailable")
        try require(items.first { ($0["path"] as? String ?? "").hasPrefix(online.path) }?["available"] == nil, "online sounds stay available")
        try require((all["offline"] as? [[String: Any]])?.first?["matching"] as? Int == 1, "offline summary counts matching sounds")
        try require(restored.search(["hideOffline": true])["total"] as? Int == 1, "offline sounds can be hidden")
        let filteredOffline = restored.search(["hideOffline": true, "filters": ["keys": ["A minor"]]])
        try require((filteredOffline["offline"] as? [[String: Any]])?.first?["matching"] as? Int == 0, "offline summary respects structured filters while hidden")
        let onlyOffline = restored.search(["filters": ["keys": ["C major"]]])
        try require((onlyOffline["offline"] as? [[String: Any]])?.first?["matching"] as? Int == 1 && onlyOffline["total"] as? Int == 1, "offline summary counts filtered unavailable matches")
        try fm.moveItem(at: temp.appendingPathComponent("Ejected"), to: offline)
        try require((restored.search(["limit": 10])["offline"] as? [[String: Any]])?.isEmpty == true, "reconnected root becomes available")
        _ = try await restored.handle("library.state.set", payload: ["state": ["query": "kick", "recent": [["query": "snare"]]]])
        restored.userFileQueue.sync { }
        let state = try await LibraryService(cacheDirectory: cache, defaultRoots: [], loadCacheAsynchronously: false).handle("library.state.get", payload: [:]) as? [String: Any]
        try require(state?["query"] as? String == "kick" && (state?["recent"] as? [[String: Any]])?.first?["query"] as? String == "snare", "browser state survives restart")
        return ["Maschine expansion and Loopcloud tempo/key/category/pack inference", "enharmonic keys and NKS category mapping",
                "cache inference upgrade without rescan", "offline roots marked, counted, hidden and restored", "remembered browser state"]
    }

    /// Manual benchmark over a copy of a real index directory (tests/LibraryIndexBenchmark.swift).
    /// Pass a scratch copy: loading may upgrade and rewrite the cache in that directory.
    static func benchmarkCachedIndex(directory: URL, queries: [String], filters: [[String: Any]] = []) -> [String: Any] {
        let loadBegin = Date()
        let library = LibraryService(cacheDirectory: directory, defaultRoots: [], loadCacheAsynchronously: false)
        let loadSeconds = Date().timeIntervalSince(loadBegin)
        func timed(_ payload: [String: Any]) -> [String: Any] {
            // Minimum of five uncached runs; the machine may be busy with other work.
            var best = Double.infinity, result: [String: Any] = [:]
            for _ in 0..<5 {
                library.lastPopulation = nil; library.lastSearchKey = ""
                let begin = Date(); result = library.search(payload); best = min(best, Date().timeIntervalSince(begin))
            }
            let items = result["items"] as? [[String: Any]] ?? []
            return ["ms": Int(best * 1000), "total": result["total"] as? Int ?? -1,
                    "top": items.prefix(3).map { "\($0["name"] ?? "") | \($0["category"] ?? "") | \($0["pack"] ?? "") | \($0["bpm"] ?? "-") | \($0["key"] ?? "-")" }]
        }
        var rows: [[String: Any]] = [["query": "", "result": timed(["limit": 150])]]
        for query in queries { rows.append(["query": query, "result": timed(["query": query, "limit": 150])]) }
        for filter in filters { rows.append(["filters": filter, "result": timed(["filters": filter, "limit": 150])]) }
        _ = library.search(["limit": 150])
        let pagedBegin = Date(); _ = library.search(["limit": 150, "offset": 150]); let pagedMs = Int(Date().timeIntervalSince(pagedBegin) * 1000)
        let facets = (library.search(["limit": 1])["facets"] as? [String: [[String: Any]]] ?? [:]).mapValues { $0.count }
        return ["entries": library.locked { library.items.count }, "loadSeconds": loadSeconds, "cachedNextPageMs": pagedMs, "facetSizes": facets, "searches": rows]
    }

    /// Synthetic large-index regression. Uses no user libraries or persistent cache writes.
    static func stressTest(count: Int = 150_000) throws -> [String: Any] {
        let count = max(1000, min(500_000, count))
        let library = LibraryService(cacheDirectory: FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-stress-\(UUID().uuidString)"), defaultRoots: [], loadCacheAsynchronously: false)
        for index in 0..<count {
            let name = String(format: "Sample_%08d", index)
            let item = Item(path: "/synthetic/Pack-\(index % 100)/\(name).wav", root: "/synthetic", name: name,
                            kind: "sample", category: index % 2 == 0 ? "Kick" : "Snare", tags: ["Loop"], pack: "Pack-\(index % 100)",
                            bpm: 120, key: "C minor", size: 1024, modified: 1, metadataSource: "fixture", previewPath: nil)
            item.prepareSearchFields()
            // This benchmark measures in-memory search; real header IO has separate fixture tests.
            item.duration = 1; item.durationChecked = true
            library.items[item.path] = item
        }
        let begin = Date()
        let broad = library.search(["limit": 150])
        let broadTime = Date().timeIntervalSince(begin)
        let pageBegin = Date()
        let paged = library.search(["limit": 150, "offset": count / 2])
        let pageTime = Date().timeIntervalSince(pageBegin)
        let found = paged["items"] as? [[String: Any]] ?? []
        guard broad["total"] as? Int == count, found.count == 150,
              found.first?["name"] as? String == String(format: "Sample_%08d", count / 2),
              found.last?["name"] as? String == String(format: "Sample_%08d", count / 2 + 149) else {
            throw error("Library stress test failed: partial-selection pagination differs from full ordering.")
        }
        let queryBegin = Date()
        let filtered = library.search(["query": "tag:loop pack:Pack-17 bpm:120 key:Cminor"])
        let queryTime = Date().timeIntervalSince(queryBegin)
        guard filtered["total"] as? Int == (count + 82) / 100 else { throw error("Library stress test failed: cached normalized filters.") }
        let blocker = DispatchSemaphore(value: 0)
        library.searchQueue.async { blocker.wait() }
        let done = DispatchGroup(), resultLock = NSLock()
        var cancelled = 0, completed = 0, latest: [String: Any] = [:]
        for index in 0..<32 {
            done.enter()
            library.enqueueSearch(["query": String(format: "Sample_%08d", count - 1 - index)]) { result in
                resultLock.lock()
                completed += 1
                if result["cancelled"] as? Bool == true { cancelled += 1 }
                if index == 31 { latest = result }
                resultLock.unlock()
                done.leave()
            }
        }
        blocker.signal()
        guard done.wait(timeout: .now() + 30) == .success else { throw error("Library stress test failed: a coalesced search promise never completed.") }
        library.searchQueue.sync { }
        guard completed == 32, cancelled == 31, latest["total"] as? Int == 1,
              library.peakSearchComputations == 1, library.pendingSearch == nil else {
            throw error("Library stress test failed: search burst was not bounded to one worker and one pending request.")
        }
        return ["items": count, "broadSeconds": broadTime, "deepPageSeconds": pageTime, "filterSeconds": queryTime,
                "burstRequests": completed, "supersededRequests": cancelled, "peakConcurrentSearches": library.peakSearchComputations]
    }
#endif
}

/// Maschine-style kit browser over the indexed kits: Product (expansion) › Bank › Sub-bank, Type / Sub-type and
/// Character, with expansion artwork from NKSArtwork. Group headers are read lazily (64 KiB each) off the search
/// queue and remembered in kit-headers-v1.json by size and date, so the large library index format is unchanged.
final class KitCatalog: @unchecked Sendable {
    struct Source {
        var path: String, name: String, pack: String, rootName: String, category: String
        var size: Int64, modified: Double, tags: [String], previewPath: String?, soundCount: Int?, metadataError: String?, available: Bool
    }
    private struct Cached: Codable { var size: Int64; var modified: Double; var header: KitLibrary.GroupHeader? }
    private struct Entry {
        let source: Source, user: Bool, product: String, bank: String, subBank: String, vendor: String, author: String
        let types: [String], subtypes: [String], characters: [String], search: String, foldedName: String
    }
    static let userProduct = "Saved kits"
    static let separator = "\u{1f}"
    let artwork: NKSArtwork
    var onEvent: ((String, [String: Any]) -> Void)?
    private let cacheURL: URL?
    private let lock = NSLock()
    private let readQueue = DispatchQueue(label: "SuperTracker.kits.headers", qos: .utility)
    private var headers: [String: Cached] = [:]
    private var loaded = false, reading = false, revision = 0, pendingCount = 0, readCount = 0

    init(cacheDirectory: URL?, artwork: NKSArtwork) {
        self.cacheURL = cacheDirectory?.appendingPathComponent("kit-headers-v1.json")
        self.artwork = artwork
    }

    var status: [String: Any] { locked { ["reading": reading, "read": readCount, "pending": pendingCount, "revision": revision] } }

    private func loadIfNeeded() {
        guard !locked({ loaded }) else { return }
        var restored: [String: Cached] = [:]
        if let url = cacheURL, let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
           (attributes[.size] as? NSNumber)?.intValue ?? 0 < 64 * 1024 * 1024, let data = try? Data(contentsOf: url) {
            restored = (try? JSONDecoder().decode([String: Cached].self, from: data)) ?? [:]
        }
        locked { if !loaded { headers = restored; loaded = true } }
    }

    /// Cached headers for current files; unknown or changed Maschine groups are queued for one background read.
    private func headers(for sources: [Source]) -> [String: KitLibrary.GroupHeader] {
        loadIfNeeded()
        var result: [String: KitLibrary.GroupHeader] = [:], missing: [Source] = []
        locked {
            for source in sources where (source.path as NSString).pathExtension.lowercased() == "mxgrp" {
                if let cached = headers[source.path], cached.size == source.size, cached.modified == source.modified { if let header = cached.header { result[source.path] = header } }
                else if source.available { missing.append(source) }
            }
        }
        if !missing.isEmpty { read(missing) }
        return result
    }

    private func read(_ sources: [Source]) {
        let start = locked { () -> Bool in
            guard !reading else { return false }
            reading = true; pendingCount = sources.count; readCount = 0
            return true
        }
        guard start else { return }
        readQueue.async { [weak self] in
            guard let self else { return }
            var lastEmit = Date()
            for (index, source) in sources.enumerated() {
                let header = autoreleasepool { KitLibrary.groupHeader(at: URL(fileURLWithPath: source.path)) }
                self.locked { self.headers[source.path] = Cached(size: source.size, modified: source.modified, header: header); self.readCount = index + 1 }
                if index % 64 == 63 { Thread.sleep(forTimeInterval: 0.002) } // Leave disk bandwidth for playback.
                if Date().timeIntervalSince(lastEmit) > 0.8 { lastEmit = Date(); self.locked { self.revision += 1 }; self.onEvent?("library.kits.progress", self.status) }
            }
            self.locked { self.reading = false; self.pendingCount = 0; self.revision += 1 }
            self.persist()
            self.onEvent?("library.kits.progress", self.status)
        }
    }

    private func persist() {
        guard let url = cacheURL else { return }
        let snapshot = locked { headers }
        guard let data = try? JSONEncoder().encode(snapshot) else { return }
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
    }

    private func entry(_ source: Source, header: KitLibrary.GroupHeader?) -> Entry {
        let ext = (source.path as NSString).pathExtension.lowercased(), folded = LibraryService.foldText
        var product: String, bank = "", subBank = "", vendor = "", author = "", types: [String] = [], subtypes: [String] = [], characters: [String] = []
        // Library content carries an NI bank chain; user groups and SuperTracker kits do not.
        let user = ext == "stkit" || (header.map { $0.chain.isEmpty } ?? !source.path.lowercased().contains("/native access content/"))
        if let header, !header.chain.isEmpty {
            product = header.chain[0]; bank = header.chain.count > 1 ? header.chain[1] : ""; subBank = header.chain.count > 2 ? header.chain[2] : ""
            vendor = header.vendor; author = header.author
        } else if user {
            // Sub-folders of a kit folder (and a kit's own expansion field) act as products; loose kits share one.
            product = source.pack == source.rootName || ["kits", "groups"].contains(source.pack.lowercased()) ? Self.userProduct : source.pack
            author = header?.author ?? ""
        } else { product = source.pack }
        if let header {
            types = Array(Set(header.types.map { $0[0] })).sorted()
            subtypes = Array(Set(header.types.filter { $0.count > 1 }.map { $0.prefix(2).joined(separator: Self.separator) })).sorted()
        } else if ext == "stkit" {
            types = [source.category.isEmpty || source.category == "Kit" ? "Kits" : source.category]
            characters = source.tags.filter { $0 != "Other" } // "Other" is the inferred placeholder, not a kit tag.
        } else { types = ["Kits"] }
        let search = folded(([source.name, product, bank, subBank, author] + types + subtypes.map { $0.replacingOccurrences(of: Self.separator, with: " ") } + characters).joined(separator: " "))
        return Entry(source: source, user: user, product: product, bank: bank, subBank: subBank, vendor: vendor, author: author,
                     types: types, subtypes: subtypes, characters: characters, search: search, foldedName: folded(source.name))
    }

    func browse(_ payload: [String: Any], sources: [Source], favorites: Set<String>) -> [String: Any] {
        let known = headers(for: sources)
        let entries = sources.map { entry($0, header: known[$0.path]) }
        let words = LibraryService.foldText(String((payload["query"] as? String ?? "").prefix(256))).split(whereSeparator: \.isWhitespace).map(String.init)
        let user = payload["user"] as? Bool ?? false
        let product = payload["product"] as? String
        let bank = product == nil ? nil : payload["bank"] as? String
        let subBank = bank == nil ? nil : payload["subBank"] as? String
        func set(_ key: String) -> Set<String> { Set((payload[key] as? [String] ?? []).prefix(128)) }
        let types = set("types"), subtypes = set("subtypes"), characters = set("characters")
        let onlyFavorites = payload["favorites"] as? Bool ?? false
        let offset = max(0, (payload["offset"] as? NSNumber)?.intValue ?? 0)
        let limit = min(1000, max(1, (payload["limit"] as? NSNumber)?.intValue ?? 200))
        var counts = [0, 0], products: [String: (count: Int, vendor: String)] = [:]
        var banks: [String: Int] = [:], subBanks: [String: Int] = [:], typeCounts: [String: Int] = [:], subtypeCounts: [String: Int] = [:], characterCounts: [String: Int] = [:]
        var matches: [(entry: Entry, score: Int)] = []
        for entry in entries {
            guard !onlyFavorites || favorites.contains(entry.source.path), words.allSatisfy({ entry.search.contains($0) }) else { continue }
            counts[entry.user ? 1 : 0] += 1
            guard entry.user == user else { continue }
            let p = product == nil || entry.product == product!
            let b = (bank == nil || entry.bank == bank!) && (subBank == nil || entry.subBank == subBank!)
            let t = types.isEmpty || entry.types.contains(where: types.contains)
            let s = subtypes.isEmpty || entry.subtypes.contains(where: subtypes.contains)
            let c = characters.isSubset(of: entry.characters)
            if t && s && c { products[entry.product, default: (0, entry.vendor)].count += 1 }
            if product != nil && p && t && s && c { banks[entry.bank, default: 0] += 1 }
            if bank != nil && p && entry.bank == bank! && t && s && c && !entry.subBank.isEmpty { subBanks[entry.subBank, default: 0] += 1 }
            if p && b && c { for value in entry.types { typeCounts[value, default: 0] += 1 } }
            if p && b && t && c { for value in entry.subtypes where types.isEmpty || types.contains(where: { value.hasPrefix($0 + Self.separator) }) { subtypeCounts[value, default: 0] += 1 } }
            if p && b && t && s { for value in entry.characters { characterCounts[value, default: 0] += 1 } }
            guard p && b && t && s && c else { continue }
            matches.append((entry, words.reduce(0) { $0 + (entry.foldedName.hasPrefix($1) ? 3 : entry.foldedName.contains($1) ? 2 : 0) }))
        }
        matches.sort { left, right in
            if left.score != right.score { return left.score > right.score }
            if product == nil, left.entry.product != right.entry.product { return left.entry.product.localizedCaseInsensitiveCompare(right.entry.product) == .orderedAscending }
            if left.entry.foldedName != right.entry.foldedName { return left.entry.foldedName.localizedStandardCompare(right.entry.foldedName) == .orderedAscending }
            return left.entry.source.path < right.entry.source.path
        }
        func art(_ product: String, vendor: String, bank: String = "", user: Bool = false) -> NKSArtwork.Match {
            product == Self.userProduct || vendor.isEmpty && !product.isEmpty && user ? NKSArtwork.Match() : artwork.match(product: product, vendor: vendor.isEmpty ? "Native Instruments" : vendor, bank: bank)
        }
        let page = matches.dropFirst(offset).prefix(limit).map { match -> [String: Any] in
            let entry = match.entry, source = entry.source, ext = (source.path as NSString).pathExtension.lowercased()
            var value: [String: Any] = ["path": source.path, "id": source.path, "kind": "kit", "name": source.name, "product": entry.product, "bank": entry.bank, "subBank": entry.subBank,
                "vendor": entry.vendor, "author": entry.author, "types": entry.types, "subtypes": entry.subtypes, "characters": entry.characters, "tags": entry.characters,
                "favorite": favorites.contains(source.path), "extension": ext, "user": entry.user, "kitFormat": ext == "stkit" ? "supertracker-kit" : "maschine-group",
                "pack": entry.product, "available": source.available]
            if let preview = source.previewPath { value["previewPath"] = preview }
            if let count = source.soundCount { value["soundCount"] = count }
            if let error = source.metadataError { value["metadataError"] = error }
            let match = art(entry.product, vendor: entry.vendor, bank: entry.bank, user: entry.user)
            if let key = match.bank ?? match.product { value["art"] = key }
            if let key = match.product { value["productArt"] = key }
            if let color = match.color { value["color"] = color }
            if !source.available { let parts = source.path.split(separator: "/"); if parts.count > 1, parts[0] == "Volumes" { value["volume"] = String(parts[1]) } }
            return value
        }
        func facet(_ counts: [String: Int], byCount: Bool = false) -> [[String: Any]] {
            counts.sorted { byCount && $0.value != $1.value ? $0.value > $1.value : $0.key.localizedCaseInsensitiveCompare($1.key) == .orderedAscending }.prefix(1000).map { ["name": $0.key, "count": $0.value] }
        }
        let productList = products.sorted { $0.key.localizedCaseInsensitiveCompare($1.key) == .orderedAscending }.map { name, value -> [String: Any] in
            var result: [String: Any] = ["name": name, "count": value.count, "vendor": value.vendor]
            let match = art(name, vendor: value.vendor, user: user)
            if let key = match.product { result["art"] = key }
            if let color = match.color { result["color"] = color }
            return result
        }
        let productVendor = product.flatMap { name in products[name]?.vendor } ?? ""
        let bankList = facet(banks).map { value -> [String: Any] in
            guard let product, let name = value["name"] as? String, !name.isEmpty else { return value }
            let match = art(product, vendor: productVendor, bank: name, user: user)
            var result = value
            if let key = match.bank { result["art"] = key }
            if let color = match.color { result["color"] = color }
            return result
        }
        let subtypeList = facet(subtypeCounts).map { value -> [String: Any] in
            let parts = (value["name"] as? String ?? "").components(separatedBy: Self.separator)
            return value.merging(["type": parts.first ?? "", "label": parts.last ?? ""]) { first, _ in first }
        }
        return ["items": Array(page), "total": matches.count, "offset": offset, "limit": limit, "products": productList, "banks": bankList, "subBanks": facet(subBanks),
                "types": facet(typeCounts), "subtypes": subtypeList, "characters": facet(characterCounts, byCount: true),
                "counts": ["library": counts[0], "user": counts[1]], "status": status]
    }

    private func locked<T>(_ body: () throws -> T) rethrows -> T { lock.lock(); defer { lock.unlock() }; return try body() }
}

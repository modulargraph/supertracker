import Foundation
import Darwin
#if canImport(VST3Host)
import VST3Host
#endif

/// The identity is the factory's audio-component class ID, never a guessed bundle name.
struct VST3PluginDescriptor: Codable, Equatable {
    let id: String
    let classID: String
    let bundlePath: String
    let name: String
    let manufacturer: String
    let version: String
    let category: String
    let isInstrument: Bool

    init(classID: String, bundlePath: String, name: String, manufacturer: String,
         version: String, category: String, isInstrument: Bool? = nil) {
        self.classID = classID.uppercased(); id = "vst3:" + classID.uppercased()
        self.bundlePath = bundlePath; self.name = name; self.manufacturer = manufacturer
        self.version = version; self.category = category
        self.isInstrument = isInstrument ?? category.split(separator: "|").contains { $0.caseInsensitiveCompare("Instrument") == .orderedSame }
    }

    var dictionary: [String: Any] {
        ["id": id, "classID": classID, "bundlePath": bundlePath, "name": name,
         "manufacturer": manufacturer, "vendor": manufacturer, "version": version,
         "category": category, "isInstrument": isInstrument,
         "kind": isInstrument ? "instrument" : "effect",
         "type": isInstrument ? "instrument" : "effect", "format": "VST3"]
    }

    fileprivate var valid: Bool {
        classID.count == 32 && classID.utf8.allSatisfy { (48...57).contains($0) || (65...70).contains($0) } &&
        id == "vst3:" + classID && bundlePath.hasPrefix("/") && bundlePath.utf8.count <= 16_384 &&
        !name.isEmpty && name.utf8.count <= 1_024 && manufacturer.utf8.count <= 1_024 &&
        version.utf8.count <= 1_024 && category.utf8.count <= 1_024
    }
}

/// Factory loading is untrusted native code. Only a disposable copy of the app opens
/// a module. The UI reads value snapshots; filesystem and child supervision stay off
/// the main thread. A bad module cannot crash or indefinitely hold up the UI process.
final class VST3Catalogue: @unchecked Sendable {
    struct ScanError: Codable, Equatable {
        let path: String
        let message: String
        var dictionary: [String: Any] { ["path": path, "message": message] }
    }
    struct Report {
        let records: [VST3PluginDescriptor]
        let scanned: Int
        let cached: Int
        let errors: [ScanError]
        let cancelled: Bool
        var dictionary: [String: Any] {
            ["plugins": records.map(\.dictionary), "scanned": scanned, "cached": cached,
             "failed": errors.count, "errors": errors.map(\.dictionary), "cancelled": cancelled]
        }
    }
    struct Configuration {
        var roots: [URL] = [FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Audio/Plug-Ins/VST3"),
                            URL(fileURLWithPath: "/Library/Audio/Plug-Ins/VST3")]
        var cacheURL: URL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/SuperTracker/Plugins/vst3-catalogue-v1.json")
        var executableURL: URL = Bundle.main.executableURL ?? URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
        var timeout: TimeInterval = 20
        var maximumBundles = 4_096
        var maximumClasses = 4_096
        var maximumOutputBytes = 2 * 1_024 * 1_024
    }
    struct ScanEnvelope: Codable {
        let schema: Int
        let bundlePath: String
        let records: [VST3PluginDescriptor]
        let error: String?
    }
    private struct Fingerprint: Codable, Equatable {
        let executablePath: String
        let size: UInt64
        let modificationTime: Double
        let inode: UInt64
        let version: String
        let plistModificationTime: Double
    }
    private struct CacheEntry: Codable {
        let path: String
        let fingerprint: Fingerprint
        let records: [VST3PluginDescriptor]
        let error: String?
    }
    private struct Cache: Codable { let schema: Int; let entries: [CacheEntry] }
    private final class Job: @unchecked Sendable {
        let force: Bool
        let token = Cancellation()
        var waiters: [CheckedContinuation<Report, Never>] = []
        // Accessed only under the catalogue lock, including while the worker runs.
        var preferredNames: [String] = []
        init(force: Bool) { self.force = force }
    }
    private final class Cancellation: @unchecked Sendable {
        private let lock = NSLock()
        private var stopped = false
        var cancelled: Bool { lock.lock(); defer { lock.unlock() }; return stopped }
        func cancel() { lock.lock(); stopped = true; lock.unlock() }
    }
    private enum Failure: LocalizedError {
        case invalid(String)
        var errorDescription: String? { switch self { case .invalid(let text): return text } }
    }

    static let shared = VST3Catalogue()
    private let configuration: Configuration
    private let queue = DispatchQueue(label: "SuperTracker.vst3-discovery", qos: .utility)
    private let lock = NSLock()
    private var entries: [CacheEntry] = []
    private var snapshot: [VST3PluginDescriptor] = []
    private var job: Job?
    private var targetedWorkers: [UUID: Cancellation] = [:]
    private var progressCallback: (([String: Any]) -> Void)?
    private var progressState: [String: Any] = ["scanning": false, "completed": 0, "total": 0]

    init(configuration: Configuration = .init()) {
        self.configuration = configuration
        if let size = (try? FileManager.default.attributesOfItem(atPath: configuration.cacheURL.path)[.size]) as? NSNumber,
           size.intValue <= 16 * 1_024 * 1_024,
           let bytes = try? Data(contentsOf: configuration.cacheURL),
           let cache = try? JSONDecoder().decode(Cache.self, from: bytes), cache.schema == 1 {
            entries = cache.entries.filter { entry in
                entry.path.hasPrefix("/") && entry.records.count <= configuration.maximumClasses &&
                entry.records.allSatisfy { $0.valid && $0.bundlePath == entry.path }
            }
            snapshot = Self.preferred(entries.flatMap(\.records), roots: configuration.roots)
        }
    }

    var records: [VST3PluginDescriptor] { lock.lock(); defer { lock.unlock() }; return snapshot }
    var status: [String: Any] { lock.lock(); defer { lock.unlock() }; return progressState }
    private var isScanning: Bool { lock.lock(); defer { lock.unlock() }; return job != nil }
    var onProgress: (([String: Any]) -> Void)? {
        get { lock.lock(); defer { lock.unlock() }; return progressCallback }
        set { lock.lock(); progressCallback = newValue; lock.unlock() }
    }

    func resolve(identifier: String, bundlePath: String? = nil) -> VST3PluginDescriptor? {
        let id = Self.canonicalIdentifier(identifier)
        lock.lock(); defer { lock.unlock() }
        if let path = bundlePath {
            guard path.hasPrefix("/") else { return nil }
            return entries.first(where: { $0.path == URL(fileURLWithPath: path).standardizedFileURL.path })?.records.first(where: { $0.id == id })
        }
        return snapshot.first { $0.id == id }
    }

    /// Concurrent refreshes share one scan. `force` retries quarantined failures and
    /// unchanged modules when starting a new scan; callers can explicitly cancel first.
    func refresh(force: Bool = false) async -> Report {
        await withCheckedContinuation { continuation in
            enqueue(force: force, preferredNames: [], continuation: continuation)
        }
    }

    /// Preset names only prioritize factories to inspect. Only the verified class ID
    /// can resolve the request. A first NKS load need not wait for every other factory.
    func resolveOrScan(identifier: String, preferredNames: [String] = [], preferredBundlePath: String? = nil) async throws -> VST3PluginDescriptor {
        if let path = preferredBundlePath {
            guard path.hasPrefix("/") else { throw Failure.invalid("The requested VST3 bundle path must be absolute.") }
            let bundle = URL(fileURLWithPath: path).standardizedFileURL
            // Check the executable/version fingerprint even on a cache hit, so a
            // replaced or removed installation cannot reuse stale version metadata.
            // This opens only the selected factory when its cache is absent/stale.
            let verified = try await scanBundle(bundle)
            let wanted = Self.canonicalIdentifier(identifier)
            guard let exact = verified.first(where: { $0.id == wanted }) else {
                throw Failure.invalid("The requested VST3 bundle \(bundle.path) does not expose class \(wanted). No other installation was substituted.")
            }
            return exact
        }
        if let found = resolve(identifier: identifier) { return found }
        enqueue(force: false, preferredNames: preferredNames, continuation: nil)
        let deadline = ProcessInfo.processInfo.systemUptime + 180
        while true {
            try Task.checkCancellation()
            if let found = resolve(identifier: identifier) { return found }
            if !isScanning { throw Failure.invalid("No installed VST3 instrument exposes class \(identifier). Scan plug-ins or install the matching instrument.") }
            if ProcessInfo.processInfo.systemUptime >= deadline {
                throw Failure.invalid("The matching VST3 class has not been found yet. Plug-in discovery continues in the background; try again when the scan finishes.")
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    /// Same convention as NKSHostPreset.isVST2Compatible; kept local so the catalogue builds standalone.
    static func isVST2Compatible(classID: String, vst2Magic: UInt32) -> Bool {
        let characters = Array(classID.uppercased())
        guard characters.count == 32 else { return false }
        let magic = String(format: "%08X", vst2Magic)
        return (String(characters[0..<6]) == "565354" && String(characters[6..<14]) == magic) ||
            (String(characters[0..<8]) == "41727475" && String(characters[16..<24]) == magic)
    }

    func resolveCompatible(vst2Magic: UInt32, isInstrument: Bool? = nil) -> VST3PluginDescriptor? {
        Self.preferredCompatible(records, vst2Magic: vst2Magic, isInstrument: isInstrument)
    }

    /// Several installed releases (e.g. two major versions with distinct classes) can
    /// replace one VST2. Each class keeps its usual installation (first in snapshot
    /// order, as `resolve(identifier:)`); among classes the newest version wins, then
    /// class ID and bundle path, so the choice never depends on scan completion order.
    static func preferredCompatible(_ records: [VST3PluginDescriptor], vst2Magic: UInt32, isInstrument: Bool? = nil) -> VST3PluginDescriptor? {
        var seen = Set<String>()
        let candidates = records.filter { isVST2Compatible(classID: $0.classID, vst2Magic: vst2Magic) &&
            (isInstrument == nil || $0.isInstrument == isInstrument) && seen.insert($0.classID).inserted }
        return candidates.min { a, b in
            let order = compareVersions(a.version, b.version)
            if order != .orderedSame { return order == .orderedDescending }
            return (a.classID, a.bundlePath) < (b.classID, b.bundlePath)
        }
    }

    /// Numeric dotted-version order ("1.10" > "1.9"); non-numeric text breaks ties.
    static func compareVersions(_ a: String, _ b: String) -> ComparisonResult {
        func parts(_ text: String) -> [Int] { text.split { !$0.isNumber }.map { Int($0.prefix(9)) ?? 0 } }
        let left = parts(a), right = parts(b)
        for index in 0..<max(left.count, right.count) {
            let x = index < left.count ? left[index] : 0, y = index < right.count ? right[index] : 0
            if x != y { return x < y ? .orderedAscending : .orderedDescending }
        }
        return a == b ? .orderedSame : (a < b ? .orderedAscending : .orderedDescending)
    }

    /// NKS identity policy. A VST3 class ID requires that exact class; when it is not
    /// installed and the preset also names a VST2 (VST.magic), the native VST2 build is
    /// used instead. A VST2-only preset prefers the native VST2, then a compatible VST3.
    static func resolveNKSComponent(vst3ClassID: String?, vst2Magic: UInt32?,
        exactVST3: (String) async throws -> String, nativeVST2: (UInt32) async throws -> String?,
        compatibleVST3: (UInt32) async throws -> String) async throws -> String {
        if let classID = vst3ClassID {
            do { return try await exactVST3("vst3:" + classID) }
            catch let error where !(error is CancellationError) {
                guard let magic = vst2Magic, let native = try await nativeVST2(magic) else { throw error }
                return native
            }
        }
        guard let magic = vst2Magic else { throw Failure.invalid("This NKS preset does not identify a VST2 or VST3 plug-in.") }
        if let native = try await nativeVST2(magic) { return native }
        return try await compatibleVST3(magic)
    }

    /// Finds the VST3 release that replaces a VST2 plug-in, scanning likely bundles first.
    func resolveCompatibleOrScan(vst2Magic: UInt32, isInstrument: Bool? = nil, preferredNames: [String] = []) async throws -> VST3PluginDescriptor {
        if let found = resolveCompatible(vst2Magic: vst2Magic, isInstrument: isInstrument) { return found }
        enqueue(force: false, preferredNames: preferredNames, continuation: nil)
        let deadline = ProcessInfo.processInfo.systemUptime + 180
        while true {
            try Task.checkCancellation()
            if let found = resolveCompatible(vst2Magic: vst2Magic, isInstrument: isInstrument) { return found }
            if !isScanning { throw Failure.invalid("No installed VST3 plug-in replaces this VST2 plug-in (\(Self.fourCC(vst2Magic))).") }
            if ProcessInfo.processInfo.systemUptime >= deadline {
                throw Failure.invalid("The matching plug-in has not been found yet. Plug-in discovery continues in the background; try again when the scan finishes.")
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    static func fourCC(_ value: UInt32) -> String {
        let bytes = [24, 16, 8, 0].map { UInt8((value >> UInt32($0)) & 0xFF) }
        return bytes.allSatisfy { (32...126).contains($0) } ? String(decoding: bytes, as: UTF8.self) : String(format: "%08X", value)
    }

    private func enqueue(force: Bool, preferredNames: [String], continuation: CheckedContinuation<Report, Never>?) {
        let hints = preferredNames.prefix(32).map(Self.nameKey).filter { $0.count >= 3 }
        lock.lock()
        if let job, !job.token.cancelled {
            if let continuation { job.waiters.append(continuation) }
            for hint in hints where !job.preferredNames.contains(hint) { job.preferredNames.append(hint) }
            lock.unlock(); return
        }
        let current = Job(force: force)
        if let continuation { current.waiters.append(continuation) }
        current.preferredNames = hints
        job = current
        lock.unlock()
        queue.async { [self] in
            lock.lock(); let previous = entries; lock.unlock()
            let result = perform(current, previous: previous)
            lock.lock()
            let waiters = current.waiters
            if job === current { job = nil }
            lock.unlock()
            for waiter in waiters { waiter.resume(returning: result) }
        }
    }

    func cancel() {
        lock.lock(); let tokens = Array(targetedWorkers.values) + (job.map { [$0.token] } ?? []); lock.unlock()
        for token in tokens { token.cancel() }
    }

    /// Verify one explicitly selected bundle, merge its classes into the shared cache,
    /// and avoid starting a complete installation scan. Serialized with full refreshes.
    func scanBundle(_ bundle: URL, force: Bool = false) async throws -> [VST3PluginDescriptor] {
        let token = Cancellation(), id = UUID(), url = bundle.standardizedFileURL
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                lock.lock(); targetedWorkers[id] = token; lock.unlock()
                queue.async { [self] in
                    defer { lock.lock(); targetedWorkers.removeValue(forKey: id); lock.unlock() }
                    do {
                        if token.cancelled { throw CancellationError() }
                        guard url.isFileURL, url.pathExtension.lowercased() == "vst3" else {
                            throw Failure.invalid("Choose an installed .vst3 bundle.")
                        }
                        let fingerprint = try Self.fingerprint(url)
                        lock.lock(); let hit = entries.first { $0.path == url.path }; lock.unlock()
                        if !force, let hit, hit.fingerprint == fingerprint {
                            if let error = hit.error { throw Failure.invalid(error) }
                            continuation.resume(returning: hit.records); return
                        }
                        progress(["scanning": true, "completed": 0, "total": 1, "path": url.path])
                        let found: [VST3PluginDescriptor]
                        do { found = try scan(url, cancelled: token) }
                        catch {
                            if token.cancelled { throw CancellationError() }
                            let item = CacheEntry(path: url.path, fingerprint: fingerprint, records: [], error: error.localizedDescription)
                            try? mergeAndSave(item)
                            throw error
                        }
                        if token.cancelled { throw CancellationError() }
                        try mergeAndSave(.init(path: url.path, fingerprint: fingerprint, records: found, error: nil))
                        progress(["scanning": false, "completed": 1, "total": 1, "path": url.path, "pluginCount": records.count, "errors": []])
                        continuation.resume(returning: found)
                    } catch {
                        progress(["scanning": false, "completed": 0, "total": 1, "path": url.path,
                                  "cancelled": token.cancelled, "errors": [ScanError(path: url.path, message: error.localizedDescription).dictionary]])
                        continuation.resume(throwing: error)
                    }
                }
            }
        } onCancel: { token.cancel() }
    }

    private func mergeAndSave(_ entry: CacheEntry) throws {
        lock.lock()
        entries.removeAll { $0.path == entry.path }; entries.append(entry)
        snapshot = Self.preferred(entries.flatMap(\.records), roots: configuration.roots)
        let copy = entries
        lock.unlock()
        try saveCache(copy)
    }

    private func saveCache(_ entries: [CacheEntry]) throws {
        try FileManager.default.createDirectory(at: configuration.cacheURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try JSONEncoder().encode(Cache(schema: 1, entries: entries))
        guard data.count <= 16 * 1_024 * 1_024 else { throw Failure.invalid("VST3 catalogue exceeds its cache size limit.") }
        try data.write(to: configuration.cacheURL, options: .atomic)
    }

    private func progress(_ value: [String: Any]) {
        lock.lock(); progressState = value; let callback = progressCallback; lock.unlock()
        if let callback { DispatchQueue.main.async { callback(value) } }
    }

    private func perform(_ job: Job, previous: [CacheEntry]) -> Report {
        var errors: [ScanError] = [], fresh: [CacheEntry] = [], scanned = 0, cached = 0
        let bundles = discover(cancelled: job.token, errors: &errors)
        let old = Dictionary(previous.map { ($0.path, $0) }, uniquingKeysWith: { first, _ in first })
        progress(["scanning": true, "completed": 0, "total": bundles.count, "errors": errors.map(\.dictionary)])
        var pending = bundles, completed = 0, visited = Set<String>()
        // A long first scan publishes partial results, but re-sorting the whole catalogue
        // after every bundle made the scan quadratic; publish at most every quarter second.
        var lastPublication = Date.distantPast
        let nameKeys = Dictionary(uniqueKeysWithValues: bundles.map { ($0.path, Self.nameKey($0.deletingPathExtension().lastPathComponent)) })
        while !pending.isEmpty {
            if job.token.cancelled { break }
            lock.lock(); let preferences = job.preferredNames; lock.unlock()
            func score(_ url: URL) -> Int {
                let name = nameKeys[url.path] ?? ""
                guard name.count >= 3 else { return Int.max }
                return preferences.firstIndex { name == $0 || name.contains($0) || $0.contains(name) } ?? Int.max
            }
            var next = 0, best = score(pending[0])
            for index in pending.indices.dropFirst() where !preferences.isEmpty {
                let value = score(pending[index])
                if value < best { next = index; best = value }
            }
            let bundle = pending.remove(at: next)
            do {
                let fingerprint = try Self.fingerprint(bundle)
                if !job.force, let hit = old[bundle.path], hit.fingerprint == fingerprint {
                    fresh.append(hit); cached += 1
                    if let message = hit.error { errors.append(.init(path: bundle.path, message: message)) }
                } else {
                    scanned += 1
                    do {
                        let found = try scan(bundle, cancelled: job.token)
                        fresh.append(.init(path: bundle.path, fingerprint: fingerprint, records: found, error: nil))
                    } catch {
                        if job.token.cancelled { break }
                        let message = error.localizedDescription
                        errors.append(.init(path: bundle.path, message: message))
                        fresh.append(.init(path: bundle.path, fingerprint: fingerprint, records: [], error: message))
                    }
                }
            } catch { errors.append(.init(path: bundle.path, message: error.localizedDescription)) }
            // Publish complete bundle results during a long first scan, retaining the
            // prior snapshot for as-yet unvisited modules. Cancel never loses that data.
            if fresh.last?.path == bundle.path { visited.insert(bundle.path) }
            if Date().timeIntervalSince(lastPublication) >= 0.25 || pending.isEmpty {
                lastPublication = Date()
                let partial = fresh + previous.filter { !visited.contains($0.path) }
                let ordered = Self.preferred(partial.flatMap(\.records), roots: configuration.roots)
                lock.lock(); entries = partial; snapshot = ordered; lock.unlock()
            }
            completed += 1
            progress(["scanning": true, "completed": completed, "total": bundles.count,
                      "path": bundle.path, "pluginCount": records.count, "errors": errors.map(\.dictionary)])
        }
        let cancelled = job.token.cancelled
        if cancelled {
            let visited = Set(fresh.map(\.path)); fresh += previous.filter { !visited.contains($0.path) }
        }
        let final = Self.preferred(fresh.flatMap(\.records), roots: configuration.roots)
        lock.lock(); entries = fresh; snapshot = final; lock.unlock()
        do { try saveCache(fresh) }
        catch { errors.append(.init(path: configuration.cacheURL.path, message: "Cannot save VST3 cache: \(error.localizedDescription)")) }
        progress(["scanning": false, "completed": fresh.count, "total": bundles.count,
                  "pluginCount": final.count, "cancelled": cancelled, "errors": errors.map(\.dictionary)])
        return Report(records: final, scanned: scanned, cached: cached, errors: errors, cancelled: cancelled)
    }

    private func discover(cancelled: Cancellation, errors: inout [ScanError]) -> [URL] {
        var paths = Set<String>(), result: [URL] = [], enumerationErrors: [ScanError] = [], visited = 0
        defer { errors += enumerationErrors }
        for root in configuration.roots {
            if cancelled.cancelled { break }
            if !FileManager.default.fileExists(atPath: root.path) { continue }
            if root.pathExtension.lowercased() == "vst3" {
                let url = root.standardizedFileURL
                if paths.insert(url.path).inserted { result.append(url) }
                continue
            }
            guard let walker = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey],
                options: [.skipsHiddenFiles], errorHandler: { url, error in
                    enumerationErrors.append(.init(path: url.path, message: error.localizedDescription)); return true
                }) else { errors.append(.init(path: root.path, message: "Cannot read VST3 folder.")); continue }
            while let url = walker.nextObject() as? URL {
                if cancelled.cancelled { break }
                visited += 1
                if visited > 100_000 {
                    enumerationErrors.append(.init(path: root.path, message: "VST3 scan reached its 100000-entry directory limit."))
                    return sortBundles(result)
                }
                if url.pathExtension.lowercased() == "vst3" {
                    walker.skipDescendants()
                    let normalized = url.standardizedFileURL
                    if paths.insert(normalized.path).inserted { result.append(normalized) }
                    if result.count >= configuration.maximumBundles {
                        errors.append(.init(path: root.path, message: "VST3 scan reached the \(configuration.maximumBundles)-bundle limit."))
                        return sortBundles(result)
                    }
                }
            }
        }
        return sortBundles(result)
    }

    private func sortBundles(_ bundles: [URL]) -> [URL] {
        func priority(_ path: String) -> Int {
            configuration.roots.firstIndex { path == $0.path || path.hasPrefix($0.path + "/") } ?? configuration.roots.count
        }
        return bundles.sorted {
            let a = priority($0.path), b = priority($1.path)
            return a == b ? $0.path < $1.path : a < b
        }
    }

    private static func fingerprint(_ bundle: URL) throws -> Fingerprint {
        let plist = bundle.appendingPathComponent("Contents/Info.plist")
        guard let size = (try FileManager.default.attributesOfItem(atPath: plist.path)[.size]) as? NSNumber,
              size.intValue <= 1_024 * 1_024 else { throw Failure.invalid("VST3 Info.plist exceeds its size limit.") }
        let data = try Data(contentsOf: plist)
        guard data.count <= 1_024 * 1_024,
              let info = try PropertyListSerialization.propertyList(from: data, options: [], format: nil) as? [String: Any],
              let name = info["CFBundleExecutable"] as? String, !name.isEmpty,
              !name.contains("/"), !name.contains("\\"), name != ".", name != ".." else {
            throw Failure.invalid("VST3 bundle has no valid executable in Contents/Info.plist.")
        }
        let executable = bundle.appendingPathComponent("Contents/MacOS").appendingPathComponent(name)
        let attributes = try FileManager.default.attributesOfItem(atPath: executable.path)
        let version = (info["CFBundleShortVersionString"] as? String ?? "") + ":" + (info["CFBundleVersion"] as? String ?? "")
        return Fingerprint(executablePath: executable.path, size: (attributes[.size] as? NSNumber)?.uint64Value ?? 0,
            modificationTime: (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0,
            inode: (attributes[.systemFileNumber] as? NSNumber)?.uint64Value ?? 0, version: version,
            plistModificationTime: ((try? FileManager.default.attributesOfItem(atPath: plist.path)[.modificationDate]) as? Date)?.timeIntervalSince1970 ?? 0)
    }

    private func scan(_ bundle: URL, cancelled: Cancellation) throws -> [VST3PluginDescriptor] {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-vst3-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true,
                                              attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: folder) }
        let output = folder.appendingPathComponent("result.json")
        let child = Process(); child.executableURL = configuration.executableURL
        child.arguments = ["--vst3-scan", bundle.path, "--output", output.path]
        // Some plug-ins log megabytes during factory entry. Never pipe that into a
        // finite unread buffer or interpret plug-in stdout as the result protocol.
        child.standardInput = FileHandle.nullDevice; child.standardOutput = FileHandle.nullDevice; child.standardError = FileHandle.nullDevice
        try child.run()
        let deadline = ProcessInfo.processInfo.systemUptime + max(0.05, configuration.timeout)
        while child.isRunning && !cancelled.cancelled && ProcessInfo.processInfo.systemUptime < deadline { usleep(10_000) }
        if child.isRunning {
            child.terminate()
            let grace = ProcessInfo.processInfo.systemUptime + 0.25
            while child.isRunning && ProcessInfo.processInfo.systemUptime < grace { usleep(5_000) }
            if child.isRunning { _ = Darwin.kill(child.processIdentifier, SIGKILL) }
            let reapDeadline = ProcessInfo.processInfo.systemUptime + 1
            while child.isRunning && ProcessInfo.processInfo.systemUptime < reapDeadline { usleep(5_000) }
            if child.isRunning { throw Failure.invalid("The isolated VST3 helper did not exit after termination; the scan was abandoned.") }
            throw Failure.invalid(cancelled.cancelled ? "VST3 scan cancelled." : "VST3 factory scan timed out after \(configuration.timeout) seconds.")
        }
        guard child.terminationReason == .exit && child.terminationStatus == 0 else {
            throw Failure.invalid("VST3 factory helper exited unsuccessfully (\(child.terminationStatus)); the plug-in was skipped.")
        }
        let attributes = try FileManager.default.attributesOfItem(atPath: output.path)
        guard let bytes = attributes[.size] as? NSNumber, bytes.intValue <= configuration.maximumOutputBytes else {
            throw Failure.invalid("VST3 factory result exceeds its size limit.")
        }
        let envelope = try JSONDecoder().decode(ScanEnvelope.self, from: Data(contentsOf: output))
        guard envelope.schema == 1, envelope.bundlePath == bundle.path,
              envelope.records.count <= configuration.maximumClasses,
              envelope.records.allSatisfy({ $0.valid && $0.bundlePath == bundle.path }) else {
            throw Failure.invalid("VST3 factory helper returned an invalid class catalogue.")
        }
        if let error = envelope.error, !error.isEmpty { throw Failure.invalid(error) }
        return Self.preferred(envelope.records, roots: [bundle])
    }

    /// One class per ID (the bundle in the earliest root wins), ordered by name. Root
    /// priority is computed once per bundle, not inside the sort comparator.
    static func preferred(_ values: [VST3PluginDescriptor], roots: [URL]) -> [VST3PluginDescriptor] {
        var priorities: [String: Int] = [:]
        for path in Set(values.map(\.bundlePath)) {
            priorities[path] = roots.firstIndex { path == $0.path || path.hasPrefix($0.path + "/") } ?? roots.count
        }
        var ids = Set<String>()
        let unique = values.map { (priority: priorities[$0.bundlePath] ?? roots.count, value: $0) }.sorted {
            let a = $0.value, b = $1.value
            return $0.priority == $1.priority ? (a.bundlePath == b.bundlePath ? a.id < b.id : a.bundlePath < b.bundlePath) : $0.priority < $1.priority
        }.map(\.value).filter { ids.insert($0.id).inserted }
        return unique.sorted {
            let order = $0.name.localizedStandardCompare($1.name)
            return order == .orderedSame ? $0.id < $1.id : order == .orderedAscending
        }
    }

    private static func nameKey(_ name: String) -> String {
        String(name.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) })
    }

    private static func canonicalIdentifier(_ identifier: String) -> String {
        identifier.lowercased().hasPrefix("vst3:") ? "vst3:" + identifier.dropFirst(5).uppercased() : "vst3:" + identifier.uppercased()
    }

    /// Entry point before normal app initialization. Call on the main thread. Output
    /// is an atomic JSON file: vendor diagnostics cannot contaminate the protocol.
    static func runScanCommand(bundle: URL, output: URL) -> Int32 {
        guard Thread.isMainThread else { return 2 }
        var found: [VST3PluginDescriptor] = [], failure: String?
        #if canImport(VST3Host)
        // Scanner processes create no instances. Release factories while vendor
        // statics are still alive, before the app's ordinary process-exit handlers.
        defer { st_vst3_release_unused_modules() }
        let info = NSDictionary(contentsOf: bundle.appendingPathComponent("Contents/Info.plist")) as? [String: Any] ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? info["CFBundleVersion"] as? String ?? ""
        let collector = FactoryCollector(bundle: bundle, version: version)
        var error = [CChar](repeating: 0, count: 4_096)
        _ = st_vst3_scan_bundle(bundle.path, { item, context in
            guard let item, let context else { return }
            let collector = Unmanaged<FactoryCollector>.fromOpaque(context).takeUnretainedValue()
            guard collector.records.count < 4_096 else { collector.overflow = true; return }
            let value = item.pointee
            collector.records.append(VST3PluginDescriptor(classID: FactoryCollector.string(value.uid),
                bundlePath: collector.bundle.path, name: FactoryCollector.string(value.name),
                manufacturer: FactoryCollector.string(value.vendor), version: collector.version,
                category: FactoryCollector.string(value.category)))
        }, Unmanaged.passUnretained(collector).toOpaque(), &error, error.count)
        found = collector.records
        let message = String(cString: error)
        if !message.isEmpty { failure = message }
        if collector.overflow { failure = "Factory exposes more than 4096 audio classes." }
        #else
        failure = "This build has no VST3 hosting module."
        #endif
        do {
            let envelope = ScanEnvelope(schema: 1, bundlePath: bundle.path, records: found, error: failure)
            try JSONEncoder().encode(envelope).write(to: output, options: .atomic)
            return 0
        } catch { return 3 }
    }
}

#if canImport(VST3Host)
private final class FactoryCollector {
    let bundle: URL
    let version: String
    var records: [VST3PluginDescriptor] = []
    var overflow = false
    init(bundle: URL, version: String) { self.bundle = bundle; self.version = version }
    static func string<T>(_ value: T) -> String {
        var copy = value
        return withUnsafeBytes(of: &copy) { bytes in
            String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
        }
    }
}
#endif

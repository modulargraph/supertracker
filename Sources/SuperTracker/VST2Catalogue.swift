import Foundation
import Darwin
import VST2Host

/// One VST2 effect class. Shell children (for example WaveShell) carry their shell ID.
struct VST2PluginDescriptor: Codable, Equatable {
    let id: String
    let uniqueID: UInt32
    let shellID: Int32
    let bundlePath: String
    let name: String
    let manufacturer: String
    let version: String
    let isInstrument: Bool
    let hasEditor: Bool
    let inputs: Int
    let outputs: Int

    init(uniqueID: UInt32, shellID: Int32, bundlePath: String, name: String, manufacturer: String,
         version: String, isInstrument: Bool, hasEditor: Bool, inputs: Int, outputs: Int) {
        self.uniqueID = uniqueID; self.shellID = shellID
        id = String(format: "vst2:%08X", uniqueID)
        self.bundlePath = bundlePath; self.name = name; self.manufacturer = manufacturer
        self.version = version; self.isInstrument = isInstrument; self.hasEditor = hasEditor
        self.inputs = inputs; self.outputs = outputs
    }

    var dictionary: [String: Any] {
        ["id": id, "uniqueID": uniqueID, "fourCC": VST3Catalogue.fourCC(uniqueID), "bundlePath": bundlePath,
         "name": name, "manufacturer": manufacturer, "vendor": manufacturer, "version": version,
         "isInstrument": isInstrument, "kind": isInstrument ? "instrument" : "effect",
         "type": isInstrument ? "instrument" : "effect", "format": "VST2"]
    }

    fileprivate var valid: Bool {
        id == String(format: "vst2:%08X", uniqueID) && bundlePath.hasPrefix("/") && !name.isEmpty &&
            name.utf8.count <= 1_024 && manufacturer.utf8.count <= 1_024 && (0...64).contains(inputs) && (1...64).contains(outputs)
    }
}

/// VST2 discovery mirrors the VST3 catalogue: every bundle is opened only by a
/// disposable child process, so a crashing plug-in cannot take down the session.
final class VST2Catalogue: @unchecked Sendable {
    struct Configuration {
        var roots: [URL] = [FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Audio/Plug-Ins/VST"),
                            URL(fileURLWithPath: "/Library/Audio/Plug-Ins/VST")]
        var cacheURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/SuperTracker/Plugins/vst2-catalogue-v1.json")
        var executableURL: URL = Bundle.main.executableURL ?? URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
        var timeout: TimeInterval = 60
    }
    struct ScanEnvelope: Codable { let schema: Int; let bundlePath: String; let records: [VST2PluginDescriptor]; let error: String? }
    private struct Fingerprint: Codable, Equatable { let size: UInt64; let modified: Double; let version: String }
    private struct CacheEntry: Codable { let path: String; let fingerprint: Fingerprint; let records: [VST2PluginDescriptor]; let error: String?; let intelOnly: Bool }
    private struct Cache: Codable { let schema: Int; let entries: [CacheEntry] }
    private enum Failure: LocalizedError {
        case invalid(String)
        var errorDescription: String? { switch self { case .invalid(let text): return text } }
    }

    static let shared = VST2Catalogue()
    private let configuration: Configuration
    private let queue = DispatchQueue(label: "SuperTracker.vst2-discovery", qos: .utility)
    private let lock = NSLock()
    private var entries: [CacheEntry] = []
    private var scanning = false
    private var cancelled = false
    private var preferred: [String] = []
    private var waiters: [CheckedContinuation<Void, Never>] = []
    var onProgress: (([String: Any]) -> Void)?

    init(configuration: Configuration = .init()) {
        self.configuration = configuration
        if let data = try? Data(contentsOf: configuration.cacheURL), data.count <= 16 * 1_024 * 1_024,
           let cache = try? JSONDecoder().decode(Cache.self, from: data), cache.schema == 1 {
            entries = cache.entries.filter { $0.path.hasPrefix("/") && $0.records.allSatisfy(\.valid) }
        }
    }

    var records: [VST2PluginDescriptor] {
        lock.lock(); defer { lock.unlock() }
        var seen = Set<String>()
        return entries.sorted { priority($0.path) == priority($1.path) ? $0.path < $1.path : priority($0.path) < priority($1.path) }
            .flatMap(\.records).filter { seen.insert($0.id).inserted }
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    /// Bundles that cannot run natively (Intel-only builds) for user-facing explanations.
    var unavailable: [[String: Any]] {
        lock.lock(); defer { lock.unlock() }
        return entries.filter { $0.records.isEmpty && $0.error != nil }.map {
            ["path": $0.path, "name": URL(fileURLWithPath: $0.path).deletingPathExtension().lastPathComponent,
             "message": $0.error ?? "", "intelOnly": $0.intelOnly]
        }
    }

    var isScanning: Bool { lock.lock(); defer { lock.unlock() }; return scanning }

    func resolve(identifier: String) -> VST2PluginDescriptor? {
        let wanted = identifier.uppercased().replacingOccurrences(of: "VST2:", with: "vst2:")
        return records.first { $0.id == wanted }
    }

    func resolve(uniqueID: UInt32, isInstrument: Bool? = nil) -> VST2PluginDescriptor? {
        records.first { $0.uniqueID == uniqueID && (isInstrument == nil || $0.isInstrument == isInstrument) }
    }

    /// Returns nil when no native VST2 exposes the ID after scanning (the caller may
    /// then use a compatible VST3 release). Names only prioritize the scan order.
    func resolveOrScan(uniqueID: UInt32, isInstrument: Bool? = nil, preferredNames: [String] = []) async throws -> VST2PluginDescriptor? {
        if let found = resolve(uniqueID: uniqueID, isInstrument: isInstrument) { return found }
        startScan(preferredNames: preferredNames)
        let deadline = ProcessInfo.processInfo.systemUptime + 240
        while true {
            try Task.checkCancellation()
            if let found = resolve(uniqueID: uniqueID, isInstrument: isInstrument) { return found }
            if !isScanning { return nil }
            if ProcessInfo.processInfo.systemUptime >= deadline { return nil }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    /// Scans new or changed bundles in the background. Concurrent callers share one pass.
    func refresh() async {
        await withCheckedContinuation { continuation in
            lock.lock(); waiters.append(continuation); lock.unlock()
            startScan(preferredNames: [])
        }
    }

    func cancel() { lock.lock(); cancelled = true; lock.unlock() }

    private func startScan(preferredNames: [String]) {
        let hints = preferredNames.map(Self.nameKey).filter { $0.count >= 3 }
        lock.lock()
        for hint in hints where !preferred.contains(hint) { preferred.append(hint) }
        if scanning { lock.unlock(); return }
        scanning = true; cancelled = false
        lock.unlock()
        queue.async { [self] in
            perform()
            lock.lock()
            scanning = false; preferred = []
            let pending = waiters; waiters = []
            lock.unlock()
            pending.forEach { $0.resume() }
        }
    }

    private func priority(_ path: String) -> Int {
        configuration.roots.firstIndex { path.hasPrefix($0.path + "/") } ?? configuration.roots.count
    }

    private func perform() {
        var bundles: [URL] = []
        for root in configuration.roots where FileManager.default.fileExists(atPath: root.path) {
            guard let walker = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]) else { continue }
            var visited = 0
            while let url = walker.nextObject() as? URL, visited < 50_000 {
                visited += 1
                if url.pathExtension.lowercased() == "vst" { walker.skipDescendants(); bundles.append(url.standardizedFileURL) }
            }
        }
        lock.lock(); let previous = Dictionary(entries.map { ($0.path, $0) }, uniquingKeysWith: { a, _ in a }); lock.unlock()
        var pending = bundles, completed = 0
        progress(["format": "VST2", "scanning": true, "completed": 0, "total": bundles.count])
        var fresh: [CacheEntry] = []
        while !pending.isEmpty {
            lock.lock(); let stop = cancelled; let hints = preferred; lock.unlock()
            if stop { break }
            var index = 0
            if !hints.isEmpty, let match = pending.firstIndex(where: { url in
                let key = Self.nameKey(url.deletingPathExtension().lastPathComponent)
                return hints.contains { key == $0 || key.contains($0) || $0.contains(key) }
            }) { index = match }
            let bundle = pending.remove(at: index)
            guard let fingerprint = Self.fingerprint(bundle) else { continue }
            let entry: CacheEntry
            if let hit = previous[bundle.path], hit.fingerprint == fingerprint { entry = hit }
            else {
                var error = [CChar](repeating: 0, count: 1024)
                if st_vst2_bundle_is_loadable(bundle.path, &error, error.count) == 0 {
                    entry = CacheEntry(path: bundle.path, fingerprint: fingerprint, records: [], error: String(cString: error), intelOnly: true)
                } else {
                    do { entry = CacheEntry(path: bundle.path, fingerprint: fingerprint, records: try scan(bundle), error: nil, intelOnly: false) }
                    catch { entry = CacheEntry(path: bundle.path, fingerprint: fingerprint, records: [], error: error.localizedDescription, intelOnly: false) }
                }
            }
            fresh.append(entry)
            lock.lock()
            entries.removeAll { $0.path == entry.path }; entries.append(entry)
            lock.unlock()
            completed += 1
            progress(["format": "VST2", "scanning": true, "completed": completed, "total": bundles.count, "path": bundle.path])
        }
        lock.lock()
        let stop = cancelled
        if !stop { let paths = Set(fresh.map(\.path)); entries.removeAll { !paths.contains($0.path) } }
        let copy = entries
        lock.unlock()
        if let data = try? JSONEncoder().encode(Cache(schema: 1, entries: copy)) {
            try? FileManager.default.createDirectory(at: configuration.cacheURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? data.write(to: configuration.cacheURL, options: .atomic)
        }
        progress(["format": "VST2", "scanning": false, "completed": completed, "total": bundles.count, "pluginCount": records.count])
    }

    private func progress(_ value: [String: Any]) {
        guard let callback = onProgress else { return }
        DispatchQueue.main.async { callback(value) }
    }

    private func scan(_ bundle: URL) throws -> [VST2PluginDescriptor] {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-vst2-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: folder) }
        let output = folder.appendingPathComponent("result.json")
        let child = Process(); child.executableURL = configuration.executableURL
        child.arguments = ["--vst2-scan", bundle.path, "--output", output.path]
        child.standardInput = FileHandle.nullDevice; child.standardOutput = FileHandle.nullDevice; child.standardError = FileHandle.nullDevice
        try child.run()
        let deadline = ProcessInfo.processInfo.systemUptime + configuration.timeout
        while child.isRunning && ProcessInfo.processInfo.systemUptime < deadline { usleep(10_000) }
        if child.isRunning {
            child.terminate(); usleep(250_000)
            if child.isRunning { _ = Darwin.kill(child.processIdentifier, SIGKILL) }
            throw Failure.invalid("VST2 plug-in scan timed out.")
        }
        guard child.terminationReason == .exit, child.terminationStatus == 0 else {
            throw Failure.invalid("The VST2 plug-in crashed while being scanned (\(child.terminationStatus)); it was skipped.")
        }
        let envelope = try JSONDecoder().decode(ScanEnvelope.self, from: Data(contentsOf: output))
        guard envelope.schema == 1, envelope.bundlePath == bundle.path, envelope.records.allSatisfy(\.valid) else {
            throw Failure.invalid("The VST2 scanner returned an invalid catalogue.")
        }
        if let error = envelope.error, !error.isEmpty, envelope.records.isEmpty { throw Failure.invalid(error) }
        return envelope.records
    }

    private static func fingerprint(_ bundle: URL) -> Fingerprint? {
        let plist = bundle.appendingPathComponent("Contents/Info.plist")
        guard let info = NSDictionary(contentsOf: plist) as? [String: Any],
              let name = info["CFBundleExecutable"] as? String, !name.contains("/") else { return nil }
        let executable = bundle.appendingPathComponent("Contents/MacOS").appendingPathComponent(name)
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: executable.path) else { return nil }
        return Fingerprint(size: (attributes[.size] as? NSNumber)?.uint64Value ?? 0,
                           modified: (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0,
                           version: (info["CFBundleShortVersionString"] as? String ?? "") + ":" + (info["CFBundleVersion"] as? String ?? ""))
    }

    private static func nameKey(_ name: String) -> String {
        String(name.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) })
    }

    fileprivate static func text<T>(_ tuple: T) -> String {
        withUnsafeBytes(of: tuple) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Child-process entry point. Writes an atomic JSON envelope.
    static func runScanCommand(bundle: URL, output: URL) -> Int32 {
        let info = NSDictionary(contentsOf: bundle.appendingPathComponent("Contents/Info.plist")) as? [String: Any] ?? [:]
        let collector = Collector(bundle: bundle, version: info["CFBundleShortVersionString"] as? String ?? info["CFBundleVersion"] as? String ?? "")
        var error = [CChar](repeating: 0, count: 4096)
        _ = st_vst2_scan_bundle(bundle.path, { item, context in
            guard let item, let context else { return }
            let collector = Unmanaged<Collector>.fromOpaque(context).takeUnretainedValue()
            let value = item.pointee
            let id = UInt32(bitPattern: value.shell_id != 0 ? value.shell_id : value.unique_id)
            guard id != 0, collector.records.count < 4096 else { return }
            var name = VST2Catalogue.text(value.name)
            if name.isEmpty { name = VST2Catalogue.text(value.product) }
            if name.isEmpty { name = collector.bundle.deletingPathExtension().lastPathComponent }
            collector.records.append(VST2PluginDescriptor(uniqueID: id, shellID: value.shell_id, bundlePath: collector.bundle.path,
                name: name, manufacturer: VST2Catalogue.text(value.vendor), version: collector.version,
                isInstrument: value.is_instrument != 0, hasEditor: value.has_editor != 0,
                inputs: Int(max(0, min(64, value.inputs))), outputs: Int(max(1, min(64, value.outputs)))))
        }, Unmanaged.passUnretained(collector).toOpaque(), &error, error.count)
        let message = String(cString: error)
        do {
            let envelope = ScanEnvelope(schema: 1, bundlePath: bundle.path, records: collector.records, error: message.isEmpty ? nil : message)
            try JSONEncoder().encode(envelope).write(to: output, options: .atomic)
            return 0
        } catch { return 1 }
    }
}

private final class Collector {
    var records: [VST2PluginDescriptor] = []
    let bundle: URL
    let version: String
    init(bundle: URL, version: String) { self.bundle = bundle; self.version = version }
}

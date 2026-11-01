import Foundation

/// Owns the execution policy for read-only media discovery. The UI receives proposals;
/// this service never changes a project store, the audio engine, or source audio files.
@MainActor
final class MediaRelinkService {
    typealias Report = [String: Any]
    struct Operations {
        var inspect: (Report, URL?) throws -> Report
        var search: (Report, URL?, [URL], [MediaRelinker.Relocation], MediaRelinker.Limits,
                     @escaping () -> Bool, @escaping (Report) -> Void) throws -> Report
        var apply: (Report, URL?, [Report], @escaping () -> Bool) throws -> Report

        static let filesystem = Operations(
            inspect: { MediaRelinker.inspect($0, projectURL: $1) },
            search: { try MediaRelinker.findCandidates($0, projectURL: $1, searchRoots: $2,
                relocations: $3, limits: $4, cancelled: $5, progress: $6) },
            apply: { try MediaRelinker.apply($0, selections: $2, projectURL: $1, cancelled: $3) })
    }

    private final class Cancellation: @unchecked Sendable {
        private let lock = NSLock()
        private var stopped = false
        var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return stopped }
        func cancel() { lock.lock(); stopped = true; lock.unlock() }
        func check() throws { if isCancelled { throw MediaRelinker.Failure.cancelled } }
    }
    private final class Search {
        let id = UUID()
        let key: Data
        let cancellation = Cancellation()
        var requestIDs: Set<String>
        var task: Task<Report, Error>!
        init(key: Data, requestID: String) { self.key = key; requestIDs = [requestID] }
    }

    var onProgress: ((Report) -> Void)?
    private let queue = DispatchQueue(label: "SuperTracker.media-relink", qos: .utility)
    private let operations: Operations
    private let limits: MediaRelinker.Limits
    private var activeSearch: Search?
    private var workers: [UUID: Cancellation] = [:]

    init(limits: MediaRelinker.Limits = .init(), operations: Operations = .filesystem) {
        self.limits = limits; self.operations = operations
    }

    func inspect(_ project: Report, projectURL: URL?) async throws -> Report {
        let id = UUID(), token = Cancellation(), inspect = operations.inspect
        workers[id] = token; defer { workers.removeValue(forKey: id) }
        return try await worker(token) { try inspect(project, projectURL) }
    }

    func search(_ project: Report, projectURL: URL?, roots: [String],
                relocations: [Report] = [], requestID: String = UUID().uuidString) async throws -> Report {
        guard roots.count <= 64, relocations.count <= 64,
              roots.allSatisfy(Self.absolutePath),
              relocations.allSatisfy({ ($0["from"] as? String).map(Self.absolutePath) == true &&
                                      ($0["to"] as? String).map(Self.absolutePath) == true }) else {
            throw MediaRelinker.Failure.invalid("Choose up to 64 absolute media folders or root mappings.")
        }
        // A repeated request shares its in-flight directory walk and hash budget.
        let key = try JSONSerialization.data(withJSONObject: ["project": project,
            "projectURL": projectURL?.path ?? "", "roots": Array(Set(roots)).sorted(),
            "relocations": relocations], options: [.sortedKeys])
        let search: Search
        if let current = activeSearch, current.key == key, !current.cancellation.isCancelled {
            search = current; search.requestIDs.insert(requestID)
        } else {
            cancelSearch()
            search = Search(key: key, requestID: requestID)
            activeSearch = search
            let operation = operations.search, bounds = limits
            let urls = roots.map { URL(fileURLWithPath: ($0 as NSString).expandingTildeInPath) }
            let mappings = relocations.map { MediaRelinker.Relocation(from: $0["from"] as! String, to: $0["to"] as! String) }
            search.task = Task { [weak self, search] in
                guard let self else { throw MediaRelinker.Failure.cancelled }
                return try await self.worker(search.cancellation) { [weak self, search] in
                    try operation(project, projectURL, urls, mappings, bounds,
                        { search.cancellation.isCancelled }, { [weak self, weak search] value in
                            Task { @MainActor in
                                guard let self, let search, self.activeSearch === search,
                                      !search.cancellation.isCancelled else { return }
                                self.progress(value, for: search)
                            }
                        })
                }
            }
            progress(["phase": "starting", "scannedFiles": 0], for: search)
        }
        do {
            var result = try await search.task.value
            try search.cancellation.check()
            if activeSearch === search {
                progress(["phase": "complete", "scannedFiles": result["scannedFiles"] ?? 0,
                          "missingCount": result["missingCount"] ?? 0], for: search)
                activeSearch = nil
            }
            result["requestId"] = requestID; result["operationId"] = search.id.uuidString
            return result
        } catch {
            if activeSearch === search { activeSearch = nil }
            throw error
        }
    }

    func apply(_ project: Report, selections: [Report], projectURL: URL?) async throws -> Report {
        let id = UUID(), token = Cancellation(), apply = operations.apply
        workers[id] = token; defer { workers.removeValue(forKey: id) }
        return try await worker(token) { try apply(project, projectURL, selections, { token.isCancelled }) }
    }

    @discardableResult func cancel() -> Report {
        let count = workers.count + (activeSearch == nil ? 0 : 1)
        cancelSearch()
        for token in workers.values { token.cancel() }
        return ["cancelled": true, "operations": count]
    }

    private func cancelSearch() {
        guard let search = activeSearch else { return }
        search.cancellation.cancel()
        progress(["phase": "cancelled"], for: search)
        activeSearch = nil
    }

    private func progress(_ value: Report, for search: Search) {
        for requestID in search.requestIDs.sorted() {
            var event = value
            event["requestId"] = requestID; event["operationId"] = search.id.uuidString
            onProgress?(event)
        }
    }

    private func worker<T>(_ token: Cancellation, operation: @escaping () throws -> T) async throws -> T {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                queue.async {
                    do {
                        try token.check()
                        let result = try operation()
                        try token.check()
                        continuation.resume(returning: result)
                    } catch { continuation.resume(throwing: error) }
                }
            }
        } onCancel: { token.cancel() }
    }

    private static func absolutePath(_ path: String) -> Bool {
        !path.isEmpty && (path.hasPrefix("/") || path.hasPrefix("~/"))
    }
}

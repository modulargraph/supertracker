import Foundation
import CryptoKit
import Darwin

/// Explicitly injected control-thread journal. It never opens a plug-in or reads/writes project state.
/// Call before vendor code, after it returns, and mark shutdown clean only after every release completes.
final class PluginRecoveryStore: @unchecked Sendable {
    enum Format: String, Codable { case audioUnit = "AU", vst3 = "VST3", vst2 = "VST2" }

    struct Identity: Codable, Equatable {
        let format: Format
        let componentID: String
        let name: String
        let bundlePath: String?
        init(format: Format, componentID: String, name: String, bundlePath: String? = nil) {
            self.format = format; self.componentID = componentID; self.name = name; self.bundlePath = bundlePath
        }
        /// AU uses the host's exact type:subtype:manufacturer ID. VST3 includes exact class ID and bundle.
        var key: String { PluginRecoveryStore.digest(format.rawValue + "\u{0}" + componentID + "\u{0}" + (bundlePath ?? "")) }
        var dictionary: [String: Any] {
            var result: [String: Any] = ["key": key, "format": format.rawValue, "componentID": componentID, "name": name]
            if let bundlePath { result["bundlePath"] = bundlePath }
            return result
        }
    }

    struct Suspect {
        let identity: Identity
        let reasons: [String]
        let sessionIDs: [String]
        let acknowledged: Bool
        let retryAuthorized: Bool
        let interruptedOperation: Bool
        var requiresReview: Bool { interruptedOperation && !retryAuthorized }
        var dictionary: [String: Any] {
            ["identity": identity.dictionary, "reasons": reasons, "sessionIDs": sessionIDs,
             "acknowledged": acknowledged, "retryAuthorized": retryAuthorized, "requiresReview": requiresReview,
             "attribution": "suspected, not proven"]
        }
    }

    struct Report {
        let suspects: [Suspect]
        let warnings: [String]
        let liveSessionsIgnored: Int
        var dictionary: [String: Any] {
            ["suspects": suspects.map(\.dictionary), "warnings": warnings, "liveSessionsIgnored": liveSessionsIgnored]
        }
    }

    enum RecoveryError: LocalizedError {
        case invalid(String)
        case reviewRequired(Identity)
        var errorDescription: String? {
            switch self {
            case .invalid(let text): return text
            case .reviewRequired(let identity):
                return "\(identity.name) was present during an interrupted plug-in session. Review the recovery report and explicitly choose Retry before loading it again. This does not prove that the plug-in caused the interruption."
            }
        }
    }

    private enum InstanceStatus: String, Codable { case loading, active, releasing }
    private struct Entry: Codable {
        var token: String
        var identity: Identity
        var status: InstanceStatus
        var instanceHint: String?
        var changedAt: Double
    }
    private struct Decision: Codable {
        var identityKey: String
        var sessionIDs: [String]
        var action: String
        var at: Double
    }
    private struct ReleasedIdentity: Codable {
        var identity: Identity
        var releasedAt: Double
    }
    private struct Journal: Codable {
        var version: Int = 1
        var configurationID: String
        var sessionID: String
        var processID: Int32
        var ownerToken: String
        var startedAt: Double
        var updatedAt: Double
        var phase: String
        var cleanShutdown: Bool
        var entries: [Entry]
        var decisions: [Decision]
        // Optional fields preserve compatibility with journals predating explicit module cleanup.
        var releasedHistory: [ReleasedIdentity]?
        var releasedHistoryTruncated: Bool?
    }
    private struct Finding {
        var identity: Identity
        var reasons: Set<String> = []
        var sessions: Set<String> = []
        var interruptedOperations: Set<String> = []
    }

    let directory: URL
    let configurationID: String
    let sessionID: String
    private let processID: Int32
    private let ownerToken = UUID().uuidString
    private let processIsAlive: (Int32) -> Bool
    private let now: () -> Date
    private let lock = NSLock()
    private var journal: Journal?
    private var findings: [String: Finding] = [:]
    private var priorDecisions: [Decision] = []
    private var warnings: [String] = []
    private var liveSessionsIgnored = 0
    private let maximumFileBytes = 1024 * 1024
    private let maximumJournals = 256
    private let maximumReleasedIdentities = 512
    private var journalURL: URL { directory.appendingPathComponent("session-\(sessionID).json") }

    /// No default directory intentionally: only the normal app explicitly injects a production store.
    /// Private native tests must supply their own temporary support directory and must not opt into it.
    init(directory: URL, configurationID: String = "default", processID: Int32 = ProcessInfo.processInfo.processIdentifier,
         processIsAlive: @escaping (Int32) -> Bool = PluginRecoveryStore.isProcessAlive,
         now: @escaping () -> Date = Date.init) throws {
        guard directory.isFileURL, !configurationID.isEmpty, configurationID.utf8.count <= 256, processID > 0 else {
            throw RecoveryError.invalid("Invalid plug-in recovery configuration.")
        }
        self.directory = directory.appendingPathComponent("configuration-" + Self.digest(configurationID), isDirectory: true)
        self.configurationID = configurationID; self.processID = processID
        self.processIsAlive = processIsAlive; self.now = now; self.sessionID = UUID().uuidString
    }

    /// Reads prior dead-owner sessions, then atomically creates this process's independent journal.
    /// Live-owner files are neither attributed as crashes nor modified. PID reuse is conservatively live.
    @discardableResult
    func startSession() throws -> Report {
        try locked {
            guard journal == nil else { return reportUnlocked() }
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            inspectPriorSessions()
            let time = now().timeIntervalSince1970
            let new = Journal(configurationID: configurationID, sessionID: sessionID, processID: processID,
                              ownerToken: ownerToken, startedAt: time, updatedAt: time, phase: "running", cleanShutdown: false,
                              entries: [], decisions: [])
            try write(new)
            journal = new
            return reportUnlocked()
        }
    }

    func recoveryReport() -> Report { locked { reportUnlocked() } }
    func canAttempt(_ identity: Identity) -> Bool {
        locked { !reportUnlocked().suspects.contains { $0.identity.key == identity.key && $0.requiresReview } }
    }

    /// Persist before instantiating/restoring state in any in-process AU or VST3 vendor code.
    /// Reuse an active instance's token for state restoration; it remains one tracked vendor instance.
    func beginLoad(_ identity: Identity, instanceHint: String? = nil, reusingToken: String? = nil) throws -> String {
        try locked {
            try Self.validate(identity)
            guard !(reportUnlocked().suspects.contains { $0.identity.key == identity.key && $0.requiresReview }) else {
                throw RecoveryError.reviewRequired(identity)
            }
            var current = try requireRunning()
            if let token = reusingToken {
                guard let index = current.entries.firstIndex(where: { $0.token == token }),
                      current.entries[index].status == .active,
                      current.entries[index].identity.key == identity.key else {
                    throw RecoveryError.invalid("Restoring plug-in state requires an active recovery token for the same exact component and bundle.")
                }
                current.entries[index].status = .loading
                current.entries[index].changedAt = now().timeIntervalSince1970
                if let instanceHint { current.entries[index].instanceHint = String(instanceHint.prefix(256)) }
                try commit(current)
                return token
            }
            guard current.entries.count < 512 else { throw RecoveryError.invalid("The recovery journal's active plug-in limit was reached.") }
            let token = UUID().uuidString
            current.entries.append(Entry(token: token, identity: identity, status: .loading,
                                         instanceHint: instanceHint.map { String($0.prefix(256)) }, changedAt: now().timeIntervalSince1970))
            try commit(current)
            return token
        }
    }

    func didLoad(_ token: String) throws { try updateStatus(token, from: [.loading], to: .active) }
    func beginRelease(_ token: String) throws { try updateStatus(token, from: [.active, .loading], to: .releasing) }
    /// Call only after the instance and its vendor resources have actually finished releasing.
    func didRelease(_ token: String) throws { try remove(token, permitted: [.releasing]) }
    /// A caught load failure returned control normally and all vendor resources finished releasing.
    /// A failed restoration of an existing instance must remain tracked until that instance is released.
    func loadFailed(_ token: String) throws { try remove(token, permitted: [.loading]) }

    func beginShutdown() throws {
        try locked {
            var current = try requireJournal()
            guard current.phase == "running" else { return }
            current.phase = "shuttingDown"
            try commit(current)
        }
    }

    /// Call immediately before global/module cleanup, after every instance release has completed.
    /// Released identities remain journaled while module termination hooks can still execute vendor code.
    func beginModuleCleanup() throws {
        try locked {
            var current = try requireJournal()
            guard ["shuttingDown", "cleaningModules"].contains(current.phase), current.entries.isEmpty else {
                throw RecoveryError.invalid("Module cleanup requires shutdown to start and every tracked instance to finish releasing.")
            }
            current.phase = "cleaningModules"
            try commit(current)
        }
    }

    /// Call only after global/module cleanup actually returns, including VST3 module termination hooks.
    /// Does not discard active entries or released-module history to manufacture a clean shutdown.
    func finishShutdown() throws {
        try locked {
            var current = try requireJournal()
            guard current.phase == "cleaningModules", current.entries.isEmpty else {
                throw RecoveryError.invalid("Plug-in shutdown cannot be marked clean until every tracked instance finished releasing and module cleanup completed.")
            }
            current.phase = "clean"; current.cleanShutdown = true
            current.releasedHistory = []; current.releasedHistoryTruncated = false
            try commit(current)
            pruneOldCleanJournals()
        }
    }

    /// Acknowledgment dismisses the explanation, but does not implicitly allow a suspect to run.
    func acknowledge(_ identityKeys: [String]) throws { try decide(identityKeys, action: "acknowledge") }
    /// An explicit user retry authorizes only the observed prior incidents, not a future crash of that plug-in.
    func authorizeRetry(_ identityKeys: [String]) throws { try decide(identityKeys, action: "retry") }

    private func decide(_ keys: [String], action: String) throws {
        try locked {
            var current = try requireRunning()
            let unique = Set(keys)
            guard unique.count <= 512, unique.allSatisfy({ findings[$0] != nil }) else { throw RecoveryError.invalid("The recovery decision refers to an unknown plug-in suspect.") }
            for key in unique.sorted() {
                let decision = Decision(identityKey: key, sessionIDs: findings[key]!.sessions.sorted(), action: action, at: now().timeIntervalSince1970)
                current.decisions.removeAll { $0.identityKey == key }
                current.decisions.append(decision)
            }
            try commit(current)
        }
    }

    private func updateStatus(_ token: String, from permitted: Set<InstanceStatus>, to status: InstanceStatus) throws {
        try locked {
            var current = try requireJournal()
            guard !current.cleanShutdown, let index = current.entries.firstIndex(where: { $0.token == token }), permitted.contains(current.entries[index].status) else {
                throw RecoveryError.invalid("The plug-in recovery token or state transition is invalid.")
            }
            current.entries[index].status = status; current.entries[index].changedAt = now().timeIntervalSince1970
            try commit(current)
        }
    }
    private func remove(_ token: String, permitted: Set<InstanceStatus>) throws {
        try locked {
            var current = try requireJournal()
            guard !current.cleanShutdown, let index = current.entries.firstIndex(where: { $0.token == token }), permitted.contains(current.entries[index].status) else {
                throw RecoveryError.invalid("The plug-in recovery token or completion is invalid.")
            }
            let entry = current.entries.remove(at: index)
            var history = current.releasedHistory ?? []
            history.removeAll { $0.identity.key == entry.identity.key }
            history.append(ReleasedIdentity(identity: entry.identity, releasedAt: now().timeIntervalSince1970))
            if history.count > maximumReleasedIdentities {
                history.removeFirst(history.count - maximumReleasedIdentities)
                current.releasedHistoryTruncated = true
            }
            current.releasedHistory = history
            try commit(current)
        }
    }

    private func inspectPriorSessions() {
        let fm = FileManager.default
        guard let files = try? fm.contentsOfDirectory(at: directory, includingPropertiesForKeys: [.contentModificationDateKey], options: [.skipsHiddenFiles]) else {
            warnings.append("Previous plug-in recovery records could not be listed."); return
        }
        let candidates = files.filter { $0.lastPathComponent.hasPrefix("session-") && $0.pathExtension == "json" }.sorted {
            let left = (try? $0.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
            let right = (try? $1.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
            return left == right ? $0.lastPathComponent < $1.lastPathComponent : left > right
        }
        if candidates.count > maximumJournals { warnings.append("Only the newest \(maximumJournals) recovery journals were inspected; older files remain untouched.") }
        for file in candidates.prefix(maximumJournals) {
            do {
                let previous = try read(file)
                guard previous.configurationID == configurationID else { continue }
                if !previous.cleanShutdown, processIsAlive(previous.processID) {
                    liveSessionsIgnored += 1
                    continue
                }
                priorDecisions += previous.decisions
                guard !previous.cleanShutdown else { continue }
                for entry in previous.entries {
                    let key = entry.identity.key
                    var finding = findings[key] ?? Finding(identity: entry.identity)
                    finding.sessions.insert(previous.sessionID)
                    if entry.status != .active { finding.interruptedOperations.insert(previous.sessionID) }
                    let reason: String
                    switch entry.status {
                    case .loading: reason = "Loading or state restoration did not finish before the previous process ended."
                    case .releasing: reason = "Releasing this instance did not finish before the previous process ended."
                    case .active:
                        reason = previous.phase == "shuttingDown" ? "This instance was still active when the previous shutdown was interrupted." : "This instance was active when the previous process ended without a clean shutdown."
                    }
                    finding.reasons.insert(reason)
                    findings[key] = finding
                }
                if ["shuttingDown", "cleaningModules"].contains(previous.phase) {
                    for released in previous.releasedHistory ?? [] {
                        let key = released.identity.key
                        var finding = findings[key] ?? Finding(identity: released.identity)
                        finding.sessions.insert(previous.sessionID)
                        finding.reasons.insert("This plug-in's instances had released before shutdown or module cleanup was interrupted. Its module may still have been loaded; this does not establish the cause.")
                        findings[key] = finding
                    }
                    if previous.releasedHistoryTruncated == true, warnings.count < 32 {
                        warnings.append("The interrupted session's released-module history retained only its latest \(maximumReleasedIdentities) distinct plug-ins; older module identities are unavailable.")
                    }
                }
            } catch {
                if warnings.count < 32 { warnings.append("Could not read \(file.lastPathComponent): \(error.localizedDescription). The original record was preserved.") }
            }
        }
    }

    private func reportUnlocked() -> Report {
        let decisions = priorDecisions + (journal?.decisions ?? [])
        let suspects = findings.keys.sorted().map { key -> Suspect in
            let finding = findings[key]!
            // Decisions are incident-specific. A retry cannot authorize an unobserved later interruption.
            let acknowledged = finding.sessions.allSatisfy { session in decisions.contains { $0.identityKey == key && $0.sessionIDs.contains(session) } }
            let authorized = finding.interruptedOperations.allSatisfy { session in
                if let current = journal?.decisions.last(where: { $0.identityKey == key && $0.sessionIDs.contains(session) }) { return current.action == "retry" }
                return decisions.filter { $0.identityKey == key && $0.sessionIDs.contains(session) }.max { $0.at < $1.at }?.action == "retry"
            }
            return Suspect(identity: finding.identity, reasons: finding.reasons.sorted(), sessionIDs: finding.sessions.sorted(), acknowledged: acknowledged, retryAuthorized: authorized, interruptedOperation: !finding.interruptedOperations.isEmpty)
        }
        return Report(suspects: suspects, warnings: warnings, liveSessionsIgnored: liveSessionsIgnored)
    }

    private func requireJournal() throws -> Journal {
        guard let journal else { throw RecoveryError.invalid("Start the recovery session before recording plug-in activity.") }
        return journal
    }
    private func requireRunning() throws -> Journal {
        let current = try requireJournal()
        guard current.phase == "running", !current.cleanShutdown else { throw RecoveryError.invalid("The plug-in recovery session is shutting down or has ended.") }
        return current
    }
    private func commit(_ value: Journal) throws {
        var updated = value; updated.updatedAt = now().timeIntervalSince1970
        try write(updated)
        journal = updated // In-memory state changes only after an atomic write succeeds.
    }
    private func write(_ value: Journal) throws {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(value)
        guard data.count <= maximumFileBytes else { throw RecoveryError.invalid("The plug-in recovery journal exceeds its size limit.") }
        try data.write(to: journalURL, options: .atomic)
    }
    private func read(_ url: URL) throws -> Journal {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        guard attributes[.type] as? FileAttributeType == .typeRegular,
              let size = attributes[.size] as? NSNumber, size.int64Value > 0, size.int64Value <= maximumFileBytes else {
            throw RecoveryError.invalid("The recovery journal is not a bounded regular file")
        }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        guard let data = try handle.read(upToCount: maximumFileBytes + 1), data.count <= maximumFileBytes else {
            throw RecoveryError.invalid("The recovery journal grew beyond its read limit")
        }
        let result = try JSONDecoder().decode(Journal.self, from: data)
        guard result.version == 1, result.processID > 0, UUID(uuidString: result.sessionID) != nil,
              UUID(uuidString: result.ownerToken) != nil, result.startedAt.isFinite, result.updatedAt.isFinite,
              result.entries.count <= 512, result.decisions.count <= 512,
              ["running", "shuttingDown", "cleaningModules", "clean"].contains(result.phase),
              result.cleanShutdown == (result.phase == "clean"), !result.cleanShutdown || result.entries.isEmpty,
              result.phase != "cleaningModules" || result.entries.isEmpty,
              (result.releasedHistory?.count ?? 0) <= maximumReleasedIdentities,
              Set(result.entries.map(\.token)).count == result.entries.count else {
            throw RecoveryError.invalid("The recovery journal's version or state is invalid")
        }
        for entry in result.entries {
            try Self.validate(entry.identity)
            guard UUID(uuidString: entry.token) != nil, entry.changedAt.isFinite else { throw RecoveryError.invalid("The recovery instance entry is invalid") }
        }
        var releasedKeys = Set<String>()
        for released in result.releasedHistory ?? [] {
            try Self.validate(released.identity)
            guard released.releasedAt.isFinite, releasedKeys.insert(released.identity.key).inserted else {
                throw RecoveryError.invalid("The released plug-in history is invalid")
            }
        }
        guard result.decisions.allSatisfy({ ["acknowledge", "retry"].contains($0.action) && $0.sessionIDs.count <= 256 && $0.at.isFinite }) else {
            throw RecoveryError.invalid("The recovery decisions are invalid")
        }
        return result
    }
    private func pruneOldCleanJournals() {
        // Retain decisions, unclean records, malformed records, and a small useful clean history.
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        let clean = files.compactMap { file -> (URL, Journal)? in
            guard file != journalURL, file.lastPathComponent.hasPrefix("session-"), file.pathExtension == "json",
                  let old = try? read(file), old.configurationID == configurationID, old.cleanShutdown, old.decisions.isEmpty,
                  !processIsAlive(old.processID) else { return nil }
            return (file, old)
        }.sorted { $0.1.updatedAt > $1.1.updatedAt }
        for (url, _) in clean.dropFirst(16) { try? FileManager.default.removeItem(at: url) }
    }
    private static func validate(_ identity: Identity) throws {
        guard !identity.componentID.isEmpty, identity.componentID.utf8.count <= 4096,
              !identity.name.isEmpty, identity.name.utf8.count <= 512, (identity.bundlePath?.utf8.count ?? 0) <= 4096 else {
            throw RecoveryError.invalid("The recovery plug-in identity is invalid.")
        }
    }
    private static func digest(_ value: String) -> String { SHA256.hash(data: Data(value.utf8)).map { String(format: "%02x", $0) }.joined() }
    static func isProcessAlive(_ pid: Int32) -> Bool {
        guard pid > 0 else { return false }
        errno = 0
        return Darwin.kill(pid, 0) == 0 || errno == EPERM
    }
    private func locked<T>(_ body: () throws -> T) rethrows -> T { lock.lock(); defer { lock.unlock() }; return try body() }
}

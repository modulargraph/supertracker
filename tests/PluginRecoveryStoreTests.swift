import Foundation

@main
struct PluginRecoveryStoreTests {
    static func main() throws {
        let fm = FileManager.default
        let temp = fm.temporaryDirectory.appendingPathComponent("SuperTracker-plugin-recovery-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: temp, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: temp) }
        var alive: Set<Int32> = []
        var time: TimeInterval = 1_700_000_000
        func store(_ pid: Int32, configuration: String = "main", root: URL? = nil) throws -> PluginRecoveryStore {
            alive.insert(pid)
            return try PluginRecoveryStore(directory: root ?? temp, configurationID: configuration, processID: pid,
                processIsAlive: { alive.contains($0) }, now: { time += 1; return Date(timeIntervalSince1970: time) })
        }
        func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
            let passed = try condition()
            if !passed { throw NSError(domain: "PluginRecoveryStoreTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
        }
        func rejects(_ message: String, _ work: () throws -> Void) throws {
            var failed = false
            do { try work() } catch { failed = true }
            try require(failed, message)
        }
        let instrument = PluginRecoveryStore.Identity(format: .vst3, componentID: "vst3:0123456789ABCDEF0123456789ABCDEF",
                                                     name: "Fixture Synth", bundlePath: "/private/fixtures/Synth.vst3")
        let effect = PluginRecoveryStore.Identity(format: .audioUnit, componentID: "1635083896:100:200", name: "Fixture Effect")
        let differentBundle = PluginRecoveryStore.Identity(format: .vst3, componentID: instrument.componentID,
                                                          name: instrument.name, bundlePath: "/private/other/Synth.vst3")
        try require(instrument.key != differentBundle.key, "VST3 bundle path forms part of exact identity")
        let first = try store(101)
        try require(!fm.fileExists(atPath: first.directory.path), "initialization alone never creates a journal")
        try rejects("activity requires explicit session startup") { _ = try first.beginLoad(instrument) }
        let initial = try first.startSession()
        try require(initial.suspects.isEmpty && initial.warnings.isEmpty, "first startup is clean")
        let attempt = try first.beginLoad(instrument, instanceHint: "Instrument 3")
        let firstFile = first.directory.appendingPathComponent("session-\(first.sessionID).json")
        let attemptJSON = try JSONSerialization.jsonObject(with: Data(contentsOf: firstFile)) as! [String: Any]
        try require((attemptJSON["entries"] as? [[String: Any]])?.first?["status"] as? String == "loading", "attempt recorded before vendor code")
        try require(attemptJSON["project"] == nil && attemptJSON["state"] == nil, "opaque project/plugin state is never journaled")
        let parallel = try store(102)
        let live = try parallel.startSession()
        try require(live.suspects.isEmpty && live.liveSessionsIgnored == 1, "another live instance is not a crash")
        try require(parallel.canAttempt(instrument), "live-owner records do not quarantine components")
        let samePID = try store(101)
        let sameReport = try samePID.startSession()
        try require(sameReport.suspects.isEmpty && sameReport.liveSessionsIgnored == 2, "same PID/new owner token remains conservatively live")
        try require(first.sessionID != samePID.sessionID, "concurrent sessions never overwrite each other's journal")
        alive.remove(101)
        let recovering = try store(103)
        let interrupted = try recovering.startSession()
        try require(interrupted.suspects.count == 1 && interrupted.suspects[0].identity == instrument, "dead owner's exact attempted identity recovered")
        try require(interrupted.suspects[0].reasons.first?.contains("Loading") == true && interrupted.suspects[0].requiresReview, "interrupted load distinguished from proven blame")
        try rejects("suspect load requires explicit retry") { _ = try recovering.beginLoad(instrument) }
        try require(recovering.canAttempt(effect) && recovering.canAttempt(differentBundle), "unrelated plug-ins are not disabled")
        let effectAttempt = try recovering.beginLoad(effect)
        try recovering.loadFailed(effectAttempt)
        try recovering.acknowledge([instrument.key])
        try require(recovering.recoveryReport().suspects[0].acknowledged && !recovering.canAttempt(instrument), "acknowledge does not silently authorize retry")
        try recovering.authorizeRetry([instrument.key])
        try require(recovering.canAttempt(instrument), "explicit retry authorizes only the selected suspect")
        try rejects("unknown recovery identities cannot be acknowledged") { try recovering.acknowledge([effect.key]) }
        let retry = try recovering.beginLoad(instrument)
        try recovering.didLoad(retry)
        try rejects("release completion requires release intent") { try recovering.didRelease(retry) }
        try recovering.beginShutdown()
        try rejects("shutdown cannot manufacture a clean journal while vendor instance lives") { try recovering.finishShutdown() }
        try rejects("module cleanup cannot begin while an instance lives") { try recovering.beginModuleCleanup() }
        try rejects("loading is blocked after shutdown begins") { _ = try recovering.beginLoad(effect) }
        try recovering.beginRelease(retry)
        try recovering.didRelease(retry)
        try rejects("last instance release does not mean global module cleanup finished") { try recovering.finishShutdown() }
        try recovering.beginModuleCleanup()
        try recovering.finishShutdown()
        let cleanFile = recovering.directory.appendingPathComponent("session-\(recovering.sessionID).json")
        let cleanJSON = try JSONSerialization.jsonObject(with: Data(contentsOf: cleanFile)) as! [String: Any]
        try require(cleanJSON["cleanShutdown"] as? Bool == true && (cleanJSON["entries"] as? [Any])?.isEmpty == true, "clean recorded only after successful releases")
        alive.remove(103)
        let afterClean = try store(104)
        let priorRetry = try afterClean.startSession()
        try require(priorRetry.suspects.count == 1 && !priorRetry.suspects[0].requiresReview, "explicit retry decision survives clean restart")
        let active = try afterClean.beginLoad(instrument)
        try afterClean.didLoad(active)
        try afterClean.beginShutdown()
        try afterClean.beginRelease(active)
        alive.remove(104)
        let afterReleaseCrash = try store(105)
        let releaseReport = try afterReleaseCrash.startSession()
        try require(releaseReport.suspects.count == 1 && releaseReport.suspects[0].requiresReview, "new interruption revokes an old incident-specific retry")
        try require(releaseReport.suspects[0].reasons.contains { $0.contains("Releasing") }, "interrupted release has a precise reason")
        try require(!releaseReport.suspects.contains { $0.identity == effect }, "caught load failure is not misattributed as a crash")
        let otherConfiguration = try store(106, configuration: "private-native-test")
        try require(try otherConfiguration.startSession().suspects.isEmpty, "configuration scopes are independent")
        let project = temp.appendingPathComponent("Project.supertracker")
        let projectBytes = Data("opaque plugin project state must remain unchanged".utf8)
        try projectBytes.write(to: project)
        try require(try Data(contentsOf: project) == projectBytes, "project state preserved")

        // Malformed/oversized/link records are bounded warnings, never rewritten or interpreted as all plug-ins failing.
        let malformed = first.directory.appendingPathComponent("session-malformed.json")
        let badBytes = Data("{broken".utf8); try badBytes.write(to: malformed)
        let oversized = first.directory.appendingPathComponent("session-oversized.json")
        try Data(repeating: 0x20, count: 1024 * 1024 + 1).write(to: oversized)
        let symbolic = first.directory.appendingPathComponent("session-link.json")
        try fm.createSymbolicLink(at: symbolic, withDestinationURL: project)
        var future = cleanJSON; future["version"] = 99
        try JSONSerialization.data(withJSONObject: future).write(to: first.directory.appendingPathComponent("session-future.json"))
        let malformedReader = try store(107)
        let malformedReport = try malformedReader.startSession()
        try require(malformedReport.warnings.count == 4, "malformed, future, oversized and symlink files become warnings")
        try require(try Data(contentsOf: malformed) == badBytes && Data(contentsOf: project) == projectBytes, "invalid originals and link targets remain unchanged")

        // A failed atomic write cannot mutate the in-memory instance list and later falsely report clean.
        let failingRoot = temp.appendingPathComponent("WriteFailure", isDirectory: true)
        let failureStore = try store(108, root: failingRoot)
        _ = try failureStore.startSession()
        let moved = failureStore.directory.appendingPathExtension("backup")
        try fm.moveItem(at: failureStore.directory, to: moved)
        try Data([1]).write(to: failureStore.directory)
        try rejects("journal-write failure propagates before vendor instantiation") { _ = try failureStore.beginLoad(effect) }
        try fm.removeItem(at: failureStore.directory)
        try fm.moveItem(at: moved, to: failureStore.directory)
        try failureStore.beginShutdown(); try failureStore.beginModuleCleanup(); try failureStore.finishShutdown()
        let activeRoot = temp.appendingPathComponent("ActiveFailure", isDirectory: true)
        let activeStore = try store(201, root: activeRoot)
        _ = try activeStore.startSession()
        let activeOne = try activeStore.beginLoad(effect); try activeStore.didLoad(activeOne)
        let activeTwo = try activeStore.beginLoad(effect); try activeStore.didLoad(activeTwo)
        alive.remove(201)
        let activeReader = try store(202, root: activeRoot)
        let activeReport = try activeReader.startSession()
        try require(activeReport.suspects.count == 1 && activeReport.suspects[0].sessionIDs.count == 1, "multiple instances group by exact component identity")
        try require(activeReader.canAttempt(effect) && !activeReport.suspects[0].requiresReview, "active neighbors remain usable without a retry decision")
        try require(activeReport.suspects[0].reasons.first?.contains("was active") == true, "active-at-interruption remains uncertain attribution")

        // Undo/preset restoration journals the existing instance rather than accumulating duplicate entries.
        let restoreRoot = temp.appendingPathComponent("StateRestoration", isDirectory: true)
        let restoreStore = try store(301, root: restoreRoot)
        _ = try restoreStore.startSession()
        let restoreToken = try restoreStore.beginLoad(instrument, instanceHint: "Instrument 7")
        try rejects("a loading token cannot begin another restoration") { _ = try restoreStore.beginLoad(instrument, reusingToken: restoreToken) }
        try restoreStore.didLoad(restoreToken)
        try rejects("restoration requires a known instance token") { _ = try restoreStore.beginLoad(instrument, reusingToken: UUID().uuidString) }
        try rejects("restoration must match the exact component") { _ = try restoreStore.beginLoad(effect, reusingToken: restoreToken) }
        try rejects("restoration must match the exact VST3 bundle") { _ = try restoreStore.beginLoad(differentBundle, reusingToken: restoreToken) }
        let restoreFile = restoreStore.directory.appendingPathComponent("session-\(restoreStore.sessionID).json")
        for index in 0..<20 {
            let sameToken = try restoreStore.beginLoad(instrument, reusingToken: restoreToken)
            let record = try JSONSerialization.jsonObject(with: Data(contentsOf: restoreFile)) as! [String: Any]
            let entries = record["entries"] as! [[String: Any]]
            try require(sameToken == restoreToken && entries.count == 1 && entries[0]["token"] as? String == restoreToken, "restoration \(index) reuses one exact entry")
            try require(entries[0]["status"] as? String == "loading" && entries[0]["instanceHint"] as? String == "Instrument 7", "restoration intent precedes vendor code and preserves its hint")
            try restoreStore.didLoad(restoreToken)
        }
        let restoreBackup = restoreStore.directory.appendingPathExtension("backup")
        try fm.moveItem(at: restoreStore.directory, to: restoreBackup)
        try Data([1]).write(to: restoreStore.directory)
        try rejects("failed journal write prevents unrecorded state restoration") { _ = try restoreStore.beginLoad(instrument, reusingToken: restoreToken) }
        try fm.removeItem(at: restoreStore.directory)
        try fm.moveItem(at: restoreBackup, to: restoreStore.directory)
        try rejects("failed restoration write retains the active state") { try restoreStore.didLoad(restoreToken) }
        _ = try restoreStore.beginLoad(instrument, instanceHint: "Instrument 8", reusingToken: restoreToken)
        alive.remove(301)
        let restoreReader = try store(302, root: restoreRoot)
        let restoreReport = try restoreReader.startSession()
        try require(restoreReport.suspects.count == 1 && restoreReport.suspects[0].reasons.contains { $0.contains("state restoration") }, "interrupted restoration remains a loading suspect")
        try restoreStore.beginRelease(restoreToken)
        try rejects("a releasing token cannot restore state") { _ = try restoreStore.beginLoad(instrument, reusingToken: restoreToken) }
        try restoreStore.didRelease(restoreToken)
        try restoreStore.beginShutdown(); try restoreStore.beginModuleCleanup(); try restoreStore.finishShutdown()

        // Vendor module/global teardown can crash after the final instance has already been released.
        let moduleRoot = temp.appendingPathComponent("ModuleCleanup", isDirectory: true)
        let moduleStore = try store(401, root: moduleRoot)
        _ = try moduleStore.startSession()
        try rejects("module cleanup requires shutdown intent") { try moduleStore.beginModuleCleanup() }
        for _ in 0..<100 {
            let token = try moduleStore.beginLoad(instrument)
            try moduleStore.didLoad(token); try moduleStore.beginRelease(token); try moduleStore.didRelease(token)
        }
        let moduleFile = moduleStore.directory.appendingPathComponent("session-\(moduleStore.sessionID).json")
        let releasedRecord = try JSONSerialization.jsonObject(with: Data(contentsOf: moduleFile)) as! [String: Any]
        try require((releasedRecord["entries"] as? [Any])?.isEmpty == true && (releasedRecord["releasedHistory"] as? [Any])?.count == 1, "hundreds of preset instances leave one released identity and no active entries")
        alive.remove(401)
        let runtimeReader = try store(402, root: moduleRoot)
        try require(try runtimeReader.startSession().suspects.isEmpty, "released history alone is not blamed during an ordinary runtime interruption")
        try moduleStore.beginShutdown()
        let shutdownReader = try store(403, root: moduleRoot)
        let shutdownReport = try shutdownReader.startSession()
        try require(shutdownReport.suspects.count == 1 && shutdownReport.suspects[0].reasons[0].contains("module cleanup"), "released identity survives interrupted shutdown before module cleanup starts")
        try moduleStore.beginModuleCleanup()
        try moduleStore.beginShutdown() // Repeated host shutdown calls must not regress cleanup phase.
        let moduleReader = try store(404, root: moduleRoot)
        let moduleReport = try moduleReader.startSession()
        try require(moduleReport.suspects.count == 1 && moduleReport.suspects[0].identity == instrument && !moduleReport.suspects[0].requiresReview && moduleReader.canAttempt(instrument), "dead owner interrupted during global cleanup retains exact released identity")
        try require(moduleReport.suspects[0].reasons[0].contains("does not establish the cause"), "released-module attribution stays explicitly uncertain")
        try moduleStore.finishShutdown()
        let normalReader = try store(405, root: moduleRoot)
        try require(try normalReader.startSession().suspects.isEmpty, "successful module cleanup and clean exit do not create suspects")
        let moduleClean = try JSONSerialization.jsonObject(with: Data(contentsOf: moduleFile)) as! [String: Any]
        try require(moduleClean["cleanShutdown"] as? Bool == true && (moduleClean["releasedHistory"] as? [Any])?.isEmpty == true, "released history is cleared only after module cleanup completes")

        // Older v1 records without optional history remain readable and do not produce invented history.
        var legacy = moduleClean
        legacy.removeValue(forKey: "releasedHistory"); legacy.removeValue(forKey: "releasedHistoryTruncated")
        try JSONSerialization.data(withJSONObject: legacy).write(to: moduleFile, options: .atomic)
        let legacyReader = try store(406, root: moduleRoot)
        let legacyReport = try legacyReader.startSession()
        try require(legacyReport.suspects.isEmpty && legacyReport.warnings.isEmpty, "legacy journal schema remains compatible")
        try require(PluginRecoveryStore.isProcessAlive(ProcessInfo.processInfo.processIdentifier) && !PluginRecoveryStore.isProcessAlive(-1), "real liveness check is non-destructive")
        try require(!attempt.isEmpty, "operation token remains available for host integration")
        print("PASS plug-in recovery: explicit opt-in, exact identities, concurrent/live-owner isolation, interrupted load/release/module-cleanup suspects, scoped retries, clean-shutdown ordering, repeated state restoration, atomic failure, malformed bounds, legacy compatibility, project preservation")
    }
}

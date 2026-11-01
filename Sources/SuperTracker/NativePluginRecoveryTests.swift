// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox

/// Isolated journals, Apple AUDelay, and one original lifetime fixture. No vendor
/// plug-in, production recovery folder, real process termination, or audio device.
enum NativePluginRecoveryTests {
    private static let failureDescription = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x72636661, componentManufacturer: 0x53745473,
        componentFlags: 0, componentFlagsMask: 0)
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(RecoveryFailureUnit.self, as: failureDescription,
            name: "SuperTracker Test: Recovery validation failure", version: 1)
    }()

    @MainActor static func run(output: URL) async throws -> [String] {
        let fm = FileManager.default
        let folder = output.appendingPathComponent("plugin-recovery", isDirectory: true)
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        let temporary = fm.temporaryDirectory.appendingPathComponent("SuperTracker-native-recovery-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: temporary, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: temporary) }
        var observations: [[String: Any]] = [], passed: [String] = []
        defer {
            let result: [String: Any] = ["passed": passed, "observations": observations]
            if let data = try? JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Plug-in recovery regression: " + message) }
        }
        func journal(_ store: PluginRecoveryStore) throws -> [String: Any] {
            try JSONSerialization.jsonObject(with: Data(contentsOf: journalURL(store))) as? [String: Any] ?? [:]
        }
        func entries(_ store: PluginRecoveryStore) throws -> [[String: Any]] {
            try journal(store)["entries"] as? [[String: Any]] ?? []
        }
        func makeStore(_ name: String) throws -> PluginRecoveryStore {
            let store = try PluginRecoveryStore(directory: temporary.appendingPathComponent(name), configurationID: "native-test")
            try store.startSession()
            return store
        }
        func verifyClean(_ store: PluginRecoveryStore, _ name: String) throws {
            let saved = try journal(store)
            observations.append(["name": name, "journal": saved])
            try require(saved["cleanShutdown"] as? Bool == true && (saved["entries"] as? [Any])?.isEmpty == true,
                        "\(name) was not clean after actual resource release: \(saved)")
        }
        func finish(_ host: AudioHost, _ store: PluginRecoveryStore) async throws {
            await host.shutdownAndWait()
            let beforeModules = try journal(store)
            try require(beforeModules["cleanShutdown"] as? Bool == false &&
                        (beforeModules["entries"] as? [Any])?.isEmpty == true,
                        "Shutdown was marked clean before module cleanup, or vendor instances remained alive.")
            try host.beginRecoveryModuleCleanup()
            // These tests instantiate only Apple/original AUs; there are no VST3
            // modules to unload between the two explicit production lifecycle hooks.
            try require(try journal(store)["phase"] as? String == "cleaningModules",
                        "Module cleanup was not recorded before the final clean marker.")
            try host.finishRecoveryShutdown()
        }
        func state(_ host: AudioHost, _ target: [String: Any]) async throws -> [String: Any] {
            guard let result = try await host.handle("plugin.state", payload: target) as? [String: Any], result["state"] is String else {
                throw AudioSampleError.invalid("AUDelay did not return opaque state.")
            }
            return result
        }
        func snapshot(_ host: AudioHost) async throws -> [String: Any] {
            try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
        }
        func settle(_ host: AudioHost, _ store: PluginRecoveryStore) async throws {
            // The recovery read performs control-thread weak-resource collection.
            // Yielding also lets the AU instantiate callback's autorelease pool drain.
            for _ in 0..<2000 {
                _ = try await host.handle("plugins.recovery", payload: [:])
                if try entries(store).isEmpty { return }
                await Task.yield()
            }
            throw AudioSampleError.invalid("Recovery resources did not finish releasing.")
        }
        let target: [String: Any] = ["track": 0, "kind": "effect", "slot": 0]

        // Real AU state writes reuse one recovery token and remain serializable.
        let cleanStore = try makeStore("clean")
        let cleanHost = try AudioHost(recoveryStore: cleanStore,midiInputIDs:[])
        defer { cleanHost.shutdown() }
        let available = try await cleanHost.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
        guard let delay = available.first(where: { ($0["name"] as? String) == "AUDelay" }),
              let delayID = delay["id"] as? String else {
            throw AudioSampleError.invalid("Recovery tests require the macOS AUDelay component.")
        }
        let identity = PluginRecoveryStore.Identity(format: .audioUnit, componentID: delayID, name: "AUDelay")
        var request = target; request["component"] = delayID
        _ = try await cleanHost.handle("plugin.load", payload: request)
        var parameter = target
        parameter["address"] = String(kDelayParam_DelayTime); parameter["normalized"] = false; parameter["value"] = 0.42
        _ = try await cleanHost.handle("plugin.parameter", payload: parameter)
        let savedDelay = try await state(cleanHost, target)
        let initialEntries = try entries(cleanStore)
        try require(initialEntries.count == 1 && initialEntries[0]["status"] as? String == "active",
                    "The loaded Apple AU must have exactly one active recovery entry.")
        let token = initialEntries[0]["token"] as? String
        for _ in 0..<4 {
            var restore = target; restore["state"] = savedDelay["state"]
            _ = try await cleanHost.handle("plugin.state", payload: restore)
            let current = try entries(cleanStore)
            try require(current.count == 1 && current[0]["token"] as? String == token && current[0]["status"] as? String == "active",
                        "Restoring a live AU accumulated or replaced its recovery token.")
        }
        let savedProject = try await snapshot(cleanHost)
        try JSONSerialization.data(withJSONObject: savedProject, options: [.prettyPrinted, .sortedKeys])
            .write(to: folder.appendingPathComponent("saved-project.json"), options: .atomic)
        try require((savedProject["audioPlugins"] as? [[String: Any]])?.first?["state"] is String,
                    "Recovery journaling interfered with project state serialization.")
        var invalidExistingState = target
        invalidExistingState["state"] = Data("invalid state for the existing AU".utf8).base64EncodedString()
        var rejectedExistingState = false
        do { _ = try await cleanHost.handle("plugin.state", payload: invalidExistingState) }
        catch { rejectedExistingState = true }
        try require(rejectedExistingState, "Invalid direct state restoration unexpectedly succeeded.")
        try await settle(cleanHost, cleanStore)
        let afterFailedState = cleanHost.status()["endpointLifetime"] as? [String: Any] ?? [:]
        try require(afterFailedState["activeInstances"] as? Int == 0,
                    "A failed direct state restore left a loading recovery token attached to an active AU.")
        _ = try await cleanHost.handle("plugin.load", payload: savedDelay)
        let reloaded = try entries(cleanStore)
        try require(reloaded.count == 1 && reloaded[0]["status"] as? String == "active" && reloaded[0]["token"] as? String != token,
                    "A failed direct restore prevented a subsequent valid AU reload.")
        try await finish(cleanHost, cleanStore)
        try verifyClean(cleanStore, "clean-loaded-state-and-close")
        passed.append("Loaded AUDelay state saves and repeated state restoration use one journal entry and close cleanly")
        passed.append("Failed direct AU state restoration releases its tracked instance and permits a subsequent valid reload")

        // A previous dead owner's exact attempted AU identity is quarantined before
        // the instantiation gate. The desired project keeps its opaque patch intact.
        let suspectRoot = temporary.appendingPathComponent("suspect")
        let fakePID: Int32 = 2_000_000_001
        let currentPID = ProcessInfo.processInfo.processIdentifier
        let interrupted = try PluginRecoveryStore(directory: suspectRoot, configurationID: "native-test",
            processID: fakePID, processIsAlive: { $0 == currentPID })
        try interrupted.startSession()
        _ = try interrupted.beginLoad(identity, instanceHint: "Track 1 effect 1")
        let originalJournal = try Data(contentsOf: journalURL(interrupted))
        let recovery = try PluginRecoveryStore(directory: suspectRoot, configurationID: "native-test",
            processIsAlive: { $0 == currentPID })
        let prior = try recovery.startSession()
        try require(prior.suspects.count == 1 && prior.suspects[0].identity.key == identity.key && prior.suspects[0].requiresReview,
                    "The exact interrupted Apple AU was not marked for review.")
        var gateCalls = 0
        var rejectRetry = false
        let recoveringHost = try AudioHost(beforePluginInstantiation: {
            gateCalls += 1
            let current = try entries(recovery)
            guard current.count == 1, current[0]["status"] as? String == "loading" else {
                throw AudioSampleError.invalid("The vendor gate ran before durable load intent.")
            }
            if rejectRetry { throw AudioSampleError.invalid("Injected unavailable plug-in dependency") }
        }, recoveryStore: recovery,midiInputIDs:[])
        defer { recoveringHost.shutdown() }
        var restorationErrors:[String] = []
        recoveringHost.onEvent = { name, payload in
            if name == "plugin.error" { restorationErrors.append(payload["error"] as? String ?? "Unknown error") }
        }
        let rows: [Any] = [NSNull()]
        let project: [String: Any] = ["version": 1, "name": "Recovery desired patch", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 0.5, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Track 1", "instrument": 0, "volume": 0.7]],
            "instruments": [["type": "synth", "volume": 0.5]],
            "patterns": [["id": 0, "length": 1, "steps": [rows], "automation": []]],
            "sequence": [0], "audioPlugins": [savedDelay], "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await recoveringHost.handle("engine.project", payload: ["project": project, "replace": true])
        await recoveringHost.pluginRestorationBarrier()?.value
        try require(restorationErrors.isEmpty, "Expected recovery skips produced a redundant load-error event.")
        let quarantined = try await snapshot(recoveringHost)
        let retained = (quarantined["audioPlugins"] as? [[String: Any]])?.first
        try require(gateCalls == 0 && retained?["component"] as? String == delayID &&
                    retained?["state"] as? String == savedDelay["state"] as? String,
                    "Quarantine entered vendor code or replaced the desired opaque patch.")
        var blocked = false
        do { _ = try await recoveringHost.handle("plugin.load", payload: request) } catch { blocked = true }
        try require(blocked && gateCalls == 0, "A direct load bypassed quarantine.")
        let report = try await recoveringHost.handle("plugins.recovery", payload: [:]) as? [String: Any] ?? [:]
        try require((report["suspects"] as? [[String: Any]])?.count == 1, "The recovery command omitted its suspect.")
        observations.append(["name": "quarantined-project", "report": report, "gateCalls": gateCalls,
                             "savedComponent": retained?["component"] ?? NSNull(), "opaqueStatePreserved": true])
        var quarantinedRead = ""
        do { _ = try await recoveringHost.handle("plugin.parameters", payload: target) } catch { quarantinedRead = error.localizedDescription }
        try require(quarantinedRead.contains("AUDelay") && quarantinedRead.contains("Review interrupted plug-ins"),
                    "A held-back insert's parameter read did not explain the recovery hold: \(quarantinedRead)")
        passed.append("Parameter reads for a crash-held insert name the plug-in and the recovery review instead of an empty slot")
        rejectRetry = true
        let failedRetry = try await recoveringHost.handle("plugins.recovery.retry", payload: ["keys": [identity.key]]) as? [String:Any] ?? [:]
        try require((failedRetry["retryErrors"] as? [Any])?.count == 1 && !recovery.canAttempt(identity),
                    "A failed retry must remain disabled with another retry available.")
        rejectRetry = false
        let retry = try await recoveringHost.handle("plugins.recovery.retry", payload: ["keys": [identity.key]]) as? [String: Any] ?? [:]
        await recoveringHost.pluginRestorationBarrier()?.value
        let parameters = try await recoveringHost.handle("plugin.parameters", payload: target) as? [[String: Any]] ?? []
        let delayTime = (parameters.first { $0["address"] as? String == String(kDelayParam_DelayTime) }?["value"] as? NSNumber)?.doubleValue ?? -1
        try require(gateCalls == 2 && abs(delayTime - 0.42) < 0.0001,
                    "Explicit retry did not load the current desired AU patch.")
        try require((retry["retryErrors"] as? [Any] ?? []).isEmpty, "Retry reported an error: \(retry)")
        try require(try Data(contentsOf: journalURL(interrupted)) == originalJournal,
                    "Recovery modified the original interrupted-session evidence.")
        observations.append(["name": "explicit-retry", "result": retry, "gateCalls": gateCalls, "delayTime": delayTime])
        try await finish(recoveringHost, recovery)
        try verifyClean(recovery, "recovered-close")
        passed.append("Recovery blocks exact suspect AU loads before vendor entry, preserves opaque desired state, and retries it only explicitly")

        // First-party studio effects are SuperTracker DSP, not vendor code: an unexpected
        // quit never holds them back, even when an older journal still lists them.
        let builtinRoot = temporary.appendingPathComponent("builtin")
        let builtinIdentity = PluginRecoveryStore.Identity(format: .audioUnit, componentID: "stfx:graindelay", name: "Grain Delay")
        let crashedBuiltin = try PluginRecoveryStore(directory: builtinRoot, configurationID: "native-test",
            processID: fakePID, processIsAlive: { $0 == currentPID })
        try crashedBuiltin.startSession()
        _ = try crashedBuiltin.beginLoad(builtinIdentity, instanceHint: "kit group effect:2")
        let builtinRecovery = try PluginRecoveryStore(directory: builtinRoot, configurationID: "native-test",
            processIsAlive: { $0 == currentPID })
        let builtinPrior = try builtinRecovery.startSession()
        try require(builtinPrior.suspects.contains { $0.identity.key == builtinIdentity.key },
                    "Fixture: the legacy interrupted journal must list the built-in effect.")
        let builtinHost = try AudioHost(recoveryStore: builtinRecovery, midiInputIDs: [])
        defer { builtinHost.shutdown() }
        var builtinErrors: [String] = []
        builtinHost.onEvent = { name, payload in if name == "plugin.error" { builtinErrors.append(payload["error"] as? String ?? "Unknown error") } }
        let builtinInserts: [[String: Any]] = [
            ["owner": "track:lane0", "kind": "effect", "slot": 0, "component": "stfx:eq", "name": "Four-band EQ", "format": "Builtin", "instanceID": "track-eq"],
            ["owner": "bus:g0", "kind": "effect", "slot": 2, "component": "stfx:graindelay", "name": "Grain Delay", "format": "Builtin", "instanceID": "group-grain"],
            ["owner": "master", "kind": "effect", "slot": 0, "component": "stfx:limiter", "name": "Peak Limiter", "format": "Builtin", "instanceID": "master-limiter"]]
        let builtinProject: [String: Any] = ["version": 1, "name": "Built-in inserts after a crash", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 0, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Track 1", "instrument": 0, "volume": 0.7, "routingID": "lane0"]],
            "instruments": [["type": "synth", "volume": 0]],
            "patterns": [["id": 0, "length": 1, "steps": [rows], "automation": []]],
            "sequence": [0], "audioPlugins": builtinInserts, "delay": ["mix": 0], "reverb": ["mix": 0],
            "routing": ["version": 1, "mixModel": "standard", "buses": [["id": "g0", "name": "Kit group", "type": "group", "volume": 1]],
                        "outputs": ["bus:g0": "master"], "sends": [], "sidechains": []]]
        _ = try await builtinHost.handle("engine.project", payload: ["project": builtinProject, "replace": true])
        await builtinHost.pluginRestorationBarrier()?.value
        try require(builtinErrors.isEmpty, "Built-in inserts failed to restore after an unexpected quit: \(builtinErrors)")
        for insert in builtinInserts {
            let payload: [String: Any] = ["owner": insert["owner"]!, "kind": "effect", "slot": insert["slot"]!, "instanceID": insert["instanceID"]!]
            let rows = try await builtinHost.handle("plugin.parameters", payload: payload) as? [[String: Any]] ?? []
            try require(!rows.isEmpty, "\(insert["name"]!) on \(insert["owner"]!) was held back or exposed no parameters after an unexpected quit.")
        }
        let builtinReport = try await builtinHost.handle("plugins.recovery", payload: [:]) as? [String: Any] ?? [:]
        let listedBuiltins = (builtinReport["suspects"] as? [[String: Any]] ?? []).filter { (($0["identity"] as? [String: Any])?["componentID"] as? String ?? "").hasPrefix("stfx:") }
        try require(listedBuiltins.isEmpty, "The recovery review still lists built-in effects: \(listedBuiltins)")
        try require(try entries(builtinRecovery).isEmpty, "Built-in effects were written to the vendor crash journal.")
        observations.append(["name": "builtin-crash-exemption", "report": builtinReport])
        try await finish(builtinHost, builtinRecovery)
        try verifyClean(builtinRecovery, "builtin-close")
        passed.append("Built-in Group, Master and track inserts load and expose parameters after an unexpected quit; they never enter the crash journal")

        // Two simultaneously live stores share a configuration without attributing
        // one another's active instances as crashes or rewriting another session.
        let parallelRoot = temporary.appendingPathComponent("parallel")
        let firstStore = try PluginRecoveryStore(directory: parallelRoot, configurationID: "native-test")
        try firstStore.startSession()
        let firstHost = try AudioHost(recoveryStore: firstStore,midiInputIDs:[])
        defer { firstHost.shutdown() }
        _ = try await firstHost.handle("plugin.load", payload: request)
        let firstBefore = try Data(contentsOf: journalURL(firstStore))
        let secondStore = try PluginRecoveryStore(directory: parallelRoot, configurationID: "native-test")
        let liveReport = try secondStore.startSession()
        try require(liveReport.suspects.isEmpty && liveReport.liveSessionsIgnored == 1 && firstStore.sessionID != secondStore.sessionID,
                    "A running host session was attributed as an interrupted plug-in.")
        let secondHost = try AudioHost(recoveryStore: secondStore,midiInputIDs:[])
        defer { secondHost.shutdown() }
        _ = try await secondHost.handle("plugin.load", payload: request)
        try require(try Data(contentsOf: journalURL(firstStore)) == firstBefore && entries(secondStore).count == 1,
                    "The second host rewrote the first host's active journal.")
        let secondBefore = try Data(contentsOf: journalURL(secondStore))
        _ = try await firstHost.handle("plugin.remove", payload: target)
        try await settle(firstHost, firstStore)
        try require(try Data(contentsOf: journalURL(secondStore)) == secondBefore,
                    "Releasing the first host's AU affected the second host's journal.")
        try await finish(firstHost, firstStore); try await finish(secondHost, secondStore)
        try verifyClean(firstStore, "parallel-first-close"); try verifyClean(secondStore, "parallel-second-close")
        passed.append("Two live AudioHosts isolate their recovery journals and never quarantine or rewrite each other")

        // A candidate rejected after actual instantiation must remain journaled until
        // its native object releases; the original fixture reads the journal in deinit.
        _ = registration
        let failedStore = try makeStore("validation-failure")
        let probe = RecoveryFailureProbe(journal: journalURL(failedStore))
        RecoveryFailureUnit.nextProbe = probe
        defer { RecoveryFailureUnit.nextProbe = nil }
        let failedHost = try AudioHost(recoveryStore: failedStore,midiInputIDs:[])
        defer { failedHost.shutdown() }
        let desc = failureDescription
        var invalid = target
        invalid["component"] = "\(desc.componentType):\(desc.componentSubType):\(desc.componentManufacturer)"
        invalid["state"] = Data("invalid opaque property list".utf8).base64EncodedString()
        var validationFailed = false
        do { _ = try await failedHost.handle("plugin.load", payload: invalid) } catch { validationFailed = true }
        try require(validationFailed, "The invalid-state fixture unexpectedly loaded.")
        try await settle(failedHost, failedStore)
        let released = probe.snapshot()
        observations.append(["name": "failed-candidate-release", "probe": released, "journal": try journal(failedStore)])
        try require(released["created"] as? Int == 1 && released["released"] as? Int == 1 &&
                    released["journalPresentDuringRelease"] as? Bool == true,
                    "Failed candidate journal cleanup occurred before native resource release: \(released)")
        let lifetime = failedHost.status()["endpointLifetime"] as? [String: Any] ?? [:]
        try require(lifetime["activeInstances"] as? Int == 0 && lifetime["retiredUnits"] as? Int == 0,
                    "The rejected candidate remained active in the graph.")
        try await finish(failedHost, failedStore)
        try verifyClean(failedStore, "failed-candidate-close")
        passed.append("A caught AU validation failure stays journaled through native deinit, then clears without a false crash suspect")
        return passed
    }

    private static func journalURL(_ store: PluginRecoveryStore) -> URL {
        store.directory.appendingPathComponent("session-\(store.sessionID).json")
    }
}

private final class RecoveryFailureProbe {
    let journal: URL
    private let lock = NSLock()
    private var created = 0, released = 0
    private var journalPresentDuringRelease = false
    init(journal: URL) { self.journal = journal }
    func didCreate() { lock.lock(); created += 1; lock.unlock() }
    func didRelease() {
        let record = (try? Data(contentsOf: journal)).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        let entries = record?["entries"] as? [[String: Any]] ?? []
        let present = entries.contains { ["loading", "releasing"].contains($0["status"] as? String ?? "") }
        lock.lock(); released += 1; journalPresentDuringRelease = present; lock.unlock()
    }
    func snapshot() -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        return ["created": created, "released": released, "journalPresentDuringRelease": journalPresentDuringRelease]
    }
}

private final class RecoveryFailureUnit: AUAudioUnit {
    static var nextProbe: RecoveryFailureProbe?
    private let probe: RecoveryFailureProbe?
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        probe = Self.nextProbe
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [try AUAudioUnitBus(format: format)])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [try AUAudioUnitBus(format: format)])
        probe?.didCreate()
    }
    deinit { probe?.didRelease() }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var internalRenderBlock: AUInternalRenderBlock {
        { _, _, _, _, _, _, _ in kAudioUnitErr_Uninitialized }
    }
}

#endif

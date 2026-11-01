// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation

/// Exercises the real Bridge -> AudioHost project publication boundary. Gates
/// determine ordering; deadlines only turn a broken fixture into a bounded failure.
/// No WebKit window, user library, installed plug-in, MIDI input or audio device
/// playback is needed to test acknowledgement and stale persistence protection.
@MainActor
enum NativeBridgeProjectSyncTests {
    static func run(directory: URL) async throws -> [String] {
        NativeUINKSAutomationFixture.register()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var checks: [String] = [], evidence: [[String: Any]] = []

        func require(_ condition: Bool, _ message: String) throws {
            guard condition else { throw TrackerError.message("BRIDGE PROJECT SYNC: " + message) }
        }
        func bytes(_ value: [String: Any]?) throws -> Data? {
            try value.map { try JSONSerialization.data(withJSONObject: $0, options: .sortedKeys) }
        }
        func pump() async { for _ in 0..<24 { await Task.yield() } }
        func withFixture(_ name: String, _ action: (Fixture) async throws -> Void) async throws {
            let fixture = try Fixture(directory: directory.appendingPathComponent(name))
            do {
                _ = try await fixture.bridge.handle("engine.project", payload: ["project": fixture.baseline, "awaitPlugins": true])
                try await action(fixture)
                await fixture.close()
            } catch {
                await fixture.close()
                throw error
            }
        }
        func requireBridgeRejection(_ result: Result<Any, Error>, _ message: String) throws {
            guard case .failure(let error) = result else { throw TrackerError.message("BRIDGE PROJECT SYNC: " + message) }
            try require(error.localizedDescription.contains("session changed while its audio project"),
                        message + ": the Bridge checkpoint/revision guard did not report the failure: " + error.localizedDescription)
        }

        do {
            try await withFixture("await-forwarded") { fixture in
                let baseline = try bytes(fixture.store.project)
                let recovery = try Data(contentsOf: fixture.store.recoveryURL)
                let desired = fixture.pluginProject("Awaited project")
                let request = fixture.begin(desired, awaitPlugins: true)
                try await fixture.gate.waitForEntry(1)
                await pump()
                try require(request.result == nil, "awaitPlugins:true acknowledged before native instantiation was released")
                try require(try bytes(fixture.store.project) == baseline, "Pending native project replaced the Bridge store before acknowledgement")
                let deferred = try await fixture.bridge.handle("project.autosave", payload: [:]) as? [String: Any]
                try require(deferred?["saved"] as? Bool == false && deferred?["deferred"] as? Bool == true,
                            "Recovery did not defer while the native project acknowledgement was pending")
                try require(try bytes(fixture.store.project) == baseline && Data(contentsOf: fixture.store.recoveryURL) == recovery,
                            "Deferred recovery changed the pending publication's store checkpoint or recovery file")
                let pending = fixture.audio.status()["endpointLifetime"] as? [String: Any] ?? [:]
                try require(pending["activeInstances"] as? Int == 0, "The supposedly blocked fixture instantiated early")
                fixture.gate.release(1)
                let result = try await request.wait()
                if case .failure(let error) = result { throw error }
                try require(try bytes(fixture.store.project) == bytes(desired), "Successful awaited project was not committed")
                let parameters = try await fixture.audio.handle("plugin.parameters", payload: ["track": 0, "kind": "instrument", "slot": 0]) as? [[String: Any]] ?? []
                try require(parameters.contains { $0["address"] as? String == String(NativeUINKSAutomationFixture.mappedAddress) },
                            "Bridge success was not backed by an instantiated original AU")
                let resumed = try await fixture.bridge.handle("project.autosave", payload: [:]) as? [String: Any]
                try require(resumed?["saved"] as? Bool == true && resumed?["deferred"] as? Bool != true,
                            "Recovery remained blocked after native project acknowledgement completed")
                try require(fixture.store.project?["name"] as? String == desired["name"] as? String,
                            "Resumed recovery replaced the acknowledged project")
                evidence.append(["case": "await-forwarded", "pendingBeforeGateRelease": true, "storeDeferred": true,
                                 "actualParameterAvailableAfterSuccess": true, "recoveryDeferredDuringWait": true,
                                 "recoveryResumedAfterAcknowledgement": true])
            }
            checks.append("Native Bridge forwards awaitPlugins and commits the project only after the gated original AU is instantiated")

            try await withFixture("legacy-early-acknowledgement") { fixture in
                let desired = fixture.pluginProject("Ordinary asynchronous publication")
                let request = fixture.begin(desired, awaitPlugins: false)
                try await fixture.gate.waitForEntry(1)
                let result = try await request.wait()
                if case .failure(let error) = result { throw error }
                try require(fixture.gate.isBlocked(1), "The early acknowledgement test released its load gate")
                try require(try bytes(fixture.store.project) == bytes(desired), "Ordinary project publication lost its existing early acknowledgement")
                evidence.append(["case": "legacy-early-acknowledgement", "completedWhileInstantiationBlocked": true])
                fixture.gate.release(1)
            }
            checks.append("Native Bridge retains ordinary asynchronous publication when awaitPlugins is false")

            try await withFixture("routing-awaits-restored-plugin") { fixture in
                var candidate = fixture.pluginProject("Routing restores the desired instrument")
                candidate["routing"] = ["version": 1, "mixModel": "standard", "buses": [],
                    "outputs": ["track:bridge-project-sync": "master"], "sends": [], "sidechains": []]
                let request = fixture.begin(candidate, awaitPlugins: false, command: "routing.apply")
                try await fixture.gate.waitForEntry(1)
                await pump()
                try require(request.result == nil, "routing.apply acknowledged before its desired plug-in was restored")
                let pending = fixture.audio.status()["endpointLifetime"] as? [String: Any] ?? [:]
                try require(pending["activeInstances"] as? Int == 0, "Routing fixture instantiated before gate release")
                fixture.gate.release(1)
                let result = try await request.wait()
                if case .failure(let error) = result { throw error }
                let parameters = try await fixture.audio.handle("plugin.parameters", payload: ["track": 0, "kind": "instrument", "slot": 0]) as? [[String: Any]] ?? []
                try require(parameters.contains { $0["address"] as? String == String(NativeUINKSAutomationFixture.mappedAddress) },
                            "routing.apply success did not leave the restored parameter ready for editing")
                evidence.append(["case": "routing-awaits-restored-plugin", "pendingBeforeGateRelease": true,
                                 "actualParameterAvailableAfterSuccess": true])
            }
            checks.append("Native Bridge routing.apply waits for desired plug-in restoration before acknowledging a ready parameter")

            try await withFixture("new-session-during-wait") { fixture in
                let request = fixture.begin(fixture.pluginProject("Old pending project"), awaitPlugins: true)
                try await fixture.gate.waitForEntry(1)
                _ = try await fixture.bridge.handle("project.new", payload: [:])
                try require(fixture.store.project == nil && fixture.store.url == nil && !fixture.store.dirty, "New did not clear the private session")
                let recovery = try Data(contentsOf: fixture.store.recoveryURL)
                // New uses engine.panic without changing AudioHost's project
                // generation. The native await succeeds after release, so only
                // the Bridge's own post-await revision/checkpoint check can stop
                // this old payload from repopulating the newly empty store.
                fixture.gate.release(1)
                try requireBridgeRejection(try await request.wait(), "Old awaited payload repopulated the new session")
                try require(fixture.store.project == nil && fixture.store.url == nil && !fixture.store.dirty, "Late publication replaced the new session")
                try require(try Data(contentsOf: fixture.store.recoveryURL) == recovery, "Late publication rewrote new-session recovery")
                evidence.append(["case": "new-session-during-wait", "bridgeGuardRejected": true, "emptySessionPreserved": true])
            }
            checks.append("Native Bridge rejects a completed old audio publication after project.new instead of repopulating the new session")

            try await withFixture("newer-publication-during-wait") { fixture in
                let request = fixture.begin(fixture.pluginProject("Superseded pending project"), awaitPlugins: true)
                try await fixture.gate.waitForEntry(1)
                var newer = fixture.baseline; newer["name"] = "Newer published project"; newer["bpm"] = 137.0
                _ = try await fixture.bridge.handle("engine.project", payload: ["project": newer, "awaitPlugins": false, "replace": true])
                let expected = try bytes(fixture.store.project)
                try require(expected == bytes(newer), "Newer project was not published before releasing the older load")
                fixture.gate.release(1)
                let result = try await request.wait()
                guard case .failure(let error) = result else { throw TrackerError.message("BRIDGE PROJECT SYNC: Superseded publication incorrectly succeeded") }
                try require(error.localizedDescription.contains("superseded") || error.localizedDescription.contains("session changed"),
                            "Superseded publication lost its useful error")
                try require(try bytes(fixture.store.project) == expected, "Old request overwrote the newer published project")
                evidence.append(["case": "newer-publication-during-wait", "oldRequestRejected": true, "newerProjectPreserved": true])
            }
            checks.append("A newer Bridge project publication survives completion of an older awaited plug-in restore")

            try await withFixture("store-checkpoint-during-wait") { fixture in
                let request = fixture.begin(fixture.pluginProject("Pending store checkpoint"), awaitPlugins: true)
                try await fixture.gate.waitForEntry(1)
                var newer = fixture.baseline; newer["name"] = "Concurrent store edit"; newer["masterVolume"] = 0.31
                try fixture.store.update(newer)
                let expected = try bytes(fixture.store.project), originalURL = fixture.store.url
                fixture.gate.release(1)
                try requireBridgeRejection(try await request.wait(), "Old payload overwrote a concurrent store edit")
                try require(try bytes(fixture.store.project) == expected && fixture.store.url == originalURL,
                            "Post-await checkpoint protection lost a concurrent store edit or file association")
                evidence.append(["case": "store-checkpoint-during-wait", "bridgeCheckpointRejected": true, "concurrentEditPreserved": true])
            }
            checks.append("Native Bridge rechecks the exact store checkpoint after awaited audio preparation, preserving concurrent edits")
        } catch {
            try? writeReport(directory: directory, passed: false, checks: checks, evidence: evidence, error: error.localizedDescription)
            throw error
        }
        try writeReport(directory: directory, passed: true, checks: checks, evidence: evidence)
        return checks
    }

    private static func writeReport(directory: URL, passed: Bool, checks: [String], evidence: [[String: Any]], error: String? = nil) throws {
        var report: [String: Any] = ["passed": passed, "checks": checks, "cases": evidence,
            "generatedAt": ISO8601DateFormatter().string(from: Date()),
            "scope": "Real Bridge and AudioHost with continuation-gated original AU instantiation",
            "installedPluginsLoaded": false, "physicalPlayback": false, "midiInputsConnected": false,
            "visibleWindowsCreated": 0, "privateProjectStore": true, "privateLibrary": true, "privateVST3Catalogue": true]
        if let error { report["error"] = error }
        try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
            .write(to: directory.appendingPathComponent("bridge-project-sync-report.json"), options: .atomic)
    }

    @MainActor private final class Fixture {
        let gate = Gate()
        let store: ProjectStore
        let audio: AudioHost
        let bridge: Bridge
        let baseline: [String: Any]
        private var requests: [Request] = []
        init(directory: URL) throws {
            store = try ProjectStore(directory: directory.appendingPathComponent("Recovery"))
            baseline = ["version": 1, "name": "Saved sync baseline", "bpm": 120.0, "linesPerBeat": 4,
                "masterVolume": 0.7, "activePattern": 0, "sequence": [0],
                "tracks": [["routingID": "bridge-project-sync", "name": "Original AU", "instrument": 0]],
                "instruments": [["type": "au", "name": "Original gated instrument"]],
                "patterns": [["id": 0, "length": 4, "steps": [[NSNull(), NSNull(), NSNull(), NSNull()]], "automation": []]],
                "audioPlugins": [], "audioClips": []]
            _ = try store.save(to: directory.appendingPathComponent("Baseline.supertracker"), snapshot: baseline, collectSamples: false)
            var configuration = VST3Catalogue.Configuration()
            configuration.roots = []
            configuration.cacheURL = directory.appendingPathComponent("PrivateVST3Catalogue.json")
            let gate = self.gate
            audio = try AudioHost(beforePluginInstantiation: { try await gate.pause() }, recoveryStore: nil,
                                  vst3Catalogue: VST3Catalogue(configuration: configuration), midiInputIDs: [])
            let library = LibraryService(cacheDirectory: directory.appendingPathComponent("Library"), defaultRoots: [], loadCacheAsynchronously: false)
            bridge = Bridge(audio: audio, library: library, store: store)
        }
        func pluginProject(_ name: String) -> [String: Any] {
            var project = baseline; project["name"] = name
            project["audioPlugins"] = [["track": 0, "kind": "instrument", "slot": 0,
                "component": NativeUINKSAutomationFixture.identifier, "instanceID": UUID().uuidString]]
            return project
        }
        func begin(_ project: [String: Any], awaitPlugins: Bool, command: String = "engine.project") -> Request {
            let request = Request(bridge: bridge, project: project, awaitPlugins: awaitPlugins, command: command)
            requests.append(request); return request
        }
        func close() async {
            requests.forEach { $0.cancel() }
            gate.releaseAll()
            bridge.shutdown()
            await audio.shutdownAndWait()
        }
    }

    @MainActor private final class Request {
        private(set) var result: Result<Any, Error>?
        private var task: Task<Void, Never>?
        init(bridge: Bridge, project: [String: Any], awaitPlugins: Bool, command: String) {
            task = Task { @MainActor [weak self] in
                do { self?.result = .success(try await bridge.handle(command, payload: ["project": project, "awaitPlugins": awaitPlugins])) }
                catch { self?.result = .failure(error) }
            }
        }
        func wait() async throws -> Result<Any, Error> {
            let deadline = Date().addingTimeInterval(8)
            while result == nil && Date() < deadline { try await Task.sleep(nanoseconds: 5_000_000) }
            guard let result else {
                cancel(); throw TrackerError.message("BRIDGE PROJECT SYNC: Timed out waiting for a project acknowledgement")
            }
            return result
        }
        func cancel() { task?.cancel() }
    }

    @MainActor private final class Gate {
        private(set) var entries = 0
        private var blocked: [Int: CheckedContinuation<Void, Error>] = [:]
        private var deadlines: [Int: Task<Void, Never>] = [:]
        func pause() async throws {
            entries += 1; let entry = entries
            try await withCheckedThrowingContinuation { continuation in
                blocked[entry] = continuation
                deadlines[entry] = Task { @MainActor [weak self] in
                    do { try await Task.sleep(nanoseconds: 10_000_000_000) } catch { return }
                    self?.finish(entry, error: TrackerError.message("BRIDGE PROJECT SYNC: Instantiation gate deadline exceeded"))
                }
            }
        }
        func waitForEntry(_ entry: Int) async throws {
            let deadline = Date().addingTimeInterval(5)
            while entries < entry && Date() < deadline { try await Task.sleep(nanoseconds: 5_000_000) }
            guard isBlocked(entry) else { throw TrackerError.message("BRIDGE PROJECT SYNC: Original AU did not reach its instantiation gate") }
        }
        func isBlocked(_ entry: Int) -> Bool { blocked[entry] != nil }
        func release(_ entry: Int) { finish(entry, error: nil) }
        private func finish(_ entry: Int, error: Error?) {
            deadlines.removeValue(forKey: entry)?.cancel()
            guard let continuation = blocked.removeValue(forKey: entry) else { return }
            if let error { continuation.resume(throwing: error) } else { continuation.resume() }
        }
        func releaseAll() { for entry in Array(blocked.keys) { finish(entry, error: CancellationError()) } }
    }
}

#endif

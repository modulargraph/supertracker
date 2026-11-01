import Foundation
import AVFoundation
import AudioToolbox

/// Loads only the explicitly supplied original asynchronous fixture. Its duplicate
/// compatibility CID is isolated in a private catalogue and always resolved by path.
enum NativeVST3CaptureTests {
    @MainActor static func run(output: URL, fixtureBundle: URL) async throws -> [String] {
        let bundle = fixtureBundle.standardizedFileURL
        let folder = output.appendingPathComponent("vst3-capture", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let classID = "5653544E6924486D6173736976652078"
        var configuration = VST3Catalogue.Configuration()
        configuration.roots = [bundle.deletingLastPathComponent()]
        configuration.cacheURL = folder.appendingPathComponent("private-catalogue.json")
        let catalogue = VST3Catalogue(configuration: configuration)
        var checks: [String] = [], failures: [String] = [], measurements: [String: Any] = [
            "fixtureBundle": bundle.path, "installedVendorsUsed": false, "physicalPlayback": false, "midiInputsDisabled": true,
            "privateCatalogue": configuration.cacheURL.path, "classID": classID]
        var active: CaptureHost?
        func write(_ status: String) throws {
            let value: [String: Any] = ["passed": false, "audioChecksPassed": status == "audioChecksComplete" && failures.isEmpty,
                "processExitPassed": false, "status": status, "scope": "original-vst3-capture", "nksStatus": "notRun",
                "checks": checks, "failures": failures, "measurements": measurements]
            try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .prettyPrinted])
                .write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Original capture fixture: " + message) }
        }
        func pass(_ name: String) throws { checks.append(name); fputs("Capture fixture PASS: \(name)\n", stderr); try write("running") }
        func saved(_ value: [String: Any], name: String, expected: Double) throws {
            try require(value["stateError"] == nil && !(value["state"] as? String ?? "").isEmpty,
                        "\(name) did not capture valid opaque state.")
            try require(value["bundlePath"] as? String == bundle.path && value["manufacturer"] as? String == "SuperTracker Tests",
                        "\(name) substituted a different bundle or manufacturer.")
            let gain = try CaptureHost.componentGain(value)
            try require(abs(gain - expected) < 0.000001, "\(name) serialized stale fixture component gain \(gain), expected \(expected).")
            var states = measurements["opaqueFixtureGains"] as? [String: Double] ?? [:]
            states[name] = gain; measurements["opaqueFixtureGains"] = states
            try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .prettyPrinted])
                .write(to: folder.appendingPathComponent(name + ".json"), options: .atomic)
        }
        func fresh(_ descriptor: VST3PluginDescriptor) async throws -> CaptureHost {
            if let active { await active.host.shutdownAndWait() }
            let value = try CaptureHost(catalogue: catalogue, descriptor: descriptor)
            active = value; try await value.initialize(); return value
        }
        func expectRejected(_ task: Task<[String: Any], Error>, name: String) async throws {
            do { _ = try await task.value; throw CaptureTestError.accepted(name) }
            catch let error as CaptureTestError { throw error }
            catch { measurements[name + "Error"] = error.localizedDescription }
        }
        try write("running")
        do {
            // The private scanner receives precisely this path. It never calls a
            // full refresh or searches installed plug-in roots for the duplicate CID.
            let found = try await catalogue.scanBundle(bundle, force: true)
            guard let descriptor = found.first(where: { $0.classID == classID }) else {
                throw AudioSampleError.invalid("The original capture fixture class is missing.")
            }
            try require(descriptor.bundlePath == bundle.path && descriptor.manufacturer == "SuperTracker Tests" &&
                        descriptor.name == "SuperTracker Async Capture Instrument" && descriptor.isInstrument,
                        "The requested module is not the original asynchronous capture fixture.")
            measurements["descriptor"] = descriptor.dictionary
            let session = try await fresh(descriptor)
            let adapter = try session.adapter(0)
            try require(adapter.requiredStateFlushFrames == 4096 && adapter.status["loadedBundleVersion"] as? String == "1.7.1",
                        "The real loaded-bundle compatibility gate was not selected.")

            try await session.edit(0.47)
            let started = Date()
            let first = try await session.capture()
            measurements["stoppedCaptureSeconds"] = Date().timeIntervalSince(started)
            try saved(first, name: "stopped-immediate-047", expected: 0.47)
            try require(!adapter.stateCaptureSnapshot.dirty && adapter.pendingStateEdits == 0,
                        "Successful capture did not acknowledge the delivered generation.")
            try await session.reload(first)
            try require(abs(try await session.value() - 0.47) < 0.000001, "Fresh component state lost stopped Gain0.47.")
            measurements["stoppedFreshPCMError"] = try await session.provePCM(0.47)
            try pass("Stopped edit uses guarded asynchronous capture and fresh state produces exact Gain0.47 PCM")

            // The engine is offline manual-rendering even while isRunning=true.
            // A normal callback drains the edit while the public capture awaits it.
            let running = try session.adapter(0)
            try session.graph.start()
            try await session.edit(0.31)
            let runningTask = Task { @MainActor in try await session.capture() }
            await Task.yield()
            try session.renderSilence(512)
            let runningState = try await runningTask.value
            try saved(runningState, name: "running-immediate-031", expected: 0.31)
            try require(!running.stateCaptureSnapshot.dirty, "Running capture left its generation unacknowledged.")
            session.graph.stop()
            try await session.reload(runningState)
            try require(abs(try await session.value() - 0.31) < 0.000001, "Fresh running state lost Gain0.31.")
            let drained = try session.adapter(0)
            try session.graph.start(); try await session.edit(0.62); try session.renderSilence(512)
            try require(drained.pendingStateEdits == 0 && drained.stateCaptureSnapshot.dirty,
                        "Ordinary delivery incorrectly cleared the dirty marker.")
            let drainedStarted = Date(), drainedState = try await session.capture()
            measurements["alreadyDrainedCaptureSeconds"] = Date().timeIntervalSince(drainedStarted)
            try saved(drainedState, name: "running-already-drained-062", expected: 0.62)
            session.graph.stop(); try await session.reload(drainedState)
            try require(abs(try await session.value() - 0.62) < 0.000001, "Already-drained capture saved stale model state.")
            try pass("Running queued and already-drained edits settle through the actual rack before opaque capture")

            let cancelledAdapter = try session.adapter(0)
            try await session.edit(0.48)
            let cancelled = Task { @MainActor in try await session.capture() }
            try await session.waitUntilPrepared(cancelledAdapter)
            cancelled.cancel(); try await expectRejected(cancelled, name: "cancelledCapture")
            try require(cancelledAdapter.stateCaptureSnapshot.dirty && cancelledAdapter.pendingStateEdits == 0,
                        "Cancellation lost the delivered-but-unsaved marker.")
            let retried = try await session.capture(); try saved(retried, name: "cancelled-retry-048", expected: 0.48)
            try await session.reload(retried)
            try require(abs(try await session.value() - 0.48) < 0.000001, "Retry after cancellation lost the edit.")
            try pass("Cancellation after dequeue preserves dirty state and retry captures the same edit")

            let editedAdapter = try session.adapter(0)
            try await session.edit(0.2)
            let changing = Task { @MainActor in try await session.capture() }
            try await session.waitUntilPrepared(editedAdapter)
            try await session.edit(0.8)
            try await expectRejected(changing, name: "concurrentEdit")
            try require(editedAdapter.stateCaptureSnapshot.dirty && editedAdapter.pendingStateEdits == 1,
                        "A concurrent edit was consumed or acknowledged by the obsolete save.")
            let latest = try await session.capture(); try saved(latest, name: "concurrent-edit-retry-08", expected: 0.8)
            try await session.reload(latest)
            try require(abs(try await session.value() - 0.8) < 0.000001, "Concurrent edit retry captured the wrong generation.")
            try pass("A new control edit invalidates the old request without losing the new pending value")

            let graphAdapter = try session.adapter(0)
            try await session.edit(0.44)
            let obsoleteGraph = Task { @MainActor in try await session.capture() }
            try await session.waitUntilPrepared(graphAdapter)
            _ = try await session.host.handle("transport.seek", payload: ["row": 1])
            try await expectRejected(obsoleteGraph, name: "graphEpoch")
            try require(!session.graph.isRunning && graphAdapter.stateCaptureSnapshot.dirty,
                        "A graph/transport change lost dirty state or unexpectedly started playback.")
            _ = try await session.capture()
            try pass("A transport epoch change invalidates capture although the graph remains stopped")

            let concurrentAdapter = try session.adapter(0)
            try await session.edit(0.66)
            let concurrentStarted = Date()
            let explicitSave = Task { @MainActor in try await session.capture() }
            try await session.waitUntilPrepared(concurrentAdapter)
            let recoverySnapshot = Task { @MainActor in
                try await session.host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            }
            let explicitState = try await explicitSave.value
            let recoveryProject = try await recoverySnapshot.value
            let overlapSeconds = Date().timeIntervalSince(concurrentStarted)
            measurements["sameGenerationOverlapSeconds"] = overlapSeconds
            try saved(explicitState, name: "overlap-explicit-save-066", expected: 0.66)
            let recoveryStates = recoveryProject["audioPlugins"] as? [[String: Any]] ?? []
            try require(recoveryStates.count == 1, "Concurrent recovery snapshot lost the plug-in.")
            try saved(recoveryStates[0], name: "overlap-recovery-snapshot-066", expected: 0.66)
            try require(overlapSeconds < 1.9 && !concurrentAdapter.stateCaptureSnapshot.dirty,
                        "Same-generation snapshots did not share the completed clean capture.")
            try await session.reload(recoveryStates[0])
            try require(abs(try await session.value() - 0.66) < 0.000001,
                        "The queued recovery snapshot captured stale state.")
            try pass("Concurrent explicit save and recovery snapshot both succeed without replacing each other's capture ticket")

            let admissionAdapter = try session.adapter(0)
            try await session.edit(0.67)
            let admittedSave = Task { @MainActor in try await session.capture() }
            try await session.waitUntilPrepared(admissionAdapter)
            let queuedSave = Task { @MainActor in try await session.capture() }
            await Task.yield(); queuedSave.cancel()
            try await expectRejected(queuedSave, name: "cancelledQueuedCapture")
            let admittedState = try await admittedSave.value
            try saved(admittedState, name: "active-save-survives-waiter-cancellation-067", expected: 0.67)
            try require(!admissionAdapter.stateCaptureSnapshot.dirty,
                        "Cancellation of a queued save invalidated the active save.")
            try await session.reload(admittedState)
            try require(abs(try await session.value() - 0.67) < 0.000001,
                        "The active save lost its value when a waiter cancelled.")
            try pass("Cancelling a queued capture leaves the active request's token and result intact")

            let oldSessionAdapter = try session.adapter(0)
            try await session.edit(0.69)
            let oldActive = Task { @MainActor in try await session.capture() }
            try await session.waitUntilPrepared(oldSessionAdapter)
            let oldQueued = Task { @MainActor in
                try await session.host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            }
            await Task.yield()
            try await session.initialize()
            try await expectRejected(oldActive, name: "activeOldSession")
            try await expectRejected(oldQueued, name: "queuedOldSession")
            try require(abs(try await session.value() - 0.25) < 0.000001,
                        "An old-session request replaced the new session's initial fixture state.")
            try pass("Active and queued old-session saves both reject after project replacement")

            try await session.edit(0.55)
            let replacementAdapter = try session.adapter(0)
            let replaced = Task { @MainActor in try await session.capture() }
            try await session.waitUntilPrepared(replacementAdapter)
            _ = try await session.host.handle("plugin.remove", payload: session.target(0))
            session.adapters.removeValue(forKey: 0)
            try await expectRejected(replaced, name: "instanceReplacement")
            try await session.load(track: 0)
            try require(abs(try await session.value() - 0.25) < 0.000001, "Replacement reused obsolete prepared state.")
            try pass("Removing the instance during settlement rejects its obsolete capture ticket")

            if bundle.lastPathComponent == "SettledCaptureEditsFixture.vst3" {
                let log = folder.appendingPathComponent("process.log")
                let previous = try session.adapter(0)
                _ = try await session.host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 60, "velocity": 127])
                try session.requireCurrentPCM(0.25)
                let cancellationMark = session.candidateCaptureCount(log)
                let cancelledLoad = Task { @MainActor in
                    try await session.load(track: 0); return ["unexpectedCommit": true] as [String: Any]
                }
                try await session.waitForCandidateCapture(after: cancellationMark, log: log)
                try require(session.graph.isRunning && session.adapters[0] === previous,
                            "Unattached candidate preparation paused or replaced the existing graph.")
                try session.requireCurrentPCM(0.25)
                cancelledLoad.cancel(); try await expectRejected(cancelledLoad, name: "candidateCaptureCancelled")
                try require(session.graph.isRunning && session.adapters[0] === previous,
                            "Cancelling candidate capture removed the previous live instrument.")
                try session.requireCurrentPCM(0.25)

                let replacementMark = session.candidateCaptureCount(log)
                let obsoleteLoad = Task { @MainActor in
                    try await session.load(track: 0); return ["unexpectedCommit": true] as [String: Any]
                }
                try await session.waitForCandidateCapture(after: replacementMark, log: log)
                try session.requireCurrentPCM(0.25)
                try await session.load(track: 0)
                try await expectRejected(obsoleteLoad, name: "candidateCaptureReplaced")
                try require(session.graph.isRunning && session.adapters[0] !== previous,
                            "The newer loading generation did not replace the obsolete candidate.")
                _ = try await session.host.handle("note.off", payload: ["track": 0, "note": 60])
                try session.renderSilence(128); session.graph.stop()
                try pass("Vendor-originated initialization edits settle before commit; candidate cancellation and replacement preserve playable existing audio")
            }

            try await session.load(track: 1)
            try await session.edit(0.11, track: 0); try await session.edit(0.22, track: 1)
            let batchStarted = Date()
            let snapshot = try await session.host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            let batchSeconds = Date().timeIntervalSince(batchStarted)
            measurements["twoInstanceCaptureSeconds"] = batchSeconds
            let plugins = snapshot["audioPlugins"] as? [[String: Any]] ?? []
            try require(plugins.count == 2 && plugins.allSatisfy { $0["stateError"] == nil && !($0["state"] as? String ?? "").isEmpty },
                        "The two-instance snapshot failed to capture every opaque state.")
            try require(batchSeconds < 1.9, "Two instances did not share a single bounded settlement interval (\(batchSeconds)s).")
            for track in 0...1 {
                guard let state = plugins.first(where: { ($0["track"] as? NSNumber)?.intValue == track }) else {
                    throw AudioSampleError.invalid("Missing captured fixture track.")
                }
                try saved(state, name: "batch-track-\(track)", expected: track == 0 ? 0.11 : 0.22)
                try await session.reload(state, track: track)
                try require(abs(try await session.value(track) - (track == 0 ? 0.11 : 0.22)) < 0.000001,
                            "The batch mixed component states across instances.")
            }
            try pass("Two dirty native instances share one settlement interval and retain independent fresh component states")
            await session.host.shutdownAndWait(); active = nil
            try write("audioChecksComplete")
            return checks
        } catch {
            failures.append(error.localizedDescription)
            if let active { await active.host.shutdownAndWait() }
            try? write("failed"); throw error
        }
    }
}

private enum CaptureTestError: LocalizedError {
    case accepted(String)
    var errorDescription: String? { switch self { case .accepted(let name): return "Obsolete capture unexpectedly succeeded: \(name)." } }
}

@MainActor private final class CaptureHost {
    let graph: AVAudioEngine
    let host: AudioHost
    let descriptor: VST3PluginDescriptor
    let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
    var adapters: [Int: VST3AudioUnit] = [:]
    init(catalogue: VST3Catalogue, descriptor: VST3PluginDescriptor) throws {
        self.descriptor = descriptor
        graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        host = try AudioHost(audioGraph: graph, vst3Catalogue: catalogue, midiInputIDs: [])
    }
    static func componentGain(_ descriptor: [String: Any]) throws -> Double {
        guard let encoded = descriptor["state"] as? String, let plist = Data(base64Encoded: encoded),
              let dictionary = try PropertyListSerialization.propertyList(from: plist, options: [], format: nil) as? [String: Any],
              let bytes = dictionary["data"] as? Data, bytes.count >= 72,
              bytes.prefix(8) == Data([83, 84, 86, 51, 1, 0, 0, 0]) else {
            throw AudioSampleError.invalid("Original fixture capture did not contain an STV3 envelope.")
        }
        let length = bytes.withUnsafeBytes { UInt64(littleEndian: $0.loadUnaligned(fromByteOffset: 40, as: UInt64.self)) }
        let magic = bytes.withUnsafeBytes { UInt32(littleEndian: $0.loadUnaligned(fromByteOffset: 56, as: UInt32.self)) }
        let version = bytes.withUnsafeBytes { UInt32(littleEndian: $0.loadUnaligned(fromByteOffset: 60, as: UInt32.self)) }
        guard length == 16 && magic == 0x53434346 && version == 1 else {
            throw AudioSampleError.invalid("Original fixture component state has the wrong identity or byte count.")
        }
        let gain = bytes.withUnsafeBytes { Double(bitPattern: UInt64(littleEndian: $0.loadUnaligned(fromByteOffset: 64, as: UInt64.self))) }
        guard gain.isFinite && (0...1).contains(gain) else { throw AudioSampleError.invalid("Original fixture opaque gain is invalid.") }
        return gain
    }
    func target(_ track: Int) -> [String: Any] { ["track": track, "kind": "instrument", "slot": 0] }
    func initialize() async throws {
        let midiIsolation = try await host.handle("midi.devices", payload: [:]) as? [String: Any] ?? [:]
        guard midiIsolation["allInputs"] as? Bool == false &&
              (midiIsolation["inputIDs"] as? [Int])?.isEmpty == true else {
            throw AudioSampleError.invalid("The capture fixture must not subscribe to external MIDI inputs.")
        }
        let rows = [Any](repeating: NSNull(), count: 4)
        let project: [String: Any] = ["version": 1, "name": "Isolated asynchronous capture fixture", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0, "loopSong": false, "metronome": false,
            "tracks": (0...1).map { ["name": "Fixture \($0)", "instrument": $0, "volume": 1, "pan": 0, "delaySend": 0, "reverbSend": 0] as [String: Any] },
            "instruments": (0...1).map { _ in ["type": "au", "volume": 1, "pan": 0] as [String: Any] },
            "patterns": [["id": 0, "length": 4, "steps": [rows, rows], "automation": []]], "sequence": [0],
            "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        await host.pluginRestorationBarrier()?.value
        try await load(track: 0)
    }
    func adapter(_ track: Int) throws -> VST3AudioUnit {
        guard let value = adapters[track] else { throw AudioSampleError.invalid("Missing fixture adapter.") }; return value
    }
    func load(track: Int, saved: [String: Any]? = nil) async throws {
        let existing = Set(graph.attachedNodes.compactMap { ($0 as? AVAudioUnit)?.auAudioUnit as? VST3AudioUnit }.map(ObjectIdentifier.init))
        var payload = saved ?? target(track)
        payload["track"] = track; payload["component"] = descriptor.id; payload["bundlePath"] = descriptor.bundlePath
        let loaded = try await host.handle("plugin.load", payload: payload) as? [String: Any] ?? [:]
        guard loaded["bundlePath"] as? String == descriptor.bundlePath,
              loaded["manufacturer"] as? String == "SuperTracker Tests",
              let unit = graph.attachedNodes.compactMap({ ($0 as? AVAudioUnit)?.auAudioUnit as? VST3AudioUnit })
                .first(where: { !existing.contains(ObjectIdentifier($0)) }) else {
            throw AudioSampleError.invalid("Fixture load did not attach the exact isolated component.")
        }
        adapters[track] = unit
    }
    func reload(_ state: [String: Any], track: Int = 0) async throws {
        graph.stop()
        _ = try await host.handle("plugin.remove", payload: target(track)); adapters.removeValue(forKey: track)
        try await load(track: track, saved: state)
    }
    func edit(_ value: Double, track: Int = 0) async throws {
        var payload = target(track); payload["address"] = "42"; payload["value"] = value; payload["normalized"] = true
        _ = try await host.handle("plugin.parameter", payload: payload)
    }
    func capture(_ track: Int = 0) async throws -> [String: Any] {
        try await host.handle("plugin.state", payload: target(track)) as? [String: Any] ?? [:]
    }
    func value(_ track: Int = 0) async throws -> Double {
        let parameters = try await host.handle("plugin.parameters", payload: target(track)) as? [[String: Any]] ?? []
        return (parameters.first(where: { $0["address"] as? String == "42" })?["normalized"] as? NSNumber)?.doubleValue ?? -1
    }
    func waitUntilPrepared(_ adapter: VST3AudioUnit) async throws {
        let clock = ContinuousClock(), deadline = clock.now.advanced(by: .seconds(2))
        while adapter.pendingStateEdits > 0 {
            guard clock.now < deadline else { throw AudioSampleError.invalid("Fixture preparation did not drain its edit.") }
            try await Task.sleep(for: .milliseconds(1))
        }
        guard adapter.stateCaptureSnapshot.dirty else { throw AudioSampleError.invalid("Fixture preparation acknowledged state before capture.") }
    }
    func renderSilence(_ frames: AVAudioFrameCount) throws {
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)!
        guard try graph.renderOffline(frames, to: buffer) == .success else { throw AudioSampleError.invalid("Fixture ordinary render failed.") }
    }
    func candidateCaptureCount(_ log: URL) -> Int {
        ((try? String(contentsOf: log, encoding: .utf8)) ?? "").components(separatedBy: "VST3 rack: prepare candidate capture").count - 1
    }
    func waitForCandidateCapture(after count: Int, log: URL) async throws {
        let clock = ContinuousClock(), deadline = clock.now.advanced(by: .seconds(4))
        while candidateCaptureCount(log) <= count {
            guard clock.now < deadline else { throw AudioSampleError.invalid("The candidate did not reach its capture preparation.") }
            try await Task.sleep(for: .milliseconds(2))
        }
        // This runs on the same actor as loading: observing the begin trace means
        // the dirty original candidate has suspended in its settlement interval.
    }
    func requireCurrentPCM(_ gain: Double) throws {
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 128)!
        guard graph.isRunning, try graph.renderOffline(128, to: buffer) == .success, let samples = buffer.floatChannelData else {
            throw AudioSampleError.invalid("The existing graph could not render during candidate capture.")
        }
        for channel in 0..<2 { for frame in 0..<Int(buffer.frameLength) {
            guard abs(Double(samples[channel][frame]) - gain) < 0.000001 else {
                throw AudioSampleError.invalid("Candidate capture interrupted existing playable PCM.")
            }
        } }
    }
    func provePCM(_ gain: Double) async throws -> Double {
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 60, "velocity": 127])
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
        guard try graph.renderOffline(512, to: buffer) == .success, let samples = buffer.floatChannelData else {
            throw AudioSampleError.invalid("Fixture fresh-state PCM render failed.")
        }
        var error = 0.0
        for channel in 0..<2 { for frame in 0..<Int(buffer.frameLength) { error = max(error, abs(Double(samples[channel][frame]) - gain)) } }
        _ = try await host.handle("note.off", payload: ["track": 0, "note": 60])
        try renderSilence(128); graph.stop()
        guard error < 0.000001 else { throw AudioSampleError.invalid("Fresh fixture DSP did not preserve gain: PCM error \(error).") }
        return error
    }
}

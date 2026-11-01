// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox

/// Regression coverage for Save/autosave racing an asynchronous AU project restore.
/// Continuation gates control scheduling; no audio device or timing sleeps are needed.
enum NativePluginStateTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("plugin-state", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let gate = InstantiationGate()
        let events = RestoreEvents()
        let host = try AudioHost(beforePluginInstantiation: { try await gate.pause() }, midiInputIDs: [])
        host.onEvent = { name, value in events.receive(name, value) }
        defer { gate.releaseAll(); host.shutdown() }
        var passed: [String] = []
        let target: [String: Any] = ["track": 0, "kind": "effect", "slot": 0]

        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Plug-in state regression: " + message) }
        }
        func component(_ name: String, in available: [[String: Any]]) throws -> String {
            guard let item = available.first(where: { ($0["name"] as? String ?? "").localizedCaseInsensitiveCompare(name) == .orderedSame }),
                  let identifier = item["id"] as? String else {
                throw AudioSampleError.invalid("Plug-in state tests require the macOS Audio Unit " + name + ".")
            }
            return identifier
        }
        func setDelay(_ value: Double) async throws {
            var payload = target
            payload["address"] = String(kDelayParam_DelayTime)
            payload["value"] = value; payload["normalized"] = false
            _ = try await host.handle("plugin.parameter", payload: payload)
        }
        func delayValue() async throws -> Double {
            let parameters = try await host.handle("plugin.parameters", payload: target) as? [[String: Any]] ?? []
            return (parameters.first { $0["address"] as? String == String(kDelayParam_DelayTime) }?["value"] as? NSNumber)?.doubleValue ?? -1
        }
        func liveState() async throws -> [String: Any] {
            guard let state = try await host.handle("plugin.state", payload: target) as? [String: Any], state["state"] is String else {
                throw AudioSampleError.invalid("AUDelay did not return serializable state.")
            }
            return state
        }
        func snapshot(_ name: String) async throws -> [String: Any] {
            let value = try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            let data = try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: folder.appendingPathComponent(name + ".json"), options: .atomic)
            return value
        }
        func savedEffect(_ values: [[String: Any]]) -> [String: Any]? {
            values.first { ($0["track"] as? Int) == 0 && ($0["kind"] as? String) == "effect" && ($0["slot"] as? Int) == 0 }
        }
        func verifyPending(_ expected: [String: Any], name: String) async throws {
            let saved = try await snapshot(name)
            let all = try await host.handle("plugin.state", payload: [:]) as? [[String: Any]] ?? []
            let selected = try await liveState()
            let expectedBlob = expected["state"] as? String
            try require(savedEffect(saved["audioPlugins"] as? [[String: Any]] ?? [])?["state"] as? String == expectedBlob,
                        "engine.snapshot replaced the requested state while \(name).")
            try require(savedEffect(all)?["state"] as? String == expectedBlob,
                        "plugin.state replaced the requested state while \(name).")
            try require(selected["state"] as? String == expectedBlob,
                        "A targeted state read replaced the requested state while \(name).")
        }
        func apply(_ project: [String: Any]) async throws {
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        }
        func applyAndFinish(_ project: [String: Any]) async throws {
            let mark = events.count
            try await apply(project)
            let result = await events.next(after: mark)
            try require(result.name == "plugins.changed", "Unexpected restore error: \(result.value)")
        }

        let available = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
        let delayID = try component("AUDelay", in: available)
        let instrumentID = try component("DLSMusicDevice", in: available)
        var load = target; load["component"] = delayID
        _ = try await host.handle("plugin.load", payload: load)
        try await setDelay(0.2); let stateA = try await liveState()
        try await setDelay(0.8); let stateB = try await liveState()
        try await setDelay(0.45); let stateC = try await liveState()
        try require(stateA["state"] as? String != stateB["state"] as? String,
                    "The two real AUDelay fixtures must contain distinct opaque state.")
        try await setDelay(0.2)

        let rows = [Any](repeating: NSNull(), count: 4)
        var project: [String: Any] = ["version": 1, "name": "AU restore state regression", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 0.5, "activePattern": 0, "loopSong": false,
            "tracks": (0..<3).map { ["name": "Track \($0 + 1)", "instrument": 0, "volume": 0.7] as [String: Any] },
            "instruments": [["type": "synth", "volume": 0.5]],
            "patterns": [["id": 0, "length": 4, "steps": [rows, rows, rows], "automation": []]],
            "sequence": [0], "audioPlugins": [stateA], "delay": ["mix": 0], "reverb": ["mix": 0]]
        try await applyAndFinish(project)
        var instrument: [String: Any] = ["track": 1, "kind": "instrument", "slot": 0,
            "component": instrumentID, "name": "DLSMusicDevice"]
        project["audioPlugins"] = [instrument, stateB]
        gate.enabled = true
        let firstCompletion = events.count
        try await apply(project)
        await gate.waitForEntry(1)
        try require(abs(try await delayValue() - 0.2) < 0.0001,
                    "The blocked restore must leave the previous live patch in place until its turn.")
        try await verifyPending(stateB, name: "pending-state-b")
        gate.release(1)
        let firstResult = await events.next(after: firstCompletion)
        try require(firstResult.name == "plugins.changed", "State B restoration did not complete.")
        try require(abs(try await delayValue() - 0.8) < 0.0001,
                    "The requested AUDelay B patch did not reach the actual Audio Unit.")
        passed.append("Snapshots and targeted AU state reads preserve the requested patch during asynchronous project loading")

        // Once reconciliation finishes, state reads must resume collecting native
        // edits instead of remaining permanently pinned to the requested document.
        try await setDelay(0.35)
        let edited = try await liveState()
        let afterEdit = try await snapshot("completed-native-edit")
        try require(equivalentState(savedEffect(afterEdit["audioPlugins"] as? [[String: Any]] ?? [])?["state"] as? String,
                                    edited["state"] as? String),
                    "A live AU edit after completed restore was lost from the next snapshot.")
        try require(!equivalentState(edited["state"] as? String, stateB["state"] as? String),
                    "The completed-state test must actually alter the real plug-in patch.")
        passed.append("Completed AU restoration resumes capturing subsequent live parameter edits")

        // Remove the first instrument so each new project has an independently
        // blocked load. A canceled old restore must never clear the newer guard.
        gate.enabled = false
        project["audioPlugins"] = [stateA]
        try await applyAndFinish(project)
        gate.enabled = true
        project["audioPlugins"] = [instrument, stateB]
        try await apply(project)
        await gate.waitForEntry(2)
        let olderRestore = host.pluginRestorationBarrier()
        instrument["track"] = 2
        project["audioPlugins"] = [instrument, stateC]
        let newestCompletion = events.count
        try await apply(project)
        await gate.waitForEntry(3)
        gate.release(2)
        await olderRestore?.value
        try require(events.count == newestCompletion, "A canceled restore published completion for the newer project.")
        try require(abs(try await delayValue() - 0.2) < 0.0001,
                    "The superseded task applied its old requested patch after cancellation.")
        try await verifyPending(stateC, name: "newer-state-c-after-old-cancellation")
        gate.release(3)
        let newestResult = await events.next(after: newestCompletion)
        let newestDelay = try await delayValue()
        try require(newestResult.name == "plugins.changed" && abs(newestDelay - 0.45) < 0.0001,
                    "The newest project did not finish with its own requested patch.")
        let newest = try await snapshot("completed-state-c")
        let newestPlugins = newest["audioPlugins"] as? [[String: Any]] ?? []
        try require(!newestPlugins.contains { ($0["track"] as? Int) == 1 && $0["kind"] as? String == "instrument" },
                    "A canceled instrument load appeared in the newer project.")
        passed.append("Canceled AU restores cannot clear a newer pending snapshot guard or publish stale plug-ins")

        gate.enabled = false
        var invalid = stateC
        invalid["state"] = Data("deliberately invalid opaque AU document".utf8).base64EncodedString()
        project["audioPlugins"] = [invalid]
        let failureMark = events.count
        try await apply(project)
        let failure = await events.next(after: failureMark)
        try require(failure.name == "plugin.error", "Invalid saved state must report restoration failure.")
        try await verifyPending(invalid, name: "failed-same-component-state")
        var oldInstanceAccessible = false
        do { _ = try await host.handle("plugin.parameters", payload: target); oldInstanceAccessible = true }
        catch { }
        try require(!oldInstanceAccessible, "A failed same-component state restore left the previous patch active.")
        _ = try await host.handle("plugin.remove", payload: target)
        let removed = try await snapshot("explicitly-removed-failed-state")
        try require((removed["audioPlugins"] as? [[String: Any]])?.isEmpty == true,
                    "Explicit removal must discard the failed state's saved descriptor.")
        passed.append("Failed same-component AU state remains recoverable in snapshots while the stale instance is removed")

        // A direct async load must reserve graph admission before it suspends; an
        // export cannot switch the same graph into manual rendering underneath it.
        gate.enabled = false
        project["audioPlugins"] = []
        try await applyAndFinish(project)
        gate.enabled = true
        let directEntry = gate.entries + 1
        let directLoad = Task { @MainActor in try await host.handle("plugin.load", payload: load) }
        await gate.waitForEntry(directEntry)
        let admissionURL = folder.appendingPathComponent("admitted-export.wav")
        try? FileManager.default.removeItem(at: admissionURL)
        var exportRefused = false
        do { _ = try await host.handle("audio.export", payload: ["path": admissionURL.path, "mode": "pattern", "tail": 0]) }
        catch { exportRefused = error.localizedDescription.contains("loading") }
        try require(exportRefused && !FileManager.default.fileExists(atPath: admissionURL.path),
                    "A suspended plug-in load allowed export to mutate its graph.")
        gate.release(directEntry)
        _ = try await directLoad.value
        _ = try await host.handle("audio.export", payload: ["path": admissionURL.path, "mode": "pattern", "tail": 0])
        try require(abs(try AudioSamples.decode(admissionURL.path).duration - 0.5) < 0.001,
                    "Export did not resume normally after direct loading completed.")
        passed.append("Suspended direct plug-in loading excludes offline graph mutation and releases export admission afterward")

        gate.enabled = false
        try await applyAndFinish(project)
        gate.enabled = true
        let canceledEntry = gate.entries + 1
        let canceledLoad = Task { @MainActor in try await host.handle("plugin.load", payload: load) }
        await gate.waitForEntry(canceledEntry)
        let replacementCompletion = events.count
        try await apply(project)
        _ = await events.next(after: replacementCompletion)
        gate.release(canceledEntry)
        var loadCanceled = false
        do { _ = try await canceledLoad.value } catch { loadCanceled = true }
        let replaced = try await snapshot("direct-load-canceled-by-new-project")
        try require(loadCanceled && (replaced["audioPlugins"] as? [[String: Any]])?.isEmpty == true,
                    "A direct load from the previous project committed after project replacement.")
        var staleDirectInstance = false
        do { _ = try await host.handle("plugin.parameters", payload: target); staleDirectInstance = true } catch { }
        try require(!staleDirectInstance, "The canceled direct load left a stale live Audio Unit.")
        passed.append("Project replacement cancels a suspended direct plug-in load before it can commit stale state")

        let removalEntry = gate.entries + 1
        instrument["track"] = 1
        project["audioPlugins"] = [instrument, stateB]
        let removalRestoreMark = events.count
        try await apply(project)
        await gate.waitForEntry(removalEntry)
        let pendingRemoval = Task { @MainActor in try await host.handle("plugin.remove", payload: target) }
        for _ in 0..<1000 {
            if host.status()["graphMutationsPending"] as? Int == 1 { break }
            await Task.yield()
        }
        try require(host.status()["graphMutationsPending"] as? Int == 1,
                    "The removal task must enter graph admission while project loading is suspended.")
        gate.enabled = false
        gate.release(removalEntry)
        _ = await events.next(after: removalRestoreMark)
        _ = try await pendingRemoval.value
        let afterPendingRemoval = try await snapshot("remove-during-project-loading")
        try require(savedEffect(afterPendingRemoval["audioPlugins"] as? [[String: Any]] ?? []) == nil,
                    "Asynchronous restoration re-created a plug-in after its explicit removal.")
        passed.append("Explicit plug-in removal during project loading cannot be undone by the pending restore")

        // The renderer's opt-in acknowledgement must cover the restoration it
        // published. Otherwise a control write immediately after acknowledgement
        // can be overwritten by the same request's delayed saved-state restore.
        let acknowledgementGate = InstantiationGate(); acknowledgementGate.enabled = true
        let acknowledgementEvents = RestoreEvents()
        let acknowledgementHost = try AudioHost(beforePluginInstantiation: { try await acknowledgementGate.pause() }, midiInputIDs: [])
        acknowledgementHost.onEvent = { name, value in acknowledgementEvents.receive(name, value) }
        defer { acknowledgementGate.releaseAll(); acknowledgementHost.shutdown() }
        var acknowledgedProject = project
        acknowledgedProject["id"] = "await-plugins-first"
        acknowledgedProject["audioPlugins"] = [stateB]
        var firstAcknowledgement: Result<Any, Error>?
        let firstAcknowledgementTask = Task { @MainActor in
            do {
                let value = try await acknowledgementHost.handle("engine.project", payload: ["project": acknowledgedProject, "replace": true, "awaitPlugins": true])
                firstAcknowledgement = .success(value)
            } catch { firstAcknowledgement = .failure(error) }
        }
        await acknowledgementGate.waitForEntry(1)
        for _ in 0..<20 { await Task.yield() }
        try require(firstAcknowledgement == nil, "awaitPlugins acknowledged a project before its gated Audio Unit loaded.")
        acknowledgementGate.release(1)
        await firstAcknowledgementTask.value
        guard case .success? = firstAcknowledgement else {
            throw AudioSampleError.invalid("Plug-in state regression: The settled awaitPlugins request did not acknowledge successfully.")
        }
        guard let acknowledgedID = stateB["instanceID"] as? String, !acknowledgedID.isEmpty else {
            throw AudioSampleError.invalid("Plug-in state regression: The real AU fixture must have an exact instance identity.")
        }
        var acknowledgedTarget = target; acknowledgedTarget["instanceID"] = acknowledgedID
        var acknowledgedEdit = acknowledgedTarget
        acknowledgedEdit["address"] = String(kDelayParam_DelayTime)
        acknowledgedEdit["value"] = 0.61; acknowledgedEdit["normalized"] = false
        _ = try await acknowledgementHost.handle("plugin.parameter", payload: acknowledgedEdit)
        await acknowledgementHost.pluginRestorationBarrier()?.value
        let acknowledgedParameters = try await acknowledgementHost.handle("plugin.parameters", payload: acknowledgedTarget) as? [[String: Any]] ?? []
        let acknowledgedDelay = (acknowledgedParameters.first { $0["address"] as? String == String(kDelayParam_DelayTime) }?["value"] as? NSNumber)?.doubleValue ?? -1
        let acknowledgedSnapshot = try await acknowledgementHost.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
        let acknowledgedSaved = savedEffect(acknowledgedSnapshot["audioPlugins"] as? [[String: Any]] ?? [])
        try require(abs(acknowledgedDelay - 0.61) < 0.0001 && acknowledgedSaved?["instanceID"] as? String == acknowledgedID,
                    "The exact-instance control edit made after acknowledgement did not survive restoration.")
        try require(!equivalentState(acknowledgedSaved?["state"] as? String, stateB["state"] as? String),
                    "The snapshot after acknowledgement retained the pre-edit saved patch.")
        passed.append("Opt-in project acknowledgement waits for restoration and preserves an immediately following exact-instance parameter edit")
        _ = try await acknowledgementHost.handle("engine.project", payload: ["project": acknowledgedProject,
            "replace": false, "forcePluginState": true, "awaitPlugins": true])
        let forcedParameters = try await acknowledgementHost.handle("plugin.parameters", payload: acknowledgedTarget) as? [[String: Any]] ?? []
        let forcedDelay = (forcedParameters.first { $0["address"] as? String == String(kDelayParam_DelayTime) }?["value"] as? NSNumber)?.doubleValue ?? -1
        try require(abs(forcedDelay - 0.8) < 0.0001,
                    "replace:false suppressed an explicit forcePluginState:true restoration of the same descriptor.")
        passed.append("Explicit forced plug-in restoration remains effective when project replacement is false")

        // Hold two different loads independently. Releasing only the superseded
        // request must reject its acknowledgement without following the newer
        // restore (which deliberately remains blocked).
        var olderAcknowledgedProject = acknowledgedProject
        olderAcknowledgedProject["id"] = "await-plugins-superseded"
        var olderInstrument = instrument; olderInstrument["track"] = 1
        olderAcknowledgedProject["audioPlugins"] = [olderInstrument, stateA]
        var oldAcknowledgement: Result<Any, Error>?
        let oldAcknowledgementTask = Task { @MainActor in
            do {
                let value = try await acknowledgementHost.handle("engine.project", payload: ["project": olderAcknowledgedProject, "replace": true, "awaitPlugins": true])
                oldAcknowledgement = .success(value)
            } catch { oldAcknowledgement = .failure(error) }
        }
        await acknowledgementGate.waitForEntry(2)
        let oldAcknowledgementRestore = acknowledgementHost.pluginRestorationBarrier()
        var newerAcknowledgedProject = acknowledgedProject
        newerAcknowledgedProject["id"] = "await-plugins-newest"
        var newerInstrument = instrument; newerInstrument["track"] = 2
        newerAcknowledgedProject["audioPlugins"] = [newerInstrument, stateC]
        let newestAcknowledgementMark = acknowledgementEvents.count
        // No awaitPlugins flag: existing callers retain the immediate contract.
        _ = try await acknowledgementHost.handle("engine.project", payload: ["project": newerAcknowledgedProject, "replace": true])
        await acknowledgementGate.waitForEntry(3)
        acknowledgementGate.release(2)
        await oldAcknowledgementRestore?.value
        for _ in 0..<1000 {
            if oldAcknowledgement != nil { break }
            await Task.yield()
        }
        guard case .failure? = oldAcknowledgement else {
            throw AudioSampleError.invalid("Plug-in state regression: A superseded acknowledgement succeeded or waited for the newer blocked restore.")
        }
        try require(acknowledgementEvents.count == newestAcknowledgementMark,
                    "The superseded acknowledgement published completion for a still-blocked newer project.")
        acknowledgementGate.release(3)
        await oldAcknowledgementTask.value
        let newestAcknowledgedResult = await acknowledgementEvents.next(after: newestAcknowledgementMark)
        try require(newestAcknowledgedResult.name == "plugins.changed", "The newer default-immediate request failed to restore after its own gate released.")
        passed.append("Superseded opt-in acknowledgements reject after their own restoration settles without waiting for a newer blocked request")

        acknowledgementGate.enabled = false
        var missingAcknowledgedProject = acknowledgedProject
        missingAcknowledgedProject["id"] = "await-plugins-missing"
        let missingAcknowledgedState: [String: Any] = ["track": 0, "kind": "effect", "slot": 0,
            "component": "1635083896:1:1", "instanceID": "await-plugins-unavailable-fixture", "name": "Unavailable AU fixture",
            "state": Data("opaque unavailable document must survive acknowledgement".utf8).base64EncodedString()]
        missingAcknowledgedProject["audioPlugins"] = [missingAcknowledgedState]
        let missingAcknowledgementMark = acknowledgementEvents.count
        _ = try await acknowledgementHost.handle("engine.project", payload: ["project": missingAcknowledgedProject, "replace": true, "awaitPlugins": true])
        let missingAcknowledgedResult = await acknowledgementEvents.next(after: missingAcknowledgementMark)
        let missingAcknowledgedSnapshot = try await acknowledgementHost.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
        let missingAcknowledgedSaved = savedEffect(missingAcknowledgedSnapshot["audioPlugins"] as? [[String: Any]] ?? [])
        try require(missingAcknowledgedResult.name == "plugin.error"
                    && missingAcknowledgedSaved?["state"] as? String == missingAcknowledgedState["state"] as? String
                    && missingAcknowledgedSaved?["instanceID"] as? String == missingAcknowledgedState["instanceID"] as? String,
                    "A settled missing-plug-in acknowledgement discarded its recoverable opaque state or hid the restore error.")
        await acknowledgementHost.shutdownAndWait()
        passed.append("Opt-in acknowledgement settles missing plug-ins while retaining their exact opaque state and reporting the restoration error")

        // Termination may overtake a vendor's asynchronous instantiate callback.
        // Cancellation must prevent that callback from attaching to a closed graph.
        for direct in [true, false] {
            let shutdownGate = InstantiationGate(); shutdownGate.enabled = true
            let closingHost = try AudioHost(beforePluginInstantiation: { try await shutdownGate.pause() }, midiInputIDs: [])
            defer { shutdownGate.releaseAll(); closingHost.shutdown() }
            var directTask: Task<Any, Error>?
            if direct { directTask = Task { @MainActor in try await closingHost.handle("plugin.load", payload: load) } }
            else {
                var closingProject = project; closingProject["audioPlugins"] = [stateB]
                _ = try await closingHost.handle("engine.project", payload: ["project": closingProject, "replace": true])
            }
            await shutdownGate.waitForEntry(1)
            let restoreBarrier = closingHost.pluginRestorationBarrier()
            closingHost.shutdown()
            try require(closingHost.status()["closed"] as? Bool == true,
                        "Shutdown did not stop a graph with a pending instantiate callback.")
            var shutdownFinished = false
            let shutdownWait = Task { @MainActor in await closingHost.shutdownAndWait(); shutdownFinished = true }
            for _ in 0..<20 { await Task.yield() }
            try require(!shutdownFinished,"Shutdown allowed module cleanup before a canceled vendor load returned.")
            shutdownGate.release(1)
            await shutdownWait.value
            if let directTask {
                var rejected = false
                do { _ = try await directTask.value } catch { rejected = true }
                try require(rejected, "A direct plug-in load committed after shutdown.")
            } else { await restoreBarrier?.value }
            let lifetime = closingHost.status()["endpointLifetime"] as? [String: Any] ?? [:]
            try require(lifetime["activeInstances"] as? Int == 0 && lifetime["activeReaders"] as? Int == 0,
                        "A canceled asynchronous plug-in task attached to the closed graph.")
        }
        passed.append("Shutdown cancels pending direct and project plug-in instantiation before late graph attachment")

        // Even a regressed Play may only start this private manual-render graph,
        // never a physical output device. Each gate proves the request reached
        // native asset preparation before the interruption is issued.
        let playCancellationCases = ["stop", "panic", "seek", "replace", "unavailable", "shutdown", "cancel"]
        for interruption in playCancellationCases + ["ordinary-edit"] {
            let playGate = InstantiationGate(); playGate.enabled = true
            let playGraph = AVAudioEngine()
            let playFormat = AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
            try playGraph.enableManualRenderingMode(.offline,format:playFormat,maximumFrameCount:64)
            let playHost = try AudioHost(beforePluginInstantiation:{ try await playGate.pause() },audioGraph:playGraph,midiInputIDs:[])
            defer { playGate.releaseAll(); playHost.shutdown() }
            var playProject = project
            playProject["id"] = "pending-play-"+interruption
            playProject["audioPlugins"] = [stateB]
            _ = try await playHost.handle("engine.project",payload:["project":playProject,"replace":true])
            await playGate.waitForEntry(1)
            var playResult:Result<Any,Error>?
            let pendingPlay = Task { @MainActor in
                do { playResult = .success(try await playHost.handle("transport.play",payload:["mode":"song"])) }
                catch { playResult = .failure(error) }
            }
            for _ in 0..<1000 {
                if playHost.status()["playPreparing"] as? Bool == true { break }
                await Task.yield()
            }
            try require(playHost.status()["playPreparing"] as? Bool == true && playResult == nil,
                        "The \(interruption) fixture did not hold Play inside native asset preparation.")
            var unavailableTask:Task<Any,Error>?
            switch interruption {
            case "stop": _ = try await playHost.handle("transport.stop",payload:[:])
            case "panic": _ = try await playHost.handle("engine.panic",payload:[:])
            case "seek": _ = try await playHost.handle("transport.seek",payload:["pattern":0,"row":2,"sequence":0])
            case "replace":
                var replacement = playProject; replacement["id"] = "replacement-session"; replacement["audioPlugins"] = []
                _ = try await playHost.handle("engine.project",payload:["project":replacement,"replace":true])
            case "unavailable":
                var unavailable = playProject
                var tracks = unavailable["tracks"] as! [[String:Any]]
                for index in tracks.indices { tracks[index]["routingID"] = "pending-play-track-\(index)" }
                unavailable["tracks"] = tracks
                unavailable["routing"] = ["version":1,"mixModel":"standard",
                    "buses":[["id":"cycle","name":"Cycle","type":"group","volume":1,"pan":0]],
                    "outputs":["track:pending-play-track-0":"bus:cycle","bus:cycle":"bus:cycle"],"sends":[],"sidechains":[]] as [String:Any]
                unavailableTask = Task { @MainActor in try await playHost.handle("engine.project",payload:["project":unavailable,"replace":true]) }
                for _ in 0..<1000 {
                    if playHost.status()["routingFailure"] is String { break }
                    await Task.yield()
                }
                try require(playHost.status()["routingFailure"] is String,"The unavailable-routing fixture did not publish its session replacement.")
            case "shutdown": playHost.shutdown()
            case "cancel": pendingPlay.cancel()
            default:
                playProject["name"] = "Ordinary edit while Play prepares"
                _ = try await playHost.handle("engine.project",payload:["project":playProject])
            }
            playGate.release(1)
            await pendingPlay.value
            _ = try await unavailableTask?.value
            if interruption == "ordinary-edit" {
                guard case .success? = playResult else {
                    throw AudioSampleError.invalid("Plug-in state regression: An ordinary project edit canceled valid pending playback.")
                }
                let buffer = AVAudioPCMBuffer(pcmFormat:playFormat,frameCapacity:64)!
                try require(playGraph.isRunning && (try playGraph.renderOffline(64,to:buffer)) == .success
                            && playHost.status()["playing"] as? Bool == true,
                            "Valid pending playback did not start the private render graph after an ordinary edit.")
                playProject["name"] = "Ordinary edit during playback"
                _ = try await playHost.handle("engine.project",payload:["project":playProject])
                try require((try playGraph.renderOffline(64,to:buffer)) == .success
                            && playHost.status()["playing"] as? Bool == true,
                            "An ordinary project edit stopped established playback.")
            } else {
                guard case .failure? = playResult else {
                    throw AudioSampleError.invalid("Plug-in state regression: \(interruption) allowed a pending Play to restart after its load finished.")
                }
                try require(!playGraph.isRunning && playHost.status()["playing"] as? Bool != true,
                            "Canceled pending Play started the graph or transport after \(interruption).")
            }
            await playHost.shutdownAndWait()
        }
        passed.append("Stop, panic, seek, project replacement, unavailable routing, shutdown and task cancellation prevent a pending Play from restarting after asset loading")
        passed.append("Ordinary project edits preserve both valid pending Play requests and established playback in a private manual-render graph")

        // Global transport commands need the same native cancellation guarantee
        // as a negative preview. Space/Play-button cancellation sends Stop alone,
        // and must not depend on the UI also issuing a negative step.preview.
        let previewCancellationCases = ["negative", "stop", "panic", "seek", "cancel"]
        var previewEvidence:[[String:Any]] = []
        for interruption in previewCancellationCases + ["ordinary-edit"] {
            let previewGate = InstantiationGate(); previewGate.enabled = true
            let previewGraph = AVAudioEngine()
            let previewFormat = AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
            try previewGraph.enableManualRenderingMode(.offline,format:previewFormat,maximumFrameCount:512)
            let previewHost = try AudioHost(beforePluginInstantiation:{ try await previewGate.pause() },audioGraph:previewGraph,midiInputIDs:[])
            defer { previewGate.releaseAll(); previewHost.shutdown() }
            var previewProject = project
            previewProject["id"] = "pending-preview-"+interruption
            previewProject["audioPlugins"] = [load]
            _ = try await previewHost.handle("engine.project",payload:["project":previewProject,"replace":true])
            await previewGate.waitForEntry(1)
            var auditionResult:Result<Any,Error>?
            let audition = Task { @MainActor in
                do { auditionResult = .success(try await previewHost.handle("step.preview",payload:["track":0,"step":["note":60,"instrument":0]])) }
                catch { auditionResult = .failure(error) }
            }
            for _ in 0..<1000 {
                if previewHost.status()["previewPreparing"] as? Bool == true { break }
                await Task.yield()
            }
            try require(previewHost.status()["previewPreparing"] as? Bool == true && auditionResult == nil,
                        "The \(interruption) audition did not wait for pending project assets.")
            switch interruption {
            case "negative":
                let stopped = try await previewHost.handle("step.preview",payload:["track":0,"step":["note":-1]]) as? [String:Any]
                try require(stopped?["auditioning"] as? Bool == false,"Stopping audition waited for a pending asset load.")
            case "stop": _ = try await previewHost.handle("transport.stop",payload:[:])
            case "panic": _ = try await previewHost.handle("engine.panic",payload:[:])
            case "seek": _ = try await previewHost.handle("transport.seek",payload:["pattern":0,"row":2,"sequence":0])
            case "cancel": audition.cancel()
            default:
                previewProject["name"] = "Ordinary edit while audition prepares"
                _ = try await previewHost.handle("engine.project",payload:["project":previewProject])
            }
            previewGate.release(1)
            await audition.value
            if interruption == "ordinary-edit" {
                guard case .success? = auditionResult else {
                    throw AudioSampleError.invalid("Plug-in state regression: An ordinary edit canceled valid pending audition.")
                }
                let buffer = AVAudioPCMBuffer(pcmFormat:previewFormat,frameCapacity:512)!
                try require(previewGraph.isRunning && (try previewGraph.renderOffline(512,to:buffer)) == .success,
                            "Valid audition did not start its private render graph after an ordinary edit.")
                let peak = buffer.floatChannelData.map { data in
                    (0..<Int(buffer.frameLength)).reduce(Float(0)) { max($0,max(abs(data[0][$1]),abs(data[1][$1]))) }
                } ?? 0
                try require(peak > 0.000001 && previewHost.status()["playing"] as? Bool == false,
                            "Valid audition was silent or started the song transport after an ordinary edit.")
                previewEvidence.append(["case":interruption,"auditionAccepted":true,"peak":peak,"transportPlaying":false])
            } else {
                guard case .failure? = auditionResult else {
                    throw AudioSampleError.invalid("Plug-in state regression: \(interruption) allowed a pending audition to restart after loading.")
                }
                try require(!previewGraph.isRunning && previewHost.status()["playing"] as? Bool == false,
                            "A pending audition restarted the graph or song after \(interruption).")
                previewEvidence.append(["case":interruption,"auditionRejected":true,"graphRunning":false,"transportPlaying":false])
            }
            await previewHost.shutdownAndWait()
        }
        passed.append("Step audition stop immediately invalidates pending preparation without starting playback or moving transport")
        passed.append("Global Stop, panic, seek and task cancellation reject a pending step audition before it can restart the audio graph")
        passed.append("An ordinary project edit preserves a pending step audition, producing audible manual-render PCM without moving song transport")

        let metrics: [String: Any] = ["initialDelaySeconds": 0.2, "firstRequestedDelaySeconds": 0.8,
            "latestRequestedDelaySeconds": 0.45, "gatedInstantiationCount": gate.entries,
            "checks": passed, "playCancellationCases":playCancellationCases,
            "previewCancellationCases":previewCancellationCases,"previewEvidence":previewEvidence,
            "physicalPlayback": false, "scheduling": "continuation gates without sleeps"]
        try JSONSerialization.data(withJSONObject: metrics, options: [.prettyPrinted, .sortedKeys])
            .write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
        return passed
    }

    private static func equivalentState(_ left: String?, _ right: String?) -> Bool {
        guard let left, let right, let lhs = Data(base64Encoded: left), let rhs = Data(base64Encoded: right),
              let a = try? PropertyListSerialization.propertyList(from: lhs, format: nil) as? NSDictionary,
              let b = try? PropertyListSerialization.propertyList(from: rhs, format: nil) as? NSDictionary else { return false }
        return a.isEqual(b)
    }

    @MainActor private final class InstantiationGate {
        var enabled = false
        private(set) var entries = 0
        private var blocked: [Int: CheckedContinuation<Void, Error>] = [:]
        private var arrivals: [Int: [CheckedContinuation<Void, Never>]] = [:]
        func pause() async throws {
            guard enabled else { return }
            entries += 1; let entry = entries
            try await withCheckedThrowingContinuation { continuation in
                blocked[entry] = continuation
                for waiter in arrivals.removeValue(forKey: entry) ?? [] { waiter.resume() }
            }
        }
        func waitForEntry(_ entry: Int) async {
            if entries >= entry { return }
            await withCheckedContinuation { arrivals[entry, default: []].append($0) }
        }
        func release(_ entry: Int) { blocked.removeValue(forKey: entry)?.resume() }
        func releaseAll() {
            let waiting = blocked.values; blocked.removeAll()
            for continuation in waiting { continuation.resume(throwing: CancellationError()) }
        }
    }

    @MainActor private final class RestoreEvents {
        struct Event { var name: String; var value: [String: Any] }
        private var values: [Event] = []
        private var waiters: [(Int, CheckedContinuation<Event, Never>)] = []
        var count: Int { values.count }
        func receive(_ name: String, _ value: [String: Any]) {
            guard name == "plugins.changed" || name == "plugin.error" else { return }
            values.append(Event(name: name, value: value))
            let ready = waiters.filter { $0.0 < values.count }
            waiters.removeAll { $0.0 < values.count }
            for (index, continuation) in ready { continuation.resume(returning: values[index]) }
        }
        func next(after index: Int) async -> Event {
            if values.count > index { return values[index] }
            return await withCheckedContinuation { waiters.append((index, $0)) }
        }
    }
}

#endif

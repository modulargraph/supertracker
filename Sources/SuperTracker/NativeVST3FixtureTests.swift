// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation

/// Production graph/state tests using only the original component compiled by
/// tests/vst3_fixture_tests.sh into test-output/Fixture.vst3. No installed vendor
/// discovery, realtime playback, or device recording is used by this suite.
enum NativeVST3FixtureTests {
    @MainActor static func run(output: URL, fixtureBundle: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("vst3-fixture", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let reportURL = folder.appendingPathComponent("metrics.json")
        let bundle = fixtureBundle.standardizedFileURL
        var catalogueConfiguration = VST3Catalogue.Configuration()
        catalogueConfiguration.roots = [bundle.deletingLastPathComponent()]
        catalogueConfiguration.cacheURL = folder.appendingPathComponent("private-catalogue.json")
        let catalogue = VST3Catalogue(configuration: catalogueConfiguration)
        let classID = "0123456789ABCDEF0123456789ABCDEF"
        let identifier = "vst3:" + classID
        let target: [String: Any] = ["track": 0, "kind": "instrument", "slot": 0]
        let epsilon: Float = 0.000001
        let sampleRate = 48_000.0, noteEnd = 18_000, totalFrames = 24_000
        var checks: [String] = [], failures: [String] = [], renders: [[String: Any]] = []
        var measurements: [String: Any] = ["fixtureBundle": bundle.path, "classID": classID,
            "parameterID": "42", "sampleRate": sampleRate, "noteEndFrame": noteEnd,
            "totalFrames": totalFrames, "installedVendorsUsed": false,
            "privateCatalogue": catalogueConfiguration.cacheURL.path]
        var activeHost: AudioHost?
        var pluginErrors: [[String: Any]] = []

        func write(_ status: String) throws {
            let report: [String: Any] = ["passed": false,
                "audioChecksPassed": status == "audioChecksComplete" && failures.isEmpty,
                "processExitPassed": false, "status": status, "scope": "original-vst3-fixture",
                "nksStatus": "notRun", "checks": checks, "failures": failures,
                "measurements": measurements, "renders": renders]
            try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
                .write(to: reportURL, options: .atomic)
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw AudioSampleError.invalid("Original VST3 fixture: " + message) }
        }
        func trace(_ message: String) { fputs("VST3 fixture: \(message)\n", stderr) }
        func completed(_ message: String) throws {
            checks.append(message); trace("passed " + message); try write("running")
        }
        func json(_ value: Any, to name: String) throws {
            try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
                .write(to: folder.appendingPathComponent(name + ".json"), options: .atomic)
        }
        func projectEqual(_ first: Any, _ second: Any) -> Bool {
            (try? JSONSerialization.data(withJSONObject: first, options: [.sortedKeys])) ==
                (try? JSONSerialization.data(withJSONObject: second, options: [.sortedKeys]))
        }
        func host() throws -> AudioHost {
            guard let activeHost else { throw AudioSampleError.invalid("The fixture audio host is not available.") }
            return activeHost
        }
        func stopped() throws {
            let status = try host().status()
            // The tracker publishes transport status from rendering. After an
            // offline pass, queued project/stop commands can leave that cached
            // flag stale until the next callback; graph.isRunning proves no DSP
            // callback can consume the edit before this test explicitly renders.
            try require(status["running"] as? Bool == false,
                        "This test requires the audio graph to be stopped before an edit (status \(status)).")
        }
        func apply(_ project: [String: Any]) async throws {
            let current = try host(), errorMark = pluginErrors.count
            _ = try await current.handle("engine.project", payload: ["project": project, "replace": true])
            await current.pluginRestorationBarrier()?.value
            try require(pluginErrors.count == errorMark, "Project restoration failed: \(pluginErrors.dropFirst(errorMark)).")
            try stopped()
        }
        func freshHost(project: [String: Any]) async throws {
            if let old = activeHost { await old.shutdownAndWait(); activeHost = nil }
            let current = try AudioHost(vst3Catalogue: catalogue, midiInputIDs: [])
            current.onEvent = { name, value in if name == "plugin.error" { pluginErrors.append(value) } }
            activeHost = current
            let midiIsolation = try await current.handle("midi.devices", payload: [:]) as? [String: Any] ?? [:]
            try require(midiIsolation["allInputs"] as? Bool == false &&
                        (midiIsolation["inputIDs"] as? [Int])?.isEmpty == true,
                        "The fixture host must not subscribe to external MIDI inputs.")
            measurements["midiInputsDisabled"] = true
            try await apply(project)
        }
        func setGain(_ value: Double) async throws {
            try stopped()
            var payload = target
            payload["address"] = "42"; payload["value"] = value; payload["normalized"] = true
            _ = try await host().handle("plugin.parameter", payload: payload)
        }
        func parameterGain() async throws -> Double {
            let parameters = try await host().handle("plugin.parameters", payload: target) as? [[String: Any]] ?? []
            guard let gain = parameters.first(where: { $0["address"] as? String == "42" }),
                  let normalized = gain["normalized"] as? NSNumber else {
                throw AudioSampleError.invalid("The original fixture's Gain ParamID42 is missing.")
            }
            return normalized.doubleValue
        }
        func delivery(_ phase: String) async throws -> [String: Any] {
            var payload = target
            payload["addresses"] = ["42"]; payload["diagnostics"] = true
            let rows = try await host().handle("plugin.parameters", payload: payload) as? [[String: Any]] ?? []
            let diagnostic = rows.first?["diagnostic"] as? [String: Any] ?? [:]
            if ProcessInfo.processInfo.environment["SUPERTRACKER_VST3_WATCH_PARAMETER"] == "42" {
                try require(rows.count == 1 && (diagnostic["parameterID"] as? NSNumber)?.intValue == 42,
                            "The passive adapter diagnostic did not identify watched ParamID42.")
            }
            var phases = measurements["parameterDelivery"] as? [String: Any] ?? [:]
            phases[phase] = rows; measurements["parameterDelivery"] = phases
            try json(rows, to: "delivery-" + phase)
            return diagnostic
        }
        func counter(_ value: [String: Any], _ key: String) -> UInt64 {
            (value[key] as? NSNumber)?.uint64Value ?? 0
        }
        func delivered(_ value: [String: Any], gain: Double, after before: [String: Any]) throws {
            guard !value.isEmpty else { return }
            try require(counter(value, "uiDequeued") > counter(before, "uiDequeued") &&
                        counter(value, "offeredPoints") > counter(before, "offeredPoints") &&
                        counter(value, "readPoints") > counter(before, "readPoints"),
                        "The edit did not cross enqueue/dequeue, final offered queue, and plugin getPoint boundaries.")
            for key in ["lastUIValue", "lastDequeuedValue", "lastOfferedValue", "lastReadValue"] {
                try require(abs(((value[key] as? NSNumber)?.doubleValue ?? -1) - gain) <= Double(epsilon),
                            "The processor-boundary diagnostic \(key) did not preserve gain \(gain).")
            }
            try require(counter(value, "pendingUIEdits") == 0 && counter(value, "midiReceived") == 0,
                        "The fixture retained pending edits or accepted a nonexistent MIDI parameter assignment.")
        }
        func liveState() async throws -> [String: Any] {
            let value = try await host().handle("plugin.state", payload: target) as? [String: Any] ?? [:]
            try require(value["component"] as? String == identifier && value["bundlePath"] as? String == bundle.path,
                        "A state read lost the fixture's exact class or bundle identity.")
            try require(!(value["state"] as? String ?? "").isEmpty, "A state read did not return opaque component/controller bytes.")
            return value
        }
        func snapshot() async throws -> [String: Any] {
            let value = try await host().handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            let plugins = value["audioPlugins"] as? [[String: Any]] ?? []
            try require(plugins.count == 1 && plugins[0]["component"] as? String == identifier &&
                        plugins[0]["bundlePath"] as? String == bundle.path && !(plugins[0]["state"] as? String ?? "").isEmpty,
                        "A project snapshot lost the fixture's location or opaque state.")
            return value
        }
        func saveAndRead(_ project: [String: Any], name: String) throws -> [String: Any] {
            let store = try ProjectStore(directory: folder.appendingPathComponent("Support"))
            let destination = folder.appendingPathComponent(name + ".supertracker")
            _ = try store.save(to: destination, snapshot: project, collectSamples: false)
            let reopened = try store.read(destination)
            try require(projectEqual(project["audioPlugins"] ?? [], reopened["audioPlugins"] ?? []),
                        "Saving/reopening \(name) changed the fixture's opaque state or location.")
            return reopened
        }
        @discardableResult func render(_ name: String, gain: Float) async throws -> DecodedSample {
            trace("render " + name)
            try stopped()
            let file = folder.appendingPathComponent(name + ".wav")
            // There is deliberately no state/snapshot call here. In particular the
            // first export after a stopped edit must deliver that edit on its own.
            let result = try await host().handle("audio.export", payload: ["path": file.path, "mode": "song", "tail": 0]) as? [String: Any] ?? [:]
            let sample = try AudioSamples.decode(file.path)
            try require(sample.sampleRate == sampleRate && sample.left.count == totalFrames && sample.right.count == totalFrames,
                        "\(name) has the wrong sample rate or frame count.")
            try require(sample.left.allSatisfy(\.isFinite) && sample.right.allSatisfy(\.isFinite), "\(name) contains non-finite PCM.")
            var leftError: Float = 0, rightError: Float = 0, silentPeak: Float = 0
            var firstLeft = -1, firstRight = -1
            for index in 0..<totalFrames {
                if firstLeft < 0 && abs(sample.left[index]) > epsilon { firstLeft = index }
                if firstRight < 0 && abs(sample.right[index]) > epsilon { firstRight = index }
                if index < noteEnd {
                    leftError = max(leftError, abs(sample.left[index] - gain))
                    rightError = max(rightError, abs(sample.right[index] - gain))
                } else {
                    silentPeak = max(silentPeak, max(abs(sample.left[index]), abs(sample.right[index])))
                }
            }
            let actualParameter = try await parameterGain()
            let compensation = result["latencyCompensation"] as? [String: Any] ?? [:]
            renders.append(["name": name, "expectedGain": gain, "parameterGain": actualParameter,
                "firstLeftFrame": firstLeft, "firstRightFrame": firstRight,
                "leftMaximumError": leftError, "rightMaximumError": rightError, "silentTailPeak": silentPeak,
                "frames": sample.left.count, "latencyCompensation": compensation])
            try write("running")
            try require(firstLeft == 0 && firstRight == 0, "\(name) note onset was not at frame zero: \(firstLeft)/\(firstRight).")
            try require(leftError <= epsilon && rightError <= epsilon,
                        "\(name) failed exact DSP gain \(gain): max channel errors \(leftError)/\(rightError), controller \(actualParameter).")
            try require(silentPeak <= epsilon, "\(name) did not end its note at frame \(noteEnd).")
            try require(abs(actualParameter - Double(gain)) <= Double(epsilon), "\(name) controller readback disagrees with its DSP gain.")
            try require((compensation["frames"] as? Int) == 0, "The zero-latency fixture unexpectedly required compensation.")
            try stopped()
            return sample
        }
        func compare(_ left: DecodedSample, _ right: DecodedSample, name: String) throws {
            try require(left.left.count == right.left.count && left.right.count == right.right.count, "\(name) changed render length.")
            var maximum: Float = 0
            for index in left.left.indices {
                maximum = max(maximum, max(abs(left.left[index] - right.left[index]), abs(left.right[index] - right.right[index])))
            }
            measurements[name] = ["maximumPCMDifference": maximum]
            try require(maximum <= epsilon, "\(name) changed deterministic PCM by \(maximum).")
        }

        try write("running")
        do {
            // Only this exact original bundle is scanned; no plug-in list/full
            // refresh path is called anywhere in the fixture test.
            let found = try await catalogue.scanBundle(bundle, force: true)
            guard let descriptor = found.first(where: { $0.classID == classID }) else {
                throw AudioSampleError.invalid("The requested bundle is not the original SuperTracker VST3 fixture.")
            }
            try require(descriptor.isInstrument && descriptor.manufacturer == "SuperTracker Tests" && descriptor.bundlePath == bundle.path,
                        "The fixture factory reported an unexpected type, manufacturer, or path.")
            measurements["descriptor"] = descriptor.dictionary
            var rows = [Any](repeating: NSNull(), count: 4)
            rows[0] = ["note": 60, "instrument": 0, "velocity": 127, "gate": 3]
            let base: [String: Any] = ["version": 1, "name": "Original VST3 fixture verification", "bpm": 120.0,
                "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0, "loopSong": false, "metronome": false,
                "tracks": [["name": "Constant fixture", "instrument": 0, "volume": 1, "pan": 0, "delaySend": 0, "reverbSend": 0]],
                "instruments": [["type": "au", "volume": 1, "pan": 0]],
                "patterns": [["id": 0, "length": 4, "steps": [rows], "automation": []]],
                "sequence": [0], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]

            try await freshHost(project: base)
            var load = target; load["component"] = identifier; load["bundlePath"] = bundle.path
            let loaded = try await host().handle("plugin.load", payload: load) as? [String: Any] ?? [:]
            try require(loaded["component"] as? String == identifier && loaded["bundlePath"] as? String == bundle.path,
                        "Rack load substituted a different fixture class or location.")
            try require(abs(try await parameterGain() - 0.25) <= Double(epsilon), "The fixture did not initialize to Gain 0.25.")
            _ = try await render("baseline-025", gain: 0.25)
            try completed("Original VST3 factory, adapter and rack render exact Gain0.25 with sample-accurate MIDI onset/release")

            let beforeEdit = try await delivery("before-stopped-edit")
            try await setGain(0.625)
            let queuedEdit = try await delivery("after-stopped-edit-before-export")
            if !queuedEdit.isEmpty {
                try require(counter(queuedEdit, "uiQueued") > counter(beforeEdit, "uiQueued") &&
                            counter(queuedEdit, "uiDequeued") == counter(beforeEdit, "uiDequeued"),
                            "A passive diagnostic unexpectedly consumed the stopped edit.")
            }
            // Do not insert plugin.state, engine.snapshot, save, or another audio
            // operation between this edit and the export: that is the regression.
            let edited = try await render("stopped-edit-export-0625", gain: 0.625)
            let exportDelivery = try await delivery("after-stopped-edit-export")
            try delivered(exportDelivery, gain: 0.625, after: beforeEdit)
            if !exportDelivery.isEmpty {
                try require(((exportDelivery["lastReadFrames"] as? NSNumber)?.intValue ?? 0) > 1,
                            "Reset consumed the pending edit before the first ordinary audio block.")
            }
            let editedSnapshot = try await snapshot()
            try json(editedSnapshot, to: "snapshot-after-export-0625")
            let reopened = try saveAndRead(editedSnapshot, name: "Export first edited state")
            try await freshHost(project: reopened)
            let restored = try await render("fresh-project-0625", gain: 0.625)
            try compare(edited, restored, name: "exportFirstVersusFreshProject")
            try completed("Stopped ParamID42 edit reaches export without state capture and survives project save/fresh-host restore")

            let repeatOne = try await render("repeat-export-0625-a", gain: 0.625)
            let repeatTwo = try await render("repeat-export-0625-b", gain: 0.625)
            try compare(restored, repeatOne, name: "firstRepeatedExport")
            try compare(repeatOne, repeatTwo, name: "secondRepeatedExport")
            try completed("Repeated offline exports preserve the fixture's edited DSP gain and identical PCM")

            let beforeImmediateEdit = try await delivery("before-immediate-state-edit")
            try await setGain(0.375)
            let immediateState = try await liveState()
            try delivered(try await delivery("after-immediate-state"), gain: 0.375, after: beforeImmediateEdit)
            try json(immediateState, to: "immediate-state-0375")
            var immediateProject = base; immediateProject["audioPlugins"] = [immediateState]
            let immediateReopened = try saveAndRead(immediateProject, name: "Immediate state before audio")
            // Change the old live instance again to make accidental reuse obvious.
            // Neither edit is rendered before the fresh instance is restored.
            try await setGain(0.875)
            try await freshHost(project: immediateReopened)
            _ = try await render("fresh-immediate-state-0375", gain: 0.375)
            _ = try await render("repeat-immediate-state-0375", gain: 0.375)
            try completed("Immediate opaque state capture before audio preserves pending edits in a fresh instance's actual PCM")

            try await setGain(0.5)
            let immediateSnapshot = try await snapshot()
            try json(immediateSnapshot, to: "immediate-project-snapshot-05")
            let snapshotReopened = try saveAndRead(immediateSnapshot, name: "Immediate project snapshot before audio")
            try await freshHost(project: snapshotReopened)
            _ = try await render("fresh-immediate-project-05", gain: 0.5)
            try completed("Immediate engine snapshot/autosave path preserves a stopped edit without a prior render")

            measurements["stateCaptureOrder"] = ["exportFirstGain": 0.625, "opaqueStateBeforeRenderGain": 0.375,
                "snapshotBeforeRenderGain": 0.5, "discardedOldInstanceGain": 0.875]
            if let current = activeHost { await current.shutdownAndWait(); activeHost = nil }
            try write("audioChecksComplete")
            return checks
        } catch {
            failures.append(error.localizedDescription)
            if let current = activeHost { await current.shutdownAndWait(); activeHost = nil }
            try? write("failed")
            throw error
        }
    }
}

#endif

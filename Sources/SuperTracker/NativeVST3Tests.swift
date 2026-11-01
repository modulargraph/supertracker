// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AudioToolbox

/// Installed-vendor integration suite, deliberately separate from the portable
/// regression suite. Every render uses the app's offline graph, never the speakers.
enum NativeVST3Tests {
    @MainActor static func run(output: URL, includeNKS: Bool = false) async throws -> [String] {
        let folder = output.appendingPathComponent("vst3", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let metricsURL = folder.appendingPathComponent("metrics.json")
        let catalogueFolder = folder.appendingPathComponent("catalogues-" + UUID().uuidString,isDirectory:true)
        try FileManager.default.createDirectory(at:catalogueFolder,withIntermediateDirectories:true)
        var vst3Configuration = VST3Catalogue.Configuration()
        vst3Configuration.roots=[];vst3Configuration.cacheURL=catalogueFolder.appendingPathComponent("vst3.json")
        let catalogue = VST3Catalogue(configuration:vst3Configuration)
        var vst2Configuration = VST2Catalogue.Configuration()
        vst2Configuration.roots=[];vst2Configuration.cacheURL=catalogueFolder.appendingPathComponent("vst2.json")
        let vst2Catalogue = VST2Catalogue(configuration:vst2Configuration)
        var checks: [String] = [], failures: [String] = [], measurements: [String: Any] = [:]
        measurements["catalogueIsolation"] = ["directory":catalogueFolder.path,"vst3Roots":[],"vst2Roots":[],
            "discovery":"explicit installed bundle scanBundle only; no user catalogue/cache or broad refresh"]
        var nksStatus = includeNKS ? "pending" : "notRun"
        func write(_ status: String) throws {
            let report: [String: Any] = ["passed": false, "audioChecksPassed": status == "audioChecksComplete" && failures.isEmpty,
                "processExitPassed": false, "status": status, "nksStatus": nksStatus, "scope": includeNKS ? "installed-vendors-with-massive-x" : "installed-vendors-without-massive-x",
                "skipped": includeNKS ? [] : ["Massive X NKS integration requires an explicit opt-in after teardown stability has been verified."],
                "checks": checks, "failures": failures, "measurements": measurements]
            try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
                .write(to: metricsURL, options: .atomic)
        }
        try write("running")
        func trace(_ message: String) { fputs("VST3 test: \(message)\n", stderr) }
        var activeHost: AudioHost?
        do {
            let host = try AudioHost(vst3Catalogue:catalogue,midiInputIDs:[],vst2Catalogue:vst2Catalogue)
            activeHost = host
            var pluginErrors: [[String: Any]] = []
            host.onEvent = { name, value in if name == "plugin.error" { pluginErrors.append(value) } }
            func require(_ value: Bool, _ message: String) throws {
                if !value { throw AudioSampleError.invalid(message) }
            }
            let midiIsolation = try await host.handle("midi.devices", payload: [:]) as? [String: Any] ?? [:]
            try require(midiIsolation["allInputs"] as? Bool == false &&
                        (midiIsolation["inputIDs"] as? [Int])?.isEmpty == true,
                        "The native plug-in diagnostic must not subscribe to external MIDI inputs.")
            measurements["midiInputsDisabled"] = true
            func stage(_ name: String, _ operation: () async throws -> Void) async {
                trace("begin stage " + name)
                do { try await operation(); checks.append(name); trace("passed stage " + name) }
                catch { failures.append(name + ": " + error.localizedDescription); trace("failed stage " + name + ": " + error.localizedDescription) }
                try? write("running")
            }
            func json(_ value: Any, _ name: String) throws {
                try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
                    .write(to: folder.appendingPathComponent(name + ".json"), options: .atomic)
            }
            func samePluginState(_ a: String?, _ b: String?) -> Bool {
                guard let a, let b, let first = Data(base64Encoded: a), let second = Data(base64Encoded: b),
                    let left = try? PropertyListSerialization.propertyList(from: first, options: [], format: nil) as? NSDictionary,
                    let right = try? PropertyListSerialization.propertyList(from: second, options: [], format: nil) as? NSDictionary else { return false }
                // Binary property-list dictionary order is not a state change. This
                // still compares every key and every byte of the opaque VST3 payload.
                return left.isEqual(to: right as! [AnyHashable: Any])
            }
            func equivalent(_ a: Any, _ b: Any) -> Bool {
                (try? JSONSerialization.data(withJSONObject: a, options: [.sortedKeys])) ==
                (try? JSONSerialization.data(withJSONObject: b, options: [.sortedKeys]))
            }
            func apply(_ project: [String: Any]) async throws {
                trace("apply project")
                let mark = pluginErrors.count
                _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
                trace("await project plug-in restoration")
                await host.pluginRestorationBarrier()?.value
                trace("project ready")
                try require(pluginErrors.count == mark, "Project plug-in restoration failed: \(pluginErrors.dropFirst(mark))")
            }
            func snapshot() async throws -> [String: Any] {
                try await host.handle("engine.snapshot", payload: [:]) as? [String: Any] ?? [:]
            }
            func target(_ kind: String = "instrument", slot: Int = 0) -> [String: Any] {
                ["track": 0, "kind": kind, "slot": slot]
            }
            func parameters(_ kind: String = "instrument", slot: Int = 0) async throws -> [[String: Any]] {
                try await host.handle("plugin.parameters", payload: target(kind, slot: slot)) as? [[String: Any]] ?? []
            }
            func state(_ kind: String = "instrument", slot: Int = 0) async throws -> [String: Any] {
                try await host.handle("plugin.state", payload: target(kind, slot: slot)) as? [String: Any] ?? [:]
            }
            func parameterValue(_ id: String, kind: String = "instrument", slot: Int = 0) async throws -> Double {
                let all = try await parameters(kind, slot: slot)
                return (all.first { $0["address"] as? String == id }?["normalized"] as? NSNumber)?.doubleValue ?? -1
            }
            @discardableResult func recordDelivery(_ phase: String, id: String) async throws -> [String: Any] {
                var payload = target()
                payload["addresses"] = [id]; payload["diagnostics"] = true
                let rows = try await host.handle("plugin.parameters", payload: payload) as? [[String: Any]] ?? []
                if ProcessInfo.processInfo.environment["SUPERTRACKER_VST3_WATCH_PARAMETER"] == id {
                    let diagnostic = rows.first?["diagnostic"] as? [String: Any]
                    try require(rows.count == 1 && (diagnostic?["parameterID"] as? NSNumber)?.stringValue == id,
                                "The native diagnostic did not expose the watched NKS parameter at phase \(phase).")
                }
                var phases = measurements["nksParameterDelivery"] as? [String: Any] ?? [:]
                phases[phase] = rows
                measurements["nksParameterDelivery"] = phases
                try json(rows, "massive-delivery-" + phase)
                try write("running")
                return rows.first?["diagnostic"] as? [String: Any] ?? [:]
            }
            func set(_ id: String, value: Double, kind: String = "instrument", slot: Int = 0) async throws {
                var payload = target(kind, slot: slot)
                payload["address"] = id; payload["value"] = value; payload["normalized"] = true
                _ = try await host.handle("plugin.parameter", payload: payload)
            }
            func load(_ descriptor: VST3PluginDescriptor, kind: String = "instrument", slot: Int = 0,
                      saved: [String: Any]? = nil) async throws -> [String: Any] {
                var payload = saved ?? target(kind, slot: slot)
                payload["track"] = 0; payload["kind"] = kind; payload["slot"] = slot; payload["component"] = descriptor.id
                trace("load " + descriptor.name)
                let result = try await host.handle("plugin.load", payload: payload) as? [String: Any] ?? [:]
                trace("loaded " + descriptor.name)
                return result
            }
            func render(_ name: String) async throws -> DecodedSample {
                trace("render " + name)
                let url = folder.appendingPathComponent(name + ".wav")
                _ = try await host.handle("audio.export", payload: ["path": url.path, "mode": "song", "tail": 0.25])
                let decoded = try AudioSamples.decode(url.path)
                try require(decoded.left.allSatisfy(\.isFinite) && decoded.right.allSatisfy(\.isFinite), "\(name) rendered non-finite PCM.")
                try require(abs(decoded.duration - 2.25) < 0.003, "\(name) did not export two musical seconds plus its tail.")
                measurements[name] = ["rms": energy(decoded), "peak": peak(decoded), "frames": decoded.left.count]
                return decoded
            }
            func installed(_ file: String, instrument: Bool) async throws -> VST3PluginDescriptor {
                let path = URL(fileURLWithPath: "/Library/Audio/Plug-Ins/VST3").appendingPathComponent(file)
                let scanned = try await catalogue.scanBundle(path)
                guard let descriptor = scanned.first(where: { $0.isInstrument == instrument }) else {
                    throw AudioSampleError.invalid("The installed \(file) factory exposes no matching audio class.")
                }
                try require(descriptor.id == "vst3:" + descriptor.classID && descriptor.classID.count == 32,
                            "Discovery did not return the factory's canonical class ID.")
                measurements[file] = descriptor.dictionary
                return descriptor
            }

            var rows = [Any](repeating: NSNull(), count: 16)
            rows[0] = ["note": 60, "instrument": 0, "velocity": 105, "gate": 7.8]
            rows[8] = ["note": 67, "instrument": 0, "velocity": 105, "gate": 7.5]
            let base: [String: Any] = ["version": 1, "name": "VST3 native verification", "bpm": 120.0,
                "linesPerBeat": 4, "masterVolume": 0.65, "activePattern": 0, "loopSong": false,
                "tracks": [["name": "Instrument", "instrument": 0, "volume": 0.7, "pan": 0, "delaySend": 0, "reverbSend": 0]],
                // Engine type2 is the shared hosted-instrument event path for AU/VST3.
                "instruments": [["type": "au", "volume": 0.7, "pan": 0]],
                "patterns": [["id": 0, "length": 16, "steps": [rows], "automation": []]], "sequence": [0],
                "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
            var jun: VST3PluginDescriptor?, chorus: VST3PluginDescriptor?, filter: VST3PluginDescriptor?

            await stage("Isolated installed VST3 factory discovery returns instrument and effect class identities") {
                jun = try await installed("Jun-6 V.vst3", instrument: true)
                chorus = try await installed("Chorus JUN-6.vst3", instrument: false)
                filter = try await installed("Filter MINI.vst3", instrument: false)
                let listed = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
                for descriptor in [jun, chorus, filter].compactMap({ $0 }) {
                    try require(listed.contains { $0["id"] as? String == descriptor.id && $0["format"] as? String == "VST3" },
                                "The combined AU/VST3 browser list omitted \(descriptor.name).")
                }
                try json(listed.filter { $0["format"] as? String == "VST3" }, "discovered-classes")
            }

            await stage("VST3 instrument MIDI, normalized parameter edits and fresh-instance opaque-state restoration") {
                guard let jun else { throw AudioSampleError.invalid("Jun-6 V discovery did not complete.") }
                try await apply(base)
                let loaded = try await load(jun)
                try require(loaded["format"] as? String == "VST3" && !(loaded["state"] as? String ?? "").isEmpty,
                            "VST3 loading did not produce a serializable rack descriptor.")
                let all = try await parameters(); try json(all, "jun-parameters")
                try require(all.contains { $0["address"] as? String == "0" && ($0["name"] as? String ?? "").lowercased().contains("volume") },
                            "The installed Jun-6 V no longer exposes its known Volume ParamID0 fixture.")
                let initial = try await render("jun-default")
                try require(energy(initial) > 0.0001, "Sequenced VST3 MIDI produced silence.")
                // Save immediately after a GUI edit, before any render can consume
                // its queued processor change. A correct controller readback alone
                // does not prove that the opaque component state contains the edit.
                try await set("0", value: 0.23)
                let saved = try await state()
                let quiet = try await render("jun-volume-023")
                try require(abs(try await parameterValue("0") - 0.23) < 0.002, "Normalized VST3 parameter edit did not reach the controller.")
                try await set("0", value: 0.72)
                let changed = try await state()
                let loud = try await render("jun-volume-072")
                try require(energy(loud) > energy(quiet) * 1.3, "The VST3 Volume parameter did not audibly change output.")
                try require(saved["state"] as? String != changed["state"] as? String, "Distinct parameter edits produced identical opaque state.")
                _ = try await host.handle("plugin.remove", payload: target())
                _ = try await load(jun, saved: saved)
                let restored = try await render("jun-fresh-restored")
                let restoredValue = try await parameterValue("0")
                measurements["freshRestoredVolume"] = restoredValue
                try require(abs(restoredValue - 0.23) < 0.002 && energy(restored) > 0.00001,
                            "Fresh VST3 instance did not restore its saved parameter and playable sound; value \(restoredValue), RMS \(energy(restored)).")
                let quietRatio = energy(restored) / max(1e-12, energy(quiet))
                try require(abs(quietRatio - 1) < 0.2 && energy(restored) < energy(loud) / 1.3,
                            "Immediately saved quiet component state restored the wrong DSP gain; restored/reference RMS ratio \(quietRatio).")
                // The second state has a large gain change from the previous DSP
                // value, making stale component state visible despite oscillator
                // phase variation and the nearby factory-default volume of 0.243.
                _ = try await host.handle("plugin.remove", payload: target())
                _ = try await load(jun, saved: changed)
                let restoredLoud = try await render("jun-fresh-restored-loud")
                let loudRatio = energy(restoredLoud) / max(1e-12, energy(loud))
                let loudValue = try await parameterValue("0")
                try require(abs(loudValue - 0.72) < 0.002 && abs(loudRatio - 1) < 0.2 && energy(restoredLoud) > energy(quiet) * 1.3,
                            "Immediately saved loud component state restored the wrong DSP gain; restored/reference RMS ratio \(loudRatio).")
                measurements["junStateBytes"] = Data(base64Encoded: saved["state"] as? String ?? "")?.count ?? 0
                measurements["junVolumeRatio"] = energy(loud) / max(1e-12, energy(quiet))
                measurements["junImmediateSaveRestore"] = ["quietRMSRatio": quietRatio, "loudRMSRatio": loudRatio, "capturedBeforeRender": true]
                try json(saved, "jun-saved-rack-state")
                // Export stops/deallocates the graph between passes. A new static
                // control edit must survive the next graph reset without needing
                // an intervening state capture to flush its processor queue.
                try await set("0", value: 0)
                let stoppedEdit = try await render("jun-stopped-edit-next-export")
                measurements["stoppedEditRMSRatio"] = energy(stoppedEdit) / max(1e-12, energy(quiet))
                try require(energy(stoppedEdit) < energy(quiet) * 0.1,
                            "A parameter edited while stopped was discarded by the next export's graph reset.")
            }

            await stage("A discovered VST3 discrete parameter snaps public edits and restores authoritative state") {
                guard let jun else { throw AudioSampleError.invalid("Jun-6 V discovery did not complete.") }
                try await apply(base)
                let loaded = try await load(jun)
                let baseline = try await state()
                guard baseline["stateError"] == nil, let originalState = baseline["state"] as? String else {
                    throw AudioSampleError.invalid("The discrete-control baseline could not be captured.")
                }
                let all = try await parameters()
                guard let selected = all.first(where: { row in
                    guard row["writable"] as? Bool == true, let count = row["stepCount"] as? Int,
                          (1...1024).contains(count), let value = (row["normalized"] as? NSNumber)?.doubleValue,
                          value.isFinite,(0...1).contains(value) else { return false }
                    return abs(value*Double(count)-(value*Double(count)).rounded()) < 0.0001
                }), let address = selected["address"] as? String, let count = selected["stepCount"] as? Int,
                      let original = (selected["normalized"] as? NSNumber)?.doubleValue,
                      let instanceID = loaded["instanceID"] as? String else {
                    throw AudioSampleError.invalid("The loaded instrument exposed no writable discrete parameter with a legal current value.")
                }
                let oldIndex = Int((original*Double(count)).rounded())
                let nextIndex = (oldIndex+1) % (count+1), wanted = Double(nextIndex)/Double(count)
                let offGrid = (Double(nextIndex)+(nextIndex<count ? 0.2 : -0.2))/Double(count)
                var edit = target();edit["instanceID"]=instanceID;edit["address"]=address;edit["normalized"]=true
                var restore = target();restore["instanceID"]=instanceID;restore["state"]=originalState
                var observation:[String:Any] = ["descriptor":selected,"component":jun.id,"instanceID":instanceID,
                    "original":original,"expected":wanted,"offGridRequest":offGrid]
                do {
                    edit["value"]=wanted
                    observation["exactEditReply"] = try await host.handle("plugin.parameter",payload:edit)
                    let exactReadback = try await parameterValue(address)
                    edit["value"]=offGrid
                    let snapped = try await host.handle("plugin.parameter",payload:edit) as? [String:Any] ?? [:]
                    let snappedReadback = try await parameterValue(address)
                    observation["exactReadback"]=exactReadback;observation["snappedEditReply"]=snapped;observation["snappedReadback"]=snappedReadback
                    try require(abs(exactReadback-wanted)<0.000001 && abs(snappedReadback-wanted)<0.000001 &&
                        abs(((snapped["normalized"] as? NSNumber)?.doubleValue ?? -1)-wanted)<0.000001,
                        "The discovered discrete control did not acknowledge and read back its legal grid value.")
                    _ = try await host.handle("plugin.state",payload:restore)
                    let restored = try await parameterValue(address);observation["restoredReadback"]=restored
                    try require(abs(restored-original)<0.000001,"Restoring the captured state did not restore the discrete control.")
                    measurements["discreteParameterProbe"]=observation
                    try json(observation,"discrete-parameter-probe")
                } catch {
                    observation["error"]=error.localizedDescription
                    do { _ = try await host.handle("plugin.state",payload:restore);observation["failureCleanupRestored"]=true }
                    catch { observation["failureCleanupError"]=error.localizedDescription }
                    measurements["discreteParameterProbe"]=observation
                    try? json(observation,"discrete-parameter-probe")
                    throw error
                }
            }

            await stage("Tracker automation lane reaches the real VST3 Volume ParamID during offline rendering") {
                guard let jun else { throw AudioSampleError.invalid("Jun-6 V discovery did not complete.") }
                try await apply(base); _ = try await load(jun)
                try await set("0", value: 0)
                let mutedReference = try await render("jun-zero-volume-reference")
                try await set("0", value: 0.23)
                var automated = try await snapshot()
                var patterns = automated["patterns"] as! [[String: Any]]
                patterns[0]["automation"] = [["track": 0, "target": "plugin:instrument:0:0", "interpolation": "step",
                    "points": [["row": 0, "value": 0.0], ["row": 8, "value": 0.7]]]]
                automated["patterns"] = patterns
                try await apply(automated)
                let rendered = try await render("jun-automated-volume")
                let before = energy(rendered, from: 0.1, to: 0.9), after = energy(rendered, from: 1.1, to: 1.9)
                let referenceNoise = energy(mutedReference, from: 0.1, to: 0.9)
                measurements["automation"] = ["beforeRMS": before, "afterRMS": after, "zeroVolumeReferenceRMS": referenceNoise, "target": "plugin:instrument:0:0"]
                // Jun-6 V models analogue noise even with Volume at zero. Compare the
                // automated interval with an actual zero-volume render, then require
                // a substantial audible increase at the automated boundary.
                try require(before <= max(0.000001, referenceNoise * 1.5) && after > max(0.0001, before * 100),
                    "VST3 automation differs from zero-volume reference or does not sound after row8; RMS \(before)/\(after), reference \(referenceNoise).")
            }

            await stage("Native sample audio traverses VST3 effects, bypass and a mixed VST3/Audio Unit serial rack") {
                guard let chorus, let filter else { throw AudioSampleError.invalid("VST3 effect discovery did not complete.") }
                let source = folder.appendingPathComponent("effect-source.wav")
                let samples = (0..<36_000).map { index -> Float in
                    let t = Double(index) / 48_000
                    let envelope = min(1, Double(index) / 96) * min(1, Double(36_000 - index) / 96)
                    return Float(envelope * (sin(2 * .pi * 220 * t) * 0.3 + sin(2 * .pi * 3_200 * t) * 0.12))
                }
                try AudioSamples.write(DecodedSample(path: source.path, sampleRate: 48_000, left: samples, right: samples, originalChannels: 2), to: source)
                var project = base
                project["instruments"] = [["type": "sample", "samplePath": source.path, "rootNote": 60, "mode": "oneshot",
                    "volume": 0.8, "attack": 0, "decay": 0, "sustain": 1, "release": 0.001, "cutoff": 20_000]]
                try await apply(project)
                let dry = try await render("native-dry")
                _ = try await load(chorus, kind: "effect")
                let chorusParameters = try await parameters("effect"); try json(chorusParameters, "chorus-parameters")
                if let mix = chorusParameters.first(where: { ($0["name"] as? String ?? "").lowercased().contains("mix") }), let id = mix["address"] as? String {
                    try await set(id, value: 1, kind: "effect")
                }
                let wet = try await render("native-vst3-chorus")
                try require(energy(wet) > 0.0001 && difference(dry, wet) > 0.00001, "VST3 chorus did not process native sample PCM.")
                var bypass = target("effect"); bypass["bypass"] = true
                _ = try await host.handle("plugin.parameter", payload: bypass)
                let bypassed = try await render("native-vst3-bypassed")
                let bypassDifference = difference(dry, bypassed)
                measurements["effectBypassDifference"] = bypassDifference
                try require(bypassDifference < 0.00001, "VST3 bypass did not reproduce dry PCM: \(bypassDifference).")
                bypass["bypass"] = false; _ = try await host.handle("plugin.parameter", payload: bypass)
                _ = try await load(filter, kind: "effect", slot: 1)
                let filterParameters = try await parameters("effect", slot: 1); try json(filterParameters, "filter-parameters")
                if let cutoff = filterParameters.first(where: { ($0["name"] as? String ?? "").lowercased().contains("cutoff") }), let id = cutoff["address"] as? String {
                    try await set(id, value: 0.25, kind: "effect", slot: 1)
                }
                let serial = try await render("native-two-vst3-effects")
                try require(energy(serial) > 0.00001 && difference(serial, wet) > 0.00001, "Second VST3 insert did not audibly alter its serial input.")
                let available = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
                guard let au = available.first(where: { $0["name"] as? String == "AULowpass" }), let id = au["id"] as? String else {
                    throw AudioSampleError.invalid("macOS AULowpass unavailable for mixed-format serial test.")
                }
                _ = try await host.handle("plugin.load", payload: ["track": 0, "kind": "effect", "slot": 2, "component": id])
                _ = try await host.handle("plugin.parameter", payload: ["track": 0, "kind": "effect", "slot": 2,
                    "address": String(kLowPassParam_CutoffFrequency), "value": 250.0, "normalized": false])
                let mixed = try await render("native-vst3-au-serial")
                try require(energy(mixed) > 0.000001 && difference(mixed, serial) > 0.00001, "Mixed VST3/Audio Unit serial processing did not reach the exported audio.")
                measurements["chorusDifferenceRMS"] = difference(dry, wet)
                measurements["twoVST3DifferenceRMS"] = difference(serial, wet)
                measurements["mixedFormatDifferenceRMS"] = difference(mixed, serial)
            }

            if includeNKS {
                nksStatus = "running"
                let failureMark = failures.count
                await stage("Direct Massive X NKS preset restores sound and binds controller pages by exact VST3 parameter IDs") {
                try require(failures.isEmpty, "The earlier VST3 graph checks failed; NKS execution was skipped.")
                let massive = try await installed("Massive X.vst3", instrument: true)
                let presetURL = URL(fileURLWithPath: "/Library/Application Support/Native Instruments/Massive X/Presets/Contender.nksf")
                let beforeBytes = try Data(contentsOf: presetURL)
                let preset = try NKSHostPreset(url: presetURL)
                try require(preset.identity.vst3ClassID == massive.classID, "NKS PLID and installed Massive X factory class do not match.")
                try await apply(base)
                let loaded = try await host.handle("plugin.loadNKS", payload: ["track": 0, "kind": "instrument", "path": presetURL.path]) as? [String: Any] ?? [:]
                try require(loaded["component"] as? String == massive.id, "NKS load selected a different factory class.")
                let all = try await parameters(); try json(all, "massive-parameters")
                let ids = Set(all.compactMap { ($0["address"] as? String).flatMap(UInt32.init) })
                guard let nks = loaded["nks"] as? [String: Any], let pages = nks["pages"] as? [[String: Any]] else {
                    throw AudioSampleError.invalid("Loaded NKS preset did not retain controller pages.")
                }
                let expected = preset.validatedMappings(vst3ClassID: massive.classID, parameterIDs: ids)
                try require(equivalent(pages, expected), "NKS page bindings differ from exact ParamID validation.")
                let mapped = pages.flatMap { $0["parameters"] as? [[String: Any]] ?? [] }.filter { $0["mapped"] as? Bool == true }
                try require(!mapped.isEmpty && mapped.allSatisfy { ($0["parameterID"] as? NSNumber).map { ids.contains($0.uint32Value) } == true },
                            "NKS mappings are empty or include nonexistent VST3 parameter IDs.")
                let sound = try await render("massive-contender-nks")
                try require(energy(sound) > 0.0001, "Accepted NKS state did not produce audible sequenced Massive X sound.")
                let macroID = String((mapped[0]["parameterID"] as! NSNumber).uint32Value)
                var initialProject = base; initialProject["audioPlugins"] = [loaded]
                var caseResults: [[String: Any]] = [], caseFailures: [String] = []
                // These cases are independent: each opens the same known initial
                // state in a new instance. A failure cannot mask the next case.
                for (name, desired, settleNanoseconds) in [("export-first", 0.31, UInt64(0)),
                                                          ("export-settle", 0.62, UInt64(1_000_000_000))] {
                    var result: [String: Any] = ["name": name, "desired": desired,
                        "settleSeconds": Double(settleNanoseconds) / 1_000_000_000, "passed": false]
                    do {
                        try await apply(base)
                        try await apply(initialProject)
                        let baselineSound = try await render("massive-" + name + "-baseline")
                        result["baselineRMS"] = energy(baselineSound)
                        try require(energy(baselineSound) > 0.0001, "The independent case did not restore its initial NKS sound.")
                        let beforeDelivery = try await recordDelivery(name + "-before-edit", id: macroID)
                        try await set(macroID, value: desired)
                        try await recordDelivery(name + "-queued-before-export", id: macroID)
                        // No state/snapshot call here: the edit must survive
                        // reset and travel in an ordinary audio block.
                        let editedSound = try await render("massive-" + name + "-edited")
                        result["editedDifferenceRMS"] = difference(baselineSound, editedSound)
                        let afterDelivery = try await recordDelivery(name + "-after-export-before-state", id: macroID)
                        if settleNanoseconds > 0 {
                            // Task.sleep yields the main run loop to plug-in workers;
                            // it neither processes audio nor forces another save.
                            try await Task.sleep(nanoseconds: settleNanoseconds)
                            try await recordDelivery(name + "-after-settle-before-state", id: macroID)
                        }
                        try json(try await state(), "massive-state-" + name + "-edited")
                        try await recordDelivery(name + "-after-state", id: macroID)
                        let editedMacro = try await parameterValue(macroID)
                        result["editedMacro"] = editedMacro; result["editedRMS"] = energy(editedSound)
                        try json(try await parameters(), "massive-parameters-" + name + "-edited")
                        var savedProject = try await snapshot()
                        try await recordDelivery(name + "-after-snapshot", id: macroID)
                        savedProject["name"] = "NKS " + name + " verification"
                        let store = try ProjectStore(directory: folder.appendingPathComponent("Support"))
                        let destination = folder.appendingPathComponent("NKS " + name + " verification.supertracker")
                        _ = try store.save(to: destination, snapshot: savedProject, collectSamples: false)
                        let reopenedProject = try store.read(destination)
                        let savedPlugins = savedProject["audioPlugins"] as? [[String: Any]] ?? []
                        let reopenedPlugins = reopenedProject["audioPlugins"] as? [[String: Any]] ?? []
                        try require(equivalent(savedPlugins, reopenedPlugins), "Project save/reopen changed opaque plug-in state or NKS metadata.")
                        _ = try await host.handle("plugin.remove", payload: target())
                        try await apply(reopenedProject)
                        try await recordDelivery(name + "-fresh-restored-before-export", id: macroID)
                        let reopened = try await render("massive-" + name + "-reopened")
                        try await recordDelivery(name + "-fresh-restored-after-export", id: macroID)
                        let reopenedMacro = try await parameterValue(macroID)
                        result["restoredMacro"] = reopenedMacro; result["restoredRMS"] = energy(reopened)
                        try json(try await parameters(), "massive-parameters-" + name + "-reopened")
                        let freshState = try await state()
                        try json(freshState, "massive-state-" + name + "-reopened")
                        // Validate delivery after recording every state artifact,
                        // so a processing/read failure remains independently visible.
                        if !afterDelivery.isEmpty {
                            let beforeRead = (beforeDelivery["readPoints"] as? NSNumber)?.uint64Value ?? 0
                            let afterRead = (afterDelivery["readPoints"] as? NSNumber)?.uint64Value ?? 0
                            let readValue = (afterDelivery["lastReadValue"] as? NSNumber)?.doubleValue ?? -1
                            let offeredValue = (afterDelivery["lastOfferedValue"] as? NSNumber)?.doubleValue ?? -1
                            let readFrames = (afterDelivery["lastReadFrames"] as? NSNumber)?.intValue ?? 0
                            result["nonzeroDelivery"] = ["beforeReadPoints": beforeRead, "afterReadPoints": afterRead,
                                "lastReadValue": readValue, "lastOfferedValue": offeredValue, "lastReadFrames": readFrames,
                                "midiReceived": afterDelivery["midiReceived"] ?? 0]
                            try require(afterRead > beforeRead && readFrames > 1 && abs(readValue - desired) < 0.003 && abs(offeredValue - desired) < 0.003,
                                        "The diagnostic did not deliver the expected edit in an ordinary audio block after reset.")
                        }
                        try require(abs(editedMacro - desired) < 0.003 && energy(editedSound) > 0.0001,
                                    "The mapped edit lost its controller value or sequenced sound.")
                        try require(energy(reopened) > 0.0001 && abs(reopenedMacro - desired) < 0.003,
                                    "Fresh restoration lost the edited NKS parameter (ID \(macroID), desired \(desired), saved \(editedMacro), restored \(reopenedMacro), RMS \(energy(reopened))).")
                        try require(equivalent(freshState["nks"] as? [String: Any] ?? [:], nks), "Fresh instance lost NKS metadata.")
                        result["passed"] = true
                    } catch {
                        result["error"] = error.localizedDescription
                        caseFailures.append(name + ": " + error.localizedDescription)
                    }
                    caseResults.append(result); measurements["nksNonzeroCases"] = caseResults
                    try write("running")
                }
                var immediate: [String: Any] = ["name": "immediate-save", "desired": 0.47,
                    "capturedBeforeAnyExport": true, "passed": false]
                do {
                    try await apply(base)
                    try await apply(initialProject)
                    let before = try await recordDelivery("immediate-save-before-edit", id: macroID)
                    try await set(macroID, value: 0.47)
                    try await recordDelivery("immediate-save-queued", id: macroID)
                    // This is the ordinary mapped-control commit sequence. There
                    // has been no export, note, or transport start in this instance.
                    let captured = try await state()
                    try require(captured["stateError"] == nil, "Immediate capture returned an unavailable state: \(captured["stateError"] ?? "unknown").")
                    try json(captured, "massive-state-immediate-save-edited")
                    let preparation = captured["hostStatus"] as? [String: Any] ?? [:]
                    immediate["statePreparation"] = preparation
                    let delivery = try await recordDelivery("immediate-save-after-state", id: macroID)
                    immediate["delivery"] = delivery
                    immediate["editedMacro"] = try await parameterValue(macroID)
                    var savedProject = initialProject
                    // Use the exact first plugin.state result, without an engine
                    // snapshot that could hide an incomplete immediate capture.
                    savedProject["audioPlugins"] = [captured]
                    let store = try ProjectStore(directory: folder.appendingPathComponent("Support"))
                    let destination = folder.appendingPathComponent("NKS immediate-save verification.supertracker")
                    _ = try store.save(to: destination, snapshot: savedProject, collectSamples: false)
                    let reopenedProject = try store.read(destination)
                    try require(equivalent(savedProject["audioPlugins"] ?? [], reopenedProject["audioPlugins"] ?? []),
                                "Immediate project serialization changed the captured state.")
                    _ = try await host.handle("plugin.remove", payload: target())
                    try await apply(reopenedProject)
                    try await recordDelivery("immediate-save-fresh-before-export", id: macroID)
                    let reopenedValue = try await parameterValue(macroID)
                    immediate["restoredMacroBeforeExport"] = reopenedValue
                    try json(try await state(), "massive-state-immediate-save-reopened-before-export")
                    // Only after capture, disk round-trip, and fresh restoration
                    // have completed do we render the first note for this case.
                    let reopened = try await render("massive-immediate-save-reopened")
                    immediate["restoredRMS"] = energy(reopened)
                    try await recordDelivery("immediate-save-fresh-after-export", id: macroID)
                    try require((preparation["stateFlushFrames"] as? NSNumber)?.intValue == 4096 &&
                                preparation["loadedBundleVersion"] as? String == "1.7.1",
                                "Immediate capture did not select the exact loaded Massive X bundle compatibility profile.")
                    if !delivery.isEmpty {
                        let priorReads = (before["readPoints"] as? NSNumber)?.uint64Value ?? 0
                        let reads = (delivery["readPoints"] as? NSNumber)?.uint64Value ?? 0
                        try require(reads > priorReads && abs(((delivery["lastReadValue"] as? NSNumber)?.doubleValue ?? -1) - 0.47) < 0.003 &&
                                    (delivery["pendingUIEdits"] as? NSNumber)?.intValue == 0 &&
                                    (delivery["lastReadFrames"] as? NSNumber)?.intValue == 4096,
                                    "Immediate capture did not deliver the queued edit to the plug-in.")
                    }
                    try require(abs(reopenedValue - 0.47) < 0.003 && energy(reopened) > 0.0001,
                                "Immediate plugin.state lost the edited NKS parameter before any export (desired0.47, restored \(reopenedValue)).")
                    immediate["passed"] = true
                } catch {
                    immediate["error"] = error.localizedDescription
                    caseFailures.append("immediate-save: " + error.localizedDescription)
                }
                caseResults.append(immediate); measurements["nksNonzeroCases"] = caseResults
                try write("running")
                try require(try Data(contentsOf: presetURL) == beforeBytes, "NKS loading changed the source preset file.")
                measurements["nks"] = ["classID": massive.classID, "pages": pages.count, "mappedControls": mapped.count,
                    "editedParamID": macroID, "stateBytes": preset.state.count, "sourceUnchanged": true]
                try json(nks, "nks-metadata")
                try require(caseFailures.isEmpty, caseFailures.joined(separator: "\n"))
                }
                nksStatus = failures.count == failureMark ? "passed" : "failed"
            }

            await stage("Rejected malformed or unresolved NKS selections preserve the previous live instrument and opaque state") {
                guard let jun else { throw AudioSampleError.invalid("Jun-6 V discovery did not complete.") }
                try await apply(base); _ = try await load(jun)
                try await set("0", value: 0.29); _ = try await render("legacy-before")
                let before = try await state()
                let unchangedControl = try await state()
                let beforeParameters = try await parameters()
                // Opaque vendor bytes have no host-defined validity beyond their
                // envelope. Massive X accepts the former four-byte payload, and
                // its legacy identity is supported. Use host-verifiable failures.
                let absentMagic: UInt32 = 0x53745421 // "StT!", private synthetic identity.
                try require(vst2Catalogue.resolve(uniqueID: absentMagic) == nil &&
                    catalogue.records.allSatisfy { !NKSHostPreset.isVST2Compatible(classID: $0.classID, vst2Magic: absentMagic) },
                    "The unresolved-identity fixture unexpectedly matches a discovered plug-in.")
                let invalidCases: [(String, Data, String)] = [
                    ("empty-state", legacyFixture(state: Data()), "no saved plug-in state"),
                    ("unsupported-state-version", legacyFixture(stateVersion: 2), "PCHK state version 2"),
                    ("unresolved-identity", legacyFixture(magic: absentMagic), "No installed VST3 plug-in replaces")
                ]
                var rejectedCases: [[String: Any]] = []
                for (name, bytes, expectedError) in invalidCases {
                    let legacy = folder.appendingPathComponent("Rejected \(name).nksf")
                    try bytes.write(to: legacy, options: .atomic)
                    var rejection: String?
                    do { _ = try await host.handle("plugin.loadNKS", payload: ["track": 0, "kind": "instrument", "path": legacy.path]) }
                    catch { rejection = error.localizedDescription }
                    try require(rejection?.contains(expectedError) == true,
                                "NKS \(name) did not fail for its expected reason: \(rejection ?? "accepted").")
                    let retained = try await state()
                    let retainedParameters = try await parameters()
                    let identicalParameters = equivalent(
                        beforeParameters.filter { $0["writable"] as? Bool == true }.map { ["address": $0["address"] ?? "", "value": $0["value"] ?? 0] },
                        retainedParameters.filter { $0["writable"] as? Bool == true }.map { ["address": $0["address"] ?? "", "value": $0["value"] ?? 0] })
                    try require(retained["stateError"] == nil && retained["instanceID"] as? String == before["instanceID"] as? String &&
                        retained["component"] as? String == before["component"] as? String &&
                        samePluginState(retained["state"] as? String, before["state"] as? String) && identicalParameters,
                        "NKS \(name) rejection changed the previous exact instance, state or parameters.")
                    rejectedCases.append(["case": name, "error": rejection!, "instanceRetained": true,
                        "stateRetained": true, "parametersRetained": true])
                    measurements["rejectedNKSSelections"] = rejectedCases
                    try write("running")
                }
                let after = try await state()
                let afterParameters = try await parameters()
                let stateEqual = samePluginState(after["state"] as? String, before["state"] as? String)
                let controlEqual = samePluginState(unchangedControl["state"] as? String, before["state"] as? String)
                measurements["legacyStateComparison"] = ["base64Equal": after["state"] as? String == before["state"] as? String,
                    "propertyListEquivalent": stateEqual, "noActionControlEquivalent": controlEqual]
                let writableBefore = beforeParameters.filter { $0["writable"] as? Bool == true }.map { ["address": $0["address"] ?? "", "value": $0["value"] ?? 0] }
                let writableAfter = afterParameters.filter { $0["writable"] as? Bool == true }.map { ["address": $0["address"] ?? "", "value": $0["value"] ?? 0] }
                try require(after["component"] as? String == before["component"] as? String && stateEqual && equivalent(writableBefore, writableAfter),
                            "A refused NKS browser selection changed the existing slot or saved state (opaqueStateEqual=\(stateEqual), noActionControlEqual=\(controlEqual)).")
                let sound = try await render("legacy-after-refusal")
                let preservedVolume = try await parameterValue("0")
                try require(energy(sound) > 0.0001 && abs(preservedVolume - 0.29) < 0.002,
                            "A refused NKS selection interrupted the previously playable instrument.")
            }
            await host.shutdownAndWait()
            activeHost = nil
            try write("audioChecksComplete")
            if !failures.isEmpty { throw AudioSampleError.invalid("VST3 native integration: " + failures.joined(separator: "\n")) }
            return checks
        } catch {
            if let host = activeHost { await host.shutdownAndWait(); activeHost = nil }
            if failures.isEmpty { failures.append(error.localizedDescription) }
            try? write("failed")
            throw error
        }
    }

    private static func energy(_ sample: DecodedSample, from: Double = 0, to: Double? = nil) -> Double {
        let first = max(0, min(sample.left.count, Int(from * sample.sampleRate)))
        let last = max(first, min(sample.left.count, Int((to ?? sample.duration) * sample.sampleRate)))
        guard last > first else { return 0 }
        var sum = 0.0
        for index in first..<last { sum += Double(sample.left[index]) * Double(sample.left[index]) + Double(sample.right[index]) * Double(sample.right[index]) }
        return sqrt(sum / Double((last - first) * 2))
    }
    private static func peak(_ sample: DecodedSample) -> Double {
        Double(max(sample.left.map(abs).max() ?? 0, sample.right.map(abs).max() ?? 0))
    }
    private static func difference(_ a: DecodedSample, _ b: DecodedSample) -> Double {
        let count = min(a.left.count, b.left.count)
        guard count > 0 else { return 0 }
        var sum = 0.0
        for index in 0..<count {
            let left = Double(a.left[index] - b.left[index]), right = Double(a.right[index] - b.right[index])
            sum += left * left + right * right
        }
        return sqrt(sum / Double(count * 2))
    }
    /// Minimal NKS container with explicit envelope/identity variations. Arbitrary
    /// opaque state bytes alone are not a reliable negative vendor-state fixture.
    private static func legacyFixture(magic: UInt32 = 0x4E692448, stateVersion: UInt32 = 1,
                                      state: Data = Data([1, 2, 3, 4])) -> Data {
        func word(_ value: UInt32) -> Data { Data((0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) }) }
        func chunk(_ tag: String, _ bytes: Data) -> Data {
            Data(tag.utf8) + word(UInt32(bytes.count)) + bytes + (bytes.count & 1 == 1 ? Data([0]) : Data())
        }
        func string(_ value: String) -> Data { Data([0xA0 | UInt8(value.utf8.count)]) + Data(value.utf8) }
        let identity = Data([0x81]) + string("VST.magic") + Data([0xCE]) +
            Data([24, 16, 8, 0].map { UInt8(truncatingIfNeeded: magic >> $0) })
        let name = Data([0x81]) + string("name") + string("Legacy fixture")
        let body = chunk("NISI", word(1) + name) + chunk("PLID", word(1) + identity) + chunk("PCHK", word(stateVersion) + state)
        return Data("RIFF".utf8) + word(UInt32(body.count + 4)) + Data("NIKS".utf8) + body
    }
}

#endif

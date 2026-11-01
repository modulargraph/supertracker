// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import CryptoKit

/// Explicit installed-vendor acceptance, invoked only by the diagnostic CLI.
/// Scans private VST2/VST3 aliases for Massive and Reaktor 6, imports a real mxgrp,
/// renders only a selected imported pad (a member track of the kit group), and never selects MIDI/audio devices.
/// It does not compare NI Maschine's sound to our independent built-in effects.
enum NativeMaschineKitTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let fm = FileManager.default
        let folder = output.appendingPathComponent("maschine-kit", isDirectory: true)
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], failures: [String] = []
        var metrics: [String: Any] = ["hardware": false, "midiInputIDs": [], "nativeMaschineExecuted": false,
            "sonicComparison": "Not performed: this verifies imported state/owner routing and generated-note PCM, not NI Maschine output equivalence."]
        defer {
            let report: [String: Any] = ["passed": failures.isEmpty && !checks.isEmpty, "checks": checks, "failures": failures, "measurements": metrics]
            if let bytes = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? bytes.write(to: folder.appendingPathComponent("report.json"), options: .atomic)
            }
        }
        func require(_ condition: Bool, _ message: String) throws {
            guard condition else { throw AudioSampleError.invalid("Maschine Kit: " + message) }
        }
        func hash(_ value: String) -> String { SHA256.hash(data: Data(value.utf8)).map { String(format: "%02x", $0) }.joined() }
        // The installed Deep Matter expansion, wherever Native Access put it (SUPERTRACKER_NI_CONTENT_DIR overrides).
        let library = ProcessInfo.processInfo.environment["SUPERTRACKER_NI_CONTENT_DIR"].map { URL(fileURLWithPath: $0, isDirectory: true).appendingPathComponent("Deep Matter Library", isDirectory: true) }
            ?? NKSCatalog.discoverRegistrations().first { $0.contentDir.hasSuffix("/Deep Matter Library") }.map { URL(fileURLWithPath: $0.contentDir, isDirectory: true) }
            ?? URL(fileURLWithPath: "/Users/Shared/Deep Matter Library", isDirectory: true)
        let sourceURL = library.appendingPathComponent("Kits/Evolet Kit.mxgrp")
        let catalogueFolder = folder.appendingPathComponent("catalogues-" + UUID().uuidString, isDirectory: true)
        let selectedRoot = catalogueFolder.appendingPathComponent("allowlisted-vst2", isDirectory: true)
        try fm.createDirectory(at: selectedRoot, withIntermediateDirectories: true)
        var allowed: [String: String] = [:]
        for name in ["Massive.vst", "Reaktor 6.vst"] {
            let installed = URL(fileURLWithPath: "/Library/Audio/Plug-Ins/VST").appendingPathComponent(name)
            guard fm.fileExists(atPath: installed.path) else { continue }
            let alias = selectedRoot.appendingPathComponent(name)
            try fm.createSymbolicLink(at: alias, withDestinationURL: installed)
            allowed[alias.path] = installed.resolvingSymlinksInPath().path
        }
        let selectedVST3Root = catalogueFolder.appendingPathComponent("allowlisted-vst3", isDirectory: true)
        try fm.createDirectory(at: selectedVST3Root, withIntermediateDirectories: true)
        var allowedVST3: [String: String] = [:]
        for name in ["Massive.vst3", "Reaktor 6.vst3"] {
            let installed = URL(fileURLWithPath: "/Library/Audio/Plug-Ins/VST3").appendingPathComponent(name)
            guard fm.fileExists(atPath: installed.path) else { continue }
            let alias = selectedVST3Root.appendingPathComponent(name)
            try fm.createSymbolicLink(at: alias, withDestinationURL: installed)
            allowedVST3[alias.path] = installed.resolvingSymlinksInPath().path
        }
        let vst2 = VST2Catalogue(configuration: .init(roots: [selectedRoot], cacheURL: catalogueFolder.appendingPathComponent("vst2.json")))
        let vst3 = VST3Catalogue(configuration: .init(roots: [selectedVST3Root], cacheURL: catalogueFolder.appendingPathComponent("vst3.json")))
        metrics["catalogueIsolation"] = ["vst2Aliases": allowed, "vst3Aliases": allowedVST3, "cacheDirectory": catalogueFolder.path]
        do {
            try require(fm.fileExists(atPath: sourceURL.path), "Required installed Evolet mxgrp is missing.")
            let sourceBytes = try Data(contentsOf: sourceURL)
            metrics["source"] = ["path": sourceURL.path, "bytes": sourceBytes.count,
                "sha256": SHA256.hash(data: sourceBytes).map { String(format: "%02x", $0) }.joined()]
            let imported = try MaschineGroupImporter.read(at: sourceURL, root: library)
            guard let kitFragment = imported["project"] as? [String: Any],
                  let kit = (kitFragment["kits"] as? [[String: Any]])?.first,
                  let sounds = kit["sounds"] as? [[String: Any]] else {
                throw AudioSampleError.invalid("Maschine Kit: importer returned no complete kit.")
            }
            try require(sounds.count == 16, "Imported kit did not retain sixteen Sounds.")
            // Kits load as track groups (what kit.read hands the UI): one member track per loaded pad.
            let fragment = try KitGroups.groupProject(fromKitProject: kitFragment)
            guard let group = (fragment["groups"] as? [[String: Any]])?.first, let busID = group["busID"] as? String,
                  let members = fragment["tracks"] as? [[String: Any]], !members.isEmpty else {
                throw AudioSampleError.invalid("Maschine Kit: the imported kit did not convert to a track group.")
            }
            metrics["importWarnings"] = imported["warnings"] ?? []
            metrics["groupMembers"] = members.count
            checks.append("Real installed Evolet mxgrp imports sixteen Sound slots and converts to a group of \(members.count) pad tracks")
            await vst2.refresh()
            let vst3Report = await vst3.refresh()
            try require(vst2.records.allSatisfy { allowed[$0.bundlePath] == URL(fileURLWithPath: $0.bundlePath).resolvingSymlinksInPath().path }, "VST2 discovery escaped the two-bundle allowlist.")
            try require(vst3.records.allSatisfy { allowedVST3[$0.bundlePath] == URL(fileURLWithPath: $0.bundlePath).resolvingSymlinksInPath().path }, "VST3 discovery escaped the two-bundle allowlist.")
            metrics["discoveredVST2"] = vst2.records.map(\.dictionary)
            metrics["unavailableVST2"] = vst2.unavailable
            metrics["vst3Discovery"] = vst3Report.dictionary
            let importedPlugins = fragment["audioPlugins"] as? [[String: Any]] ?? []
            let instruments = fragment["instruments"] as? [[String: Any]] ?? []
            // These are the decimal IDs serialized in the installed mxgrp, not
            // IDs inferred from a runtime bundle name or a matching display name.
            let massiveID: UInt32 = 1_315_523_937, reaktorID: UInt32 = 1_315_525_174
            func target(_ legacyID: UInt32, label: String) throws -> (sound: [String: Any], plugin: [String: Any], owner: String, index: Int) {
                let component = String(format: "vst2:%08X", legacyID)
                guard let plugin = importedPlugins.first(where: { $0["kind"] as? String == "instrument" && $0["component"] as? String == component }),
                      let metadata = plugin["maschinePluginState"] as? [String: Any],
                      (metadata["vst2UniqueID"] as? NSNumber)?.uint32Value == legacyID,
                      let owner = plugin["owner"] as? String,
                      let index = members.firstIndex(where: { "track:" + ($0["routingID"] as? String ?? "") == owner }),
                      let instrumentIndex = members[index]["instrument"] as? Int, instruments.indices.contains(instrumentIndex),
                      instruments[instrumentIndex]["type"] as? String == "au",
                      !(plugin["state"] as? String ?? "").isEmpty else {
                    throw AudioSampleError.invalid("Maschine Kit: imported \(label) has no exact legacy-ID, member-bound playable instrument and validated opaque state.")
                }
                return (members[index], plugin, owner, index)
            }
            func project(for selected: (sound: [String: Any], plugin: [String: Any], owner: String, index: Int)) -> [String: Any] {
                var project = fragment
                var notes = [Any](repeating: NSNull(), count: 16)
                notes[0] = ["note": 60, "velocity": 110, "gate": 6.0]
                notes[8] = ["note": 67, "velocity": 100, "gate": 6.0]
                var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: 16), count: members.count)
                steps[selected.index] = notes
                project["version"] = 1; project["id"] = UUID().uuidString; project["name"] = "Imported \(selected.sound["name"] ?? "pad") acceptance"
                project["bpm"] = 120.0; project["linesPerBeat"] = 4; project["masterVolume"] = 0.5
                project["activePattern"] = 0; project["loopSong"] = false; project["sequence"] = [0]
                project["patterns"] = [["id": 0, "length": 16, "steps": steps, "automation": []]]
                project["audioClips"] = []; project["delay"] = ["mix": 0]; project["reverb"] = ["mix": 0]
                // Only the selected real instrument is instantiated. Unselected
                // pads retain their imported definitions but receive no notes.
                project["audioPlugins"] = importedPlugins.filter {
                    let owner = $0["owner"] as? String
                    return owner == selected.owner && ($0["kind"] as? String == "instrument" || $0["format"] as? String == "Builtin") ||
                        owner == "bus:" + busID && $0["format"] as? String == "Builtin"
                }
                return project
            }
            func makeHost() throws -> AudioHost {
                let graph = AVAudioEngine()
                let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
                try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
                return try AudioHost(audioGraph: graph, vst3Catalogue: vst3, midiInputIDs: [], vst2Catalogue: vst2)
            }
            func readParameters(_ host: AudioHost, owner: String) async throws -> [String: Double] {
                let rows = try await host.handle("plugin.parameters", payload: ["owner": owner, "kind": "instrument", "slot": 0]) as? [[String: Any]] ?? []
                var values: [String: Double] = [:]
                for row in rows where row["writable"] as? Bool != false {
                    if let address = row["address"] as? String, let value = (row["normalized"] as? NSNumber)?.doubleValue ?? (row["value"] as? NSNumber)?.doubleValue, value.isFinite { values[address] = value }
                }
                return values
            }
            func render(_ host: AudioHost, owner: String, label: String) async throws -> [String: Any] {
                let destination = folder.appendingPathComponent(label, isDirectory: true)
                let result = try await host.handle("audio.export", payload: ["mode": "stems", "path": destination.path,
                    "owners": [owner, "bus:" + busID, "master"], "tail": 0.25, "encoding": "float32"]) as? [String: Any] ?? [:]
                let stems = result["stems"] as? [[String: Any]] ?? []
                try require(Set(stems.compactMap { $0["owner"] as? String }) == Set([owner, "bus:" + busID, "master"]), "Export omitted selected pad, group or master.")
                var measured: [String: Any] = [:]
                for stem in stems {
                    guard let path = stem["path"] as? String, let stemOwner = stem["owner"] as? String else { throw AudioSampleError.invalid("Invalid exported stem record.") }
                    let pcm = try AudioSamples.decode(path)
                    try require(pcm.left.count == 108_000 && pcm.right.count == pcm.left.count, "Wrong two-second song plus quarter-second tail extent.")
                    var squares = 0.0, peak = 0.0
                    for index in pcm.left.indices {
                        let left = Double(pcm.left[index]), right = Double(pcm.right[index])
                        try require(left.isFinite && right.isFinite, "Nonfinite imported instrument PCM.")
                        squares += left * left + right * right; peak = max(peak, abs(left), abs(right))
                    }
                    let rms = sqrt(squares / Double(pcm.left.count * 2))
                    try require(rms > 0.00001 && peak > 0.00005, "\(label) \(stemOwner) is silent (RMS \(rms)).")
                    measured[stemOwner] = ["path": path, "rms": rms, "peak": peak, "frames": pcm.left.count]
                }
                return measured
            }
            func verify(_ legacyID: UInt32, label: String) async throws {
                let selected = try target(legacyID, label: label)
                let native = vst2.resolve(uniqueID: legacyID, isInstrument: true)
                let compatible = vst3.resolveCompatible(vst2Magic: legacyID, isInstrument: true)
                guard let expectedComponent = native?.id ?? compatible?.id else {
                    throw AudioSampleError.invalid("Maschine Kit: \(label) has neither its exact VST2 nor an exact predecessor-compatible VST3 available.")
                }
                let expectedFormat = native == nil ? "VST3" : "VST2"
                if native == nil {
                    try require(compatible.map { VST3Catalogue.isVST2Compatible(classID: $0.classID, vst2Magic: legacyID) } == true,
                        "Replacement class does not encode the original VST2 identity.")
                }
                var host: AudioHost? = try makeHost()
                do {
                    let initial = project(for: selected)
                    var pluginErrors: [[String: Any]] = []
                    host!.onEvent = { name, value in if name == "plugin.error" { pluginErrors.append(value) } }
                    _ = try await host!.handle("engine.project", payload: ["project": initial, "replace": true, "awaitPlugins": true])
                    await host!.pluginRestorationBarrier()?.value
                    try require(pluginErrors.isEmpty, "Imported state restore emitted plugin.error: \(pluginErrors).")
                    let before = try await host!.handle("plugin.state", payload: ["owner": selected.owner, "kind": "instrument", "slot": 0]) as? [String: Any] ?? [:]
                    let importedMetadata = selected.plugin["maschinePluginState"] as? [String: Any] ?? [:]
                    metrics[label + "Migration"] = ["migration": before["migration"] ?? NSNull(),
                        "importedPresetIndex": importedMetadata["presetIndex"] ?? NSNull(),
                        "programSelectionEvidence": "The archive program index is retained. Raw fallback acceptance alone does not prove selected-program equivalence; no NI reference render was performed."]
                    try require(before["component"] as? String == expectedComponent && before["format"] as? String == expectedFormat && before["owner"] as? String == selected.owner,
                        "Native instance did not select the exact compatible identity for its pad track.")
                    if native == nil {
                        let controls = try await host!.handle("plugin.parameters", payload: ["owner": selected.owner, "kind": "instrument", "slot": 0]) as? [[String: Any]] ?? []
                        guard let control = controls.first(where: {
                            $0["writable"] as? Bool == true && (($0["stepCount"] as? NSNumber)?.intValue ?? 0) == 0 &&
                                (($0["max"] as? NSNumber)?.doubleValue ?? 0) > (($0["min"] as? NSNumber)?.doubleValue ?? 0)
                        }), let address = control["address"] as? String, let original = (control["normalized"] as? NSNumber)?.doubleValue,
                              original.isFinite else {
                            throw AudioSampleError.invalid("Migrated instrument exposes no writable continuous control for the same-source replay probe.")
                        }
                        let proposed = original < 0.5 ? 0.73 : 0.27
                        let changed = try await host!.handle("plugin.parameter", payload: ["owner": selected.owner, "kind": "instrument", "slot": 0,
                            "address": address, "value": proposed, "normalized": true]) as? [String: Any] ?? [:]
                        guard let accepted = (changed["normalized"] as? NSNumber)?.doubleValue, abs(accepted - original) > 0.01 else {
                            throw AudioSampleError.invalid("Continuous-control probe was not accepted by the migrated instrument.")
                        }
                        // Ordinary publication still contains the original VST2
                        // bank. It must preserve edits in this migrated instance.
                        _ = try await host!.handle("engine.project", payload: ["project": initial, "awaitPlugins": true])
                        await host!.pluginRestorationBarrier()?.value
                        let retained = try await readParameters(host!, owner: selected.owner)
                        let retainedState = try await host!.handle("plugin.state", payload: ["owner": selected.owner, "kind": "instrument", "slot": 0]) as? [String: Any] ?? [:]
                        try require(pluginErrors.isEmpty && retained[address].map { abs($0 - accepted) < 0.00001 } == true &&
                            retainedState["instanceID"] as? String == before["instanceID"] as? String && retainedState["component"] as? String == expectedComponent,
                            "Reapplying the same imported VST2 descriptor replaced the migrated instance or erased its control edit.")
                        _ = try await host!.handle("plugin.parameter", payload: ["owner": selected.owner, "kind": "instrument", "slot": 0,
                            "address": address, "value": original, "normalized": true])
                        metrics[label + "SameSourceReplay"] = ["address": address, "original": original, "edited": accepted,
                            "retained": retained[address]!, "sameInstance": true]
                        checks.append("Migrated \(label) retains native control edits when the original imported bank is republished")
                    }
                    let first = try await render(host!, owner: selected.owner, label: label + "-imported")
                    // Capture the controls at the same post-render boundary as
                    // the saved opaque state, not before host tempo is delivered.
                    let parameters = try await readParameters(host!, owner: selected.owner)
                    try require(!parameters.isEmpty, "Imported instrument exposes no stable writable parameter evidence.")
                    let snapshot = try await host!.handle("engine.snapshot", payload: [:]) as! [String: Any]
                    let savedPlugin = (snapshot["audioPlugins"] as? [[String: Any]] ?? []).first { $0["owner"] as? String == selected.owner && $0["kind"] as? String == "instrument" }
                    guard let savedState = savedPlugin?["state"] as? String, let savedData = Data(base64Encoded: savedState), !savedData.isEmpty,
                          let savedDocument = try PropertyListSerialization.propertyList(from: savedData, options: [], format: nil) as? [String: Any] else {
                        throw AudioSampleError.invalid("Snapshot lost the native plug-in document state.")
                    }
                    try require(savedPlugin?["component"] as? String == expectedComponent && savedPlugin?["format"] as? String == expectedFormat,
                        "Snapshot retained the pre-migration descriptor instead of the actual native instance.")
                    try require(savedDocument["format"] as? String == "SuperTracker." + expectedFormat,
                        "Snapshot retained an incompatible opaque state envelope.")
                    if expectedFormat == "VST3" {
                        try require(savedDocument["classID"] as? String == compatible?.classID,
                            "Migrated state did not capture the exact replacement class.")
                    }
                    let store = try ProjectStore(directory: folder.appendingPathComponent("store-" + label))
                    let projectURL = folder.appendingPathComponent(label + ".supertracker")
                    _ = try store.save(to: projectURL, snapshot: snapshot, collectSamples: false)
                    let reopened = try store.read(projectURL)
                    let reopenedPlugin = (reopened["audioPlugins"] as? [[String: Any]] ?? []).first { $0["owner"] as? String == selected.owner && $0["kind"] as? String == "instrument" }
                    try require(reopenedPlugin?["state"] as? String == savedState && reopenedPlugin?["component"] as? String == expectedComponent && reopenedPlugin?["format"] as? String == expectedFormat,
                        "Project Save/Open altered canonical identity or opaque state.")
                    await host!.shutdownAndWait(); host = nil
                    host = try makeHost()
                    pluginErrors = []
                    host!.onEvent = { name, value in if name == "plugin.error" { pluginErrors.append(value) } }
                    _ = try await host!.handle("engine.project", payload: ["project": reopened, "replace": true, "awaitPlugins": true])
                    await host!.pluginRestorationBarrier()?.value
                    try require(pluginErrors.isEmpty, "Fresh-instance restore emitted plugin.error: \(pluginErrors).")
                    let reopenedState = try await host!.handle("plugin.state", payload: ["owner": selected.owner, "kind": "instrument", "slot": 0]) as? [String: Any] ?? [:]
                    try require(reopenedState["component"] as? String == expectedComponent && reopenedState["format"] as? String == expectedFormat && reopenedState["owner"] as? String == selected.owner,
                        "Fresh Save/Open restored the wrong native format, component or pad owner.")
                    let restored = try await readParameters(host!, owner: selected.owner)
                    try require(Set(parameters.keys) == Set(restored.keys), "Fresh instance changed exposed parameter identity.")
                    let error = parameters.map { abs($0.value - restored[$0.key]!) }.max() ?? 0
                    try require(error < 0.00001, "Fresh instance did not restore saved controls (maximum error \(error)).")
                    let second = try await render(host!, owner: selected.owner, label: label + "-reopened")
                    metrics[label] = ["status": "passed", "padName": selected.sound["name"] ?? "", "owner": selected.owner,
                        "legacyUniqueID": legacyID, "component": expectedComponent, "format": expectedFormat,
                        "migratedToVST3": native == nil, "importedStateSHA256": hash(selected.plugin["state"] as! String),
                        "savedStateSHA256": hash(savedState), "parameterCount": parameters.count, "maximumParameterError": error,
                        "importedRender": first, "reopenedRender": second, "savedProject": projectURL.path]
                    await host!.shutdownAndWait(); host = nil
                    checks.append("Imported \(label) pad track restores real opaque state, renders owner/group/master PCM, and survives Save/Open into a fresh instance")
                } catch {
                    if let host { await host.shutdownAndWait() }
                    throw error
                }
            }
            try await verify(massiveID, label: "Massive")
            if vst2.resolve(uniqueID: reaktorID, isInstrument: true) != nil || vst3.resolveCompatible(vst2Magic: reaktorID, isInstrument: true) != nil {
                do { try await verify(reaktorID, label: "Reaktor") }
                catch {
                    // Optional dependency probe is never counted as passed. The
                    // report retains the exact importer/ensemble/plug-in error.
                    metrics["Reaktor"] = ["status": "unavailable-or-failed", "reason": error.localizedDescription]
                    metrics["optionalFailures"] = ["Reaktor: " + error.localizedDescription]
                }
            } else {
                metrics["Reaktor"] = ["status": "unavailable", "reason": "Neither allowlisted Reaktor 6 VST2 nor its exact compatible VST3 was discoverable."]
            }
            return checks
        } catch {
            failures.append(error.localizedDescription)
            throw error
        }
    }
}

#endif

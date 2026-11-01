// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AudioToolbox

/// Installed-vendor VST2 integration checks. Renders use the offline export graph only.
/// Run with: SuperTracker --vst2-self-test <output-folder>
enum NativeVST2Tests {
    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("vst2", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], failures: [String] = [], measurements: [String: Any] = [:]
        func trace(_ message: String) { fputs("VST2 test: \(message)\n", stderr) }
        func require(_ value: Bool, _ message: String) throws { if !value { throw AudioSampleError.invalid(message) } }
        // The production catalogue has no public targeted VST2 scan method.
        // Enumerate only two private aliases: never the installed plug-in parent.
        let catalogueFolder = folder.appendingPathComponent("catalogues-" + UUID().uuidString,isDirectory:true)
        let discoveryRoot = catalogueFolder.appendingPathComponent("known-vst2",isDirectory:true)
        try FileManager.default.createDirectory(at:discoveryRoot,withIntermediateDirectories:true)
        var knownBundles:[String:String]=[:]
        for name in ["DX7 V.vst","Massive X.vst"] {
            let installed=URL(fileURLWithPath:"/Library/Audio/Plug-Ins/VST").appendingPathComponent(name)
            guard FileManager.default.fileExists(atPath:installed.path) else { continue }
            let alias=discoveryRoot.appendingPathComponent(name)
            try FileManager.default.createSymbolicLink(at:alias,withDestinationURL:installed)
            knownBundles[alias.path]=installed.resolvingSymlinksInPath().path
        }
        var vst2Configuration=VST2Catalogue.Configuration()
        vst2Configuration.roots=[discoveryRoot];vst2Configuration.cacheURL=catalogueFolder.appendingPathComponent("vst2.json")
        let catalogue=VST2Catalogue(configuration:vst2Configuration)
        var vst3Configuration=VST3Catalogue.Configuration()
        vst3Configuration.roots=[];vst3Configuration.cacheURL=catalogueFolder.appendingPathComponent("vst3.json")
        let vst3Catalogue=VST3Catalogue(configuration:vst3Configuration)
        measurements["catalogueIsolation"]=["directory":catalogueFolder.path,"knownVST2Aliases":knownBundles,
            "vst3Roots":[],"discovery":"private aliases for DX7 V/Massive X only; targeted Massive X VST3 fallback"]
        let host = try AudioHost(vst3Catalogue:vst3Catalogue,midiInputIDs:[],vst2Catalogue:catalogue)
        var pluginErrors: [[String: Any]] = []
        host.onEvent = { name, value in if name == "plugin.error" { pluginErrors.append(value) } }
        func stage(_ name: String, _ operation: () async throws -> Void) async {
            trace("begin " + name)
            do { try await operation(); checks.append(name); trace("passed " + name) }
            catch { failures.append(name + ": " + error.localizedDescription); trace("FAILED " + name + ": " + error.localizedDescription) }
        }
        func target(_ kind: String = "instrument", slot: Int = 0) -> [String: Any] { ["track": 0, "kind": kind, "slot": slot] }
        func apply(_ project: [String: Any]) async throws {
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
            await host.pluginRestorationBarrier()?.value
        }
        func render(_ name: String) async throws -> Double {
            let url = folder.appendingPathComponent(name + ".wav")
            _ = try await host.handle("audio.export", payload: ["path": url.path, "mode": "song", "tail": 0.25])
            let decoded = try AudioSamples.decode(url.path)
            try require(decoded.left.allSatisfy(\.isFinite) && decoded.right.allSatisfy(\.isFinite), "\(name) rendered non-finite PCM.")
            var sum = 0.0
            for index in decoded.left.indices { sum += Double(decoded.left[index] * decoded.left[index] + decoded.right[index] * decoded.right[index]) }
            let rms = sqrt(sum / Double(max(1, decoded.left.count * 2)))
            measurements[name] = ["rms": rms, "frames": decoded.left.count]
            return rms
        }
        func parameters(_ kind: String = "instrument") async throws -> [[String: Any]] {
            try await host.handle("plugin.parameters", payload: target(kind)) as? [[String: Any]] ?? []
        }

        var rows = [Any](repeating: NSNull(), count: 16)
        rows[0] = ["note": 60, "instrument": 0, "velocity": 110, "gate": 7.8]
        rows[8] = ["note": 67, "instrument": 0, "velocity": 110, "gate": 7.5]
        let base: [String: Any] = ["version": 1, "name": "VST2 native verification", "bpm": 120.0,
            "linesPerBeat": 4, "masterVolume": 0.65, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Instrument", "instrument": 0, "volume": 0.7, "pan": 0, "delaySend": 0, "reverbSend": 0]],
            "instruments": [["type": "au", "volume": 0.7, "pan": 0]],
            "patterns": [["id": 0, "length": 16, "steps": [rows], "automation": []]], "sequence": [0],
            "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]

        var dx7: VST2PluginDescriptor?
        await stage("Private targeted VST2 discovery returns the installed DX7 V instrument") {
            await catalogue.refresh()
            let records = catalogue.records
            measurements["vst2Count"] = records.count
            measurements["vst2Unavailable"] = catalogue.unavailable.count
            try require(records.allSatisfy { descriptor in
                knownBundles[descriptor.bundlePath] == URL(fileURLWithPath:descriptor.bundlePath).resolvingSymlinksInPath().path
            },"Private discovery escaped the explicitly selected installed bundles.")
            measurements["discoveredVST2"] = records.map(\.dictionary)
            measurements["unavailableVST2"] = catalogue.unavailable
            dx7 = records.first { $0.name == "DX7 V" && URL(fileURLWithPath:$0.bundlePath).lastPathComponent == "DX7 V.vst" }
            try require(dx7 != nil, "DX7 V VST2 was not discovered (\(records.count) VST2 classes).")
            let listed = try await host.handle("plugins.list", payload: [:]) as? [[String: Any]] ?? []
            try require(listed.contains { $0["format"] as? String == "VST2" }, "plugins.list omitted VST2 plug-ins.")
        }

        await stage("VST2 instrument plays sequenced MIDI and round-trips its state into a fresh instance") {
            guard let dx7 else { throw AudioSampleError.invalid("No DX7 V.") }
            try await apply(base)
            var payload = target(); payload["component"] = dx7.id
            let loaded = try await host.handle("plugin.load", payload: payload) as? [String: Any] ?? [:]
            try require(loaded["format"] as? String == "VST2" && !(loaded["state"] as? String ?? "").isEmpty, "VST2 load returned no saved state.")
            let all = try await parameters()
            measurements["dx7Parameters"] = all.count
            try require(!all.isEmpty, "The VST2 instrument exposed no parameters.")
            let first = try await render("dx7-default")
            try require(first > 0.0005, "Sequenced MIDI into the VST2 instrument was silent (RMS \(first)).")
            let state = try await host.handle("plugin.state", payload: target()) as? [String: Any] ?? [:]
            _ = try await host.handle("plugin.remove", payload: target())
            var restore = state; restore["track"] = 0; restore["kind"] = "instrument"; restore["slot"] = 0
            _ = try await host.handle("plugin.load", payload: restore)
            let again = try await render("dx7-restored")
            try require(again > 0.0005 && abs(again - first) / first < 0.25, "Restored VST2 instance sounds different (\(first) vs \(again)).")
        }

        await stage("VST2-identity NKS preset loads into the native VST2 plug-in with mapped controls") {
            // An Arturia DX7 V NKS preset ("FM - ANGLE TINE.nksf"), named by SUPERTRACKER_DX7_NKS_PRESET.
            let path = ProcessInfo.processInfo.environment["SUPERTRACKER_DX7_NKS_PRESET"] ?? ""
            guard FileManager.default.fileExists(atPath: path) else { throw AudioSampleError.invalid("Fixture preset missing: set SUPERTRACKER_DX7_NKS_PRESET to an Arturia DX7 V .nksf preset.") }
            try await apply(base)
            let loaded = try await host.handle("plugin.loadNKS", payload: ["track": 0, "kind": "instrument", "path": path]) as? [String: Any] ?? [:]
            try require((loaded["component"] as? String)?.hasPrefix("vst2:") == true, "NKS did not choose the native VST2 (\(loaded["component"] ?? "nil")).")
            let pages = (loaded["nks"] as? [String: Any])?["pages"] as? [[String: Any]] ?? []
            let mapped = pages.flatMap { $0["parameters"] as? [[String: Any]] ?? [] }.filter { $0["mapped"] as? Bool == true }
            measurements["dx7NKSMapped"] = mapped.count
            try require(!mapped.isEmpty, "No NKS controls mapped to VST2 parameters.")
            let rms = try await render("dx7-nks")
            try require(rms > 0.0005, "The NKS preset produced silence.")
        }

        await stage("VST2-identity NKS with no available native VST2 loads the targeted compatible VST3") {
            let path = "/Library/Application Support/Native Instruments/Massive X/Presets/Init_-_Massive_X.nksf"
            guard FileManager.default.fileExists(atPath: path) else { throw AudioSampleError.invalid("Fixture preset missing.") }
            let massiveURL=URL(fileURLWithPath:"/Library/Audio/Plug-Ins/VST3/Massive X.vst3")
            let available=try await vst3Catalogue.scanBundle(massiveURL)
            try require(available.contains{$0.isInstrument},"The targeted Massive X VST3 scan returned no instrument.")
            try await apply(base)
            let loaded = try await host.handle("plugin.loadNKS", payload: ["track": 0, "kind": "instrument", "path": path]) as? [String: Any] ?? [:]
            try require(available.contains{$0.id == loaded["component"] as? String}, "NKS did not choose the targeted compatible VST3.")
            let rms = try await render("massivex-vst2-nks")
            try require(rms > 0.0005, "The compatible VST3 produced silence (RMS \(rms)).")
        }

        await host.shutdownAndWait()
        let report: [String: Any] = ["passed": failures.isEmpty, "checks": checks, "failures": failures, "measurements": measurements, "pluginErrors": pluginErrors,"scope":"installed DX7 V/Massive X bundles only, private injected catalogues/caches, no user catalogue refresh"]
        try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]).write(to: folder.appendingPathComponent("report.json"))
        if !failures.isEmpty { throw AudioSampleError.invalid(failures.joined(separator: "\n")) }
        return checks
    }
}

#endif

// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Regression for AI parameter edits that NI Raum "ignored" on a fresh return bus: Raum queues each write until
/// its next render and posts no parameter-change notification, so Apple's AUv2 bridge kept reporting the value
/// from before the write and every read-back said nothing changed. Loads the real installed AU (default Raum;
/// SUPERTRACKER_DEFERRED_PARAMETER_AU names another "type:subtype:manufacturer") on a return bus with no
/// inputs in a private offline graph. No audio device. Skips when the AU is not installed.
enum NativeDeferredParameterTests {
    @MainActor static func run(output: URL) async throws -> [String] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        func require(_ condition: Bool, _ message: @autoclosure () -> String) throws {
            if !condition { throw AudioSampleError.invalid("Deferred parameters: " + message()) }
        }
        let component = ProcessInfo.processInfo.environment["SUPERTRACKER_DEFERRED_PARAMETER_AU"] ?? "1635083896:1315513425:760105261"
        let parts = component.split(separator: ":").compactMap { UInt32($0) }
        try require(parts.count == 3, "invalid component \(component)")
        let description = AudioComponentDescription(componentType: parts[0], componentSubType: parts[1], componentManufacturer: parts[2],
                                                    componentFlags: 0, componentFlagsMask: 0)
        guard AudioComponentFindNext(nil, [description]) != nil else { return ["SKIP \(component) is not installed"] }

        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, midiInputIDs: [], midiInputSourceProvider: { [] }, midiOutputDestinationProvider: { [] })
        defer { host.shutdown() }
        // A fresh return bus ("Space") with nothing sent to it, as createBus makes it.
        let target: [String: Any] = ["owner": "bus:space", "kind": "effect", "slot": 0]
        let project: [String: Any] = ["id": "deferred-parameters", "version": 1, "name": "Deferred parameters", "bpm": 120.0, "linesPerBeat": 4,
            "tracks": [["name": "Keys", "routingID": "keys", "instrument": 0, "volume": 0.8, "pan": 0]],
            "instruments": [["type": "sample", "name": "Empty"]],
            "patterns": [["id": 0, "name": "A", "length": 16, "steps": [[Any](repeating: NSNull(), count: 16)], "automation": []]],
            "sequence": [0], "activePattern": 0, "audioClips": [],
            "audioPlugins": [target.merging(["component": component, "instanceID": "space-fx"]) { $1 }],
            "routing": ["version": 1, "mixModel": "standard",
                        "buses": [["id": "space", "name": "Space", "type": "return", "volume": 1, "pan": 0, "mute": false, "solo": false]],
                        "outputs": ["track:keys": "master", "bus:space": "master"],
                        "sends": [], "sidechains": []] as [String: Any]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        guard let item = host.rack.instances.values.first(where: { $0.instanceID == "space-fx" }) else {
            throw AudioSampleError.invalid("Deferred parameters: the AU did not load on the return bus")
        }
        try host.start()

        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
        func render(_ quanta: Int) throws {
            for _ in 0..<quanta { try require(try host.renderOfflineQuantum(frames: 512, to: buffer) == .success, "offline render failed") }
        }
        func read() async throws -> [String: Double] {
            let list = try await host.handle("plugin.parameters", payload: target) as? [[String: Any]] ?? []
            return Dictionary(list.compactMap { item in (item["normalized"] as? NSNumber).map { (item["address"] as? String ?? "", $0.doubleValue) } }) { a, _ in a }
        }
        func write(_ values: [(String, Double)]) async throws -> [String: Double] {
            var written: [String: Double] = [:]
            for (address, value) in values {
                let answer = try await host.handle("plugin.parameter", payload: target.merging(["address": address, "value": value, "normalized": true]) { $1 }) as? [String: Any]
                guard let sent = (answer?["written"] as? NSNumber)?.doubleValue else { throw AudioSampleError.invalid("Deferred parameters: plugin.parameter did not report written") }
                written[address] = sent
            }
            return written
        }
        func mismatches(_ wanted: [String: Double], _ now: [String: Double]) -> [String] {
            wanted.keys.sorted().compactMap { key in abs((now[key] ?? -1) - wanted[key]!) <= 1e-3 ? nil : "\(key): wanted \(wanted[key]!), read \(now[key] ?? -1)" }
        }
        var checks: [String] = [], renders = 0
        // AVAudioEngine's prepare() used to cut this insert's output: it rendered never, and applied no queued write.
        let token = item.node.auAudioUnit.token(byAddingRenderObserver: { flags, _, _, _ in if flags.contains(.unitRenderAction_PostRender) { renders += 1 } })
        defer { item.node.auAudioUnit.removeRenderObserver(token) }
        try render(4)
        try require(renders >= 4 && !graph.outputConnectionPoints(for: item.node, outputBus: 0).isEmpty,
                    "the insert on an input-less return bus is not rendered (\(renders) renders) after the graph started")
        checks.append("an insert on a return bus with no inputs stays connected and renders")
        let initial = try await read()
        // Continuous (high-resolution) parameters only: Raum's switches snap a position to their own steps, and Sync
        // moves Predelay, which would make the check about the plug-in's couplings rather than the host.
        let list = try await host.handle("plugin.parameters", payload: target) as? [[String: Any]] ?? []
        let chosen = Array(list.filter {
            ($0["stepCount"] == nil) && ($0["writable"] as? Bool ?? false)
                && (($0["flags"] as? NSNumber)?.uint32Value ?? 0) & AudioUnitParameterOptions.flag_IsHighResolution.rawValue != 0
                && !["-", "", "Predelay"].contains($0["name"] as? String ?? "")
        }.compactMap { $0["address"] as? String }.prefix(8))
        try require(chosen.count >= 4, "expected several continuous parameters, got \(chosen)")
        let burst = chosen.enumerated().map { ($0.element, abs((initial[$0.element] ?? 0) - 0.9) < 0.05 ? 0.15 : 0.9 - Double($0.offset) * 0.07) }
        let written = try await write(burst)
        // Before a render the plug-in may still hold the old values; that is what the API must not call "applied".
        try render(2)
        let after = try await read()
        try require(mismatches(written, after).isEmpty, "a burst read back unchanged after rendering: \(mismatches(written, after))")
        checks.append("a burst of \(burst.count) writes on an input-less return bus reads back after rendering")

        let captured = try await host.handle("plugin.state", payload: target) as? [String: Any]
        guard let state = captured?["state"] as? String else { throw AudioSampleError.invalid("Deferred parameters: no captured state") }
        let other = try await write(chosen.map { ($0, 0.05) })
        try render(2)
        try require(mismatches(other, try await read()).isEmpty, "single follow-up writes did not read back")
        _ = try await host.handle("plugin.state", payload: target.merging(["state": state]) { $1 })
        try render(2)
        let restored = try await read()
        try require(mismatches(written, restored).isEmpty, "the state captured after the burst did not hold it: \(mismatches(written, restored))")
        checks.append("the state captured after rendering holds the burst and restores it")
        return checks
    }
}
#endif

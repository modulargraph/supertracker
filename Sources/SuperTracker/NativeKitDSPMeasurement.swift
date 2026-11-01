// In-app measurement: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import Darwin

/// Render cost of a real imported Maschine group (with its imported built-in Sound and
/// group effects) next to eight empty hardware-template tracks, as percent of real time,
/// through the full offline production graph. Vendor instruments are left out (the
/// hardware editors only send MIDI). No device, MIDI or vendor plug-in participates.
///   --kit-dsp-measurement <file.mxgrp> <library root> <output>   ST_KIT_DSP_SECONDS (default 8), ST_KIT_DSP_BLOCKS (default 128)
enum NativeKitDSPMeasurement {
    @MainActor static func run(group: URL, library: URL, output: URL) async throws -> [String: Any] {
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let imported = try MaschineGroupImporter.read(at: group, root: library)
        guard let kitFragment = imported["project"] as? [String: Any] else { throw AudioSampleError.invalid("The group did not import.") }
        let fragment = try KitGroups.groupProject(fromKitProject: kitFragment)
        guard let members = fragment["tracks"] as? [[String: Any]], let busID = (fragment["groups"] as? [[String: Any]])?.first?["busID"] as? String else {
            throw AudioSampleError.invalid("The imported kit did not convert to a track group.")
        }
        var project = fragment
        var tracks: [[String: Any]] = (0..<8).map { ["routingID": "hw\($0)", "name": "Track \($0 + 1)", "instrument": -1, "volume": 0.8, "pan": 0] }
        tracks += members
        let rows = 64
        var steps = [[Any]](repeating: [Any](repeating: NSNull(), count: rows), count: tracks.count)
        // A basic beat: kick on quarters, snare on backbeats, hats on eighths, other pads sparse.
        for (index, _) in members.enumerated() {
            steps[8 + index] = (0..<rows).map { row -> Any in
                let hit: Bool
                switch index { case 0: hit = row % 4 == 0; case 1: hit = row % 8 == 4; case 2: hit = row % 2 == 0; default: hit = row % 16 == (index * 5) % 16 && index < 8 }
                return hit ? ["note": 60, "velocity": 100, "gate": 1.0] as [String: Any] : NSNull()
            }
        }
        let plugins = (fragment["audioPlugins"] as? [[String: Any]] ?? []).filter { $0["format"] as? String == "Builtin" }
        let bypassed = plugins.filter { $0["bypass"] as? Bool == true }.count
        project["tracks"] = tracks; project["audioPlugins"] = plugins
        project["version"] = 1; project["id"] = "kit-dsp"; project["name"] = "Kit DSP"; project["bpm"] = 120.0; project["linesPerBeat"] = 4
        project["masterVolume"] = 0.5; project["activePattern"] = 0; project["loopSong"] = true; project["sequence"] = [0]
        project["patterns"] = [["id": 0, "length": rows, "steps": steps, "automation": []]]
        project["audioClips"] = []; project["delay"] = ["mix": 0]; project["reverb"] = ["mix": 0]
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, vst3Catalogue: VST3Catalogue(configuration: .init(roots: [], cacheURL: output.appendingPathComponent("vst3.json"))),
            midiInputIDs: [], vst2Catalogue: VST2Catalogue(configuration: .init(roots: [], cacheURL: output.appendingPathComponent("vst2.json"))))
        defer { host.shutdown() }
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
        await host.awaitProjectAssets()
        _ = try await host.handle("transport.play", payload: ["mode": "pattern", "fromStart": true])
        var kinds: [String: Int] = [:]
        for node in graph.attachedNodes {
            kinds[(node as? AVAudioUnit)?.auAudioUnit.audioUnitName ?? String(describing: type(of: node)), default: 0] += 1
        }
        let nodeSummary = kinds.sorted { $0.value > $1.value }.map { "\($0.key)=\($0.value)" }.joined(separator: ", ")
        print("kit dsp: \(group.lastPathComponent) nodes \(graph.attachedNodes.count): \(nodeSummary)")
        let rendered = NativeRenderPerformanceTests.renderedNodes(graph)
        print("kit dsp: \(group.lastPathComponent) rendered nodes \(rendered.count): \(rendered.summary)")
        let environment = ProcessInfo.processInfo.environment
        let seconds = Double(environment["ST_KIT_DSP_SECONDS"] ?? "") ?? 8
        // ST_KIT_DSP_BLOCKS: comma-separated block sizes (default 128).
        let blockSizes = (environment["ST_KIT_DSP_BLOCKS"] ?? "128").split(separator: ",").compactMap { Int($0) }.filter { (1...4096).contains($0) }
        var peak: Float = 0
        var perBlock: [[String: Any]] = []
        for size in blockSizes {
            let block = AVAudioFrameCount(size)
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: block)!
            func render(_ blocks: Int) throws {
                for _ in 0..<blocks {
                    guard try host.renderOfflineQuantum(frames: block, to: buffer) == .success else { throw AudioSampleError.invalid("Render failed.") }
                    for n in 0..<Int(block) { peak = max(peak, abs(buffer.floatChannelData![0][n])) }
                }
            }
            try render(Int(48_000 / Double(block)))   // one second warm-up
            var trials: [Double] = []
            for _ in 0..<5 {
                let blocks = Int(seconds * 48_000 / Double(block) / 5)
                let start = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
                try render(blocks)
                let elapsed = Double(clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - start) / 1e9
                trials.append(100 * elapsed / (Double(blocks) * Double(block) / 48_000))
            }
            trials.sort()
            perBlock.append(["blockFrames": size, "percentRealtimeMedian": trials[2], "percentRealtimeTrials": trials])
            print("kit dsp: \(group.lastPathComponent) members \(members.count) effects \(plugins.count) (bypassed \(bypassed)) block \(size) median \(String(format: "%.2f", trials[2]))% of realtime, peak \(peak)")
        }
        let first = perBlock.first ?? [:]
        let result: [String: Any] = ["group": group.lastPathComponent, "members": members.count, "builtinEffects": plugins.count,
            "bypassedEffects": bypassed, "bus": busID, "blockFrames": first["blockFrames"] ?? 0, "percentRealtimeMedian": first["percentRealtimeMedian"] ?? 0,
            "percentRealtimeTrials": first["percentRealtimeTrials"] ?? [], "blocks": perBlock, "peak": Double(peak),
            "nodes": graph.attachedNodes.count, "nodeKinds": kinds, "renderedNodes": rendered.count]
        if let data = try? JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: output.appendingPathComponent("kit-dsp.json"), options: .atomic)
        }
        return result
    }
}
#endif

import Foundation
import AVFoundation
import TrackerEngine

/// "Render selection to sample" (see AudioRenderSelection.swift). The selection is
/// rendered by a private AudioHost on its own manual-rendering AVAudioEngine, loaded
/// with a copy of the project (captured plug-in state, only the plug-ins on the
/// selected tracks' path). The live graph is never stopped, paused or re-planned, so
/// rendering works the same while stopped and during playback.
///
/// Commands: `render.selection` {pattern, tracks, firstRow, lastRow, mode: master|track,
/// tail, channels, sampleRate, normalize, name} → {path, frames, sampleRate, channels,
/// musicalFrames, tailFrames, tailEnd: silence|maximum, peak, ...};
/// `render.cancelSelection` cancels a running render. Progress: `render.selectionProgress`.
@MainActor final class AudioRenderSelectionJobs {
    static var active: [ObjectIdentifier: AudioRenderSelectionJobs] = [:]
    var cancelled = false
    let id = UUID().uuidString
}

extension AudioHost {
    @MainActor func handleRenderSelectionCommand(_ command: String, _ payload: [String: Any]) async throws -> Any? {
        switch command {
        case "render.selection": return try await renderSelection(payload)
        case "render.cancelSelection":
            let job = AudioRenderSelectionJobs.active[ObjectIdentifier(self)]
            job?.cancelled = true
            return ["cancelled": job != nil]
        default: return nil
        }
    }

    /// Where rendered samples are written: next to recordings, collected into the
    /// project's media folder on Save like any other sample.
    static func renderSelectionDestination(name: String) throws -> URL {
        let readable = name.replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "–", with: "-")
        return try AudioRecordingLocation.destination(["name": readable])
    }

    @MainActor func renderSelection(_ payload: [String: Any], destination: URL? = nil) async throws -> [String: Any] {
        let request = try AudioRenderSelection.request(payload)
        let key = ObjectIdentifier(self)
        guard AudioRenderSelectionJobs.active[key] == nil else { throw AudioSampleError.invalid("A selection render is already running.") }
        let job = AudioRenderSelectionJobs()
        AudioRenderSelectionJobs.active[key] = job
        defer { AudioRenderSelectionJobs.active[key] = nil }
        func progress(_ phase: String, _ value: Double) {
            emit("render.selectionProgress", ["job": job.id, "phase": phase, "progress": max(0, min(1, value))])
        }
        func checkCancelled() throws { if job.cancelled || closed { throw AudioSampleError.invalid("Render cancelled.") } }

        // The current project with fresh plug-in state (no graph pause, as autosave).
        progress("capturing", 0)
        guard let canonical = try await handle("engine.snapshot", payload: [:]) as? [String: Any] else {
            throw AudioSampleError.invalid("The audio engine did not provide the project.")
        }
        try checkCancelled()
        let plan = try AudioRenderSelection.plan(canonical, request: request, sampleRate: sampleRate)
        for plugin in plan.project["audioPlugins"] as? [[String: Any]] ?? [] where plugin["stateError"] != nil {
            throw AudioSampleError.invalid("\(plugin["name"] as? String ?? "A plug-in") could not provide its current state. Retry after its edits finish.")
        }

        let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2)!
        let quantum: AVAudioFrameCount = 1024
        let privateGraph = AVAudioEngine()
        try privateGraph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let renderer = try AudioHost(audioGraph: privateGraph, midiInputIDs: [], midiInputSourceProvider: { [] }, midiOutputDestinationProvider: { [] })
        var shutDown = false
        func close() async { if !shutDown { shutDown = true; await renderer.shutdownAndWait() } }
        do {
            progress("loading", 0.02)
            _ = try await renderer.handle("engine.project", payload: ["project": plan.project, "replace": true, "awaitPlugins": true])
            await renderer.awaitProjectAssets()
            try checkCancelled()
            if let failure = renderer.routingFailure { throw AudioSampleError.invalid("The selection's routing cannot be rendered: " + failure) }
            if renderer.unavailableProject != nil { throw AudioSampleError.invalid("The selection's routing cannot be rendered.") }
            let loaded = renderer.rack.snapshot(fresh: false).count
            guard loaded >= plan.keptPlugins else {
                throw AudioSampleError.invalid("\(plan.keptPlugins - loaded) plug-in\(plan.keptPlugins - loaded == 1 ? "" : "s") on the selection's signal path could not be loaded for the render.")
            }
            renderer.nativeSourceUsesHostClock = false
            // Keeps the private host's maintenance timer from restarting its graph mid-render.
            renderer.exporting = true
            renderer.graph.reset(); try renderer.resetRenderCache(); st_engine_panic(renderer.engine)
            st_engine_seek(renderer.engine, Int32(clamping: plan.pattern), Int32(request.firstRow), 0)
            st_engine_play(renderer.engine, 1)
            renderer.graph.prepare(); renderer.rack.refreshLatencyCompensation()
            guard !renderer.rack.latencyExceedsLimit else {
                throw AudioSampleError.invalid("This signal path exceeds the two-second latency compensation limit.")
            }
            let compensation = Int64(renderer.rack.compensationLatencyFrames)
            renderer.rack.resetRenderFailures()
            try renderer.graph.start()

            let threshold = AudioRenderSelection.linear(dB: request.thresholdDB)
            let hold = Int64((request.holdSeconds * sampleRate).rounded())
            let maximumTail = Int64((request.maximumTail * sampleRate).rounded())
            var left: [Float] = [], right: [Float] = []
            let expected = Int((plan.musicalFrames ?? Int64(sampleRate * 4)) + maximumTail / 4)
            left.reserveCapacity(expected); right.reserveCapacity(expected)
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: quantum) else { throw AudioSampleError.invalid("Cannot allocate the render buffer.") }
            var graphFrames: Int64 = 0, lastLoud: Int64 = -1, retries = 0
            var musicalFrames = plan.musicalFrames, decision = AudioRenderSelection.TailDecision.render
            // A tempo-mapped selection still ends: bound it by the longest allowed range.
            let hardLimit = Int64(AudioRenderSelection.maximumMusicalSeconds * sampleRate) + maximumTail + hold + compensation + Int64(quantum)
            var lastYield = 0 as Int64
            while decision == .render {
                try checkCancelled()
                guard graphFrames < hardLimit else { throw AudioSampleError.invalid("The selection did not reach its end. The render stopped.") }
                let status = try renderer.renderOfflineQuantum(frames: quantum, to: buffer)
                if let failure = renderer.rack.renderFailure() {
                    throw AudioSampleError.invalid((failure["message"] as? String ?? "A plug-in failed to render.") + " The render stopped.")
                }
                switch status {
                case .success: retries = 0
                case .cannotDoInCurrentContext, .insufficientDataFromInputNode:
                    retries += 1
                    if retries > 100 { throw AudioSampleError.invalid("A plug-in could not render offline.") }
                    continue
                case .error: throw AudioSampleError.invalid("Audio rendering failed.")
                @unknown default: throw AudioSampleError.invalid("Unknown offline rendering status.")
                }
                let count = Int64(buffer.frameLength)
                let skip = Int(max(0, min(count, compensation - graphFrames)))
                graphFrames += count
                if Int(count) > skip, let data = buffer.floatChannelData {
                    let start = Int64(left.count)
                    let l = UnsafeBufferPointer(start: data[0] + skip, count: Int(count) - skip)
                    let r = UnsafeBufferPointer(start: data[1] + skip, count: Int(count) - skip)
                    left.append(contentsOf: l); right.append(contentsOf: r)
                    let loud = AudioRenderSelection.lastLoud(l, r, threshold: threshold)
                    if loud >= 0 { lastLoud = start + Int64(loud) }
                }
                var state = st_engine_status(renderer.engine)
                let stopped = st_status_transport_playing(&state) == 0
                if stopped, musicalFrames == nil { musicalFrames = max(0, graphFrames - compensation) }
                let musical = musicalFrames ?? Int64.max
                decision = AudioRenderSelection.tailDecision(rendered: Int64(left.count), lastLoud: lastLoud, musicalFrames: musical,
                    musicalEnded: stopped || Int64(left.count) >= musical, holdFrames: hold, maximumTailFrames: maximumTail)
                if graphFrames - lastYield >= 32_768 {
                    lastYield = graphFrames
                    let total = Double(musical == Int64.max ? Int64(left.count) * 2 : musical + maximumTail)
                    progress("rendering", 0.05 + 0.85 * min(1, Double(left.count) / max(1, total)))
                    await Task.yield()
                }
            }
            renderer.graph.stop()
            await close()
            try checkCancelled()

            progress("writing", 0.92)
            let keep: Int64, tailEnd: String
            switch decision {
            case .silent(let frames): keep = frames; tailEnd = "silence"
            case .maximum(let frames): keep = frames; tailEnd = "maximum"
            case .render: keep = Int64(left.count); tailEnd = "silence" // not reached: the loop ends on a decision
            }
            let fade = tailEnd == "maximum" ? Int(sampleRate * 0.01) : 0
            var output = AudioRenderSelection.finish(left: left, right: right, frames: Int(keep), fadeFrames: fade,
                                                     channels: request.channels, normalize: request.normalize)
            left = []; right = []
            guard !output.channels.isEmpty, !output.channels[0].isEmpty, output.peak > 0 else {
                throw AudioSampleError.invalid("The selection rendered silence. Check the tracks' mute, routing and instruments.")
            }
            if request.sampleRate != Int(sampleRate) {
                output.channels = try Self.resample(output.channels, from: sampleRate, to: Double(request.sampleRate))
            }
            let url = try destination ?? Self.renderSelectionDestination(name: request.name)
            try Self.writeFloatWAV(output.channels, sampleRate: Double(request.sampleRate), to: url)
            let musical = musicalFrames ?? keep
            var result: [String: Any] = ["path": url.path, "name": request.name, "frames": output.channels[0].count,
                "sampleRate": request.sampleRate, "channels": output.channels.count, "musicalFrames": musical,
                "renderedFrames": keep, "tailFrames": max(0, keep - musical), "tailEnd": tailEnd, "peak": output.peak,
                "normalizeGain": output.gain, "mode": request.throughMaster ? "master" : "track",
                "latencyFrames": compensation, "plugins": plan.keptPlugins, "rootNote": 60]
            if !plan.warnings.isEmpty { result["warnings"] = plan.warnings }
            progress("done", 1)
            return result
        } catch {
            renderer.graph.stop()
            await close()
            throw error
        }
    }

    static func resample(_ channels: [[Float]], from source: Double, to target: Double) throws -> [[Float]] {
        guard let input = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: source, channels: AVAudioChannelCount(channels.count), interleaved: false),
              let output = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: target, channels: AVAudioChannelCount(channels.count), interleaved: false),
              let converter = AVAudioConverter(from: input, to: output),
              let inBuffer = AVAudioPCMBuffer(pcmFormat: input, frameCapacity: AVAudioFrameCount(channels[0].count)) else {
            throw AudioSampleError.invalid("Cannot convert the render's sample rate.")
        }
        converter.sampleRateConverterQuality = AVAudioQuality.max.rawValue
        converter.sampleRateConverterAlgorithm = AVSampleRateConverterAlgorithm_Mastering
        inBuffer.frameLength = AVAudioFrameCount(channels[0].count)
        for (index, channel) in channels.enumerated() { channel.withUnsafeBufferPointer { inBuffer.floatChannelData![index].update(from: $0.baseAddress!, count: channel.count) } }
        let capacity = AVAudioFrameCount(Double(channels[0].count) * target / source) + 4096
        guard let outBuffer = AVAudioPCMBuffer(pcmFormat: output, frameCapacity: capacity) else { throw AudioSampleError.invalid("Cannot allocate the converted render.") }
        var supplied = false, failure: NSError?
        let status = converter.convert(to: outBuffer, error: &failure) { _, state in
            if supplied { state.pointee = .endOfStream; return nil }
            supplied = true; state.pointee = .haveData; return inBuffer
        }
        guard status != .error, failure == nil else { throw AudioSampleError.invalid("Sample-rate conversion failed: \(failure?.localizedDescription ?? "unknown error").") }
        return (0..<channels.count).map { Array(UnsafeBufferPointer(start: outBuffer.floatChannelData![$0], count: Int(outBuffer.frameLength))) }
    }

    /// 32-bit float WAV (keeps peaks above 0 dBFS when not normalized).
    static func writeFloatWAV(_ channels: [[Float]], sampleRate: Double, to url: URL) throws {
        let count = channels.first?.count ?? 0
        let settings: [String: Any] = [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: sampleRate,
            AVNumberOfChannelsKey: channels.count, AVLinearPCMBitDepthKey: 32, AVLinearPCMIsFloatKey: true,
            AVLinearPCMIsBigEndianKey: false, AVLinearPCMIsNonInterleaved: false]
        let partial = url.deletingLastPathComponent().appendingPathComponent("." + url.lastPathComponent + ".partial")
        try? FileManager.default.removeItem(at: partial)
        do {
            let file = try AVAudioFile(forWriting: partial, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false)
            guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 65_536) else { throw AudioSampleError.invalid("Cannot allocate the sample writer.") }
            var offset = 0
            while offset < count {
                let frames = min(65_536, count - offset)
                buffer.frameLength = AVAudioFrameCount(frames)
                for (index, channel) in channels.enumerated() {
                    channel.withUnsafeBufferPointer { buffer.floatChannelData![index].update(from: $0.baseAddress! + offset, count: frames) }
                }
                try file.write(from: buffer)
                offset += frames
            }
            if #available(macOS 15.0, *) { file.close() }
        }
        try FileManager.default.moveItem(at: partial, to: url)
    }
}

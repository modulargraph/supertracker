import AVFoundation
import Foundation

// Standalone: swiftc -O Sources/SuperTracker/AudioPreview.swift tests/AudioPreviewTests.swift -o /tmp/audio-preview-tests
// Renders previews offline only (manual-rendering AVAudioEngine, never the output device).
@main
struct AudioPreviewTests {
    static let rate = 48_000.0

    static func check(_ condition: @autoclosure () -> Bool, _ message: String) {
        if !condition() { print("FAIL " + message); exit(1) }
    }

    static func clip(frames: Int, _ value: (Int) -> Float) -> PreviewPCM {
        let pcm = PreviewPCM(capacity: frames, estimatedTotalFrames: frames, duration: Double(frames) / rate, sampleRate: rate)
        var l = (0..<frames).map(value), r = l
        l.withUnsafeBufferPointer { lp in r.withUnsafeBufferPointer { rp in _ = pcm.append(lp.baseAddress!, rp.baseAddress!, frames) } }
        r.removeAll()
        return pcm
    }

    static func sine(_ frequency: Double, amplitude: Float = 0.9, phase: Double = 0, frames: Int = 48_000) -> PreviewPCM {
        clip(frames: frames) { amplitude * Float(sin(2 * .pi * frequency * Double($0) / rate + phase)) }
    }

    /// Renders `blocks` blocks of `size` frames, running `actions[block]` before each block.
    static func render(_ mixer: PreviewVoiceMixer, blocks: Int, size: Int = 256, actions: [Int: () -> Void] = [:]) -> [Float] {
        var out: [Float] = [], l = [Float](repeating: 0, count: size), r = l
        for block in 0..<blocks {
            actions[block]?()
            for i in 0..<size { l[i] = 0; r[i] = 0 }
            l.withUnsafeMutableBufferPointer { lp in r.withUnsafeMutableBufferPointer { rp in _ = mixer.render(frames: size, left: lp.baseAddress!, right: rp.baseAddress!) } }
            out += l
        }
        return out
    }

    static func maxJump(_ samples: [Float]) -> Float {
        var worst: Float = 0
        for i in 1..<samples.count { worst = max(worst, abs(samples[i] - samples[i - 1])) }
        return worst
    }

    static func ms(_ start: UInt64) -> Double {
        var base = mach_timebase_info_data_t(); mach_timebase_info(&base)
        return Double(mach_absolute_time() - start) * Double(base.numer) / Double(base.denom) / 1e6
    }

    static func writeSine(_ url: URL, rate fileRate: Double, seconds: Double, frequency: Double, channels: AVAudioChannelCount = 2, phase: Double = 0) throws {
        let format = AVAudioFormat(standardFormatWithSampleRate: fileRate, channels: channels)!
        let file = try AVAudioFile(forWriting: url, settings: [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: fileRate, AVNumberOfChannelsKey: channels, AVLinearPCMBitDepthKey: 32, AVLinearPCMIsFloatKey: true], commonFormat: .pcmFormatFloat32, interleaved: false)
        let frames = AVAudioFrameCount(fileRate * seconds)
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)!
        buffer.frameLength = frames
        for c in 0..<Int(channels) { for i in 0..<Int(frames) { buffer.floatChannelData![c][i] = 0.8 * Float(sin(2 * .pi * frequency * Double(i) / fileRate + phase)) } }
        try file.write(from: buffer)
    }

    static func main() async throws {
        setvbuf(stdout, nil, _IOLBF, 0)
        // Natural slope of a 0.9 sine at 440 Hz is 0.052 per sample; a 2 ms ramp adds ≤ 0.9/96.

        // 1. Switching previews crossfades instead of cutting.
        do {
            let a = sine(440), b = sine(440, phase: .pi) // b starts at full opposite polarity mid-cycle
            // Hard cut reference: concatenate a (to a peak) and b (from its peak).
            let cutAt = 10 * 256 + 27
            var hard = (0..<cutAt).map { a.left[$0] }
            hard += (0..<2000).map { b.left[$0 + 60] }
            let mixer = PreviewVoiceMixer(sampleRate: rate)
            let out = render(mixer, blocks: 40, actions: [0: { mixer.start(a, volume: 1) }, 10: { mixer.start(b, volume: 1) }, 20: { mixer.start(sine(440, phase: 1.3), volume: 1) }])
            check(maxJump(hard) > 0.5, "the hard-cut reference must show a discontinuity (\(maxJump(hard)))")
            // During an overlap two 440 Hz signals may move together: bound = 2 × slope + ramp slope.
            check(maxJump(out) < 2 * 0.052 + 0.9 / Float(mixer.fadeInFrames) + 0.002, "crossfaded switch max jump \(maxJump(out)) must stay near the signal slope")
            check(mixer.activeVoices == 1, "faded voices retire")
            print("PASS crossfade switch: max sample jump \(String(format: "%.4f", maxJump(out))) (hard cut reference \(String(format: "%.3f", maxJump(hard))))")
        }

        // 2. Starting from silence on a DC-offset clip and stopping are ramped.
        do {
            let dc = clip(frames: 24_000) { _ in 0.8 }
            let mixer = PreviewVoiceMixer(sampleRate: rate)
            let out = render(mixer, blocks: 30, actions: [2: { mixer.start(dc, volume: 1) }, 12: { mixer.stop() }])
            let jump = maxJump(out)
            check(jump <= 0.8 / Float(mixer.fadeInFrames) + 1e-5, "DC fade-in jump \(jump)")
            check(out[13 * 256 + 64] == 0 || out[12 * 256 + mixer.fadeOutFrames + 1] == 0, "stop fades to silence within the fade-out")
            check(mixer.activeVoices == 0, "stopped voice retires")
            let hard = render(mixer, blocks: 4, actions: [0: { mixer.start(dc, volume: 1) }, 2: { mixer.stop(fade: false) }])
            check(hard[2 * 256] == 0 && mixer.activeVoices == 0, "an immediate stop silences at once")
            print("PASS fade-in from silence and fade-out on stop (max jump \(String(format: "%.5f", jump)))")
        }

        // 3. Rapid switching never exceeds the voice pool or clicks.
        do {
            let mixer = PreviewVoiceMixer(sampleRate: rate)
            var actions: [Int: () -> Void] = [:]
            // A start every 128 frames (2.7 ms, far faster than any UI debounce), random phases:
            // at most five voices overlap, each moving ≤ 0.5·2π·440/48000 = 0.029 per sample.
            for block in 0..<60 { actions[block] = { mixer.start(sine(440, amplitude: 0.5, phase: Double(block) * 2.1), volume: 1) } }
            let out = render(mixer, blocks: 70, size: 128, actions: actions)
            check(maxJump(out) < 5 * 0.029 + 0.5 / Float(mixer.fadeInFrames) + 0.01, "rapid switching max jump \(maxJump(out))")
            check(mixer.activeVoices <= PreviewVoiceMixer.voiceCount, "voice pool bounded")
            print("PASS rapid switching (60 starts in 160 ms): max jump \(String(format: "%.4f", maxJump(out)))")
        }

        // 4. Head plus tail stream play as one continuous signal.
        do {
            let total = 30_000, head = sine(300, frames: 10_000)
            let full = (0..<total).map { 0.9 * Float(sin(2 * .pi * 300 * Double($0) / rate)) }
            let stream = PreviewStream(estimatedFrames: total - 10_000, chunkFrames: 4096)
            let tail = Array(full[10_000...])
            tail.withUnsafeBufferPointer { p in _ = stream.append(p.baseAddress!, p.baseAddress!, 7000); _ = stream.append(p.baseAddress! + 7000, p.baseAddress! + 7000, tail.count - 7000) }
            stream.finish()
            let mixer = PreviewVoiceMixer(sampleRate: rate, fadeInSeconds: 1 / rate)
            mixer.start(head, stream: stream, volume: 1)
            let out = render(mixer, blocks: 130)
            var diff: Float = 0
            for i in 1..<total { diff = max(diff, abs(out[i] - full[i])) }
            check(diff < 1e-5, "head/tail playback matches the source (\(diff))")
            check(out[total + 10] == 0 && mixer.activeVoices == 0, "voice ends at the stream end")
            check(stream.readPosition >= total - 10_000, "reader position published")
            print("PASS head + streamed tail continuity")
        }

        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("st-preview-\(getpid())", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }

        // 5. Decoded head + tail equals one full decode across a 44.1 → 48 kHz conversion.
        do {
            let url = folder.appendingPathComponent("long.wav")
            try writeSine(url, rate: 44_100, seconds: 10, frequency: 523)
            let head = try PreviewDecoder.head(path: url.path, targetRate: rate, headSeconds: 4, fullSeconds: 8, isCancelled: { false })
            check(!head.complete && head.frames == 4 * 48_000, "a 10 s file caches a 4 s head (\(head.frames))")
            let stream = PreviewStream(estimatedFrames: head.estimatedTotalFrames - head.frames + 48_000)
            PreviewDecoder.streamTail(path: url.path, after: head, into: stream, lead: 60)
            check(stream.finished, "tail decode finishes")
            var full: [Float] = []
            let file = try AVAudioFile(forReading: url)
            try PreviewDecoder.decode(file: file, targetRate: rate, skip: 0, limit: .max, isCancelled: { false }) { l, _, n in full += UnsafeBufferPointer(start: l, count: n); return true }
            let mixer = PreviewVoiceMixer(sampleRate: rate, fadeInSeconds: 1 / rate)
            mixer.start(head, stream: stream, volume: 1)
            let out = render(mixer, blocks: full.count / 256)
            var diff: Float = 0
            for i in 1..<out.count { diff = max(diff, abs(out[i] - full[i])) }
            let boundary = Array(out[(head.frames - 32)..<(head.frames + 32)])
            check(diff < 1e-5, "streamed tail continues the converted head sample-exactly (\(diff))")
            check(maxJump(boundary) < 0.8 * 2 * .pi * 523 / 48_000 + 0.01, "no discontinuity at the head/tail seam")
            print("PASS 44.1 kHz file: 4 s head + streamed tail == full decode (max diff \(diff))")
        }

        // 6. Cache: hits, LRU memory cap, offline volumes and missing files.
        do {
            var paths: [String] = []
            for index in 0..<5 {
                let url = folder.appendingPathComponent("one-shot-\(index).wav")
                try writeSine(url, rate: 48_000, seconds: 1, frequency: 200 + Double(index) * 50, channels: index == 0 ? 1 : 2)
                paths.append(url.path)
            }
            // ~1 s at 48 kHz stereo float with headroom ≈ 0.42 MB each; cap 1.2 MB holds two.
            let cache = PreviewCache(capacityBytes: 1_200_000)
            var t = mach_absolute_time()
            let first = try await cache.head(paths[0], rate: rate, priority: .userInitiated)
            let missMs = ms(t)
            check(!first.cached && first.pcm.complete, "first request decodes")
            check(first.pcm.left[100] == first.pcm.right[100], "mono duplicates to both channels")
            var hitTimes: [Double] = []
            for _ in 0..<20 { t = mach_absolute_time(); let again = try await cache.head(paths[0], rate: rate, priority: .userInitiated); hitTimes.append(ms(t)); check(again.cached, "second request hits") }
            hitTimes.sort()
            for path in paths[1...] { _ = try await cache.head(path, rate: rate, priority: .utility) }
            check(cache.bytes <= 1_200_000 && cache.count <= 2, "memory cap evicts least recently used (\(cache.bytes) bytes, \(cache.count) entries)")
            let evicted = try await cache.head(paths[0], rate: rate, priority: .userInitiated)
            check(!evicted.cached, "evicted entry decodes again")
            t = mach_absolute_time()
            do { _ = try await cache.head("/Volumes/SuperTrackerMissingDrive-\(getpid())/a.wav", rate: rate, priority: .utility); check(false, "offline volume must fail") }
            catch PreviewDecodeError.offline {}
            check(ms(t) < 500, "offline volumes fail fast")  // generous: the suite runs jobs in parallel
            do { _ = try await cache.head(folder.appendingPathComponent("missing.wav").path, rate: rate, priority: .utility); check(false, "missing file must fail") } catch {}
            check(cache.recentlyFailed(folder.appendingPathComponent("missing.wav").path), "missing files are not retried in a loop")
            // Concurrent requests for one key share a decode.
            let shared = PreviewCache()
            let sharedPath = paths[3]
            async let x = shared.head(sharedPath, rate: rate, priority: .utility)
            async let y = shared.head(sharedPath, rate: rate, priority: .userInitiated)
            let (rx, ry) = try await (x, y)
            check(rx.pcm === ry.pcm && shared.misses == 1, "in-flight decodes are shared")
            print("PASS cache: miss \(String(format: "%.2f", missMs)) ms, hit median \(String(format: "%.3f", hitTimes[hitTimes.count / 2])) ms, LRU cap, offline fast-fail, negative cache, shared in-flight decode")
        }

        // 7. Prefetch order: nearest first, biased toward the direction of travel.
        check(PreviewPrefetchPlan.order(selected: 5, count: 20, direction: 1, radius: 3) == [5, 6, 4, 7, 3, 8, 2], "forward order")
        check(PreviewPrefetchPlan.order(selected: 0, count: 3, direction: -1, radius: 3) == [0, 1, 2], "edge order")
        print("PASS prefetch order")

        // 8. Before/after in a real (offline, manual-rendering) AVAudioEngine:
        //    old path = open file + pause graph + reconnect player + schedule + start + play, hard stop on switch;
        //    new path = cached head + persistent source node crossfade.
        do {
            let a = folder.appendingPathComponent("a.wav"), b = folder.appendingPathComponent("b.wav")
            try writeSine(a, rate: 44_100, seconds: 3, frequency: 440)
            try writeSine(b, rate: 44_100, seconds: 3, frequency: 440, phase: .pi / 2) // b starts at its peak
            let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
            func engine() throws -> AVAudioEngine {
                let engine = AVAudioEngine()
                try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 512)
                return engine
            }
            func pull(_ engine: AVAudioEngine, _ frames: Int, into out: inout [Float]) throws {
                let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
                var left = frames
                while left > 0 {
                    let n = min(512, left)
                    _ = try engine.renderOffline(AVAudioFrameCount(n), to: buffer)
                    out += UnsafeBufferPointer(start: buffer.floatChannelData![0], count: Int(buffer.frameLength))
                    left -= n
                }
            }
            func firstSound(_ samples: [Float], from: Int) -> Int { (from..<samples.count).first { abs(samples[$0]) > 1e-4 }.map { $0 - from } ?? -1 }

            // Old path.
            let old = try engine(), player = AVAudioPlayerNode()
            old.attach(player); old.connect(player, to: old.mainMixerNode, format: format)
            old.mainMixerNode.outputVolume = 1
            var oldOut: [Float] = [], oldTimes: [Double] = []
            func oldPreview(_ url: URL) throws {
                let t = mach_absolute_time()
                let file = try AVAudioFile(forReading: url)
                player.stop(); old.pause(); old.disconnectNodeOutput(player); old.connect(player, to: old.mainMixerNode, format: file.processingFormat)
                player.scheduleFile(file, at: nil, completionHandler: nil)
                if !old.isRunning { try old.start() }
                player.play()
                oldTimes.append(ms(t))
            }
            try old.start()
            try oldPreview(a); let oldStartA = oldOut.count; try pull(old, 12_000, into: &oldOut)
            let oldSwitch = oldOut.count; try oldPreview(b); try pull(old, 12_000, into: &oldOut)
            let oldSeam = Array(oldOut[(oldSwitch - 64)..<(oldSwitch + 256)])

            // New path.
            let new = try engine(), preview = AudioPreviewPlayer(format: format)
            new.attach(preview.node); new.connect(preview.node, to: new.mainMixerNode, format: format)
            try new.start()
            var newOut: [Float] = [], newTimes: [Double] = [], coldTimes: [Double] = []
            func newPreview(_ url: URL, cold: Bool = false) async throws {
                let t = mach_absolute_time()
                guard let loaded = try await preview.load(url.path) else { return }
                preview.begin(url.path, pcm: loaded.pcm, volume: 1, began: loaded.started)
                (cold ? { coldTimes.append(ms(t)) } : { newTimes.append(ms(t)) })()
            }
            try await newPreview(a, cold: true); try pull(new, 512, into: &newOut); preview.stop(fade: false); newOut.removeAll()
            _ = preview.prefetch([["path": b.path]])
            for _ in 0..<200 where preview.cache.count < 2 { try await Task.sleep(nanoseconds: 2_000_000) }
            check(preview.cache.count == 2, "prefetch warms the neighbour")
            try await newPreview(a); let newStartA = newOut.count; try pull(new, 12_000, into: &newOut)
            let newSwitch = newOut.count; try await newPreview(b); try pull(new, 12_000, into: &newOut)
            let newSeam = Array(newOut[(newSwitch - 64)..<(newSwitch + 1024)])
            for _ in 0..<10 { try await newPreview(b); try await newPreview(a) }
            for _ in 0..<10 { try oldPreview(b); try oldPreview(a) }
            oldTimes.sort(); newTimes.sort()

            let oldJump = maxJump(oldSeam), newJump = maxJump(newSeam)
            print(String(format: "MEASURE start cost: old median %.2f ms (file open + graph pause/reconnect), new cached median %.3f ms, new uncached %.2f ms", oldTimes[oldTimes.count / 2], newTimes[newTimes.count / 2], coldTimes[0]))
            print(String(format: "MEASURE first audible frame after start: old %d, new %d (both within the next render quantum)", firstSound(oldOut, from: oldStartA), firstSound(newOut, from: newStartA)))
            print(String(format: "MEASURE switch seam max sample jump: old %.3f, new %.4f (signal slope bound %.4f)", oldJump, newJump, 0.8 * 2 * Float.pi * 440 / 48_000))
            check(newJump < 0.1, "new path switches without a discontinuity (\(newJump))")
            check(newTimes[newTimes.count / 2] < 2, "cached previews start within a couple of milliseconds")
            check(firstSound(newOut, from: newStartA) >= 0 && firstSound(newOut, from: newStartA) < 512, "cached preview is audible in the first render quantum")
            print("PASS offline engine before/after")
        }
        // 9. Optional: command cost against the real output device, with the engine muted
        //    (mainMixer volume 0, preview volume 0), so nothing is audible.
        if ProcessInfo.processInfo.environment["SUPERTRACKER_PREVIEW_REALTIME"] == "1" {
            let a = folder.appendingPathComponent("a.wav"), b = folder.appendingPathComponent("b.wav")
            let engine = AVAudioEngine(), player = AVAudioPlayerNode()
            let format = AVAudioFormat(standardFormatWithSampleRate: engine.outputNode.outputFormat(forBus: 0).sampleRate, channels: 2)!
            let preview = AudioPreviewPlayer(format: format)
            engine.attach(player); engine.connect(player, to: engine.mainMixerNode, format: format)
            engine.attach(preview.node); engine.connect(preview.node, to: engine.mainMixerNode, format: format)
            engine.mainMixerNode.outputVolume = 0; player.volume = 0
            try engine.start()
            var oldTimes: [Double] = [], newTimes: [Double] = []
            for index in 0..<12 {
                let url = index % 2 == 0 ? a : b
                var t = mach_absolute_time()
                let file = try AVAudioFile(forReading: url)
                player.stop(); engine.pause(); engine.disconnectNodeOutput(player); engine.connect(player, to: engine.mainMixerNode, format: file.processingFormat)
                player.scheduleFile(file, at: nil, completionHandler: nil); try engine.start(); player.play()
                oldTimes.append(ms(t))
                player.stop()
                t = mach_absolute_time()
                if let loaded = try await preview.load(url.path) { preview.begin(url.path, pcm: loaded.pcm, volume: 0, began: loaded.started) }
                newTimes.append(ms(t))
                try await Task.sleep(nanoseconds: 20_000_000)
            }
            preview.stop(fade: false); engine.stop()
            oldTimes.sort(); newTimes.sort()
            print(String(format: "MEASURE realtime device (muted): old median %.2f ms (max %.2f), new median %.3f ms (max %.3f) + one IO buffer (%.1f ms at 512 frames)",
                         oldTimes[6], oldTimes.last!, newTimes[6], newTimes.last!, 512 / format.sampleRate * 1000))
        }
        print("PASS audio preview tests")
    }
}

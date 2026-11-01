// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox

/// Generated source-clock PCM and private offline graphs only. Wall-clock waits
/// let asynchronous writers settle; they never determine the recorded interval.
enum NativeRecordingFenceTests {
    private static let rate = 48_000.0

    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("recording-fences", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], observations: [[String: Any]] = []
        var completed = false
        func writeProgress() {
            let report: [String: Any] = ["passed": completed, "checks": checks,
                "observations": observations, "sampleRate": rate, "physicalPlayback": false,
                "physicalMIDI": false, "microphoneAccess": false, "vendorPlugins": false]
            if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        func recordCheck(_ check: String) {
            checks.append(check)
            writeProgress()
        }
        defer { writeProgress() }
        writeProgress()

        // An old queued tap may arrive before the first source quantum. A later
        // 4,800-frame packet contains both fences of a 64-frame recording.
        do {
            let url = folder.appendingPathComponent("short-interval.wav")
            let capture = try recorder(url: url, epoch: 1)
            capture.receive(try generated(start: -4800, count: 4800), at: time(-4800))
            let beforeStart = await capture.progress()
            try require(beforeStart.frames == 0 && beforeStart.startFrame == nil && beforeStart.error == nil,
                "A delayed packet before clock acceptance must not establish the recording start.")
            capture.observeManualQuantum(frame: 1024, frames: 64)
            capture.requestStop()
            capture.observeManualQuantum(frame: 1088, frames: 64)
            let pending = await capture.progress()
            try require(pending.startFrame == 1024 && pending.endFrame == 1088 && !pending.complete && pending.frames == 0,
                "An accepted short interval must still wait for its actual tap watermark.")
            capture.receive(try generated(start: 0, count: 4800), at: time(0))
            let final = await capture.finish()
            try require(final.error == nil && final.complete && final.frames == 64 && final.receivedThrough == 4800,
                "A 4,800-frame tap packet must select exactly the accepted 64 source frames.")
            observations.append(try verifyGenerated(url: url, start: 1024, count: 64, progress: final))
            recordCheck("A 64-frame interval wholly inside a delayed 4,800-frame packet preserves exact first/last PCM and excludes pre-start/post-stop audio")
        }

        do {
            let url=folder.appendingPathComponent("contended-boundary.wav")
            let format=AVAudioFormat(standardFormatWithSampleRate:rate,channels:2)!
            let boundary=try AudioRecordingBoundary(sampleRate:rate,epoch:40)
            var file: AVAudioFile? = try AVAudioFile(forWriting:url,settings:[AVFormatIDKey:kAudioFormatLinearPCM,
                AVSampleRateKey:rate,AVNumberOfChannelsKey:2,AVLinearPCMBitDepthKey:32,AVLinearPCMIsFloatKey:true],
                commonFormat:.pcmFormatFloat32,interleaved:false)
            var reads=0
            let capture=try AudioRecording(source:.master,file:file!,path:url.path,instrument:nil,
                channels:AudioRecordingChannelPlan.master(sourceFormat:format),boundary:boundary,manualClock:true,
                boundarySnapshot:{ reads += 1; return reads <= 4 ? nil : boundary.trySnapshot() })
            // Give the recorder sole writer ownership, just like recorder(...).
            // Its final barrier can then close the WAV before verification.
            file = nil
            capture.observeManualQuantum(frame:1024,frames:64); capture.requestStop()
            capture.observeManualQuantum(frame:1088,frames:64)
            capture.receive(try generated(start:0,count:4800),at:time(0))
            let complete=await capture.progress()
            try require(complete.complete && complete.error == nil,"The exact interval must be written before the late interruption fixture.")
            capture.fail(failure("Injected configuration change after the accepted end."))
            let final=await capture.finish(interruption:failure("Injected shutdown after the accepted end."))
            try require(final.error == nil && final.complete && final.frames == 64 && reads >= 5,
                "Transient seqlock contention must retain the packet until its fresh end fence is readable.")
            observations.append(try verifyGenerated(url:url,start:1024,count:64,progress:final))
            recordCheck("Transient source-boundary publication races retain exact PCM and interruptions after the completed interval cannot mark it partial")
        }

        // Start midway through one packet, then accept Stop before the next tap
        // arrives. Its post-stop suffix must never reach the file.
        do {
            let url = folder.appendingPathComponent("pending-tail.wav")
            let capture = try recorder(url: url, epoch: 2)
            capture.observeManualQuantum(frame: 1024, frames: 3776)
            capture.receive(try generated(start: 0, count: 4800), at: time(0))
            let prefix = await capture.progress()
            try require(prefix.frames == 3776 && prefix.receivedThrough == 4800 && prefix.endFrame == nil && !prefix.complete,
                "The first packet must write only the source interval after Start.")
            capture.observeManualQuantum(frame: 4800, frames: 1344)
            capture.requestStop()
            capture.observeManualQuantum(frame: 6144, frames: 64)
            let pending = await capture.progress()
            try require(pending.error == nil && pending.endFrame == 6144 && pending.frames == 3776 && !pending.complete,
                "Stop acknowledgement must not report completion while the source tail is missing.")
            capture.receive(try generated(start: 4800, count: 4800), at: time(4800))
            let final = await capture.finish()
            try require(final.error == nil && final.complete && final.frames == 5120 && final.receivedThrough == 9600,
                "The delayed final batch must complete exactly [1024,6144).")
            observations.append(try verifyGenerated(url: url, start: 1024, count: 5120, progress: final))
            recordCheck("Stop remains pending until the timestamped tail arrives and crops both boundary packets to the exact accepted interval")
        }

        do {
            let url = folder.appendingPathComponent("writer-failure-prefix.wav")
            let fault = WriteFault()
            let capture = try recorder(url: url, epoch: 3, beforeWrite: { try fault.beforeWrite() })
            capture.observeManualQuantum(frame: 0, frames: 64)
            capture.receive(try generated(start: 0, count: 64), at: time(0))
            let prefix = await capture.progress()
            try require(prefix.frames == 64 && prefix.error == nil, "The writer-failure fixture needs a committed PCM prefix.")
            capture.observeManualQuantum(frame: 64, frames: 64)
            capture.requestStop()
            capture.observeManualQuantum(frame: 128, frames: 64)
            capture.receive(try generated(start: 64, count: 64), at: time(64))
            capture.receive(try generated(start: 128, count: 64), at: time(128))
            let final = await capture.finish()
            try require(final.error != nil && final.frames == 64 && fault.attempts == 2,
                "A failed write must neither count its frames nor allow subsequent packets to write.")
            observations.append(try verifyGenerated(url: url, start: 0, count: 64, progress: final))
            recordCheck("A writer failure preserves a readable exact PCM prefix and never counts the failed write or resumes later writes")
        }

        do {
            // Source-clock failures are sticky and must retain already written
            // PCM. These test the recorder integration, not just the pure fence.
            let errors: [AudioRecordingBoundary.ErrorCode] = [.invalidTimestamp, .sampleRateChanged, .epochChanged]
            for (index, expected) in errors.enumerated() {
                let url = folder.appendingPathComponent("source-error-\(index).wav")
                let capture = try recorder(url: url, epoch: UInt64(10 + index))
                capture.observeManualQuantum(frame: 0, frames: 64)
                capture.receive(try generated(start: 0, count: 64), at: time(0))
                let prefix = await capture.progress()
                try require(prefix.frames == 64 && prefix.error == nil, "Source-error fixture did not write its prefix.")
                let boundary = capture.boundary!
                switch expected {
                case .invalidTimestamp:
                    _ = boundary.observeQuantum(sampleTime: .nan, frames: 64, sampleRate: rate,
                        epoch: boundary.epoch, sampleTimeValid: false)
                case .sampleRateChanged:
                    _ = boundary.observeQuantum(frame: 64, frames: 64, sampleRate: 44_100, epoch: boundary.epoch)
                case .epochChanged:
                    _ = boundary.observeQuantum(frame: 64, frames: 64, sampleRate: rate, epoch: boundary.epoch + 1)
                default: throw failure("Unexpected source-clock error fixture.")
                }
                capture.receive(try generated(start: 64, count: 64), at: time(64))
                let final = await capture.finish()
                try require((final.error as? AudioRecordingBoundary.ErrorCode) == expected && final.frames == 64,
                    "The recorder did not retain the expected sticky source-clock error \(expected).")
                observations.append(try verifyGenerated(url: url, start: 0, count: 64, progress: final))
            }
            for missingTimestamp in [true, false] {
                let url = folder.appendingPathComponent(missingTimestamp ? "missing-tap-time.wav" : "wrong-tap-rate.wav")
                let capture = try recorder(url: url, epoch: missingTimestamp ? 20 : 21)
                capture.observeManualQuantum(frame: 0, frames: 64)
                capture.receive(try generated(start: 0, count: 64), at: time(0))
                let prefix = await capture.progress()
                try require(prefix.frames == 64 && prefix.error == nil, "Tap-error fixture did not write its prefix.")
                capture.observeManualQuantum(frame: 64, frames: 64)
                capture.receive(try generated(start: 64, count: 64),
                    at: missingTimestamp ? nil : AVAudioTime(sampleTime: 64, atRate: 44_100))
                let final = await capture.finish()
                try require(final.error != nil && final.frames == 64, "An invalid tap clock must fail without appending its PCM.")
                observations.append(try verifyGenerated(url: url, start: 0, count: 64, progress: final))
            }
            recordCheck("Invalid source timestamps, source-rate changes, clock epochs, and missing/mismatched tap clocks fail while preserving written PCM")
        }

        do {
            let url = folder.appendingPathComponent("interrupted-tail-prefix.wav")
            let capture = try recorder(url: url, epoch: 30)
            capture.observeManualQuantum(frame: 0, frames: 128)
            capture.receive(try generated(start: 0, count: 64), at: time(0))
            let prefix = await capture.progress()
            try require(prefix.frames == 64, "The interrupted-tail fixture must write its prefix before Stop.")
            capture.requestStop()
            capture.observeManualQuantum(frame: 128, frames: 64)
            let pending = await capture.progress()
            try require(!pending.complete && pending.endFrame == 128 && pending.error == nil,
                "The fixture must have a known Stop boundary with an incomplete tail.")
            let final = await capture.finish(interruption: failure("Injected tail interruption."))
            try require(final.error != nil && final.frames == 64 && !final.complete,
                "Finishing an interrupted tail must report partial audio rather than successful completion.")
            observations.append(try verifyGenerated(url: url, start: 0, count: 64, progress: final))
            capture.receive(try generated(start: 64, count: 64), at: time(64))
            let afterClose = await capture.progress()
            try require(afterClose.frames == 64 && afterClose.error != nil,
                "A packet arriving after the final writer barrier must not reopen or extend the file.")
            _ = try verifyGenerated(url: url, start: 0, count: 64, progress: afterClose)
            recordCheck("An interrupted missing tail reports partial audio, closes a readable prefix, and rejects packets arriving after finalization")
        }

        observations.append(try await hostSetupFailure(folder: folder))
        recordCheck("Rejected recording setup returns an initially idle graph to stopped and preserves an already-running graph without publishing capture ownership")
        observations.append(try await hostShortInterval(folder: folder))
        recordCheck("The private offline host records exactly its first 64 master frames while Stop waits for a larger asynchronous tap batch")
        observations.append(try await hostNoProgress(folder: folder))
        recordCheck("A non-advancing source fails Stop within its bounded timeout, preserves written master PCM, and clears recording ownership")
        completed = true
        return checks
    }

    @MainActor private static func hostSetupFailure(folder: URL) async throws -> [String: Any] {
        let (graph, host, buffer) = try offlineHost()
        defer { host.shutdown() }
        try await prepare(host)
        let missing = folder.appendingPathComponent("missing-\(UUID().uuidString)",isDirectory:true)
            .appendingPathComponent("recording.wav")
        try require(!graph.isRunning,"The recording setup failure fixture must begin with an idle graph.")
        var messages: [String] = []
        for initiallyRunning in [false,true] {
            if initiallyRunning {
                _ = try await host.handle("note.on",payload:["track":0,"instrument":0,"note":67,"velocity":110])
                try require(graph.isRunning,"The running setup failure fixture must retain its existing graph.")
            }
            var rejected = false
            do { _ = try await host.handle("audio.record",payload:["source":"master","path":missing.path]) }
            catch { rejected = true; messages.append(error.localizedDescription) }
            try require(rejected && graph.isRunning == initiallyRunning && host.status()["recording"] as? Bool == false &&
                host.status()["recordingStopping"] as? Bool == false,
                "A rejected Record changed prior graph intent or retained recording ownership (running was \(initiallyRunning)).")
        }
        try render(host:host,frames:512,buffer:buffer)
        let signal = try samples(buffer)
        try require(signal.flatMap { $0 }.contains { abs($0) > 0.00001 },
            "The running graph's original note must remain audible after recording setup fails.")
        try require(!FileManager.default.fileExists(atPath:missing.path),"Rejected setup unexpectedly created its invalid destination.")
        return ["name":"host-setup-failure","idleRemainedStopped":true,"runningRemainedRunning":true,
            "recordingOwnershipCleared":true,"errors":messages]
    }

    @MainActor private static func hostShortInterval(folder: URL) async throws -> [String: Any] {
        let (graph, host, buffer) = try offlineHost()
        defer { host.shutdown() }
        try await prepare(host)
        func formatEvidence(_ value: AVAudioFormat) -> [String: Any] {
            ["sampleRate":value.sampleRate,"channels":value.channelCount,
             "commonFormat":value.commonFormat.rawValue,"interleaved":value.isInterleaved]
        }
        var startup: [String: Any] = ["beforeRecord":formatEvidence(graph.mainMixerNode.outputFormat(forBus:0)),
            "manualRendering":formatEvidence(graph.manualRenderingFormat),"runningBeforeRecord":graph.isRunning]
        defer {
            if let data = try? JSONSerialization.data(withJSONObject:startup,options:[.prettyPrinted,.sortedKeys]) {
                try? data.write(to:folder.appendingPathComponent("host-short-startup.json"),options:.atomic)
            }
        }
        let url = folder.appendingPathComponent("host-exact-64.wav")
        _ = try await host.handle("audio.record", payload: ["source": "master", "path": url.path])
        startup["afterRecord"] = formatEvidence(graph.mainMixerNode.outputFormat(forBus:0))
        startup["runningAfterRecord"] = graph.isRunning
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 67, "velocity": 110])
        let start = graph.manualRenderingSampleTime
        try render(host: host, frames: 64, buffer: buffer)
        let expected = try samples(buffer)
        try require(expected.flatMap { $0 }.contains { abs($0) > 0.00001 },
            "The 64-frame host fixture must contain audible original synthesized PCM.")
        var stopFinished = false
        let stopTask = Task { @MainActor () throws -> [String: Any] in
            defer { stopFinished = true }
            return try await host.handle("audio.stopRecord", payload: [:]) as? [String: Any] ?? [:]
        }
        try await waitUntil("Host Stop did not enter its asynchronous stopping state.") {
            host.status()["recordingStopping"] as? Bool == true || stopFinished
        }
        try require(!stopFinished && host.status()["recordingStopping"] as? Bool == true,
            "Stop must wait for a new source quantum and its tap watermark.")
        // Only the first new quantum acknowledges Stop. Extra renders flush the
        // platform's delayed tap batch; none belongs to the accepted interval.
        try render(host: host, frames: 64, buffer: buffer)
        var flushedFrames = 64
        while !stopFinished && flushedFrames < 32_768 {
            try render(host: host, frames: 1024, buffer: buffer)
            flushedFrames += 1024
            await Task.yield()
        }
        let result = try await stopTask.value
        try require(integer(result, "frames") == 64 && result["partial"] as? Bool == false &&
            result["sourceStartFrame"] as? String == String(start) &&
            result["sourceEndFrame"] as? String == String(start + 64),
            "Host Stop did not preserve the exact 64-frame source interval: \(result).")
        let actual = try read(url)
        try compare(actual, expected: expected, tolerance: 0.00000025, label: "Host 64-frame master capture")
        try require(host.status()["recording"] as? Bool == false && host.status()["recordingStopping"] as? Bool == false,
            "Successful Stop must clear both recording and pending-stop status.")
        return ["name": "host-exact-64", "path": url.path, "frames": actual[0].count,
            "sourceStartFrame": String(start), "sourceEndFrame": String(start + 64),
            "flushRenderFrames": flushedFrames, "result": result,
            "firstLeft": actual[0].first ?? 0, "lastLeft": actual[0].last ?? 0]
    }

    @MainActor private static func hostNoProgress(folder: URL) async throws -> [String: Any] {
        let (_, host, buffer) = try offlineHost()
        defer { host.shutdown() }
        try await prepare(host)
        let url = folder.appendingPathComponent("host-no-progress-prefix.wav")
        var finalEvent: [String: Any]?
        host.onEvent = { name, payload in
            if name == "audio.recording", payload["recording"] as? Bool == false { finalEvent = payload }
        }
        _ = try await host.handle("audio.record", payload: ["source": "master", "path": url.path])
        _ = try await host.handle("note.on", payload: ["track": 0, "instrument": 0, "note": 60, "velocity": 100])
        var expected = [[Float](), [Float]()]
        for frames in [AVAudioFrameCount(4096), 4096, 1408] {
            try render(host: host, frames: frames, buffer: buffer)
            let quantum = try samples(buffer)
            for channel in 0..<2 { expected[channel].append(contentsOf: quantum[channel]) }
            await Task.yield()
        }
        try await waitUntil("The no-progress fixture did not commit a master tap prefix.") {
            number(host.status(), "recordingDuration") >= 4800 / rate
        }
        let before = host.status()
        let started = ContinuousClock.now
        var stopError: Error?
        do { _ = try await host.handle("audio.stopRecord", payload: [:]) }
        catch { stopError = error }
        let elapsed = seconds(started.duration(to: ContinuousClock.now))
        try require(stopError != nil && elapsed >= 1.5 && elapsed < 6,
            "A source that never acknowledges Stop must fail within the bounded two-second wait; elapsed \(elapsed), error \(String(describing: stopError)).")
        guard let event = finalEvent else { throw failure("The failed host recording did not publish its preserved partial-file result.") }
        let written = Int(integer(event, "frames"))
        try require(event["partial"] as? Bool == true && event["sourceEndFrame"] is NSNull &&
            written >= 4800 && written <= expected[0].count && event["path"] as? String == url.path,
            "A timed-out recording must describe the actual preserved prefix and retain its unresolved source boundary: \(event).")
        let actual = try read(url)
        try compare(actual, expected: expected.map { Array($0.prefix(written)) }, tolerance: 0.00000025,
            label: "Timed-out host recording prefix")
        let after = host.status()
        try require(after["recording"] as? Bool == false && after["recordingStopping"] as? Bool == false,
            "Timed-out Stop must release recording ownership and clear pending-stop state.")
        let repeated = try await host.handle("audio.stopRecord", payload: [:]) as? [String: Any] ?? [:]
        try require(repeated["recording"] as? Bool == false,
            "Stop after failed finalization must be harmless and must not retain the failed task.")
        try require(stopError!.localizedDescription.contains(url.path), "Stop failure must tell the caller where its partial PCM was preserved.")
        return ["name": "host-no-progress", "path": url.path, "elapsedSeconds": elapsed,
            "frames": written, "recordingDurationBeforeStop": number(before, "recordingDuration"),
            "error": stopError!.localizedDescription, "result": event,
            "firstLeft": actual[0].first ?? 0, "lastLeft": actual[0].last ?? 0]
    }

    @MainActor private static func offlineHost() throws -> (AVAudioEngine, AudioHost, AVAudioPCMBuffer) {
        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let host = try AudioHost(audioGraph: graph, midiInputIDs: [], midiInputSourceProvider: { [] })
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
        return (graph, host, buffer)
    }

    @MainActor private static func prepare(_ host: AudioHost) async throws {
        let project: [String: Any] = ["version": 1, "name": "Original recording fence fixture", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 0.5, "activePattern": 0, "loopSong": false, "metronome": false,
            "tracks": [["name": "Native oscillator", "instrument": 0, "volume": 0.5, "pan": 0,
                        "delaySend": 0, "reverbSend": 0]],
            "instruments": [["type": "synth", "waveform": "sine", "volume": 0.5, "attack": 0,
                             "decay": 0, "sustain": 1, "release": 0.001]],
            "patterns": [["id": 0, "length": 16, "steps": [[Any]](), "automation": []]],
            "sequence": [0], "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
    }

    @MainActor private static func render(host: AudioHost, frames: AVAudioFrameCount, buffer: AVAudioPCMBuffer) throws {
        let result = try host.renderOfflineQuantum(frames: frames, to: buffer)
        try require(result == .success && buffer.frameLength == frames,
            "Private offline render failed to produce \(frames) frames: \(result.rawValue).")
    }

    @MainActor private static func waitUntil(_ message: String, _ predicate: () -> Bool) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(1))
        while !predicate() {
            try require(ContinuousClock.now < deadline, message)
            try await Task.sleep(for: .milliseconds(1))
        }
    }

    private static func recorder(url: URL, epoch: UInt64, beforeWrite: (() throws -> Void)? = nil) throws -> AudioRecording {
        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        let plan = try AudioRecordingChannelPlan.master(sourceFormat: format)
        let file = try AVAudioFile(forWriting: url, settings: [AVFormatIDKey: kAudioFormatLinearPCM,
            AVSampleRateKey: rate, AVNumberOfChannelsKey: 2, AVLinearPCMBitDepthKey: 32,
            AVLinearPCMIsFloatKey: true, AVLinearPCMIsBigEndianKey: false],
            commonFormat: .pcmFormatFloat32, interleaved: false)
        return try AudioRecording(source: .master, file: file, path: url.path, instrument: nil,
            channels: plan, boundary: AudioRecordingBoundary(sampleRate: rate, epoch: epoch),
            manualClock: true, beforeWrite: beforeWrite)
    }

    private static func time(_ frame: Int64) -> AVAudioTime { AVAudioTime(sampleTime: frame, atRate: rate) }

    private static func signal(_ frame: Int64, channel: Int) -> Float {
        // Every generated frame and channel has a distinct exactly represented
        // binary fraction. This detects even a one-frame fence displacement.
        Float(frame + 16_384 + Int64(channel) * 16_384) / 65_536
    }

    private static func generated(start: Int64, count: Int) throws -> AVAudioPCMBuffer {
        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(count)),
              let data = buffer.floatChannelData else { throw failure("Cannot allocate timestamped fixture PCM.") }
        buffer.frameLength = AVAudioFrameCount(count)
        for channel in 0..<2 {
            for frame in 0..<count { data[channel][frame] = signal(start + Int64(frame), channel: channel) }
        }
        return buffer
    }

    private static func samples(_ buffer: AVAudioPCMBuffer) throws -> [[Float]] {
        guard let data = buffer.floatChannelData, buffer.format.channelCount == 2, !buffer.format.isInterleaved else {
            throw failure("The PCM fixture must remain planar Float32 stereo.")
        }
        return (0..<2).map { Array(UnsafeBufferPointer(start: data[$0], count: Int(buffer.frameLength))) }
    }

    private static func read(_ url: URL) throws -> [[Float]] {
        let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
        try require(file.processingFormat.sampleRate == rate && file.processingFormat.channelCount == 2 &&
            file.length > 0 && file.length <= 32_768, "The finalized recording has an invalid format or frame count at \(url.path).")
        guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length)) else {
            throw failure("Cannot allocate the recorded PCM verification buffer.")
        }
        try file.read(into: buffer)
        try require(Int64(buffer.frameLength) == file.length, "The finalized WAV did not expose its complete PCM prefix.")
        return try samples(buffer)
    }

    private static func compare(_ actual: [[Float]], expected: [[Float]], tolerance: Float, label: String) throws {
        try require(actual.count == 2 && expected.count == 2 && actual[0].count == expected[0].count &&
            actual[1].count == expected[1].count, "\(label) has an incorrect channel or frame count.")
        for channel in 0..<2 {
            for frame in actual[channel].indices {
                try require(actual[channel][frame].isFinite && abs(actual[channel][frame] - expected[channel][frame]) <= tolerance,
                    "\(label) changed channel \(channel), frame \(frame): \(actual[channel][frame]) versus \(expected[channel][frame]).")
            }
        }
    }

    private static func verifyGenerated(url: URL, start: Int64, count: Int, progress: AudioRecording.Progress) throws -> [String: Any] {
        let actual = try read(url)
        let expected = (0..<2).map { channel in (0..<count).map { signal(start + Int64($0), channel: channel) } }
        try compare(actual, expected: expected, tolerance: 0, label: url.lastPathComponent)
        try require(progress.frames == Int64(count), "Written-frame progress disagrees with the actual finalized WAV.")
        return ["name": url.deletingPathExtension().lastPathComponent, "path": url.path, "frames": count,
            "expectedFirstFrame": String(start), "expectedLastFrame": String(start + Int64(count) - 1),
            "firstLeft": actual[0][0], "lastLeft": actual[0][count - 1],
            "sourceStartFrame": progress.startFrame.map { String($0) as Any } ?? NSNull(),
            "sourceEndFrame": progress.endFrame.map { String($0) as Any } ?? NSNull(),
            "receivedThrough": progress.receivedThrough.map { String($0) as Any } ?? NSNull(),
            "complete": progress.complete, "error": progress.error?.localizedDescription ?? ""]
    }

    private static func number(_ value: [String: Any], _ key: String) -> Double {
        (value[key] as? NSNumber)?.doubleValue ?? -.infinity
    }
    private static func integer(_ value: [String: Any], _ key: String) -> Int64 {
        (value[key] as? NSNumber)?.int64Value ?? -1
    }
    private static func seconds(_ duration: Duration) -> Double {
        let components = duration.components
        return Double(components.seconds) + Double(components.attoseconds) / 1_000_000_000_000_000_000
    }
    private static func require(_ condition: Bool, _ message: String) throws {
        if !condition { throw failure(message) }
    }
    private static func failure(_ message: String) -> AudioRecordingError {
        .invalid("Recording fence regression: " + message)
    }

    /// Called only by the recorder's serial writer queue; inspected after finish.
    private final class WriteFault: @unchecked Sendable {
        private(set) var attempts = 0
        func beforeWrite() throws {
            attempts += 1
            if attempts == 2 { throw NativeRecordingFenceTests.failure("Injected second-write failure.") }
        }
    }
}

#endif

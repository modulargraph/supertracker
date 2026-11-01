import Foundation
import AVFoundation
import TrackerEngine

/// Freeze by recording: a track whose sound is made outside the computer (a hardware
/// synth played by an editor plug-in or an external MIDI instrument, returning through a
/// line input) cannot be rendered offline. Its print is captured in real time instead.
///
/// * Capture point: the track strip output of the flat mixer (`rack.outputNode`, which the
///   meter tap already observes), i.e. after the line input, the track's inserts and its
///   plug-in/hardware-return delay compensation. Every track is aligned there, so the
///   content at graph frame T is song frame `songFrame(T - compensationLatency)`; the
///   native clock answers that mapping (`st_engine_clock_span` with the upstream latency).
///   The print therefore lands exactly where the compensated live return did, including
///   the hardware-return estimate (`AudioDeviceSelection.hardwareReturnSeconds`).
/// * One take = Play the take project from the song start (Song mode, no loop), write the
///   track's strip into frame `songFrame` of the print until the musical end plus the tail
///   (auto: until the return stays below `recordedSilence`, bounded by the maximum tail).
/// * The meter tap is delivered off the render thread, so the take allocates freely; it
///   only holds the render lock for the read-only clock query.
final class AudioFreezeTakeTap {
    private let lock = NSLock()
    private var take: AudioFreezeTake?
    func set(_ value: AudioFreezeTake?) { lock.lock(); take = value; lock.unlock() }
    /// Meter-tap thread.
    func receive(_ buffer: AVAudioPCMBuffer, at time: AVAudioTime, track: Int) {
        lock.lock(); let current = take; lock.unlock()
        guard let current, current.track == track else { return }
        current.append(buffer, at: time)
    }
}

final class AudioFreezeTake {
    struct Chunk { let frame: Int64; let hostTime: UInt64?; let left: [Float]; let right: [Float] }
    let track: Int
    private let lock = NSLock()
    private var chunks: [Chunk] = []
    private(set) var invalidTime = false
    init(track: Int) { self.track = track }

    func append(_ buffer: AVAudioPCMBuffer, at time: AVAudioTime) {
        let frames = Int(buffer.frameLength)
        guard frames > 0, let data = buffer.floatChannelData else { return }
        guard time.isSampleTimeValid else { lock.lock(); invalidTime = true; lock.unlock(); return }
        let left = Array(UnsafeBufferPointer(start: data[0], count: frames))
        let right = buffer.format.channelCount > 1 ? Array(UnsafeBufferPointer(start: data[1], count: frames)) : left
        lock.lock(); chunks.append(Chunk(frame: time.sampleTime, hostTime: time.isHostTimeValid ? time.hostTime : nil, left: left, right: right)); lock.unlock()
    }

    func drain() -> [Chunk] { lock.lock(); defer { chunks.removeAll(); lock.unlock() }; return chunks }
}

extension AudioHost {
    /// Song frame of the content at `graphFrame` after `latency` frames of upstream delay,
    /// or nil while the transport is not playing that frame (pre-roll, stopped, not retained).
    /// Real-time native sources render at the graph frame of their host time (the main-mixer
    /// timebase), so a tap buffer is mapped the same way; offline it is its sample time.
    @MainActor func freezeTakeSongFrame(_ chunk: AudioFreezeTake.Chunk, latency: Int64) -> Int64? {
        var span = STClockSpan()
        renderLock.lock()
        let graphFrame = nativeSourceUsesHostClock ? chunk.hostTime.flatMap { midiTimebase.graphFrame(atHostTime: $0) } ?? chunk.frame : chunk.frame
        let status = st_engine_clock_span(engine, graphFrame, 1, latency, &span)
        renderLock.unlock()
        guard status == ST_CLOCK_OK, span.context.flags & Int32(ST_CLOCK_PLAYING) != 0,
              span.context.flags & Int32(ST_CLOCK_PREROLL) == 0 else { return nil }
        return span.context.songFrame
    }

    /// Records one real-time take of `project` (a print project from AudioFreezeTrack) and
    /// writes the track's strip, song-aligned, to `url`. Returns the musical frame count.
    @MainActor func recordFreezeTake(_ project: [String: Any], track: Int, to url: URL, tail: Double, auto: Bool,
                                     monitor: Bool, progress: (Double) -> Void) async throws -> Int64 {
        var value = project
        value["loopSong"] = false; value["metronome"] = false; value.removeValue(forKey: "songLoop")
        _ = try await handleCore("engine.project", payload: ["project": value, "awaitPlugins": true])
        #if SELFTESTS
        // Self-tests drive a manual graph themselves (no audio device); the take path is the same.
        let manual = graph.isInManualRenderingMode
        #else
        let manual = false
        #endif
        guard !exporting, manual || !graph.isInManualRenderingMode else { throw AudioFreezeTrack.Failure(message: "Freeze by recording needs live audio. Finish the export first.") }
        guard track < meterTaps, rack.outputNode(track: track) != nil else { throw AudioFreezeTrack.Failure(message: "This track has no mixer strip to record.") }
        let duration = st_engine_duration(engine, 1)
        guard duration > 0, duration < 3600 else { throw AudioFreezeTrack.Failure(message: "Freeze by recording supports songs up to one hour.") }
        let rate = sampleRate
        let musical = Int64(duration * rate), limit = musical + Int64((tail * rate).rounded())
        let hold = Int64(0.5 * rate)
        guard let file = try? AVAudioFile(forWriting: url, settings: AudioExportFormat.float32.settings, commonFormat: .pcmFormatFloat32, interleaved: false),
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 65_536) else {
            throw AudioFreezeTrack.Failure(message: "Cannot create the recorded print file.")
        }
        let take = AudioFreezeTake(track: track)
        let mixer = graph.mainMixerNode, volume = mixer.outputVolume
        if !monitor { mixer.outputVolume = 0 }
        freezeTrackRuntime.tap.set(take)
        defer { freezeTrackRuntime.tap.set(nil); st_engine_stop(engine); panicExternal(); mixer.outputVolume = volume }
        let first = (value["sequence"] as? [Any])?.first.map { PayloadValue.integer($0, 0) } ?? 0
        _ = try await handleCore("transport.seek", payload: ["pattern": first, "row": 0, "sequence": 0])
        _ = try await handleCore("transport.play", payload: ["mode": "song"])
        let latency = Int64(rack.compensationLatencyFrames)
        let quantum = manual ? AVAudioPCMBuffer(pcmFormat: graph.manualRenderingFormat, frameCapacity: 512) : nil
        if manual { nativeSourceUsesHostClock = false }
        var offset: Int64?, pending: [AudioFreezeTake.Chunk] = [], written: Int64 = 0, lastLoud: Int64 = -1
        let started = ContinuousClock.now, deadline = started.advanced(by: .seconds(duration + tail + 10))
        func write(_ chunk: AudioFreezeTake.Chunk, offset: Int64) throws {
            var frame = chunk.frame - offset, index = 0
            if frame < written { index = Int(min(Int64(chunk.left.count), written - frame)); frame += Int64(index) }
            // A missing tap buffer (overload) becomes silence at its song position.
            while written < frame, written < limit {
                let count = Int(min(Int64(buffer.frameCapacity), frame - written, limit - written))
                buffer.frameLength = AVAudioFrameCount(count)
                for c in 0..<2 { buffer.floatChannelData![c].update(repeating: 0, count: count) }
                try file.write(from: buffer); written += Int64(count)
            }
            let count = Int(min(Int64(chunk.left.count - index), limit - written))
            guard count > 0 else { return }
            buffer.frameLength = AVAudioFrameCount(count)
            chunk.left.withUnsafeBufferPointer { buffer.floatChannelData![0].update(from: $0.baseAddress! + index, count: count) }
            chunk.right.withUnsafeBufferPointer { buffer.floatChannelData![1].update(from: $0.baseAddress! + index, count: count) }
            for i in 0..<count where abs(chunk.left[index + i]) > AudioFreezeTrack.recordedSilence || abs(chunk.right[index + i]) > AudioFreezeTrack.recordedSilence {
                lastLoud = written + Int64(i)
            }
            try file.write(from: buffer); written += Int64(count)
        }
        while written < limit {
            if auto, written >= musical + hold, written - max(lastLoud, musical) >= hold { break }
            guard ContinuousClock.now < deadline else { throw AudioFreezeTrack.Failure(message: "The recording did not keep up with playback. Check the audio device and try again.") }
            guard !closed else { throw AudioFreezeTrack.Failure(message: "The audio host closed while recording the freeze.") }
            if let quantum {
                for _ in 0..<16 { _ = try renderOfflineQuantum(frames: 512, to: quantum) }
                try await Task.sleep(for: .milliseconds(2))
            } else { try await Task.sleep(for: .milliseconds(15)) }
            if take.invalidTime { throw AudioFreezeTrack.Failure(message: "The track strip delivered audio without a timestamp; the take cannot be aligned.") }
            pending += take.drain()
            if offset == nil {
                for chunk in pending {
                    // First chunk whose first frame plays song time fixes graph -> song.
                    if let song = freezeTakeSongFrame(chunk, latency: latency) { offset = chunk.frame - song; break }
                }
                if offset == nil {
                    // Keep at most a few seconds of pre-roll; fail when playback never arrives.
                    if let last = pending.last, let firstFrame = pending.first?.frame, last.frame - firstFrame > Int64(4 * rate) { pending.removeFirst(pending.count / 2) }
                    if started.duration(to: .now) > .seconds(4) { throw AudioFreezeTrack.Failure(message: "Playback did not start on the track strip; the take was not recorded.") }
                    continue
                }
            }
            guard let offset else { continue }
            for chunk in pending {
                // While playing, the clock must agree with the fixed mapping (no restart or jump).
                // (Host-time mapping may round by a frame; a real jump is far larger.)
                if chunk.frame - offset < musical, let song = freezeTakeSongFrame(chunk, latency: latency), abs(chunk.frame - song - offset) > 2 {
                    throw AudioFreezeTrack.Failure(message: "Playback jumped while recording the freeze (seek, loop or audio restart). Try again without touching the transport.")
                }
                try write(chunk, offset: offset)
            }
            pending.removeAll()
            var transport = st_engine_status(engine)
            if written < musical - Int64(rate), st_status_transport_playing(&transport) == 0 {
                throw AudioFreezeTrack.Failure(message: "Playback stopped before the end of the song; the freeze was cancelled.")
            }
            progress(Double(written) / Double(max(1, limit)))
        }
        if #available(macOS 15.0, *) { file.close() }
        return musical
    }
}

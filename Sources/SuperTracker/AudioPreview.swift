import AVFoundation
import Darwin
import Foundation
import os

// Browser preview audio: decoded off the main and audio threads, cached in a
// memory-bounded LRU, and played by one persistent source node whose voices
// crossfade. The node is attached once and never disconnected, so starting or
// switching a preview never pauses the audio graph.

/// Planar stereo PCM at the graph sample rate. Immutable once published; the
/// cache and any playing voice share it.
final class PreviewPCM: @unchecked Sendable {
    let left: UnsafeMutablePointer<Float>
    let right: UnsafeMutablePointer<Float>
    let capacity: Int
    private(set) var frames = 0
    /// True when `frames` covers the whole file; otherwise a tail stream continues it.
    fileprivate(set) var complete = false
    let estimatedTotalFrames: Int
    /// Seconds of the whole source file.
    let duration: Double
    let sampleRate: Double
    var bytes: Int { capacity * 2 * MemoryLayout<Float>.size }

    init(capacity: Int, estimatedTotalFrames: Int, duration: Double, sampleRate: Double) {
        self.capacity = max(1, capacity)
        left = .allocate(capacity: self.capacity); right = .allocate(capacity: self.capacity)
        left.initialize(repeating: 0, count: self.capacity); right.initialize(repeating: 0, count: self.capacity)
        self.estimatedTotalFrames = estimatedTotalFrames; self.duration = duration; self.sampleRate = sampleRate
    }
    deinit { left.deallocate(); right.deallocate() }

    /// Appends up to the capacity; returns the number of frames taken.
    @discardableResult func append(_ l: UnsafePointer<Float>, _ r: UnsafePointer<Float>, _ count: Int) -> Int {
        let n = min(count, capacity - frames)
        guard n > 0 else { return 0 }
        (left + frames).update(from: l, count: n); (right + frames).update(from: r, count: n)
        frames += n
        return n
    }
}

/// Append-only single-producer/single-consumer PCM stream for the part of a
/// long preview after its cached head. The writer runs on a background queue;
/// the render thread only reads published frames through raw pointers.
final class PreviewStream: @unchecked Sendable {
    let chunkFrames: Int
    let maxChunks: Int
    /// Each chunk holds `chunkFrames` left samples followed by `chunkFrames` right samples.
    let chunks: UnsafeMutablePointer<UnsafeMutablePointer<Float>?>
    /// [0] published frames, [1] finished, [2] reader position, [3] cancelled, [4] underruns.
    let state: UnsafeMutablePointer<Int64>
    private var freed = 0

    init(estimatedFrames: Int, chunkFrames: Int = 16_384) {
        self.chunkFrames = chunkFrames
        maxChunks = max(4, (estimatedFrames + chunkFrames - 1) / chunkFrames + 8)
        chunks = .allocate(capacity: maxChunks); chunks.initialize(repeating: nil, count: maxChunks)
        state = .allocate(capacity: 5); state.initialize(repeating: 0, count: 5)
    }
    deinit {
        for index in 0..<maxChunks { chunks[index]?.deallocate() }
        chunks.deallocate(); state.deallocate()
    }

    var published: Int { Int(OSAtomicAdd64Barrier(0, state)) }
    var readPosition: Int { Int(OSAtomicAdd64Barrier(0, state + 2)) }
    var cancelled: Bool { OSAtomicAdd64Barrier(0, state + 3) != 0 }
    var finished: Bool { OSAtomicAdd64Barrier(0, state + 1) != 0 }
    var underruns: Int { Int(OSAtomicAdd64Barrier(0, state + 4)) }
    func cancel() { _ = OSAtomicCompareAndSwap64Barrier(0, 1, state + 3) }
    func finish() { _ = OSAtomicCompareAndSwap64Barrier(0, 1, state + 1) }

    /// Writer only. Returns false when the stream is cancelled or out of chunk slots.
    func append(_ l: UnsafePointer<Float>, _ r: UnsafePointer<Float>, _ count: Int) -> Bool {
        var written = 0, total = published
        while written < count {
            if cancelled { return false }
            let chunk = total / chunkFrames, offset = total % chunkFrames
            guard chunk < maxChunks else { return false }
            if chunks[chunk] == nil {
                let storage = UnsafeMutablePointer<Float>.allocate(capacity: chunkFrames * 2)
                storage.initialize(repeating: 0, count: chunkFrames * 2)
                chunks[chunk] = storage
                OSMemoryBarrier()
            }
            let n = min(count - written, chunkFrames - offset), storage = chunks[chunk]!
            (storage + offset).update(from: l + written, count: n)
            (storage + chunkFrames + offset).update(from: r + written, count: n)
            written += n; total += n
            OSMemoryBarrier()
            _ = OSAtomicAdd64Barrier(Int64(n), state)
        }
        releaseConsumed()
        return true
    }

    /// Frees chunks the reader passed at least one whole chunk ago (writer only).
    func releaseConsumed() {
        let safe = readPosition / chunkFrames - 1
        while freed < safe, freed < maxChunks {
            chunks[freed]?.deallocate(); chunks[freed] = nil; freed += 1
        }
    }
}

/// Mixes up to `voiceCount` preview voices with per-sample gain ramps. Main
/// thread starts/stops voices; the render thread mixes. Render touches only
/// plain values and raw pointers, never retains or releases objects.
final class PreviewVoiceMixer: @unchecked Sendable {
    struct Voice {
        var active = false
        var left: UnsafePointer<Float>?
        var right: UnsafePointer<Float>?
        var headFrames = 0
        var chunks: UnsafeMutablePointer<UnsafeMutablePointer<Float>?>?
        var streamState: UnsafeMutablePointer<Int64>?
        var chunkFrames = 0
        var position = 0
        var gain: Float = 0
        var target: Float = 0
        var delta: Float = 0
        var volume: Float = 1
        var serial: UInt64 = 0
    }
    static let voiceCount = 8
    let sampleRate: Double
    let fadeInFrames: Int
    let fadeOutFrames: Int
    private let voices: UnsafeMutablePointer<Voice>
    private let lock: UnsafeMutablePointer<os_unfair_lock>
    private var owners: [(pcm: PreviewPCM?, stream: PreviewStream?)]
    private var serial: UInt64 = 0

    init(sampleRate: Double, fadeInSeconds: Double = 0.002, fadeOutSeconds: Double = 0.012) {
        self.sampleRate = sampleRate
        fadeInFrames = max(1, Int(sampleRate * fadeInSeconds)); fadeOutFrames = max(1, Int(sampleRate * fadeOutSeconds))
        voices = .allocate(capacity: Self.voiceCount); voices.initialize(repeating: Voice(), count: Self.voiceCount)
        lock = .allocate(capacity: 1); lock.initialize(to: os_unfair_lock())
        owners = Array(repeating: (nil, nil), count: Self.voiceCount)
    }
    deinit { voices.deinitialize(count: Self.voiceCount); voices.deallocate(); lock.deallocate() }

    /// Fades every sounding voice out and fades `pcm` (plus its optional tail stream) in.
    @discardableResult func start(_ pcm: PreviewPCM, stream: PreviewStream? = nil, volume: Float) -> UInt64 {
        var released: [(PreviewPCM?, PreviewStream?)] = []
        released.reserveCapacity(Self.voiceCount + 1) // no allocation while holding the render lock
        os_unfair_lock_lock(lock)
        reclaimLocked(&released)
        fadeOutLocked()
        var slot = (0..<Self.voiceCount).first { !voices[$0].active }
        if slot == nil {
            // Every slot is still fading: steal the quietest one.
            slot = (0..<Self.voiceCount).min { voices[$0].gain < voices[$1].gain }
            released.append(owners[slot!]); owners[slot!].stream?.cancel()
        }
        let index = slot!
        serial &+= 1
        var voice = Voice()
        voice.active = true; voice.left = UnsafePointer(pcm.left); voice.right = UnsafePointer(pcm.right)
        voice.headFrames = pcm.frames; voice.volume = max(0, volume)
        if let stream { voice.chunks = stream.chunks; voice.streamState = stream.state; voice.chunkFrames = stream.chunkFrames }
        voice.gain = 0; voice.target = 1; voice.delta = 1 / Float(fadeInFrames); voice.serial = serial
        voices[index] = voice
        owners[index] = (pcm, stream)
        let value = serial
        os_unfair_lock_unlock(lock)
        released.removeAll()
        return value
    }

    /// Fades all voices out (or silences them at once when `fade` is false).
    func stop(fade: Bool = true) {
        var released: [(PreviewPCM?, PreviewStream?)] = []
        released.reserveCapacity(Self.voiceCount + 1) // no allocation while holding the render lock
        os_unfair_lock_lock(lock)
        if fade { fadeOutLocked() }
        else { for index in 0..<Self.voiceCount where voices[index].active { voices[index].active = false } }
        reclaimLocked(&released)
        os_unfair_lock_unlock(lock)
        released.removeAll()
    }

    /// Releases voices the render thread finished (main thread).
    func reclaim() {
        var released: [(PreviewPCM?, PreviewStream?)] = []
        released.reserveCapacity(Self.voiceCount + 1) // no allocation while holding the render lock
        os_unfair_lock_lock(lock); reclaimLocked(&released); os_unfair_lock_unlock(lock)
        released.removeAll()
    }

    var activeVoices: Int {
        os_unfair_lock_lock(lock); defer { os_unfair_lock_unlock(lock) }
        return (0..<Self.voiceCount).filter { voices[$0].active }.count
    }

    /// Whether a voice that is not fading out is sounding.
    var sounding: Bool {
        os_unfair_lock_lock(lock); defer { os_unfair_lock_unlock(lock) }
        return (0..<Self.voiceCount).contains { voices[$0].active && voices[$0].target > 0 }
    }

    private func fadeOutLocked() {
        for index in 0..<Self.voiceCount where voices[index].active && voices[index].target > 0 {
            voices[index].target = 0
            voices[index].delta = -max(voices[index].gain, 1e-4) / Float(fadeOutFrames)
        }
    }

    private func reclaimLocked(_ released: inout [(PreviewPCM?, PreviewStream?)]) {
        for index in 0..<Self.voiceCount where !voices[index].active && (owners[index].pcm != nil || owners[index].stream != nil) {
            owners[index].stream?.cancel()
            released.append(owners[index]); owners[index] = (nil, nil)
            voices[index] = Voice()
        }
    }

    /// Render thread. Adds the preview mix into `left`/`right` (which the caller
    /// zeroed). Returns false when no voice produced audio.
    func render(frames: Int, left: UnsafeMutablePointer<Float>, right: UnsafeMutablePointer<Float>) -> Bool {
        os_unfair_lock_lock(lock); defer { os_unfair_lock_unlock(lock) }
        var any = false
        for index in 0..<Self.voiceCount where voices[index].active {
            any = true
            var voice = voices[index]
            var produced = 0
            while produced < frames && voice.active {
                var source: (UnsafePointer<Float>, UnsafePointer<Float>)
                var available: Int
                if voice.position < voice.headFrames, let l = voice.left, let r = voice.right {
                    available = voice.headFrames - voice.position
                    source = (l + voice.position, r + voice.position)
                } else if let state = voice.streamState, let chunks = voice.chunks {
                    let tail = voice.position - voice.headFrames
                    let published = Int(OSAtomicAdd64Barrier(0, state))
                    if tail >= published {
                        if OSAtomicAdd64Barrier(0, state + 1) != 0 { voice.active = false }
                        else { _ = OSAtomicAdd64Barrier(1, state + 4); break }
                        continue
                    }
                    let chunk = tail / voice.chunkFrames, offset = tail % voice.chunkFrames
                    guard let storage = chunks[chunk] else { voice.active = false; continue }
                    available = min(published - tail, voice.chunkFrames - offset)
                    source = (UnsafePointer(storage + offset), UnsafePointer(storage + voice.chunkFrames + offset))
                } else { voice.active = false; continue }
                let n = min(available, frames - produced)
                var gain = voice.gain
                let target = voice.target, delta = voice.delta, volume = voice.volume
                for i in 0..<n {
                    if gain != target {
                        gain += delta
                        if (delta > 0 && gain >= target) || (delta < 0 && gain <= target) { gain = target }
                    }
                    let g = gain * volume
                    left[produced + i] += source.0[i] * g
                    right[produced + i] += source.1[i] * g
                    if gain <= 0 && target <= 0 {
                        voice.active = false; voice.position += i + 1; produced += i + 1
                        break
                    }
                }
                voice.gain = gain
                if voice.active { voice.position += n; produced += n }
            }
            if let state = voice.streamState {
                let tail = Int64(max(0, voice.position - voice.headFrames))
                let current = OSAtomicAdd64Barrier(0, state + 2)
                if tail > current { _ = OSAtomicAdd64Barrier(tail - current, state + 2) }
                if !voice.active { _ = OSAtomicCompareAndSwap64Barrier(0, 1, state + 3) }
            }
            voices[index] = voice
        }
        return any
    }
}

enum PreviewDecodeError: LocalizedError {
    case offline(String), unreadable(String), cancelled
    var errorDescription: String? {
        switch self {
        case .offline(let path): return "The drive holding “\((path as NSString).lastPathComponent)” is offline."
        case .unreadable(let message): return message
        case .cancelled: return "The preview was superseded."
        }
    }
}

/// Decodes files to planar stereo at a target rate. Head and tail decodes feed
/// the converter identical input blocks from the file start, so the tail
/// continues the head sample-exactly.
enum PreviewDecoder {
    static let blockFrames: AVAudioFrameCount = 8192

    /// True when `path` is on a /Volumes/<name> drive that is not mounted.
    static func volumeOffline(_ path: String) -> Bool {
        let parts = path.split(separator: "/", omittingEmptySubsequences: true)
        guard parts.count >= 2, parts[0] == "Volumes" else { return false }
        var directory: ObjCBool = false
        return !FileManager.default.fileExists(atPath: "/Volumes/" + parts[1], isDirectory: &directory) || !directory.boolValue
    }

    struct Info { let size: Int64; let modified: Double }
    static func info(_ path: String) -> Info? {
        var value = stat()
        guard stat(path, &value) == 0, (value.st_mode & S_IFMT) == S_IFREG else { return nil }
        return Info(size: Int64(value.st_size), modified: Double(value.st_mtimespec.tv_sec) + Double(value.st_mtimespec.tv_nsec) / 1e9)
    }

    /// Streams converted frames to `emit` starting at output frame `skip`.
    /// Returns true when the file end was reached.
    @discardableResult
    static func decode(file: AVAudioFile, targetRate: Double, skip: Int, limit: Int, isCancelled: () -> Bool,
                       emit: (UnsafePointer<Float>, UnsafePointer<Float>, Int) -> Bool) throws -> Bool {
        let input = file.processingFormat
        let channels = Int(input.channelCount)
        guard channels > 0, input.sampleRate > 0 else { throw PreviewDecodeError.unreadable("The preview file has no audio channels.") }
        guard let inBuffer = AVAudioPCMBuffer(pcmFormat: input, frameCapacity: blockFrames) else { throw PreviewDecodeError.unreadable("Cannot allocate preview audio.") }
        var skipped = 0, emitted = 0
        func deliver(_ buffer: AVAudioPCMBuffer) -> Bool {
            guard let data = buffer.floatChannelData else { return false }
            var start = 0, count = Int(buffer.frameLength)
            if skipped < skip { let drop = min(count, skip - skipped); skipped += drop; start = drop; count -= drop }
            count = min(count, limit - emitted)
            guard count > 0 else { return emitted < limit }
            let l = UnsafePointer(data[0] + start), r = UnsafePointer(data[min(1, Int(buffer.format.channelCount) - 1)] + start)
            emitted += count
            return emit(l, r, count) && emitted < limit
        }
        if abs(input.sampleRate - targetRate) < 0.5 {
            file.framePosition = AVAudioFramePosition(min(Int64(skip), file.length)); skipped = skip
            while true {
                if isCancelled() { throw PreviewDecodeError.cancelled }
                if file.framePosition >= file.length { return true }
                try file.read(into: inBuffer, frameCount: blockFrames)
                if inBuffer.frameLength == 0 { return true }
                if !deliver(inBuffer) { return file.framePosition >= file.length }
            }
        }
        guard let output = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: targetRate, channels: input.channelCount, interleaved: false),
              let converter = AVAudioConverter(from: input, to: output),
              let outBuffer = AVAudioPCMBuffer(pcmFormat: output, frameCapacity: AVAudioFrameCount(Double(blockFrames) * targetRate / input.sampleRate) + 1024)
        else { throw PreviewDecodeError.unreadable("Cannot convert the preview to the audio device rate.") }
        file.framePosition = 0
        var readError: Error?, ended = false
        while true {
            if isCancelled() { throw PreviewDecodeError.cancelled }
            outBuffer.frameLength = 0
            var conversionError: NSError?
            let status = converter.convert(to: outBuffer, error: &conversionError) { packets, outStatus in
                if ended || file.framePosition >= file.length { ended = true; outStatus.pointee = .endOfStream; return nil }
                do {
                    try file.read(into: inBuffer, frameCount: min(blockFrames, packets))
                } catch { readError = error; ended = true; outStatus.pointee = .endOfStream; return nil }
                if inBuffer.frameLength == 0 { ended = true; outStatus.pointee = .endOfStream; return nil }
                outStatus.pointee = .haveData; return inBuffer
            }
            if let readError { throw readError }
            if status == .error { throw conversionError ?? PreviewDecodeError.unreadable("The preview could not be converted.") }
            if outBuffer.frameLength > 0, !deliver(outBuffer) { return false }
            if status == .endOfStream { return true }
        }
    }

    /// Decodes the cached head: the whole file when it is at most `fullSeconds`,
    /// otherwise its first `headSeconds`.
    static func head(path: String, targetRate: Double, headSeconds: Double, fullSeconds: Double, isCancelled: () -> Bool) throws -> PreviewPCM {
        if volumeOffline(path) { throw PreviewDecodeError.offline(path) }
        let file: AVAudioFile
        do { file = try AVAudioFile(forReading: URL(fileURLWithPath: path)) }
        catch { throw PreviewDecodeError.unreadable(error.localizedDescription) }
        let rate = file.processingFormat.sampleRate
        let estimated = Int((Double(file.length) * targetRate / max(rate, 1)).rounded(.up))
        let whole = Double(estimated) <= fullSeconds * targetRate
        let limit = whole ? estimated + 4096 : Int(headSeconds * targetRate)
        let pcm = PreviewPCM(capacity: limit, estimatedTotalFrames: estimated, duration: Double(file.length) / max(rate, 1), sampleRate: targetRate)
        let ended = try decode(file: file, targetRate: targetRate, skip: 0, limit: limit, isCancelled: isCancelled) { l, r, n in
            pcm.append(l, r, n) == n
        }
        pcm.complete = ended || whole
        return pcm
    }

    /// Feeds `stream` with the frames after `pcm`'s head. Blocks the calling
    /// (background) thread, staying at most `lead` seconds ahead of the reader.
    static func streamTail(path: String, after pcm: PreviewPCM, into stream: PreviewStream, lead: Double = 6) {
        defer { stream.finish() }
        guard let file = try? AVAudioFile(forReading: URL(fileURLWithPath: path)) else { return }
        let leadFrames = Int(lead * pcm.sampleRate)
        _ = try? decode(file: file, targetRate: pcm.sampleRate, skip: pcm.frames, limit: Int.max, isCancelled: { stream.cancelled }) { l, r, n in
            while !stream.cancelled && stream.published - stream.readPosition > leadFrames {
                stream.releaseConsumed(); usleep(10_000)
            }
            return stream.append(l, r, n)
        }
    }
}

/// Memory-capped LRU of decoded preview heads, with in-flight de-duplication
/// and a short negative cache so missing or broken files are not retried in a loop.
final class PreviewCache: @unchecked Sendable {
    struct Key: Hashable { let path: String; let size: Int64; let modified: Double; let rate: Double }
    private struct Entry { let pcm: PreviewPCM; var use: UInt64 }
    let capacityBytes: Int
    let headSeconds: Double
    let fullSeconds: Double
    private let lock = NSLock()
    private var entries: [Key: Entry] = [:]
    private var inflight: [Key: Task<PreviewPCM, Error>] = [:]
    private var failures: [String: Date] = [:]
    private var clock: UInt64 = 0
    private(set) var bytes = 0
    private(set) var hits = 0
    private(set) var misses = 0

    init(capacityBytes: Int = 64 << 20, headSeconds: Double = 4, fullSeconds: Double = 8) {
        self.capacityBytes = capacityBytes; self.headSeconds = headSeconds; self.fullSeconds = fullSeconds
    }

    var count: Int { lock.lock(); defer { lock.unlock() }; return entries.count }

    func key(_ path: String, rate: Double) -> Key? {
        guard let info = PreviewDecoder.info(path) else { return nil }
        return Key(path: path, size: info.size, modified: info.modified, rate: rate)
    }

    func cached(_ key: Key) -> PreviewPCM? {
        lock.lock(); defer { lock.unlock() }
        guard var entry = entries[key] else { return nil }
        clock &+= 1; entry.use = clock; entries[key] = entry
        return entry.pcm
    }

    func recentlyFailed(_ path: String, within seconds: TimeInterval = 30) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard let date = failures[path] else { return false }
        if Date().timeIntervalSince(date) > seconds { failures[path] = nil; return false }
        return true
    }

    func insert(_ pcm: PreviewPCM, for key: Key) {
        lock.lock(); defer { lock.unlock() }
        if let old = entries[key] { bytes -= old.pcm.bytes }
        clock &+= 1; entries[key] = Entry(pcm: pcm, use: clock); bytes += pcm.bytes
        failures[key.path] = nil
        while bytes > capacityBytes, entries.count > 1,
              let oldest = entries.min(by: { $0.value.use < $1.value.use }), oldest.key != key {
            bytes -= oldest.value.pcm.bytes; entries[oldest.key] = nil
        }
    }



    /// Returns the cached head, joins a decode already running for it, or decodes it.
    func head(_ path: String, rate: Double, priority: TaskPriority, isStale: @escaping @Sendable () -> Bool = { false }) async throws -> (pcm: PreviewPCM, cached: Bool) {
        try await Task.detached(priority: priority) { try await self.resolve(path, rate: rate, priority: priority, isStale: isStale) }.value
    }

    static func isCancellation(_ error: Error) -> Bool {
        if error is CancellationError { return true }
        if case PreviewDecodeError.cancelled? = error as? PreviewDecodeError { return true }
        return false
    }

    private func resolve(_ path: String, rate: Double, priority: TaskPriority, isStale: @escaping @Sendable () -> Bool) async throws -> (pcm: PreviewPCM, cached: Bool) {
        if PreviewDecoder.volumeOffline(path) { throw PreviewDecodeError.offline(path) }
        guard let key = key(path, rate: rate) else {
            noteFailure(path); throw PreviewDecodeError.unreadable("The preview file is missing: " + (path as NSString).lastPathComponent)
        }
        // A joined decode may belong to a prefetch that was cancelled; decode again then.
        let headSeconds = self.headSeconds, fullSeconds = self.fullSeconds
        for _ in 0..<3 {
            enum Step { case hit(PreviewPCM), join(Task<PreviewPCM, Error>), decode(Task<PreviewPCM, Error>) }
            let step: Step = locked {
                if var entry = entries[key] {
                    clock &+= 1; entry.use = clock; entries[key] = entry; hits += 1
                    return .hit(entry.pcm)
                }
                if let running = inflight[key] { return .join(running) }
                misses += 1
                let task = Task.detached(priority: priority) { () throws -> PreviewPCM in
                    try PreviewDecoder.head(path: path, targetRate: rate, headSeconds: headSeconds, fullSeconds: fullSeconds, isCancelled: isStale)
                }
                inflight[key] = task
                return .decode(task)
            }
            switch step {
            case .hit(let pcm): return (pcm, true)
            case .join(let running):
                do { return (try await running.value, false) }
                catch where Self.isCancellation(error) && !isStale() {
                    // The joined decode was cancelled with its owner (a superseded prefetch). Forget it here: the
                    // owner may not have removed it yet, and joining it again would fail the same way.
                    locked { if inflight[key] == running { inflight[key] = nil } }
                    continue
                }
            case .decode(let task):
                do {
                    let pcm = try await task.value
                    locked { if inflight[key] == task { inflight[key] = nil } }
                    insert(pcm, for: key)
                    return (pcm, false)
                } catch {
                    locked { if inflight[key] == task { inflight[key] = nil } }
                    if !Self.isCancellation(error) { noteFailure(path) }
                    throw error
                }
            }
        }
        throw PreviewDecodeError.cancelled
    }

    private func locked<T>(_ body: () -> T) -> T { lock.lock(); defer { lock.unlock() }; return body() }

    private func noteFailure(_ path: String) {
        lock.lock(); failures[path] = Date(); lock.unlock()
    }
}

/// Decides which neighbouring items to warm, nearest first, biased toward the
/// direction of travel. Shared by tests and the host.
enum PreviewPrefetchPlan {
    static func order(selected: Int, count: Int, direction: Int, radius: Int) -> [Int] {
        guard count > 0, selected >= 0, selected < count else { return [] }
        var result = [selected]
        let ahead = direction < 0 ? -1 : 1
        for distance in 1...max(1, radius) {
            for index in [selected + ahead * distance, selected - ahead * distance] where index >= 0 && index < count {
                result.append(index)
            }
        }
        return result
    }
}

/// Owns the persistent preview source node, the decoded-preview cache and the
/// background prefetch queue. Used from the main actor by AudioHost.
@MainActor final class AudioPreviewPlayer {
    let mixer: PreviewVoiceMixer
    let cache: PreviewCache
    let node: AVAudioSourceNode
    let sampleRate: Double
    /// Resolves the group-preview audio of a kit path (a .stkit or Maschine group).
    var kitPreviewResolver: (@Sendable (String) -> String?)?
    private var generation: UInt64 = 0
    private let prefetchGeneration = OSAllocatedUnfairLock(initialState: UInt64(0))
    private var prefetchTask: Task<Void, Never>?
    private(set) var lastStartLatency: Double = 0
    nonisolated private static let maximumPrefetch = 8
    nonisolated private static let streamQueue = DispatchQueue(label: "SuperTracker.preview.stream", qos: .userInitiated, attributes: .concurrent)

    init(format: AVAudioFormat, cache: PreviewCache = PreviewCache()) {
        sampleRate = format.sampleRate
        let mixer = PreviewVoiceMixer(sampleRate: format.sampleRate)
        self.mixer = mixer; self.cache = cache
        node = Self.makeNode(mixer: mixer, format: format)
    }

    /// The render block captures only the mixer: no actor isolation, no allocation.
    nonisolated private static func makeNode(mixer: PreviewVoiceMixer, format: AVAudioFormat) -> AVAudioSourceNode {
        AVAudioSourceNode(format: format) { isSilence, _, frameCount, audioBufferList -> OSStatus in
            let buffers = UnsafeMutableAudioBufferListPointer(audioBufferList)
            let frames = Int(frameCount)
            guard buffers.count >= 2, let l = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let r = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            l.update(repeating: 0, count: frames); r.update(repeating: 0, count: frames)
            isSilence.pointee = ObjCBool(!mixer.render(frames: frames, left: l, right: r))
            return noErr
        }
    }

    /// Decodes or fetches the preview head. Returns nil when superseded by a
    /// newer preview or a stop.
    func load(_ path: String) async throws -> (pcm: PreviewPCM, cached: Bool, started: UInt64)? {
        generation &+= 1
        let token = generation, began = mach_absolute_time()
        let result = try await cache.head(path, rate: sampleRate, priority: .userInitiated)
        guard token == generation else { return nil }
        return (result.pcm, result.cached, began)
    }

    /// Starts `pcm` (crossfading out anything playing) and its tail stream when the file is longer than the head.
    func begin(_ path: String, pcm: PreviewPCM, volume: Float, began: UInt64) {
        var stream: PreviewStream?
        if !pcm.complete {
            let tail = PreviewStream(estimatedFrames: max(0, pcm.estimatedTotalFrames - pcm.frames) + Int(sampleRate))
            stream = tail
            Self.streamQueue.async { PreviewDecoder.streamTail(path: path, after: pcm, into: tail) }
        }
        mixer.start(pcm, stream: stream, volume: volume)
        var timebase = mach_timebase_info_data_t(); mach_timebase_info(&timebase)
        lastStartLatency = Double(mach_absolute_time() - began) * Double(timebase.numer) / Double(timebase.denom) / 1e6
    }

    func stop(fade: Bool = true) { generation &+= 1; mixer.stop(fade: fade) }

    /// Replaces the prefetch queue: stale work is cancelled between decode blocks.
    func prefetch(_ requests: [[String: Any]]) -> [String: Any] {
        let token = prefetchGeneration.withLock { value -> UInt64 in value &+= 1; return value }
        prefetchTask?.cancel()
        let entries = requests.prefix(Self.maximumPrefetch).compactMap { request -> (path: String?, kit: String?)? in
            let path = (request["path"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            let kit = (request["kit"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            return path == nil && kit == nil ? nil : (path, kit)
        }
        guard !entries.isEmpty else { prefetchTask = nil; return ["queued": 0] }
        let cache = self.cache, rate = sampleRate, resolver = kitPreviewResolver, lock = prefetchGeneration
        let stale: @Sendable () -> Bool = { lock.withLock { $0 } != token }
        prefetchTask = Task.detached(priority: .utility) {
            var warmed = 0
            for entry in entries {
                if stale() || Task.isCancelled { break }
                var path = entry.path
                if path == nil, let kit = entry.kit {
                    if PreviewDecoder.volumeOffline(kit) { continue }
                    path = resolver?(kit)
                }
                guard let path, !PreviewDecoder.volumeOffline(path), !cache.recentlyFailed(path) else { continue }
                if let key = cache.key(path, rate: rate), cache.cached(key) != nil { continue }
                if (try? await cache.head(path, rate: rate, priority: .utility, isStale: stale)) != nil { warmed += 1 }
            }
            _ = warmed
        }
        return ["queued": entries.count]
    }

    func status() -> [String: Any] {
        ["cachedPreviews": cache.count, "cacheBytes": cache.bytes, "cacheHits": cache.hits, "cacheMisses": cache.misses,
         "voices": mixer.activeVoices, "lastStartMs": lastStartLatency]
    }
}

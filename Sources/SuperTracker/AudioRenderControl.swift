import Foundation
import Darwin

/// Passive render-thread telemetry. All writers hold the host render lock;
/// control readers make one coherent attempt and never block an audio callback.
final class AudioRenderClockDiagnostics {
    private let words:UnsafeMutablePointer<Int64>
    init() { words = .allocate(capacity:18);words.initialize(repeating:0,count:18) }
    deinit { words.deinitialize(count:18);words.deallocate() }
    private func store(_ value:Int64,_ index:Int) { renderControlAtomicStore(value,words.advanced(by:index)) }
    func stamp(master:Bool,sampleTime:Double,flags:UInt32,frames:UInt32,bus:Int32 = -1,hostTime:UInt64 = 0) {
        OSAtomicIncrement64Barrier(words)
        let base=master ? 1 : 5
        OSAtomicIncrement64Barrier(words.advanced(by:base))
        store(Int64(bitPattern:sampleTime.bitPattern),base+1);store(Int64(flags),base+2);store(Int64(frames),base+3)
        if !master { store(Int64(bus),9) }
        store(Int64(bitPattern:hostTime),master ? 15:16)
        OSAtomicIncrement64Barrier(words)
    }
    func prepared(frame:Int64,status:Int32,epoch:UInt64,flags:Int32) {
        OSAtomicIncrement64Barrier(words)
        OSAtomicIncrement64Barrier(words.advanced(by:10));store(frame,11);store(Int64(status),12)
        store(Int64(bitPattern:epoch),13);store(Int64(flags),14)
        OSAtomicIncrement64Barrier(words)
    }
    func snapshot()->[String:Any]? {
        let before=OSAtomicAdd64Barrier(0,words);guard before & 1 == 0 else { return nil }
        let copy=(1..<17).map { OSAtomicAdd64Barrier(0,words.advanced(by:$0)) }
        guard before == OSAtomicAdd64Barrier(0,words) else { return nil }
        return ["masterCalls":copy[0],"masterSampleTime":Double(bitPattern:UInt64(bitPattern:copy[1])),"masterFlags":copy[2],"masterFrames":copy[3],
            "sourceCalls":copy[4],"sourceSampleTime":Double(bitPattern:UInt64(bitPattern:copy[5])),"sourceFlags":copy[6],"sourceFrames":copy[7],"sourceBus":copy[8],
            "preparedCalls":copy[9],"preparedFrame":copy[10],"retainedClockStatus":copy[11],"retainedClockEpoch":String(UInt64(bitPattern:copy[12])),"retainedClockFlags":copy[13],"masterHostTime":String(UInt64(bitPattern:copy[14])),"sourceHostTime":String(UInt64(bitPattern:copy[15]))]
    }
}

/// Immutable-by-copy routing data: both optionals contain only scalar values. Keeping
/// endpoint lookup and ownership outside this type avoids ARC work in audio callbacks.
struct AudioMIDIRoute: Sendable {
    var external = false
    var destination: Int32? = nil
    var channel: UInt8 = 0
    var output: MIDIHost.OutputRoute? = nil
    /// Host ticks this route's notes are sent ahead of their render time (a synth heard back
    /// through a compensated line input; see AudioHost.returnedNoteLeadSeconds).
    var leadTicks: UInt32 = 0
}

/// Read-only view of exactly `AudioRenderControl.routeCount` routes, one per native
/// instrument. Its storage belongs to AudioRenderControl and
/// is valid until the next serialized `read()` call, or until that owner is destroyed.
/// It must not be retained across another audio callback or passed to another reader.
struct AudioRenderControlSnapshot {
    let tempo: Double
    let routes: UnsafePointer<AudioMIDIRoute>
}

/// A bounded control-to-render handoff. There is one control producer for `publish`
/// and one serialized audio consumer for `read`. The consumer makes one attempt and
/// keeps its previous coherent snapshot if publication is in progress or changes.
/// CPU telemetry travels in the opposite direction through a separate atomic word.
/// The owning audio host must stop its callbacks before releasing this object.
final class AudioRenderControl {
    /// Mirrors ProjectLayout.instrumentSanityLimit (one route per native instrument).
    static let routeCount = 1024
    private static let wordCount = 1 + routeCount * 3
    private let version: UnsafeMutablePointer<Int64>
    private let words: UnsafeMutablePointer<Int64>
    private let cpu: UnsafeMutablePointer<Int64>
    private let first: UnsafeMutablePointer<AudioMIDIRoute>
    private let second: UnsafeMutablePointer<AudioMIDIRoute>

    // These fields belong exclusively to the serialized audio reader. The control
    // producer writes only version/words; it never accesses either decoded buffer.
    private var usingFirst = true
    private var acceptedVersion: Int64 = 0
    private var acceptedTempo = 120.0

    init() {
        version = .allocate(capacity: 1)
        words = .allocate(capacity: Self.wordCount)
        cpu = .allocate(capacity: 1)
        first = .allocate(capacity: Self.routeCount)
        second = .allocate(capacity: Self.routeCount)
        version.initialize(to: 0)
        words.initialize(repeating: 0, count: Self.wordCount)
        words[0] = Int64(bitPattern: Double(120).bitPattern)
        cpu.initialize(to: 0)
        first.initialize(repeating: AudioMIDIRoute(), count: Self.routeCount)
        second.initialize(repeating: AudioMIDIRoute(), count: Self.routeCount)
    }

    deinit {
        version.deinitialize(count: 1); version.deallocate()
        words.deinitialize(count: Self.wordCount); words.deallocate()
        cpu.deinitialize(count: 1); cpu.deallocate()
        first.deinitialize(count: Self.routeCount); first.deallocate()
        second.deinitialize(count: Self.routeCount); second.deallocate()
    }

    /// Control thread only. Short route arrays clear their unused slots; extra routes
    /// are ignored. Plain bit packing preserves nil versus zero and all identifier bits.
    func publish(tempo: Double, routes: [AudioMIDIRoute]) {
        publish(tempo: tempo, routes: routes, afterMarkingBusy: nil)
    }

    /// Control-side test seam for proving that an interrupted publisher cannot stall
    /// audio. The ordinary API passes nil; the render path never stores or invokes it.
    func publish(tempo: Double, routes: [AudioMIDIRoute], afterMarkingBusy: (() -> Void)?) {
        OSAtomicIncrement64Barrier(version) // odd: readers retain their current buffer
        afterMarkingBusy?()
        renderControlAtomicStore(Int64(bitPattern: tempo.bitPattern), words)
        let count = min(Self.routeCount, routes.count)
        for index in 0..<Self.routeCount {
            let route = index < count ? routes[index] : AudioMIDIRoute()
            var metadata = UInt64(UInt32(bitPattern: route.destination ?? 0))
            if route.external { metadata |= 1 << 32 }
            if route.destination != nil { metadata |= 1 << 33 }
            metadata |= UInt64(route.channel) << 40
            let endpoints: UInt64
            var identity = UInt64(route.leadTicks) << 32
            if let output = route.output {
                metadata |= 1 << 34
                endpoints = UInt64(output.port) | UInt64(output.endpoint) << 32
                identity |= UInt64(UInt32(bitPattern: output.uniqueID))
            } else {
                endpoints = 0
            }
            let base = words.advanced(by: 1 + index * 3)
            renderControlAtomicStore(Int64(bitPattern: metadata), base)
            renderControlAtomicStore(Int64(bitPattern: endpoints), base.advanced(by: 1))
            renderControlAtomicStore(Int64(bitPattern: identity), base.advanced(by: 2))
        }
        OSAtomicIncrement64Barrier(version) // even: the complete publication is visible
    }

    /// Audio thread only. No allocation, locks, retries, ARC values, or callback
    /// invocation. All shared words are atomic, so a rejected candidate is data-race
    /// free even when the producer modifies several route fields during the read.
    @inline(__always)
    func read() -> AudioRenderControlSnapshot {
        let before = OSAtomicAdd64Barrier(0, version)
        if before & 1 != 0 || before == acceptedVersion { return currentSnapshot() }
        let candidate = usingFirst ? second : first
        let tempo = Double(bitPattern: UInt64(bitPattern: OSAtomicAdd64Barrier(0, words)))
        for index in 0..<Self.routeCount {
            let base = words.advanced(by: 1 + index * 3)
            let metadata = UInt64(bitPattern: OSAtomicAdd64Barrier(0, base))
            let endpoints = UInt64(bitPattern: OSAtomicAdd64Barrier(0, base.advanced(by: 1)))
            let identity = UInt64(bitPattern: OSAtomicAdd64Barrier(0, base.advanced(by: 2)))
            let output: MIDIHost.OutputRoute?
            if metadata & (1 << 34) != 0 {
                output = MIDIHost.OutputRoute(port: UInt32(truncatingIfNeeded: endpoints),
                    endpoint: UInt32(truncatingIfNeeded: endpoints >> 32),
                    uniqueID: Int32(bitPattern: UInt32(truncatingIfNeeded: identity)))
            } else { output = nil }
            candidate[index] = AudioMIDIRoute(external: metadata & (1 << 32) != 0,
                destination: metadata & (1 << 33) != 0 ? Int32(bitPattern: UInt32(truncatingIfNeeded: metadata)) : nil,
                channel: UInt8(truncatingIfNeeded: metadata >> 40), output: output,
                leadTicks: UInt32(truncatingIfNeeded: identity >> 32))
        }
        let after = OSAtomicAdd64Barrier(0, version)
        if after == before && after & 1 == 0 {
            acceptedTempo = tempo; acceptedVersion = after; usingFirst.toggle()
        }
        return currentSnapshot()
    }

    /// One serialized audio producer. Preserve bits exactly: callers compute and
    /// validate the metric; this type only publishes it atomically.
    @inline(__always)
    func storeCPU(_ value: Double) {
        renderControlAtomicStore(Int64(bitPattern: value.bitPattern), cpu)
    }

    @inline(__always)
    func loadCPU() -> Double {
        Double(bitPattern: UInt64(bitPattern: OSAtomicAdd64Barrier(0, cpu)))
    }

    @inline(__always)
    private func currentSnapshot() -> AudioRenderControlSnapshot {
        AudioRenderControlSnapshot(tempo: acceptedTempo, routes: UnsafePointer(usingFirst ? first : second))
    }
}

/// Each word has exactly one writer; other accesses are atomic loads (add zero).
/// Consequently its value cannot change between these two instructions, and a single
/// CAS suffices. In particular, publishing CPU telemetry never enters a retry loop.
@inline(__always)
private func renderControlAtomicStore(_ value: Int64, _ pointer: UnsafeMutablePointer<Int64>) {
    let old = OSAtomicAdd64Barrier(0, pointer)
    _ = OSAtomicCompareAndSwap64Barrier(old, value, pointer)
}

/// Scalar state owned by the serialized master render observer. Measures the
/// complete master graph interval, including preflight and downstream processors;
/// device conversion and work outside that graph are not part of this interval.
struct AudioRenderUtilization {
    private var startTicks: UInt64?
    private var expectedFrames: UInt32 = 0

    mutating func begin(at ticks: UInt64, frames: UInt32) {
        startTicks = ticks
        expectedFrames = frames
    }

    mutating func finish(at ticks: UInt64, frames: UInt32, sampleRate: Double,
                         secondsPerTick: Double) -> Double? {
        let start = startTicks
        startTicks = nil
        guard let start, ticks >= start, frames > 0, frames == expectedFrames,
              sampleRate.isFinite, sampleRate > 0,
              secondsPerTick.isFinite, secondsPerTick > 0 else { return nil }
        let value = Double(ticks - start) * secondsPerTick * sampleRate / Double(frames) * 100
        return value.isFinite ? value : nil
    }
}

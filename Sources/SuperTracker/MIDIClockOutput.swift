import Foundation
import CoreMIDI
import os

/// MIDI beat clock (24 pulses per quarter note) to the chosen MIDI outputs, sent from the audio render thread
/// with host-time stamps. Clock alone sets a device's tempo (arpeggiators, synced LFOs and delays) without
/// running its sequencer; Start/Stop/Continue, which do run it, go only to outputs with transport enabled.
/// While the transport is stopped the clock keeps running at the project tempo, so devices stay in tempo.
final class MIDIClockOutput {
    struct Target { var route: MIDIHost.OutputRoute; var transport: Bool }
    static let capacity = 64
    static let pulsesPerBeat = 24.0

    // Written on the control thread under the lock; the render thread only try-locks (never waits).
    private let lock: UnsafeMutablePointer<os_unfair_lock>
    private let targets: UnsafeMutablePointer<Target>
    private var count = 0
    private var lead = 0.0
    // Render-thread state.
    private var wasPlaying = false
    private var freeBeat = 0.0
    /// Tests observe (status, timestamp) here instead of CoreMIDI. Production leaves it nil.
    var testSink: ((UInt8, UInt64) -> Void)?

    init() {
        lock = .allocate(capacity: 1); lock.initialize(to: os_unfair_lock())
        targets = .allocate(capacity: Self.capacity)
    }
    deinit { lock.deallocate(); targets.deallocate() }

    /// Seconds the clock runs ahead of the song, so a device whose audio returns through the interface
    /// is heard in time with SuperTracker's own sounds.
    func setLead(_ seconds: Double) {
        os_unfair_lock_lock(lock); defer { os_unfair_lock_unlock(lock) }
        lead = seconds.isFinite ? max(0, min(0.5, seconds)) : 0
    }

    var isActive: Bool { os_unfair_lock_lock(lock); defer { os_unfair_lock_unlock(lock) }; return count > 0 }

    func setTargets(_ items: [Target]) {
        os_unfair_lock_lock(lock); defer { os_unfair_lock_unlock(lock) }
        // A device that loses transport (or the clock) while playing gets a Stop so its sequencer does not hang on.
        if wasPlaying {
            for index in 0..<count where targets[index].transport
                && !items.contains(where: { $0.transport && $0.route.endpoint == targets[index].route.endpoint }) {
                send(targets[index].route, 0xFC, 0)
            }
        }
        count = min(items.count, Self.capacity)
        for index in 0..<count { targets[index] = items[index] }
    }

    /// One render quantum. `beat` is the song position (quarter-note beats) at its first frame.
    @inline(__always) private func send(_ route: MIDIHost.OutputRoute, _ status: UInt8, _ stamp: UInt64, _ data1: UInt8 = 0, _ data2: UInt8 = 0) {
        if let testSink { testSink(status, stamp) } else { route.send(status: status, data1: data1, data2: data2, timestamp: stamp) }
    }

    func render(beat: Double, playing: Bool, tempo: Double, frames: UInt32, sampleRate: Double,
                hostTime: UInt64, ticksPerFrame: Double) {
        guard hostTime > 0, tempo.isFinite, tempo > 0, sampleRate > 0, beat.isFinite, os_unfair_lock_trylock(lock) else { return }
        defer { os_unfair_lock_unlock(lock) }
        guard count > 0 else { wasPlaying = playing; return }
        let beatsPerFrame = tempo / 60 / sampleRate, span = Double(frames) * beatsPerFrame
        let start = playing ? max(0, beat) : freeBeat
        if playing != wasPlaying {
            for index in 0..<count where targets[index].transport {
                let route = targets[index].route
                if !playing { send(route, 0xFC, hostTime) }
                else if start < 1e-6 { send(route, 0xFA, hostTime) }
                else {
                    // Song Position Pointer counts sixteenth notes; Continue resumes from it.
                    let sixteenths = min(16383, Int((start * 4).rounded(.down)))
                    send(route, 0xF2, hostTime, UInt8(sixteenths & 0x7F), UInt8(sixteenths >> 7))
                    send(route, 0xFB, hostTime)
                }
            }
            wasPlaying = playing
        }
        // The pulses of the beats `lead` ahead of this block are sent during it.
        let ahead = start + lead * tempo / 60
        var pulse = (ahead * Self.pulsesPerBeat - 1e-9).rounded(.up)
        while pulse / Self.pulsesPerBeat < ahead + span {
            let offset = max(0, (pulse / Self.pulsesPerBeat - ahead) / beatsPerFrame)
            let stamp = hostTime + UInt64(offset * ticksPerFrame)
            for index in 0..<count { send(targets[index].route, 0xF8, stamp) }
            pulse += 1
        }
        freeBeat = start + span
        if freeBeat > 1e9 { freeBeat = 0 }
    }
}

/// The clock choice, kept across launches: whether clock is sent, and per MIDI output (by CoreMIDI unique ID)
/// whether it gets clock and whether it gets Start/Stop. Outputs default to clock on, Start/Stop off.
struct MIDIClockPreferences: Codable, Equatable {
    struct Device: Codable, Equatable { var clock = true; var transport = false }
    var enabled = false
    var devices: [String: Device] = [:]
    /// Clock lead in milliseconds; nil follows the measured input-path latency.
    var leadMilliseconds: Double?

    static func load(_ url: URL?) -> MIDIClockPreferences {
        guard let url, let data = try? Data(contentsOf: url) else { return MIDIClockPreferences() }
        return (try? JSONDecoder().decode(MIDIClockPreferences.self, from: data)) ?? MIDIClockPreferences()
    }
    func save(_ url: URL?) throws {
        guard let url else { return }
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONEncoder().encode(self).write(to: url, options: .atomic)
    }
    func device(_ id: Int32) -> Device { devices[String(id)] ?? Device() }

    /// Applies a `midi.clock` request: `enabled`, and `devices` as [{id, clock, transport}].
    mutating func apply(_ payload: [String: Any]) {
        if let value = payload["enabled"] as? Bool { enabled = value }
        if payload["leadMilliseconds"] is NSNull { leadMilliseconds = nil }
        else if let value = (payload["leadMilliseconds"] as? NSNumber)?.doubleValue, value.isFinite { leadMilliseconds = max(0, min(500, value)) }
        for item in payload["devices"] as? [[String: Any]] ?? [] {
            guard let id = (item["id"] as? NSNumber)?.int32Value else { continue }
            var entry = device(id)
            if let clock = item["clock"] as? Bool { entry.clock = clock }
            if let transport = item["transport"] as? Bool { entry.transport = transport }
            devices[String(id)] = entry
        }
    }
}

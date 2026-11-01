import Foundation
import CoreMIDI

// MIDI clock output (MIDIClockOutput): 24 pulses per beat in time with the song position, a free-running clock
// while stopped, and Start/Stop only for outputs with transport enabled.
@main
struct MIDIClockOutputTests {
    static func main() throws {
        func require(_ ok: Bool, _ message: String) { if !ok { print("FAIL MIDI clock: \(message)"); exit(1) } }
        let clock = MIDIClockOutput()
        var sent: [(UInt8, UInt64)] = []
        clock.testSink = { sent.append(($0, $1)) }
        let route = MIDIHost.OutputRoute(port: 1, endpoint: 2, uniqueID: 3)
        // 120 BPM at 48 kHz: one beat is 24 000 frames, one pulse 1000 frames. One host tick per frame.
        func block(_ beat: Double, playing: Bool, frames: UInt32 = 4800, host: UInt64 = 1_000_000) {
            clock.render(beat: beat, playing: playing, tempo: 120, frames: frames, sampleRate: 48_000, hostTime: host, ticksPerFrame: 1)
        }
        block(0, playing: true)
        require(sent.isEmpty, "nothing is sent without outputs")
        clock.setTargets([.init(route: route, transport: false)])
        block(0, playing: true)
        let pulses = sent.filter { $0.0 == 0xF8 }
        require(pulses.count == 5 && pulses.map(\.1) == [1_000_000, 1_001_000, 1_002_000, 1_003_000, 1_004_000], "pulses every 1/24 beat: \(sent)")
        require(!sent.contains { [0xFA, 0xFB, 0xFC].contains($0.0) }, "clock-only outputs never get Start/Stop: \(sent)")

        sent.removeAll(); clock.setTargets([.init(route: route, transport: true)])
        block(0.2, playing: true)
        sent.removeAll(); block(0.4, playing: false)
        require(sent.first?.0 == 0xFC, "Stop goes to transport outputs: \(sent)")
        require(sent.filter { $0.0 == 0xF8 }.count == 5, "the clock keeps running while stopped: \(sent)")
        sent.removeAll(); block(0, playing: true)
        require(sent.first?.0 == 0xFA && sent.dropFirst().first?.0 == 0xF8, "Play from the start sends Start before the first pulse: \(sent)")
        sent.removeAll(); block(0.2, playing: false); sent.removeAll(); block(2.5, playing: true)
        require(sent.count >= 2 && sent[0].0 == 0xF2 && sent[1].0 == 0xFB, "Play mid-song sends Song Position then Continue: \(sent)")
        sent.removeAll(); block(1.0 / 48, playing: true, frames: 600)
        require(sent.filter { $0.0 == 0xF8 }.count == 1 && sent[0].1 == 1_000_500, "a pulse between block starts lands at its frame: \(sent)")
        sent.removeAll(); clock.setTargets([])
        require(sent.first?.0 == 0xFC, "removing a transport output while playing stops its sequencer: \(sent)")

        sent.removeAll(); clock.setTargets([.init(route: route, transport: false)]); clock.setLead(0.01)
        block(0.2, playing: true); sent.removeAll(); block(1.0 / 48, playing: true, frames: 600)
        require(sent.filter { $0.0 == 0xF8 }.count == 1 && abs(Int64(sent[0].1) - 1_000_020) <= 1, "a 10 ms lead sends each pulse 10 ms (480 frames) early: \(sent)")
        clock.setLead(0)

        var preferences = MIDIClockPreferences()
        require(preferences.device(9) == .init(clock: true, transport: false), "outputs default to clock on, Start/Stop off")
        preferences.apply(["enabled": true, "devices": [["id": NSNumber(value: 9), "transport": true]]])
        require(preferences.enabled && preferences.device(9) == .init(clock: true, transport: true), "a request changes only the fields it names")
        preferences.apply(["leadMilliseconds": NSNumber(value: 18)]); require(preferences.leadMilliseconds == 18, "a manual clock lead is kept")
        preferences.apply(["leadMilliseconds": NSNull()]); require(preferences.leadMilliseconds == nil, "an empty lead returns to automatic")
        print("PASS 9 MIDI clock checks")
    }
}

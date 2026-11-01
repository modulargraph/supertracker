import AudioToolbox
import Foundation
import VST3Host

@main enum VST3TransportContextTests {
    struct Failure: Error { let message: String }
    static var checks = 0, groups = 0
    static func require(_ value: @autoclosure () -> Bool, _ message: String) throws {
        guard value() else { throw Failure(message: message) }; checks += 1
    }
    static func has(_ context: STVST3Transport, _ bit: UInt32) -> Bool { context.valid_fields & bit != 0 }
    static func stamp(_ value: Double, valid: Bool = true) -> AudioTimeStamp {
        var timestamp = AudioTimeStamp(); timestamp.mSampleTime = value
        if valid { timestamp.mFlags = .sampleTimeValid }
        return timestamp
    }
    static func main() throws {
        let explicit = UInt32(ST_VST3_TRANSPORT_EXPLICIT), continuous = UInt32(ST_VST3_TRANSPORT_CONTINUOUS_VALID)
        let music = UInt32(ST_VST3_TRANSPORT_MUSIC_VALID), tempoFlag = UInt32(ST_VST3_TRANSPORT_TEMPO_VALID)
        let meter = UInt32(ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID), transportFlag = UInt32(ST_VST3_TRANSPORT_TRANSPORT_VALID)
        let barFlag = UInt32(ST_VST3_TRANSPORT_BAR_VALID), cycleFlag = UInt32(ST_VST3_TRANSPORT_CYCLE_VALID)

        let absent = VST3TransportContext.make(timestamp: stamp(1234), musical: nil, transport: nil)
        try require(absent.sample_position == 1234 && absent.continuous_sample_position == 1234, "Valid graph fallback coordinates changed")
        try require(absent.valid_fields == explicit | continuous, "Absent callbacks fabricated valid tempo/music/meter/transport")
        try require(absent.playing == 0 && absent.cycling == 0 && absent.recording == 0, "Absent callbacks fabricated active transport")
        groups += 1

        var musicalCalls = 0, transportCalls = 0, requestedFields = false
        let failedMusic: AUHostMusicalContextBlock = { tempo, numerator, denominator, beat, nextBeat, downbeat in
            musicalCalls += 1; requestedFields = tempo != nil && numerator != nil && denominator != nil && beat != nil && downbeat != nil && nextBeat == nil
            tempo?.pointee = 177; numerator?.pointee = 7; denominator?.pointee = 8
            beat?.pointee = 19.5; downbeat?.pointee = 17.5; return false
        }
        let failedTransport: AUHostTransportStateBlock = { flags, sample, start, end in
            transportCalls += 1; flags?.pointee = [.moving, .cycling, .recording]
            sample?.pointee = 9000; start?.pointee = 16; end?.pointee = 32; return false
        }
        let failed = VST3TransportContext.make(timestamp: stamp(1234), musical: failedMusic, transport: failedTransport)
        try require(musicalCalls == 1 && transportCalls == 1 && requestedFields, "Wrong AU callback fields or repeated callback")
        try require(failed.sample_position == 1234 && failed.valid_fields == explicit | continuous, "A false callback leaked its outputs")
        try require(failed.playing == 0 && failed.cycling == 0 && failed.recording == 0, "A false callback leaked transport flags")
        groups += 1

        let partialMusic: AUHostMusicalContextBlock = { tempo, _, _, _, _, _ in tempo?.pointee = 95; return true }
        let partialTransport: AUHostTransportStateBlock = { _, sample, _, _ in sample?.pointee = -640; return true }
        let partial = VST3TransportContext.make(timestamp: stamp(80_000), musical: partialMusic, transport: partialTransport)
        try require(partial.valid_fields == explicit | continuous | tempoFlag, "Untouched partial outputs were treated as valid")
        try require(partial.tempo == 95 && partial.sample_position == -640 && partial.continuous_sample_position == 80_000, "Partial valid fields were lost")
        let flagsOnly: AUHostTransportStateBlock = { flags, _, _, _ in flags?.pointee = [.moving]; return true }
        let flags = VST3TransportContext.make(timestamp: stamp(1000), musical: nil, transport: flagsOnly)
        try require(flags.sample_position == 1000 && flags.playing == 1 && has(flags, transportFlag), "Partial flags callback failed its safe graph fallback")
        groups += 1

        // The callbacks already describe the individual processor's block. Do
        // not extrapolate using graph time: seeks, loops and PDC separate them.
        let positions: [(Int64, Int64, Double, Double)] = [
            (96_000, 48_000, 2, 0),       // playing before a seek
            (96_512, 12_000, 0.5, 0),    // seek backwards, graph keeps advancing
            (97_024, 24_000, 1, 0),      // loop returns to its start
            (97_536, 23_936, 23936.0 / 24_000, 0), // processor's latency-adjusted coordinate
            (98_048, -12_000, -0.5, -4)  // negative preroll
        ]
        for (graph, project, beat, bar) in positions {
            var sawSamplePointer = false, sawCyclePointers = false
            let musical: AUHostMusicalContextBlock = { tempo, numerator, denominator, position, _, downbeat in
                tempo?.pointee = 120; numerator?.pointee = 4; denominator?.pointee = 4
                position?.pointee = beat; downbeat?.pointee = bar; return true
            }
            let transport: AUHostTransportStateBlock = { flags, position, start, end in
                sawSamplePointer = position != nil; sawCyclePointers = start != nil && end != nil
                flags?.pointee = [.moving, .cycling, .recording, .changed]; position?.pointee = Double(project)
                start?.pointee = 1; end?.pointee = 2; return true
            }
            let result = VST3TransportContext.make(timestamp: stamp(Double(graph)), musical: musical, transport: transport)
            try require(sawSamplePointer && sawCyclePointers, "AU timeline/cycle outputs were not requested")
            try require(result.sample_position == project && result.continuous_sample_position == graph, "Project time was confused with continuous graph time")
            try require(result.beat_position == beat && result.bar_start_beat == bar, "Processor beat or downbeat was extrapolated")
            try require(result.cycle_start_beat == 1 && result.cycle_end_beat == 2 && result.cycling == 1, "Valid cycle was lost")
            try require(result.playing == 1 && result.recording == 1 && result.valid_fields == explicit | 127, "Valid callback fields/flags were lost")
        }
        groups += 1

        let stopped: AUHostTransportStateBlock = { flags, position, start, end in
            flags?.pointee = [.recording]; position?.pointee = 441; start?.pointee = 1; end?.pointee = 9; return true
        }
        let stop = VST3TransportContext.make(timestamp: stamp(99_999), musical: nil, transport: stopped)
        try require(stop.sample_position == 441 && stop.playing == 0 && stop.recording == 1, "Stopped recording/prepared state was lost")
        try require(!has(stop, cycleFlag) && stop.cycling == 0, "Noncycling AU callback fabricated valid cycle bounds")
        groups += 1

        for value in [Double.nan, Double.infinity, -Double.infinity, 1e15, -1e15, 1.5, -1.5] {
            let result = VST3TransportContext.make(timestamp: stamp(value), musical: nil, transport: nil)
            try require(result.valid_fields == explicit && result.sample_position == 0, "Invalid/fractional graph sample became a valid clock")
            let transport: AUHostTransportStateBlock = { flags, position, _, _ in flags?.pointee = []; position?.pointee = value; return true }
            let fallback = VST3TransportContext.make(timestamp: stamp(2048), musical: nil, transport: transport)
            try require(fallback.sample_position == 2048 && fallback.continuous_sample_position == 2048, "Invalid/fractional AU position did not use safe graph fallback")
        }
        let unflagged = VST3TransportContext.make(timestamp: stamp(4567, valid: false), musical: nil, transport: nil)
        try require(unflagged.valid_fields == explicit && unflagged.sample_position == 0, "Unflagged graph timestamp advertised valid position")
        let callbackOnly: AUHostTransportStateBlock = { flags, position, _, _ in flags?.pointee = []; position?.pointee = -9; return true }
        let onlyProject = VST3TransportContext.make(timestamp: stamp(4567, valid: false), musical: nil, transport: callbackOnly)
        try require(onlyProject.sample_position == -9 && !has(onlyProject, continuous), "Valid AU project coordinate depended on unavailable graph coordinate")
        groups += 1

        for (numerator, denominator) in [(Double.nan, 4), (Double.infinity, 4), (-1.0, 4), (0.0, 4), (3.5, 4),
                                         (Double(Int32.max) + 1, 4), (4.0, 0), (4.0, -1), (4.0, Int.max)] {
            let callback: AUHostMusicalContextBlock = { tempo, top, bottom, beat, _, bar in
                tempo?.pointee = 120; top?.pointee = numerator; bottom?.pointee = denominator; beat?.pointee = 1; bar?.pointee = 0; return true
            }
            let result = VST3TransportContext.make(timestamp: stamp(0), musical: callback, transport: nil)
            try require(!has(result, meter) && has(result, tempoFlag) && has(result, music) && has(result, barFlag), "Malformed meter trapped or contaminated other valid fields")
        }
        groups += 1

        for value in [Double.nan, Double.infinity, -1, 0, 10_001] {
            let callback: AUHostMusicalContextBlock = { tempo, _, _, beat, _, _ in tempo?.pointee = value; beat?.pointee = 0; return true }
            let result = VST3TransportContext.make(timestamp: stamp(0), musical: callback, transport: nil)
            try require(!has(result, tempoFlag) && has(result, music), "Invalid tempo advertised valid BPM or lost independent music")
        }
        for (beat, bar, expectedMusic, expectedBar) in [(Double.nan, -4.0, false, true), (1.0, Double.infinity, true, false),
                                                       (1.0, 2.0, true, false), (1e12, 1e12, false, false), (-0.5, -4.0, true, true)] {
            let callback: AUHostMusicalContextBlock = { _, _, _, position, _, downbeat in position?.pointee = beat; downbeat?.pointee = bar; return true }
            let result = VST3TransportContext.make(timestamp: stamp(0), musical: callback, transport: nil)
            try require(has(result, music) == expectedMusic && has(result, barFlag) == expectedBar, "Invalid beat/downbeat advertised incorrect validity")
        }
        groups += 1

        for (start, end) in [(Double.nan, 4.0), (0.0, Double.infinity), (4.0, 4.0), (8.0, 4.0), (-1e12, 4.0)] {
            let callback: AUHostTransportStateBlock = { flags, _, first, last in flags?.pointee = [.moving, .cycling]; first?.pointee = start; last?.pointee = end; return true }
            let result = VST3TransportContext.make(timestamp: stamp(0), musical: nil, transport: callback)
            try require(result.playing == 1 && has(result, transportFlag) && !has(result, cycleFlag) && result.cycling == 0,
                        "Malformed cycle advertised active/valid loop or lost independent playing state")
        }
        let invalidFlags: AUHostTransportStateBlock = { flags, position, _, _ in flags?.pointee = AUHostTransportStateFlags(rawValue: UInt.max); position?.pointee = 99; return true }
        let invalid = VST3TransportContext.make(timestamp: stamp(0), musical: nil, transport: invalidFlags)
        try require(!has(invalid, transportFlag) && invalid.playing == 0 && invalid.sample_position == 99, "Untouched flags sentinel was accepted or destroyed a valid sample position")
        groups += 1

        print("PASS \(groups) VST3 AU transport callback groups / \(checks) checks: exact project/continuous clocks, seek/loop/PDC/preroll coordinates, optional outputs, bounds and validity")
    }
}

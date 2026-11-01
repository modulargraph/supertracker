import AudioToolbox
import VST3Host

/// AU callback outputs describe this processor's current block. Project sample
/// position can jump at seek/loop boundaries; the graph clock is a distinct value.
/// The fallback fields are deliberately not advertised as optional valid data.
enum VST3TransportContext {
    static func make(timestamp: AudioTimeStamp, musical: AUHostMusicalContextBlock?,
                     transport: AUHostTransportStateBlock?) -> STVST3Transport {
        var context = STVST3Transport()
        context.valid_fields = UInt32(ST_VST3_TRANSPORT_EXPLICIT)
        context.tempo = 120; context.numerator = 4; context.denominator = 4
        if timestamp.mFlags.contains(.sampleTimeValid), let graphSample = sample(timestamp.mSampleTime) {
            context.sample_position = graphSample
            context.continuous_sample_position = graphSample
            context.valid_fields |= UInt32(ST_VST3_TRANSPORT_CONTINUOUS_VALID)
        }

        var tempo = Double.nan, numerator = Double.nan, denominator = 0
        var beat = Double.nan, bar = Double.nan
        if musical?(&tempo, &numerator, &denominator, &beat, nil, &bar) == true {
            if tempo.isFinite, tempo > 0, tempo <= 10_000 {
                context.tempo = tempo; context.valid_fields |= UInt32(ST_VST3_TRANSPORT_TEMPO_VALID)
            }
            if numerator.isFinite, numerator >= 1, numerator <= Double(Int32.max),
               numerator.rounded(.towardZero) == numerator, denominator >= 1, denominator <= Int(Int32.max) {
                context.numerator = Int32(numerator); context.denominator = Int32(denominator)
                context.valid_fields |= UInt32(ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID)
            }
            if musicalPosition(beat) {
                context.beat_position = beat; context.valid_fields |= UInt32(ST_VST3_TRANSPORT_MUSIC_VALID)
            }
            if musicalPosition(bar), !musicalPosition(beat) || bar <= beat {
                context.bar_start_beat = bar; context.valid_fields |= UInt32(ST_VST3_TRANSPORT_BAR_VALID)
            }
        }

        // Invalid sentinels distinguish an untouched optional pointer from valid
        // zero/stopped state when a partially implemented callback returns true.
        var flags = AUHostTransportStateFlags(rawValue: UInt.max)
        var projectSample = Double.nan, cycleStart = Double.nan, cycleEnd = Double.nan
        if transport?(&flags, &projectSample, &cycleStart, &cycleEnd) == true {
            if let position = sample(projectSample) { context.sample_position = position }
            let knownFlags = AUHostTransportStateFlags.changed.rawValue | AUHostTransportStateFlags.moving.rawValue |
                AUHostTransportStateFlags.recording.rawValue | AUHostTransportStateFlags.cycling.rawValue
            if flags.rawValue & ~knownFlags == 0 {
                context.valid_fields |= UInt32(ST_VST3_TRANSPORT_TRANSPORT_VALID)
                context.playing = flags.contains(.moving) ? 1 : 0
                context.recording = flags.contains(.recording) ? 1 : 0
                if flags.contains(.cycling), musicalPosition(cycleStart), musicalPosition(cycleEnd), cycleStart < cycleEnd {
                    context.cycling = 1; context.cycle_start_beat = cycleStart; context.cycle_end_beat = cycleEnd
                    context.valid_fields |= UInt32(ST_VST3_TRANSPORT_CYCLE_VALID)
                }
            }
        }
        return context
    }

    private static func sample(_ value: Double) -> Int64? {
        guard value.isFinite, abs(value) < 1e15, value.rounded(.towardZero) == value else { return nil }
        return Int64(value)
    }
    private static func musicalPosition(_ value: Double) -> Bool { value.isFinite && abs(value) < 1e12 }
}

import Foundation

/// Project-wide sizes and the engine's channel numbering, mirrored in pure Swift so model
/// code builds without the engine module (the audio host checks they agree at startup).
///
/// Tracks and instruments are sized per project; the limits only reject absurd input.
/// Channels are tracks (channel == track index); the return bus follows them but never
/// sits below 16, so projects with up to 16 tracks keep the historical return bus 16.
/// Kits are track groups: their members are ordinary tracks.
enum ProjectLayout {
    /// Mirrors ST_TRACK_SANITY_LIMIT / ST_INSTRUMENT_SANITY_LIMIT / ST_MIN_RETURN_BUS in TrackerEngine.h.
    static let trackSanityLimit = 1024
    static let instrumentSanityLimit = 1024
    static let minimumReturnBus = 16
    static func returnBus(trackCount: Int) -> Int { max(minimumReturnBus, trackCount) }
    /// Logical channels the engine accepts for a track count (st_channel_count): the tracks
    /// plus silent padding up to the return bus.
    static func channelCount(trackCount: Int) -> Int { returnBus(trackCount: trackCount) }
    /// Largest channel index space any valid project can use.
    static let channelCapacity = channelCount(trackCount: trackSanityLimit)

    static let nativeAutomationRanges:[String:ClosedRange<Double>] = [
        "volume":0...2,"pan":-1...1,"cutoff":20...20000,"resonance":0...1,"delay":0...1,"reverb":0...1,
        "pitch":-48...48,"attack":0...10,"decay":0...10,"sustain":0...1,"release":0...20,
        "drive":0...1,"bitDepth":1...24,"lfoRate":0...100,"lfoDepth":0...1,"start":0...1,"end":0...1,
        "loopStart":0...1,"loopEnd":0...1,"grainSize":0.005...2,"grainPosition":0...1,"sliceCount":1...128,
        "instrumentVolume":0...2,"instrumentPan":-1...1,"tune":-48...48,"filterAttack":0...10,
        "filterDecay":0...10,"filterSustain":0...1,"filterRelease":0...20,"filterEnvAmount":-1...1,
        "lfo2Rate":0...100,"lfo2Depth":0...1,"lfoBeats":0.03125...64,"lfo2Beats":0.03125...64,"wavetablePosition":0...1,
        "grainDensity":1...8,"grainSpray":0...1,"grainPitchSpread":0...24,"grainScan":-4...4,"grainReverse":0...1,"grainStereo":0...1]

    /// The project's instruments (within the sanity limit), as the engine numbers them.
    static func instruments(_ project: [String: Any]) -> [[String: Any]] {
        Array((project["instruments"] as? [[String: Any]] ?? []).prefix(instrumentSanityLimit))
    }

    /// Track mix settings by channel.
    static func channels(_ project: [String: Any]) -> [Int: [String: Any]] {
        Dictionary(uniqueKeysWithValues: (project["tracks"] as? [[String: Any]] ?? []).enumerated().map { ($0.offset, $0.element) })
    }
}

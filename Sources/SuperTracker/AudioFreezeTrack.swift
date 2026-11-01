import Foundation
import AVFoundation
import CryptoKit

/// User-facing Track Freeze built on the existing offline track bounce.
///
/// Design (deliberately simple and robust):
/// * Canonical project: every note, instrument, plug-in descriptor (with its
///   captured state), clip and automation lane stays in the project. A frozen
///   track only gains `tracks[i].frozen`, a small record that names the printed
///   WAV file. Save/Open/Undo therefore need nothing special; ProjectStore
///   collects the print like any other referenced audio file.
/// * Runtime projection (`runtimeProject`): the Host never sees the frozen
///   track's plug-ins, notes, source automation or original clips. Its plug-ins
///   are unloaded by the ordinary rack restore (CPU released), and the print is
///   added as a sample-aligned timeline clip on the same track. The clip enters
///   the track at the point where the instrument used to enter, so fader, pan,
///   mute, solo, sends and downstream routing stay live and are applied once.
/// * Print (`renderProject`): the existing `audio.export` track mode renders the
///   song with the track at unity/centre/unmuted, sends at zero and its mix
///   automation removed, so the print is "instrument + inserts" only. Latency
///   compensation is trimmed by the export, so print frame 0 is song frame 0.
///
/// * Pattern prints: every pattern the track uses is also printed twice, as a
///   single pass + tail ("head", the first pass after Play) and as the steady-
///   state looping pass + tail ("loop", rendered as the pattern repeated until
///   the tail has wrapped). Pattern mode plays them through engine pattern clips
///   (st_engine_add_pattern_clip) aligned to the pattern's rows, so Play from the
///   cursor row, loop ranges, wraps and pattern exports match the live track.
///
/// Known semantic limits (documented in docs/TRACK_FREEZE_UX.md):
/// * Projects with a tempo map get no pattern prints (silent in Pattern mode).
/// * Flat (non-routed) mixer: inserts originally followed the fader and the
///   delay/reverb sends tapped the dry instrument. Frozen playback applies the
///   fader after the printed inserts and sends tap the printed sound. With a
///   unity fader, centred pan and no sends the result is identical.
/// * Tail: the print contains the effect tail. In a looping song the print
///   clip ends at the musical end so the loop length is unchanged (the tail
///   that would ring over the loop point is dropped). In a non-looping song and
///   in every export (exports never loop) the whole tail plays; the song's
///   natural end therefore moves out by the printed tail.
enum AudioFreezeTrack {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// Mix targets stay live on a frozen track (they are applied after the print).
    static let mixTargets: Set<String> = ["volume", "pan", "delay", "reverb"]
    static let recordKey = "frozen"
    static let printClipPrefix = "freeze-print-"


    /// One pattern's prints: `path` = first pass + tail, `loopPath` = steady-state
    /// looping pass + tail. Frame 0 of both is the pattern's row 0.
    struct PatternPrint {
        let pattern: Int, path: String, sha256: String, loopPath: String, loopSha256: String
        let frames: Int64, passFrames: Int64, loopFrames: Int64, passes: Int

        var dictionary: [String: Any] {
            // The loop file is a nested {path} so ProjectStore collects/relocates it like any media path.
            ["pattern": pattern, "path": path, "sha256": sha256, "frames": frames, "passFrames": passFrames, "passes": passes,
             "loop": ["path": loopPath, "sha256": loopSha256, "frames": loopFrames] as [String: Any]]
        }

        static func parse(_ value: Any?) -> PatternPrint? {
            guard let raw = value as? [String: Any], let path = raw["path"] as? String, !path.isEmpty,
                  let loop = raw["loop"] as? [String: Any], let loopPath = loop["path"] as? String, !loopPath.isEmpty,
                  let pattern = (raw["pattern"] as? NSNumber)?.intValue, (0..<256).contains(pattern) else { return nil }
            let frames = (raw["frames"] as? NSNumber)?.int64Value ?? 0, pass = (raw["passFrames"] as? NSNumber)?.int64Value ?? 0
            let loopFrames = (loop["frames"] as? NSNumber)?.int64Value ?? 0
            guard frames > 0, pass > 0, loopFrames > 0 else { return nil }
            return PatternPrint(pattern: pattern, path: path, sha256: raw["sha256"] as? String ?? "", loopPath: loopPath,
                                loopSha256: loop["sha256"] as? String ?? "", frames: frames, passFrames: pass, loopFrames: loopFrames,
                                passes: (raw["passes"] as? NSNumber)?.intValue ?? 2)
        }
    }

    struct Record {
        let id: String, path: String, sha256: String
        let sampleRate: Double, frames: Int64, musicalFrames: Int64, tailFrames: Int64
        let musicalEndBeat: Double, tailSeconds: Double
        let dormantPluginIDs: [String], fingerprint: String, mixMode: String, createdAt: String
        var patternPrints: [PatternPrint] = []
        /// Patterns with content that could not be printed (tempo map or too many patterns).
        var unprintedPatterns: [Int] = []
        /// Flat mixer: the live pre-insert gain (track volume x master) the inserts were
        /// printed at; playback divides it out so the unchanged fader is exact (see `printGain`).
        var printGain: Double = 1
        /// "auto": tail ended where the print fell below the silence threshold (bounded by
        /// `tailSeconds`); "fixed": exactly `tailSeconds`.
        var tailMode = "fixed"
        /// "offline" (rendered) or "recorded" (hardware return captured in real time).
        var method = "offline"

        var dictionary: [String: Any] {
            ["version": 1, "id": id, "path": path, "sha256": sha256, "sampleRate": sampleRate,
             "frames": frames, "musicalFrames": musicalFrames, "tailFrames": tailFrames,
             "musicalEndBeat": musicalEndBeat, "tailSeconds": tailSeconds,
             "dormantPluginIDs": dormantPluginIDs, "fingerprint": fingerprint,
             "mixMode": mixMode, "createdAt": createdAt,
             "patternPrints": patternPrints.map(\.dictionary), "unprintedPatterns": unprintedPatterns,
             "printGain": printGain, "tailMode": tailMode, "method": method]
        }

        static func parse(_ value: Any?) throws -> Record {
            guard let raw = value as? [String: Any] else { throw Failure(message: "Freeze record must be an object.") }
            func number(_ key: String) throws -> Double {
                guard let n = PayloadValue.number(raw[key], in: -.greatestFiniteMagnitude ... .greatestFiniteMagnitude) else {
                    throw Failure(message: "Freeze record is missing \(key).")
                }
                return n
            }
            func string(_ key: String) throws -> String {
                guard let s = raw[key] as? String, !s.isEmpty else { throw Failure(message: "Freeze record is missing \(key).") }
                return s
            }
            guard (raw["version"] as? NSNumber)?.intValue == 1 else { throw Failure(message: "Unsupported freeze record version.") }
            func frameCount(_ key: String) throws -> Int64 {
                let value = try number(key)
                guard (0...100_000_000).contains(value), value.rounded(.towardZero) == value,
                      let exact = Int64(exactly:value) else {
                    throw Failure(message: "Freeze record has an invalid \(key) frame count.")
                }
                return exact
            }
            let rate = try number("sampleRate"), frames = try frameCount("frames")
            let musical = try frameCount("musicalFrames"), tail = try frameCount("tailFrames")
            let end = try number("musicalEndBeat")
            guard (8_000...384_000).contains(rate), musical > 0, tail >= 0, frames == musical + tail, frames <= 100_000_000, end > 0 else {
                throw Failure(message: "Freeze record has inconsistent frame counts.")
            }
            return Record(id: try string("id"), path: try string("path"), sha256: raw["sha256"] as? String ?? "",
                          sampleRate: rate, frames: frames, musicalFrames: musical, tailFrames: tail,
                          musicalEndBeat: end, tailSeconds: (raw["tailSeconds"] as? NSNumber)?.doubleValue ?? 0,
                          dormantPluginIDs: raw["dormantPluginIDs"] as? [String] ?? [],
                          fingerprint: raw["fingerprint"] as? String ?? "", mixMode: raw["mixMode"] as? String ?? "flat",
                          createdAt: raw["createdAt"] as? String ?? "",
                          patternPrints: (raw["patternPrints"] as? [Any] ?? []).compactMap(PatternPrint.parse),
                          unprintedPatterns: (raw["unprintedPatterns"] as? [Any] ?? []).compactMap { ($0 as? NSNumber)?.intValue },
                          printGain: (raw["printGain"] as? NSNumber).map { max(minimumPrintGain, min(maximumPrintGain, $0.doubleValue)) } ?? 1,
                          tailMode: raw["tailMode"] as? String == "auto" ? "auto" : "fixed",
                          method: raw["method"] as? String == "recorded" ? "recorded" : "offline")
        }
    }

    struct Frozen { let index: Int, owner: String?, record: Record }

    // MARK: Canonical inspection

    static func hasFrozenTracks(_ project: [String: Any]) -> Bool {
        (project["tracks"] as? [[String: Any]] ?? []).contains { $0[recordKey] != nil }
    }

    /// Valid frozen tracks. A malformed record is reported by `issues` and is
    /// treated as live (never a half-frozen graph).
    static func frozenTracks(_ project: [String: Any]) -> (frozen: [Frozen], issues: [Int: String]) {
        var result: [Frozen] = [], issues: [Int: String] = [:]
        for (index, track) in (project["tracks"] as? [[String: Any]] ?? []).enumerated() where track[recordKey] != nil {
            do { result.append(Frozen(index: index, owner: owner(track), record: try Record.parse(track[recordKey]))) }
            catch { issues[index] = error.localizedDescription }
        }
        return (result, issues)
    }

    static func owner(_ track: [String: Any]) -> String? {
        (track["routingID"] as? String).map { "track:" + $0 }
    }

    /// Explicit plug-in descriptors, mirroring AudioHost.pluginDescriptions for
    /// legacy projects that keep plug-ins on instruments/tracks.
    static func pluginDescriptors(_ project: [String: Any]) -> [[String: Any]] {
        if let explicit = project["audioPlugins"] as? [[String: Any]] { return explicit }
        let instruments = project["instruments"] as? [[String: Any]] ?? []
        var result: [[String: Any]] = []
        for (track, item) in (project["tracks"] as? [[String: Any]] ?? []).enumerated() {
            let instrument = PayloadValue.integer(item["instrument"], 0)
            if instruments.indices.contains(instrument), var plugin = instruments[instrument]["plugin"] as? [String: Any], plugin["component"] != nil {
                plugin["track"] = track; plugin["kind"] = "instrument"; plugin["slot"] = 0; result.append(plugin)
            }
            for (slot, value) in (item["plugins"] as? [Any] ?? []).enumerated() {
                guard var plugin = value as? [String: Any], plugin["component"] != nil else { continue }
                plugin["track"] = track; plugin["kind"] = "effect"; plugin["slot"] = slot; result.append(plugin)
            }
        }
        return result
    }

    static func pluginBelongs(_ plugin: [String: Any], index: Int, owner: String?) -> Bool {
        if let pluginOwner = plugin["owner"] as? String { return pluginOwner == owner }
        return PayloadValue.integer(plugin["track"], 0) == index
    }

    static func plugins(of index: Int, in project: [String: Any]) -> [[String: Any]] {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { return [] }
        let owner = owner(tracks[index])
        return pluginDescriptors(project).filter { pluginBelongs($0, index: index, owner: owner) }
    }

    private static func laneBelongs(_ lane: [String: Any], index: Int, owner: String?) -> Bool {
        if let laneOwner = lane["owner"] as? String { return laneOwner == owner }
        return PayloadValue.integer(lane["track"], 0) == index
    }

    private static func isMixLane(_ lane: [String: Any]) -> Bool {
        let target = lane["target"] as? String ?? "volume"
        return mixTargets.contains(target) || target.hasPrefix("send:")
    }

    private static func isMixFX(_ fx: Any?) -> Bool {
        guard let fx = fx as? [String: Any] else { return false }
        return mixTargets.contains(fx["type"] as? String ?? "")
    }

    /// True when `pattern` holds printable content for the track: a note or
    /// source command (mix commands excluded) or a source automation lane.
    static func trackUses(_ pattern: [String: Any], index: Int, owner: String?) -> Bool {
        let steps = pattern["steps"] as? [[Any]] ?? []
        if steps.indices.contains(index), steps[index].contains(where: { raw in
            guard var step = raw as? [String: Any] else { return false }
            for key in ["fx1", "fx2"] where isMixFX(step[key]) { step.removeValue(forKey: key) }
            return !step.isEmpty
        }) { return true }
        return (pattern["automation"] as? [[String: Any]] ?? []).contains { laneBelongs($0, index: index, owner: owner) && !isMixLane($0) }
    }

    static let maximumPatternPrints = 48

    /// Pattern ids to print for the track: sequenced patterns first (song order),
    /// then other used patterns; at most `maximumPatternPrints`.
    static func patternsToPrint(_ project: [String: Any], index: Int) -> (print: [Int], skipped: [Int]) {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { return ([], []) }
        let owner = owner(tracks[index])
        var used: [Int: Bool] = [:], order: [Int] = []
        for (position, pattern) in (project["patterns"] as? [[String: Any]] ?? []).enumerated() {
            let id = PayloadValue.integer(pattern["id"], position)
            if (0..<256).contains(id), trackUses(pattern, index: index, owner: owner) { used[id] = true }
        }
        for id in (project["sequence"] as? [Any] ?? [0]).map({ PayloadValue.integer($0, 0) }) where used[id] == true && !order.contains(id) { order.append(id) }
        for id in used.keys.sorted() where !order.contains(id) { order.append(id) }
        return (Array(order.prefix(maximumPatternPrints)), Array(order.dropFirst(maximumPatternPrints)))
    }

    /// Exact frame length of one pass of `pattern` (rows, swing, tempo).
    static func passFrames(_ project: [String: Any], pattern: Int, sampleRate: Double) -> Double {
        let patterns = project["patterns"] as? [[String: Any]] ?? []
        let item = patterns.enumerated().first { PayloadValue.integer($0.element["id"], $0.offset) == pattern }?.element ?? [:]
        let rows = Double(max(1, min(256, PayloadValue.integer(item["length"], 64))))
        let swing = (project["swing"] as? NSNumber)?.doubleValue ?? 0
        let lpb = Double(max(1, PayloadValue.integer(project["linesPerBeat"], 4))), bpm = (project["bpm"] as? NSNumber)?.doubleValue ?? 120
        let beats = (rows + (Int(rows) % 2 == 1 ? swing : 0)) / lpb
        return beats * 60 / bpm * sampleRate
    }

    /// Song-transport render of one pattern repeated `passes` times (pattern
    /// export semantics: no timeline clips).
    static func patternRenderProject(_ render: [String: Any], pattern: Int, passes: Int) -> [String: Any] {
        var result = render
        result["sequence"] = Array(repeating: pattern, count: max(1, passes))
        result["activePattern"] = pattern
        result["audioClips"] = [[String: Any]]()
        result.removeValue(forKey: "songLoop")
        result.removeValue(forKey: "patternClips")
        return result
    }

    // MARK: Fingerprint (staleness)

    /// Everything that is baked into the print. Mix controls, names and colours
    /// are excluded; tempo, sequence and pattern lengths are included.
    static func fingerprint(_ project: [String: Any], index: Int) throws -> String {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { throw Failure(message: "Choose an existing track.") }
        return try AudioFreezeTrackFingerprint.fingerprint(project, index: index)
    }

    // MARK: Admission

    /// Routing shapes whose meaning would change when the print replaces the
    /// live chain are rejected with a concrete reason instead of approximated.
    /// Why the track's sound is made outside the computer (nil when it renders offline):
    /// a live line input (the hardware return) or an external MIDI instrument.
    static func hardwareSource(_ project: [String: Any], index: Int) -> String? {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { return nil }
        let track = tracks[index]
        if (track["audioInput"] as? [String: Any])?["enabled"] as? Bool == true { return "receives a live line input" }
        let instruments = project["instruments"] as? [[String: Any]] ?? []
        let instrument = PayloadValue.integer(track["instrument"], 0)
        if instruments.indices.contains(instrument), instruments[instrument]["type"] as? String == "midi" { return "plays an external MIDI instrument" }
        return nil
    }

    /// Mixer the track plays through: "flat" or "routed" (standard routing graph).
    static func mixMode(_ project: [String: Any]) -> String {
        guard let routing = project["routing"] as? [String: Any], routing["enabled"] as? Bool != false else { return "flat" }
        return "routed"
    }

    /// `recorded`: freeze by recording the hardware return in real time. Offline freeze
    /// refuses hardware tracks (it would print silence); recording needs the return as
    /// this track's line input. A routed project is recorded through `flatTakeProject`.
    static func validateFreezable(_ project: [String: Any], index: Int, recorded: Bool = false, requireInput: Bool = true) throws {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { throw Failure(message: "Choose an existing track to freeze.") }
        let track = tracks[index]
        let name = track["name"] as? String ?? "Track \(index + 1)"
        guard track[recordKey] == nil else { throw Failure(message: "\(name) is already frozen.") }
        let hardware = hardwareSource(project, index: index)
        if let hardware, !recorded {
            throw Failure(message: "\(name) \(hardware), so its sound cannot be rendered offline. Choose Freeze by recording to capture its return in real time.")
        }
        if recorded {
            guard !requireInput || (track["audioInput"] as? [String: Any])?["enabled"] as? Bool == true else {
                throw Failure(message: "\(name) has no live line input to record. Route the hardware’s audio return into this track (Line inputs) before freezing by recording.")
            }
        }
        if let routing = project["routing"] as? [String: Any], routing["enabled"] as? Bool != false, let owner = owner(track) {
            for send in routing["sends"] as? [[String: Any]] ?? [] {
                if send["destination"] as? String == owner { throw Failure(message: "\(name) receives a send. Remove the send before freezing.") }
                if send["source"] as? String == owner, send["tap"] as? String == "preInserts" {
                    throw Failure(message: "\(name) has a pre-insert send, which would hear the printed effects. Change it to pre- or post-fader before freezing.")
                }
            }
            for sidechain in routing["sidechains"] as? [[String: Any]] ?? [] where sidechain["source"] as? String == owner && sidechain["tap"] as? String == "preInserts" {
                throw Failure(message: "\(name) feeds a pre-insert sidechain. Change its tap before freezing.")
            }
            for (_, destination) in routing["outputs"] as? [String: Any] ?? [:] where destination as? String == owner {
                throw Failure(message: "Another channel is routed into \(name). Freeze the source channel or change its output first.")
            }
        }
    }

    // MARK: Gain staging

    static let minimumPrintGain = 0.25, maximumPrintGain = 2.0

    /// The print point is post-insert, pre-fader: fader, pan, mute/solo, sends and mix
    /// automation stay live after it. Standard routing already places the fader after the
    /// inserts, so a unity print is exact. The flat mixer applies track volume x master
    /// *before* the inserts, so a nonlinear insert (compressor, saturation) heard a
    /// different level live than a unity print would give it. There the inserts are
    /// printed at the live pre-insert gain and playback divides it out: unchanged faders
    /// are exact, and later fader moves act after the printed inserts.
    static func printGain(_ project: [String: Any], index: Int) -> Double {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index), mixMode(project) == "flat",
              plugins(of: index, in: project).contains(where: { $0["kind"] as? String == "effect" }) else { return 1 }
        let value = PayloadValue.number(tracks[index], "volume", 0.8) * PayloadValue.number(project, "masterVolume", 0.7)
        return (minimumPrintGain...maximumPrintGain).contains(value) ? value : 1
    }

    /// Remaining (documented) differences of a frozen flat-mixer track, for the reply.
    static func stagingNotes(_ project: [String: Any], index: Int) -> [String] {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index), mixMode(project) == "flat",
              plugins(of: index, in: project).contains(where: { $0["kind"] as? String == "effect" }) else { return [] }
        let track = tracks[index]
        return PayloadValue.number(track, "delaySend", 0) > 0 || PayloadValue.number(track, "reverbSend", 0) > 0
            ? ["Its delay/reverb sends now hear the printed inserts (live they tapped the instrument before the inserts)."] : []
    }

    // MARK: Render project (print)

    /// The project used for the offline print: the track at unity/centre,
    /// unmuted, no sends, master at unity and mix automation removed. Other
    /// frozen tracks keep their prints, limited to their musical end, so the
    /// song duration seen by the export is the canonical song duration.
    static func renderProject(_ canonical: [String: Any], index: Int) throws -> [String: Any] {
        var result = canonical
        var tracks = canonical["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { throw Failure(message: "Choose an existing track to freeze.") }
        let trackOwner = owner(tracks[index])
        for i in tracks.indices { tracks[i]["solo"] = false }
        // Track export renders master at unity; the flat pre-insert gain rides on the track volume.
        tracks[index]["volume"] = printGain(canonical, index: index); tracks[index]["pan"] = 0.0; tracks[index]["mute"] = false
        tracks[index]["delaySend"] = 0.0; tracks[index]["reverbSend"] = 0.0
        result["tracks"] = tracks
        result["masterVolume"] = 1.0
        var patterns = canonical["patterns"] as? [[String: Any]] ?? []
        for p in patterns.indices {
            var steps = patterns[p]["steps"] as? [[Any]] ?? []
            if steps.indices.contains(index) {
                steps[index] = steps[index].map { raw -> Any in
                    guard var step = raw as? [String: Any] else { return raw }
                    for key in ["fx1", "fx2"] where isMixFX(step[key]) { step.removeValue(forKey: key) }
                    return step
                }
                patterns[p]["steps"] = steps
            }
            patterns[p]["automation"] = (patterns[p]["automation"] as? [[String: Any]] ?? []).filter {
                !(laneBelongs($0, index: index, owner: trackOwner) && isMixLane($0))
            }
        }
        result["patterns"] = patterns
        var projected = try runtimeProject(result, tails: false)
        projected.removeValue(forKey: "patternClips")
        return projected
    }

    /// A take records the track strip of the flat mixer. A routed project's print project
    /// plays flat for the take: the track's own chain is unchanged, and the print's unity
    /// fader and master (`printGain` is 1 when routed) make the flat pre-insert gain unity
    /// too, so the print equals the routed strip after its inserts. Buses and their inserts
    /// only shaped the monitor mix of the take, so they are left out until `restore`.
    static func flatTakeProject(_ project: [String: Any]) -> [String: Any] {
        guard mixMode(project) == "routed" else { return project }
        var result = project
        result.removeValue(forKey: "routing")
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        result["audioPlugins"] = (project["audioPlugins"] as? [[String: Any]])?.compactMap { plugin -> [String: Any]? in
            guard let owner = plugin["owner"] as? String else { return plugin }
            if owner.hasPrefix("bus:") { return nil }
            var plugin = plugin
            if owner.hasPrefix("track:"), plugin["track"] == nil,
               let index = tracks.firstIndex(where: { self.owner($0) == owner }) { plugin["track"] = index }
            return plugin
        }
        result["patterns"] = (project["patterns"] as? [[String: Any]])?.map { pattern in
            var pattern = pattern
            pattern["automation"] = (pattern["automation"] as? [[String: Any]] ?? []).filter { !(($0["owner"] as? String)?.hasPrefix("bus:") ?? false) }
            return pattern
        }
        return result
    }

    static func withoutLineInputs(_ project: [String: Any]) -> [String: Any] {
        var result = project
        result["tracks"] = (project["tracks"] as? [[String: Any]] ?? []).map { track in
            guard var input = track["audioInput"] as? [String: Any], input["enabled"] as? Bool == true else { return track }
            var track = track; input["enabled"] = false; track["audioInput"] = input
            return track
        }
        return result
    }

    // MARK: Runtime projection

    /// Canonical -> Host project. Idempotent for projects without frozen tracks
    /// (returns the input unchanged) and never fails because of a missing print:
    /// an unavailable print is silent and reported by `status`.
    /// `tails: false` limits every print to its musical end regardless of loop.
    static func runtimeProject(_ canonical: [String: Any], tails: Bool = true) throws -> [String: Any] {
        let frozen = frozenTracks(canonical).frozen
        if frozen.isEmpty { return canonical }
        var result = canonical
        let indices = Set(frozen.map(\.index))
        // Plug-ins of frozen tracks are removed from the Host project: the rack
        // unloads them. Their descriptors/state remain in the canonical project.
        var dormantIDs = Set<String>()
        let allPlugins = pluginDescriptors(canonical)
        var livePlugins: [[String: Any]] = []
        for plugin in allPlugins {
            if frozen.contains(where: { pluginBelongs(plugin, index: $0.index, owner: $0.owner) }) {
                if let id = plugin["instanceID"] as? String { dormantIDs.insert(id) }
            } else { livePlugins.append(plugin) }
        }
        result["audioPlugins"] = livePlugins
        var tracks = canonical["tracks"] as? [[String: Any]] ?? []
        for i in indices where tracks.indices.contains(i) {
            tracks[i].removeValue(forKey: "plugins")
            // A recorded (hardware) print replaces the live return: the line input is closed,
            // so it neither doubles the print nor holds hardware-return latency in the plan.
            if var input = tracks[i]["audioInput"] as? [String: Any], input["enabled"] as? Bool == true {
                input["enabled"] = false; tracks[i]["audioInput"] = input
            }
        }
        if canonical["audioPlugins"] == nil {
            // Explicit descriptors now describe everything; drop legacy fields.
            for i in tracks.indices { tracks[i].removeValue(forKey: "plugins") }
            if var instruments = canonical["instruments"] as? [[String: Any]] {
                for i in instruments.indices { instruments[i].removeValue(forKey: "plugin") }
                result["instruments"] = instruments
            }
        }
        result["tracks"] = tracks
        // Notes and source FX of frozen tracks are printed. Only live mix step
        // commands remain, as control-only rows.
        var patterns = canonical["patterns"] as? [[String: Any]] ?? []
        for p in patterns.indices {
            var steps = patterns[p]["steps"] as? [[Any]] ?? []
            for track in steps.indices where indices.contains(track) {
                steps[track] = steps[track].map { raw -> Any in
                    guard let step = raw as? [String: Any] else { return NSNull() }
                    let note = PayloadValue.integer(step["note"], -3)
                    var live: [String: Any] = [:]
                    if note >= 0 || note == -3 {
                        for key in ["fx1", "fx2"] where isMixFX(step[key]) { live[key] = step[key] }
                    }
                    return live.isEmpty ? NSNull() : live
                }
            }
            patterns[p]["steps"] = steps
            patterns[p]["automation"] = (patterns[p]["automation"] as? [[String: Any]] ?? []).filter { lane in
                for item in frozen where laneBelongs(lane, index: item.index, owner: item.owner) { return isMixLane(lane) }
                return true
            }
        }
        result["patterns"] = patterns
        // Original clips on a frozen track are part of the print.
        var clips = (canonical["audioClips"] as? [[String: Any]] ?? []).filter { !indices.contains(PayloadValue.integer($0["track"], 0)) }
        for item in frozen.sorted(by: { $0.index < $1.index }) {
            let name = (tracks.indices.contains(item.index) ? tracks[item.index]["name"] as? String : nil) ?? "Track \(item.index + 1)"
            clips.append(["id": printClipPrefix + item.record.id, "name": name + " (frozen)", "path": item.record.path,
                          "track": item.index, "startBeat": 0.0, "durationBeats": item.record.musicalEndBeat,
                          "sourceOffset": 0.0, "gain": 1 / item.record.printGain, "fadeIn": 0.0, "fadeOut": 0.0, "muted": false,
                          "freezePrint": true, "freezeMusicalEndBeat": item.record.musicalEndBeat, "freezeMusicalFrames": item.record.musicalFrames,
                          "freezeTail": tails ? "auto" : "never"])
        }
        result["audioClips"] = clips
        // Pattern prints play through engine pattern clips (Pattern mode only),
        // within the engine's 256 clip-asset budget.
        var patternClips = canonical["patternClips"] as? [[String: Any]] ?? []
        var budget = 256 - clips.count - 2 * patternClips.count
        for item in frozen.sorted(by: { $0.index < $1.index }) {
            for print in item.record.patternPrints where budget >= 2 {
                patternClips.append(["path": print.path, "loopPath": print.loopPath, "track": item.index,
                                     "pattern": print.pattern, "gain": 1 / item.record.printGain, "freezePrint": true])
                budget -= 2
            }
        }
        if !patternClips.isEmpty { result["patternClips"] = patternClips }
        if !dormantIDs.isEmpty, var routing = canonical["routing"] as? [String: Any] {
            routing["sidechains"] = (routing["sidechains"] as? [[String: Any]] ?? []).filter { sidechain in
                guard let destination = sidechain["destination"] as? [String: Any], let id = destination["instanceID"] as? String else { return true }
                return !dormantIDs.contains(id)
            }
            result["routing"] = routing
        }
        result["freezeRuntime"] = true
        return result
    }

    /// Final engine-level adjustment (AudioHost.applyEngineProject). Export and
    /// non-looping songs play the whole printed tail; looping songs stop the
    /// print at the musical end so the loop length is unchanged.
    static func engineProject(_ data: [String: Any]) -> [String: Any] {
        if data["freezeRuntime"] as? Bool == true, data["patternExport"] as? Bool == true {
            // Pattern export renders the active pattern once on the song transport
            // without timeline clips: play each frozen track's first-pass print
            // from beat 0 with its whole tail (exports never loop).
            let active = PayloadValue.integer(data["activePattern"], 0)
            var result = data
            var clips = data["audioClips"] as? [[String: Any]] ?? []
            for clip in data["patternClips"] as? [[String: Any]] ?? [] where PayloadValue.integer(clip["pattern"], -1) == active {
                clips.append(["id": printClipPrefix + "pattern-\(active)-\(PayloadValue.integer(clip["track"], 0))", "path": clip["path"] ?? "",
                              "track": clip["track"] ?? 0, "startBeat": 0.0, "durationBeats": 0.0, "sourceOffset": 0.0,
                              "gain": clip["gain"] ?? 1.0, "fadeIn": 0.0, "fadeOut": 0.0, "muted": false])
            }
            result["audioClips"] = clips
            result.removeValue(forKey: "patternClips")
            return result
        }
        guard data["freezeRuntime"] as? Bool == true, let clips = data["audioClips"] as? [[String: Any]],
              clips.contains(where: { $0["freezePrint"] as? Bool == true }) else { return data }
        let loop = data["loopSong"] as? Bool ?? false
        var result = data
        var tails: [[String: Any]] = []
        result["audioClips"] = clips.map { clip -> [String: Any] in
            guard clip["freezePrint"] as? Bool == true else { return clip }
            var value = clip
            let end = (clip["freezeMusicalEndBeat"] as? NSNumber)?.doubleValue ?? 0
            if loop, clip["freezeTail"] as? String != "never", let start = (clip["freezeMusicalFrames"] as? NSNumber)?.int64Value, start > 0 {
                // The printed tail rings over the whole-song loop point (engine song tail),
                // exactly where the live chain's tail overlaps the next pass.
                tails.append(["path": clip["path"] ?? "", "track": clip["track"] ?? 0, "startFrame": start, "gain": clip["gain"] ?? 1.0])
            }
            if loop || clip["freezeTail"] as? String == "never" {
                // A hair inside the musical end: the print never extends the song.
                value["durationBeats"] = max(1e-6, end - 1e-7)
            } else {
                value["durationBeats"] = 0.0 // source-limited: whole tail
            }
            return value
        }
        if !tails.isEmpty { result["songTailClips"] = tails }
        return result
    }

    /// Re-add dormant descriptors (with retained state) to a live plug-in list,
    /// so snapshots/events never drop a frozen track's plug-ins.
    static func mergingDormant(_ live: [[String: Any]], canonical: [String: Any]?) -> [[String: Any]] {
        guard let canonical, hasFrozenTracks(canonical) else { return live }
        let frozen = frozenTracks(canonical).frozen
        guard !frozen.isEmpty else { return live }
        func key(_ plugin: [String: Any]) -> String {
            let tracks = canonical["tracks"] as? [[String: Any]] ?? []
            let index = PayloadValue.integer(plugin["track"], 0)
            let owner = plugin["owner"] as? String ?? (tracks.indices.contains(index) ? owner(tracks[index]) : nil) ?? "legacy-\(index)"
            return "\(owner):\(plugin["kind"] as? String ?? "effect"):\(PayloadValue.integer(plugin["slot"], 0))"
        }
        var result = live
        var present = Set(live.map(key))
        let liveIDs = Set(live.compactMap { $0["instanceID"] as? String })
        for plugin in pluginDescriptors(canonical) where frozen.contains(where: { pluginBelongs(plugin, index: $0.index, owner: $0.owner) }) {
            if let id = plugin["instanceID"] as? String, liveIDs.contains(id) { continue }
            guard present.insert(key(plugin)).inserted else { continue }
            result.append(plugin)
        }
        return result
    }

    static func frozenIndex(forPluginPayload payload: [String: Any], canonical: [String: Any]?) -> Int? {
        guard let canonical else { return nil }
        let frozen = frozenTracks(canonical).frozen
        if let owner = payload["owner"] as? String { return frozen.first { $0.owner == owner }?.index }
        guard payload["track"] != nil else { return nil }
        let track = PayloadValue.integer(payload["track"], -1)
        return frozen.first { $0.index == track }?.index
    }

    // MARK: Files

    static func printDirectory() throws -> URL {
        let base: URL
        if let override = ProcessInfo.processInfo.environment["SUPERTRACKER_SUPPORT_DIR"], override.hasPrefix("/") {
            base = URL(fileURLWithPath: override, isDirectory: true)
        } else {
            base = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
                .appendingPathComponent("SuperTracker", isDirectory: true)
        }
        let folder = base.appendingPathComponent("Freeze", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        // Export destinations must not traverse symbolic links (e.g. /var -> /private/var).
        // (Foundation's resolvingSymlinksInPath maps /private/var back to /var.)
        guard let resolved = realpath(folder.path, nil) else { return folder }
        defer { free(resolved) }
        return URL(fileURLWithPath: String(cString: resolved), isDirectory: true)
    }

    static func sha256(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hash = SHA256()
        while let data = try handle.read(upToCount: 1 << 20), !data.isEmpty { hash.update(data: data) }
        return hash.finalize().map { String(format: "%02x", $0) }.joined()
    }

    static func auditionMessage(_ track: [String: Any], index: Int) -> String {
        let name = track["name"] as? String ?? "Track \(index + 1)"
        return "\(name) is frozen: its instrument is printed to audio and its plug-ins are released. Unfreeze it to audition or play notes."
    }

    // MARK: Tail length

    /// Auto tail: a print ends where it stays below this level (-90 dBFS offline; a recorded
    /// hardware return has an analogue noise floor, so -66 dBFS), plus a short margin.
    static let offlineSilence: Float = 0.000_031_6, recordedSilence: Float = 0.000_5
    static let defaultMaximumTail = 10.0
    static func tailMargin(_ rate: Double) -> Int64 { Int64(0.02 * rate) }

    /// Frames to keep: the musical part plus the tail up to the last sample above
    /// `threshold` and the margin. A note cut, OFF or choke that silences the chain early
    /// therefore gives a short print; a long reverb keeps ringing up to the rendered bound.
    static func audibleLength(_ url: URL, musicalFrames: Int64, threshold: Float) throws -> Int64 {
        let file = try AVAudioFile(forReading: url)
        guard musicalFrames < file.length else { return file.length }
        guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 65_536) else {
            throw Failure(message: "Cannot allocate the tail analysis buffer.")
        }
        file.framePosition = max(0, musicalFrames)
        var last = musicalFrames - 1
        while file.framePosition < file.length {
            let start = file.framePosition
            try file.read(into: buffer, frameCount: AVAudioFrameCount(min(65_536, file.length - start)))
            let frames = Int(buffer.frameLength)
            if frames == 0 { break }
            for channel in 0..<Int(buffer.format.channelCount) {
                guard let data = buffer.floatChannelData?[channel] else { continue }
                var index = frames - 1
                while index >= 0, abs(data[index]) <= threshold { index -= 1 }
                if index >= 0 { last = max(last, start + Int64(index)) }
            }
        }
        return min(file.length, max(musicalFrames, last + 1 + tailMargin(file.processingFormat.sampleRate)))
    }

    /// Shortens a float print in place to its first `frames` frames, with a short fade
    /// over the margin so a cut noise floor never clicks.
    static func truncate(_ url: URL, to frames: Int64, fadeFrames: Int64 = 0) throws {
        let input = try AVAudioFile(forReading: url)
        guard frames < input.length else { return }
        let temporary = url.deletingLastPathComponent().appendingPathComponent(".trim-" + UUID().uuidString + ".wav")
        defer { try? FileManager.default.removeItem(at: temporary) }
        do {
            let output = try AVAudioFile(forWriting: temporary, settings: AudioExportFormat.float32.settings,
                                         commonFormat: .pcmFormatFloat32, interleaved: false)
            guard let buffer = AVAudioPCMBuffer(pcmFormat: input.processingFormat, frameCapacity: 65_536) else {
                throw Failure(message: "Cannot allocate the print trim buffer.")
            }
            let fadeStart = frames - min(frames, fadeFrames)
            while input.framePosition < frames {
                let start = input.framePosition
                try input.read(into: buffer, frameCount: AVAudioFrameCount(min(65_536, frames - start)))
                if buffer.frameLength == 0 { break }
                if start + Int64(buffer.frameLength) > fadeStart, fadeFrames > 0 {
                    for channel in 0..<Int(buffer.format.channelCount) {
                        guard let data = buffer.floatChannelData?[channel] else { continue }
                        for i in 0..<Int(buffer.frameLength) where start + Int64(i) >= fadeStart {
                            data[i] *= Float(frames - start - Int64(i)) / Float(fadeFrames)
                        }
                    }
                }
                try output.write(from: buffer)
            }
        }
        _ = try FileManager.default.replaceItemAt(url, withItemAt: temporary)
    }

    /// Copies frames [start, end of file) of a float print into a new file.
    static func trimPrint(_ source: URL, from start: Int64, to destination: URL) throws -> Int64 {
        let input = try AVAudioFile(forReading: source)
        guard start >= 0, start < input.length else { throw Failure(message: "The pattern print is shorter than its loop start.") }
        let output = try AVAudioFile(forWriting: destination, settings: AudioExportFormat.float32.settings,
                                     commonFormat: .pcmFormatFloat32, interleaved: false)
        guard let buffer = AVAudioPCMBuffer(pcmFormat: input.processingFormat, frameCapacity: 65_536) else {
            throw Failure(message: "Cannot allocate the pattern print buffer.")
        }
        input.framePosition = start
        var written: Int64 = 0
        while input.framePosition < input.length {
            try input.read(into: buffer, frameCount: AVAudioFrameCount(min(65_536, input.length - input.framePosition)))
            if buffer.frameLength == 0 { break }
            try output.write(from: buffer); written += Int64(buffer.frameLength)
        }
        return written
    }

}

/// Host-side state: the last canonical project the UI published. The Host's
/// own `project` holds the runtime projection.
final class AudioFreezeTrackRuntime {
    var canonical: [String: Any]?
    var busy = false
    var publication = UUID()
    /// Track meter taps feed a running freeze-by-recording take (AudioFreezeRecording).
    let tap = AudioFreezeTakeTap()
    func mergeDormant(_ live: [[String: Any]]) -> [[String: Any]] {
        AudioFreezeTrack.mergingDormant(live, canonical: canonical)
    }
}

extension AudioHost {
    /// Public command entry. Projects canonical projects for the Host and keeps
    /// every project/plug-in list returned to the UI canonical.
    @MainActor func handle(_ command: String, payload: [String: Any]) async throws -> Any {
        let runtime = freezeTrackRuntime
        switch command {
        case "freeze.track": return try await freezeTrack(payload)
        case "freeze.unfreeze": return try await unfreezeTrack(payload)
        case "freeze.status": return freezeStatus()
        default: break
        }
        let projectCommands: Set<String> = ["engine.project", "routing.apply"]
        if runtime.busy, projectCommands.contains(command) || command.hasPrefix("plugin.load") || command == "plugin.remove" || command == "plugin.move" {
            throw AudioFreezeTrack.Failure(message: "Wait for the track freeze to finish.")
        }
        if projectCommands.contains(command) {
            let raw: [String: Any]?
            if command == "engine.project" { raw = payload["project"] as? [String: Any] ?? payload }
            else { raw = payload["project"] as? [String: Any] }
            guard let raw else { return try await handleCore(command, payload: payload) }
            // The canonical document is the current model: retired fields and
            // instrument types are migrated before freezing projects it.
            let canonical = ProjectMigration.migrated(raw)
            let previous = runtime.canonical, previousPublication = runtime.publication
            let publication = UUID()
            let projected = try AudioFreezeTrack.runtimeProject(canonical)
            var next = payload
            next["project"] = projected
            runtime.publication = publication
            runtime.canonical = canonical
            do {
                let result = try await handleCore(command, payload: next)
                if command != "engine.project", AudioFreezeTrack.hasFrozenTracks(canonical), var value = result as? [String: Any] {
                    value["project"] = canonical
                    if value["routing"] != nil { value["routing"] = canonical["routing"] ?? NSNull() }
                    return value
                }
                return result
            } catch {
                if runtime.publication == publication {
                    runtime.canonical = previous; runtime.publication = previousPublication
                }
                throw error
            }
        }
        if command == "step.preview" || command == "note.on", let canonical = runtime.canonical, AudioFreezeTrack.hasFrozenTracks(canonical) {
            // Live audition of a frozen track would be silent (released AU) or dry
            // (native instrument without its released inserts): say why instead.
            let index = PayloadValue.integer(payload["track"], -1)
            let note = command == "note.on" ? PayloadValue.integer(payload["note"], 60) : PayloadValue.integer((payload["step"] as? [String: Any])?["note"], -1)
            let tracks = canonical["tracks"] as? [[String: Any]] ?? []
            if note >= 0, tracks.indices.contains(index), tracks[index][AudioFreezeTrack.recordKey] != nil {
                throw AudioFreezeTrack.Failure(message: AudioFreezeTrack.auditionMessage(tracks[index], index: index))
            }
        }
        if ["plugin.load", "plugin.loadNKS", "plugin.remove", "plugin.move", "plugin.editor", "plugin.preset"].contains(command),
           let index = AudioFreezeTrack.frozenIndex(forPluginPayload: payload, canonical: runtime.canonical) {
            let tracks = runtime.canonical?["tracks"] as? [[String: Any]] ?? []
            let name = tracks.indices.contains(index) ? tracks[index]["name"] as? String ?? "Track \(index + 1)" : "This track"
            throw AudioFreezeTrack.Failure(message: "\(name) is frozen. Unfreeze it to change its plug-ins.")
        }
        let result = try await handleCore(command, payload: payload)
        guard let canonical = runtime.canonical, AudioFreezeTrack.hasFrozenTracks(canonical) else { return result }
        if command == "engine.snapshot", var snapshot = result as? [String: Any], snapshot["freezeRuntime"] as? Bool == true {
            let plugins = snapshot["audioPlugins"]
            snapshot = canonical
            snapshot["audioPlugins"] = plugins
            return snapshot
        }
        if command == "plugin.move", var value = result as? [String: Any], let plugins = value["audioPlugins"] as? [[String: Any]] {
            value["audioPlugins"] = runtime.mergeDormant(plugins)
            if value["routing"] != nil { value["routing"] = canonical["routing"] ?? NSNull() }
            return value
        }
        return result
    }

    /// `freeze.track {track, tail?, maxTail?, method?, patterns?, monitor?}`.
    /// * tail: seconds (fixed), or omitted/"auto": each print keeps its tail until it falls
    ///   below the silence threshold, bounded by maxTail (default 10 s, at most 30).
    /// * method "record": freeze by recording (hardware tracks; see AudioFreezeRecording).
    ///   patterns: false records only the song print (Pattern mode then stays silent).
    @MainActor private func freezeTrack(_ payload: [String: Any]) async throws -> [String: Any] {
        let runtime = freezeTrackRuntime
        guard !runtime.busy else { throw AudioFreezeTrack.Failure(message: "A track freeze is already in progress.") }
        guard let canonical = runtime.canonical else { throw AudioFreezeTrack.Failure(message: "Load a project before freezing a track.") }
        let index = PayloadValue.integer(payload["track"], -1)
        let recorded = payload["method"] as? String == "record"
        #if SELFTESTS
        let requireInput = payload["testAnySource"] as? Bool != true
        #else
        let requireInput = true
        #endif
        try AudioFreezeTrack.validateFreezable(canonical, index: index, recorded: recorded, requireInput: requireInput)
        let current = status()
        if current["playing"] as? Bool == true || current["playPreparing"] as? Bool == true { throw AudioFreezeTrack.Failure(message: "Stop playback before freezing.") }
        if current["recording"] as? Bool == true { throw AudioFreezeTrack.Failure(message: "Finish recording before freezing.") }
        let auto = !(payload["tail"] is NSNumber)
        let maximumTail = max(0, min(30, (payload["maxTail"] as? NSNumber)?.doubleValue ?? AudioFreezeTrack.defaultMaximumTail))
        let tail = auto ? maximumTail : max(0, min(30, (payload["tail"] as? NSNumber)?.doubleValue ?? 3))
        let threshold = recorded ? AudioFreezeTrack.recordedSilence : AudioFreezeTrack.offlineSilence
        let monitor = payload["monitor"] as? Bool != false
        runtime.busy = true
        defer { runtime.busy = false }
        // Fresh plug-in state for every descriptor, so the canonical project and
        // Undo retain exactly what is about to be printed.
        guard let snapshot = try await handleCore("engine.snapshot", payload: [:]) as? [String: Any],
              let livePlugins = snapshot["audioPlugins"] as? [[String: Any]] else {
            throw AudioFreezeTrack.Failure(message: "The audio host returned no plug-in state.")
        }
        var fresh = canonical
        fresh["audioPlugins"] = runtime.mergeDormant(livePlugins)
        let trackPlugins = AudioFreezeTrack.plugins(of: index, in: fresh)
        for plugin in trackPlugins {
            let name = plugin["name"] as? String ?? plugin["component"] as? String ?? "A plug-in"
            if plugin["missing"] as? Bool == true { throw AudioFreezeTrack.Failure(message: "\(name) is unavailable. Load or remove it before freezing.") }
            if let error = plugin["stateError"] as? String { throw AudioFreezeTrack.Failure(message: "\(name): \(error)") }
            if plugin["state"] == nil { throw AudioFreezeTrack.Failure(message: "\(name) did not report its state; it cannot be restored after Unfreeze.") }
        }
        let printGain = AudioFreezeTrack.printGain(fresh, index: index)
        let render = try AudioFreezeTrack.renderProject(fresh, index: index)
        let id = UUID().uuidString.lowercased()
        let folder = try AudioFreezeTrack.printDirectory()
        let tracks = fresh["tracks"] as? [[String: Any]] ?? []
        let safeName = ((tracks[index]["name"] as? String) ?? "Track").components(separatedBy: CharacterSet.alphanumerics.inverted).filter { !$0.isEmpty }.joined(separator: "-")
        let url = folder.appendingPathComponent("\(safeName.isEmpty ? "Track" : safeName)-\(id.prefix(8)).wav")
        func restore(_ project: [String: Any], awaitPlugins: Bool) async throws {
            runtime.canonical = project
            _ = try await handleCore("engine.project", payload: ["project": try AudioFreezeTrack.runtimeProject(project), "awaitPlugins": awaitPlugins])
        }
        var created: [URL] = [url]
        var patternPrints: [AudioFreezeTrack.PatternPrint] = []
        var (printPatterns, unprinted) = AudioFreezeTrack.patternsToPrint(fresh, index: index)
        if recorded, payload["patterns"] as? Bool == false { unprinted = printPatterns + unprinted; printPatterns = [] }
        let steps = 1 + 2 * printPatterns.count
        var step = 0, latency: Any = NSNull()
        func progress(_ fraction: Double, _ phase: String) {
            emit("freeze.progress", ["track": index, "progress": (Double(step) + max(0, min(1, fraction))) / Double(steps),
                                     "phase": phase, "recording": recorded])
        }
        /// One print of `project` into `destination`: offline track export, or a real-time take.
        /// Auto tails are then cut where the print falls silent. Returns (musical, total) frames.
        func print(_ project: [String: Any], to destination: URL, tail seconds: Double, phase: String) async throws -> (Int64, Int64) {
            progress(0, phase)
            let musical: Int64
            if recorded {
                musical = try await recordFreezeTake(AudioFreezeTrack.flatTakeProject(project), track: index, to: destination, tail: seconds, auto: auto, monitor: monitor) { progress($0, phase) }
            } else {
                // Only this track is printed: other tracks' live line inputs (hardware returns)
                // close for the offline render, and `restore` reopens them afterwards.
                _ = try await handleCore("engine.project", payload: ["project": AudioFreezeTrack.withoutLineInputs(project), "awaitPlugins": true])
                let exported = try await handleCore("audio.export", payload: ["path": destination.path, "mode": "track", "track": index, "tail": seconds]) as? [String: Any] ?? [:]
                musical = (exported["musicalFrames"] as? NSNumber)?.int64Value ?? 0
                latency = exported["latencyCompensation"] ?? NSNull()
            }
            if auto {
                let keep = try AudioFreezeTrack.audibleLength(destination, musicalFrames: musical, threshold: threshold)
                try AudioFreezeTrack.truncate(destination, to: keep, fadeFrames: recorded ? AudioFreezeTrack.tailMargin(sampleRate) : 0)
            }
            step += 1
            return (musical, try AVAudioFile(forReading: destination).length)
        }
        let musical: Int64
        do {
            runtime.canonical = fresh
            musical = try await print(render, to: url, tail: tail, phase: "song").0
            // Pattern prints: the first pass (+ tail) and the steady-state looping
            // pass (+ tail), rendered as the pattern repeated until the tail wraps.
            for pattern in printPatterns {
                let base = url.deletingPathExtension().lastPathComponent + "-p\(pattern)"
                let head = folder.appendingPathComponent(base + ".wav"), looped = folder.appendingPathComponent(base + "-loop.wav")
                let rendered = folder.appendingPathComponent(base + "-render.wav")
                created += [head, looped, rendered]
                let (headMusical, headFrames) = try await print(AudioFreezeTrack.patternRenderProject(render, pattern: pattern, passes: 1), to: head, tail: tail, phase: "pattern \(pattern) first pass")
                let rate = try AVAudioFile(forReading: head).processingFormat.sampleRate
                let pass = AudioFreezeTrack.passFrames(fresh, pattern: pattern, sampleRate: rate)
                // Auto: the head's measured tail decides how many passes wrap into the loop print
                // (with headroom for tails that build up while looping).
                let loopTail = auto ? min(maximumTail, Double(headFrames - headMusical) / rate * 1.5 + 0.1) : tail
                let passes = max(2, min(256, 1 + Int((loopTail * rate / pass).rounded(.up))))
                _ = try await print(AudioFreezeTrack.patternRenderProject(render, pattern: pattern, passes: passes), to: rendered, tail: loopTail, phase: "pattern \(pattern) loop")
                let loopStart = Int64((Double(passes - 1) * pass - 1e-6).rounded(.up))
                let loopFrames = try AudioFreezeTrack.trimPrint(rendered, from: loopStart, to: looped)
                try? FileManager.default.removeItem(at: rendered)
                patternPrints.append(AudioFreezeTrack.PatternPrint(pattern: pattern, path: head.path, sha256: try AudioFreezeTrack.sha256(of: head),
                    loopPath: looped.path, loopSha256: try AudioFreezeTrack.sha256(of: looped), frames: headFrames,
                    passFrames: Int64(pass.rounded()), loopFrames: loopFrames, passes: passes))
            }
        } catch {
            for file in created { try? FileManager.default.removeItem(at: file) }
            try? await restore(fresh, awaitPlugins: false)
            throw error
        }
        do {
            let file = try AVAudioFile(forReading: url)
            let rate = file.processingFormat.sampleRate
            let frames = Int64(file.length)
            guard musical > 0, frames >= musical, file.processingFormat.channelCount == 2 else {
                throw AudioFreezeTrack.Failure(message: "The track print has an unexpected length or channel layout.")
            }
            let seconds = Double(musical) / rate
            let endBeat = seconds * ((fresh["bpm"] as? NSNumber)?.doubleValue ?? 120) / 60
            let record = AudioFreezeTrack.Record(id: id, path: url.path, sha256: try AudioFreezeTrack.sha256(of: url), sampleRate: rate,
                frames: frames, musicalFrames: musical, tailFrames: frames - musical, musicalEndBeat: endBeat, tailSeconds: tail,
                dormantPluginIDs: trackPlugins.compactMap { $0["instanceID"] as? String },
                fingerprint: try AudioFreezeTrack.fingerprint(fresh, index: index),
                mixMode: AudioFreezeTrack.mixMode(fresh), createdAt: ISO8601DateFormatter().string(from: Date()),
                patternPrints: patternPrints, unprintedPatterns: unprinted,
                printGain: printGain, tailMode: auto ? "auto" : "fixed", method: recorded ? "recorded" : "offline")
            var frozen = fresh
            var nextTracks = tracks
            nextTracks[index][AudioFreezeTrack.recordKey] = record.dictionary
            frozen["tracks"] = nextTracks
            try await restore(frozen, awaitPlugins: true)
            return ["track": index, "record": record.dictionary, "audioPlugins": fresh["audioPlugins"] ?? [], "project": frozen,
                    "releasedPlugins": trackPlugins.count, "latencyCompensation": latency,
                    "compensationAfter": rack.latencyStatus(),
                    "patternPrints": patternPrints.count, "unprintedPatterns": unprinted,
                    "notes": AudioFreezeTrack.stagingNotes(fresh, index: index)]
        } catch {
            for file in created { try? FileManager.default.removeItem(at: file) }
            try? await restore(fresh, awaitPlugins: false)
            throw error
        }
    }

    @MainActor private func unfreezeTrack(_ payload: [String: Any]) async throws -> [String: Any] {
        let runtime = freezeTrackRuntime
        guard !runtime.busy else { throw AudioFreezeTrack.Failure(message: "A track freeze is already in progress.") }
        guard var project = runtime.canonical else { throw AudioFreezeTrack.Failure(message: "Load a project before unfreezing a track.") }
        let index = PayloadValue.integer(payload["track"], -1)
        var tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index), tracks[index][AudioFreezeTrack.recordKey] != nil else {
            throw AudioFreezeTrack.Failure(message: "Choose a frozen track to unfreeze.")
        }
        if status()["playing"] as? Bool == true { throw AudioFreezeTrack.Failure(message: "Stop playback before unfreezing.") }
        runtime.busy = true
        defer { runtime.busy = false }
        let previous = project
        tracks[index].removeValue(forKey: AudioFreezeTrack.recordKey)
        project["tracks"] = tracks
        runtime.canonical = project
        do {
            _ = try await handleCore("engine.project", payload: ["project": try AudioFreezeTrack.runtimeProject(project), "awaitPlugins": true])
        } catch {
            runtime.canonical = previous
            _ = try? await handleCore("engine.project", payload: ["project": try AudioFreezeTrack.runtimeProject(previous), "awaitPlugins": true])
            throw error
        }
        // Report plug-ins that did not come back exactly (missing/state errors).
        let expected = AudioFreezeTrack.plugins(of: index, in: project)
        let live = (try await handleCore("plugin.state", payload: [:]) as? [[String: Any]]) ?? []
        var problems: [String] = []
        for plugin in expected {
            let id = plugin["instanceID"] as? String
            let match = live.first { id != nil && $0["instanceID"] as? String == id }
            if match == nil || match?["stateError"] != nil || match?["missing"] as? Bool == true {
                problems.append(plugin["name"] as? String ?? plugin["component"] as? String ?? "plug-in")
            }
        }
        return ["track": index, "project": project, "restoredPlugins": expected.count - problems.count,
                "problems": problems]
    }

    @MainActor func freezeStatus() -> [String: Any] {
        guard let canonical = freezeTrackRuntime.canonical else { return ["tracks": [Any](), "busy": freezeTrackRuntime.busy] }
        let (frozen, issues) = AudioFreezeTrack.frozenTracks(canonical)
        var rows: [[String: Any]] = frozen.map { item in
            let readable = { (path: String) in FileManager.default.isReadableFile(atPath: path) }
            let available = readable(item.record.path) && item.record.patternPrints.allSatisfy { readable($0.path) && readable($0.loopPath) }
            let current = (try? AudioFreezeTrack.fingerprint(canonical, index: item.index)) ?? ""
            // Pattern mode plays every pattern the track currently uses only when each has a print.
            let printed = Set(item.record.patternPrints.map(\.pattern))
            let tracks = canonical["tracks"] as? [[String: Any]] ?? []
            let owner = tracks.indices.contains(item.index) ? AudioFreezeTrack.owner(tracks[item.index]) : nil
            let missing = (canonical["patterns"] as? [[String: Any]] ?? []).enumerated().compactMap { position, pattern -> Int? in
                let id = PayloadValue.integer(pattern["id"], position)
                return AudioFreezeTrack.trackUses(pattern, index: item.index, owner: owner) && !printed.contains(id) ? id : nil
            }
            return ["track": item.index, "id": item.record.id, "available": available,
                    "stale": !item.record.fingerprint.isEmpty && current != item.record.fingerprint,
                    "path": item.record.path, "dormantPlugins": item.record.dormantPluginIDs.count,
                    "patternPrints": item.record.patternPrints.count, "patternPlayback": missing.isEmpty, "unprintedPatterns": missing]
        }
        for (index, message) in issues { rows.append(["track": index, "available": false, "stale": false, "error": message]) }
        return ["tracks": rows, "busy": freezeTrackRuntime.busy]
    }
}

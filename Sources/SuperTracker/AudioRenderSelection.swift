import Foundation

/// "Render selection to sample": the pure part. A pattern cell selection (tracks and
/// an inclusive row range) becomes a render project that plays only those tracks'
/// notes from the first selected row to the end of the last one, through the
/// tracks' own production path (instrument, inserts, sends, group buses, returns and,
/// when asked, the master). The host renders it in a private offline graph
/// (AudioHost+RenderSelection.swift), so live playback is never paused.
///
/// Decisions:
/// - Clean start: playback starts at the first selected row, so only notes that start
///   inside the selection sound. Notes held from earlier rows are not chased.
/// - The pattern ends after the last selected row; notes still held there are
///   released (the engine's natural end), so release envelopes and effect tails are
///   rendered until the output stays below the silence threshold, up to a maximum.
/// - Other tracks keep their place (indices, groups, routing) but are muted, without
///   notes and without plug-ins, so they add nothing and load nothing.
/// - Through master: the master fader and master inserts apply ("what you hear").
///   Track only: everything up to the master input (tracks, groups, returns), the
///   master at unity and without inserts or master automation.
enum AudioRenderSelection {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    static let maximumTailSeconds = 60.0
    static let defaultTailSeconds = 10.0
    static let defaultThresholdDB = -70.0
    static let defaultHoldSeconds = 0.2
    /// Longest musical range rendered (the sample is held in memory).
    static let maximumMusicalSeconds = 600.0

    struct Request: Equatable {
        var pattern: Int
        var tracks: [Int]
        var firstRow: Int
        var lastRow: Int
        var throughMaster = true
        var maximumTail = AudioRenderSelection.defaultTailSeconds
        var thresholdDB = AudioRenderSelection.defaultThresholdDB
        var holdSeconds = AudioRenderSelection.defaultHoldSeconds
        var channels = 2
        var normalize = false
        var sampleRate = 48_000
        var name = "Render"

        var rowCount: Int { lastRow - firstRow + 1 }
    }

    static func request(_ payload: [String: Any]) throws -> Request {
        guard let pattern = PayloadValue.integer(payload["pattern"]) else { throw Failure(message: "Choose a pattern to render.") }
        let rawTracks = (payload["tracks"] as? [Any] ?? []).compactMap { PayloadValue.integer($0) }
        let tracks = Array(Set(rawTracks)).sorted()
        guard !tracks.isEmpty, tracks.count == rawTracks.count, tracks.allSatisfy({ $0 >= 0 }) else {
            throw Failure(message: "Select one or more tracks to render.")
        }
        guard let first = PayloadValue.integer(payload["firstRow"]), let last = PayloadValue.integer(payload["lastRow"]),
              first >= 0, last >= first, last < 256 else { throw Failure(message: "Select a row range to render.") }
        var result = Request(pattern: pattern, tracks: tracks, firstRow: first, lastRow: last)
        if let mode = payload["mode"] as? String {
            guard ["master", "track"].contains(mode) else { throw Failure(message: "Choose Through master or Track only.") }
            result.throughMaster = mode == "master"
        }
        if payload["tail"] != nil {
            guard let tail = PayloadValue.number(payload["tail"], in: 0...maximumTailSeconds) else {
                throw Failure(message: "Choose a maximum tail between 0 and \(Int(maximumTailSeconds)) seconds.")
            }
            result.maximumTail = tail
        }
        if payload["thresholdDB"] != nil {
            guard let value = PayloadValue.number(payload["thresholdDB"], in: -120 ... -20) else { throw Failure(message: "Choose a silence threshold between −120 and −20 dBFS.") }
            result.thresholdDB = value
        }
        if payload["holdSeconds"] != nil {
            guard let value = PayloadValue.number(payload["holdSeconds"], in: 0.01...5) else { throw Failure(message: "Choose a silence hold between 10 ms and 5 s.") }
            result.holdSeconds = value
        }
        if payload["channels"] != nil {
            guard let value = PayloadValue.integer(payload["channels"]), [1, 2].contains(value) else { throw Failure(message: "Choose mono or stereo.") }
            result.channels = value
        }
        if payload["sampleRate"] != nil {
            guard let value = PayloadValue.integer(payload["sampleRate"]), [44_100, 48_000, 96_000].contains(value) else {
                throw Failure(message: "Choose 44.1, 48 or 96 kHz.")
            }
            result.sampleRate = value
        }
        result.normalize = PayloadValue.boolean(payload["normalize"]) ?? false
        if let name = PayloadValue.text(payload["name"])?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
            result.name = String(name.prefix(120))
        }
        return result
    }

    // MARK: Project

    struct Plan {
        let project: [String: Any]
        /// Engine pattern index to seek (the pattern id).
        let pattern: Int
        /// Exact musical length in 48 kHz frames, nil with a tempo map (the host then
        /// uses the engine's natural end).
        let musicalFrames: Int64?
        let keptPlugins: Int
        let warnings: [String]
    }

    private static func owner(_ track: [String: Any]) -> String? { (track["routingID"] as? String).map { "track:" + $0 } }

    static func patternPosition(_ project: [String: Any], id: Int) -> Int? {
        (project["patterns"] as? [[String: Any]] ?? []).enumerated()
            .first { PayloadValue.integer($0.element["id"], $0.offset) == id }?.offset
    }

    /// Explicit plug-in descriptors with track owners resolved (mirrors
    /// AudioHost.pluginDescriptions for legacy instrument/track plug-ins).
    static func pluginDescriptors(_ project: [String: Any]) -> [[String: Any]] {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        var items: [[String: Any]]
        if let explicit = project["audioPlugins"] as? [[String: Any]] { items = explicit }
        else {
            items = []
            let instruments = project["instruments"] as? [[String: Any]] ?? []
            for (index, track) in tracks.enumerated() {
                let instrument = PayloadValue.integer(track["instrument"], 0)
                if instruments.indices.contains(instrument), var plugin = instruments[instrument]["plugin"] as? [String: Any], plugin["component"] != nil {
                    plugin["track"] = index; plugin["kind"] = "instrument"; plugin["slot"] = 0; items.append(plugin)
                }
                for (slot, value) in (track["plugins"] as? [Any] ?? []).enumerated() {
                    guard var plugin = value as? [String: Any], plugin["component"] != nil else { continue }
                    plugin["track"] = index; plugin["kind"] = "effect"; plugin["slot"] = slot; items.append(plugin)
                }
            }
        }
        guard project["routing"] != nil else { return items }
        return items.map { value in
            var item = value
            if item["owner"] == nil {
                let index = PayloadValue.integer(item["track"], 0)
                if tracks.indices.contains(index), let id = owner(tracks[index]) { item["owner"] = id }
            }
            return item
        }
    }

    /// Track index a plug-in descriptor belongs to, nil for buses, returns and master.
    static func pluginTrack(_ plugin: [String: Any], tracks: [[String: Any]]) -> Int? {
        if let owner = plugin["owner"] as? String {
            guard owner.hasPrefix("track:") else { return nil }
            return tracks.firstIndex { self.owner($0) == owner }
        }
        return PayloadValue.integer(plugin["track"], 0)
    }

    /// Owners (other than tracks) that the selected tracks can reach through outputs
    /// and sends, transitively. Nil when the project has no routing graph.
    static func reachableOwners(_ project: [String: Any], from sources: Set<String>) -> Set<String>? {
        guard let routing = project["routing"] as? [String: Any] else { return nil }
        let outputs = routing["outputs"] as? [String: Any] ?? [:]
        let sends = routing["sends"] as? [[String: Any]] ?? []
        var reached = sources, queue = Array(sources)
        while let next = queue.popLast() {
            var targets: [String] = []
            if let output = outputs[next] as? String { targets.append(output) } else if outputs[next] == nil { targets.append("master") }
            targets += sends.filter { $0["source"] as? String == next }.compactMap { $0["destination"] as? String }
            for target in targets where reached.insert(target).inserted { queue.append(target) }
        }
        return reached
    }

    static func hasNotes(_ steps: [Any], from first: Int, to last: Int) -> Bool {
        guard first < steps.count else { return false }
        return steps[first...min(last, steps.count - 1)].contains { raw in
            if let step = raw as? [String: Any] { return !noteCells(step).isEmpty }
            return false
        }
    }

    /// The cells of a step that play a note: column 0 and its extra note columns (`columns`).
    static func noteCells(_ step: [String: Any]) -> [[String: Any]] {
        ([step] + (step["columns"] as? [Any] ?? []).compactMap { $0 as? [String: Any] }).filter { PayloadValue.integer($0["note"], -1) >= 0 }
    }

    /// Exact length of the rows in frames (rows, swing and tempo), nil with a tempo map.
    static func musicalFrames(_ project: [String: Any], firstRow: Int, lastRow: Int, sampleRate: Double) -> Int64? {
        guard project["timing"] == nil else { return nil }
        let swing = min(0.95, max(0, Double(Float(PayloadValue.double(project["swing"]) ?? 0))))
        let lpb = Double(min(32, max(1, PayloadValue.integer(project["linesPerBeat"], 4))))
        let bpm = min(999, max(20, PayloadValue.double(project["bpm"]) ?? 120))
        func row(_ index: Int) -> Double { Double(index) + (index % 2 == 1 ? swing : 0) }
        return Int64(((row(lastRow + 1) - row(firstRow)) * sampleRate * 60 / bpm / lpb).rounded())
    }

    /// The render project for `request` from a canonical project with captured plug-in state.
    /// `hardwareComponents` are plug-ins that drive external hardware (their editors send
    /// MIDI); a selected track that uses one is refused, so the private render never
    /// instantiates them.
    static func plan(_ canonical: [String: Any], request: Request, hardwareComponents: Set<String> = [], sampleRate: Double = 48_000) throws -> Plan {
        var project = canonical
        var tracks = canonical["tracks"] as? [[String: Any]] ?? []
        guard request.tracks.allSatisfy({ tracks.indices.contains($0) }) else { throw Failure(message: "A selected track no longer exists.") }
        guard let position = patternPosition(canonical, id: request.pattern) else { throw Failure(message: "The selected pattern no longer exists.") }
        var patterns = canonical["patterns"] as? [[String: Any]] ?? []
        var pattern = patterns[position]
        let length = max(1, min(256, PayloadValue.integer(pattern["length"], 64)))
        guard request.lastRow < length else { throw Failure(message: "The selection extends past the end of the pattern.") }
        let instruments = canonical["instruments"] as? [[String: Any]] ?? []
        let selected = Set(request.tracks)
        let steps = pattern["steps"] as? [[Any]] ?? []
        let descriptors = pluginDescriptors(canonical)
        var warnings: [String] = []

        func trackName(_ index: Int) -> String { tracks[index]["name"] as? String ?? "Track \(index + 1)" }
        var anyNotes = false
        for index in request.tracks {
            let track = tracks[index], name = trackName(index)
            if track["frozen"] != nil { throw Failure(message: "\(name) is frozen. Unfreeze it to render its notes.") }
            if (track["audioInput"] as? [String: Any])?["enabled"] as? Bool == true {
                throw Failure(message: "\(name) plays live line input, which cannot be rendered offline. Record it to a sample first.")
            }
            let row = steps.indices.contains(index) ? steps[index] : []
            var used = Set([PayloadValue.integer(track["instrument"], 0)])
            if request.firstRow < row.count {
                for raw in row[request.firstRow...min(request.lastRow, row.count - 1)] {
                    guard let step = raw as? [String: Any] else { continue }
                    for cell in noteCells(step) { if let explicit = PayloadValue.integer(cell["instrument"]), explicit >= 0 { used.insert(explicit) } }
                }
            }
            for instrument in used where instruments.indices.contains(instrument) && instruments[instrument]["type"] as? String == "midi" {
                throw Failure(message: "\(name) plays an external MIDI instrument. Record its audio before rendering it.")
            }
            for plugin in descriptors where pluginTrack(plugin, tracks: tracks) == index {
                if let component = plugin["component"] as? String, hardwareComponents.contains(component) {
                    throw Failure(message: "\(name) uses \(plugin["name"] as? String ?? "a hardware editor"), which controls external hardware. Record its audio before rendering it.")
                }
            }
            if hasNotes(row, from: request.firstRow, to: request.lastRow) { anyNotes = true }
        }
        guard anyNotes else { throw Failure(message: "The selection has no notes to render.") }

        // The pattern ends after the last selected row; only the selected tracks keep notes.
        var renderedSteps: [[Any]] = []
        for index in tracks.indices {
            let source = steps.indices.contains(index) ? steps[index] : []
            var row = [Any](repeating: NSNull(), count: request.lastRow + 1)
            if selected.contains(index) {
                for r in request.firstRow...request.lastRow where r < source.count { row[r] = source[r] }
            }
            renderedSteps.append(row)
        }
        pattern["steps"] = renderedSteps
        pattern["length"] = request.lastRow + 1
        let selectedOwners = Set(request.tracks.compactMap { owner(tracks[$0]) })
        pattern["automation"] = (pattern["automation"] as? [[String: Any]] ?? []).filter { lane in
            if let laneOwner = lane["owner"] as? String {
                if laneOwner.hasPrefix("track:") { return selectedOwners.contains(laneOwner) }
                return request.throughMaster || laneOwner != "master"
            }
            return selected.contains(PayloadValue.integer(lane["track"], 0))
        }
        patterns[position] = pattern
        project["patterns"] = patterns

        for index in tracks.indices {
            tracks[index]["solo"] = false
            tracks[index].removeValue(forKey: "audioInput")
            tracks[index].removeValue(forKey: "frozen")
            if selected.contains(index) { tracks[index]["mute"] = false }
            else { tracks[index]["mute"] = true; tracks[index]["plugins"] = [Any]() }
        }
        project["tracks"] = tracks

        // Plug-ins: the selected tracks', and those of every bus, return and master the
        // selected tracks reach (all non-track owners in a project without routing).
        let reachable = reachableOwners(canonical, from: selectedOwners)
        let kept = descriptors.filter { plugin in
            if let index = pluginTrack(plugin, tracks: tracks) { return selected.contains(index) }
            let owner = plugin["owner"] as? String ?? ""
            if owner == "master" { return request.throughMaster }
            if let reachable, owner.hasPrefix("bus:") { return reachable.contains(owner) }
            return true
        }
        project["audioPlugins"] = kept
        let keptIDs = Set(kept.compactMap { $0["instanceID"] as? String })
        if var routing = project["routing"] as? [String: Any] {
            if let sidechains = routing["sidechains"] as? [[String: Any]] {
                routing["sidechains"] = sidechains.filter { sidechain in
                    guard let id = (sidechain["destination"] as? [String: Any])?["instanceID"] as? String else { return true }
                    return keptIDs.contains(id)
                }
            }
            if var buses = routing["buses"] as? [[String: Any]] {
                for index in buses.indices { buses[index]["solo"] = false }
                routing["buses"] = buses
            }
            project["routing"] = routing
        }
        if !request.throughMaster { project["masterVolume"] = 1.0 }
        project["sequence"] = [request.pattern]
        project["activePattern"] = request.pattern
        project["loopSong"] = false
        project["metronome"] = false
        project["audioClips"] = [[String: Any]]()
        project["patternExport"] = true
        project.removeValue(forKey: "songLoop")
        project.removeValue(forKey: "patternClips")
        project.removeValue(forKey: "freezeRuntime")

        let clipTracks = Set((canonical["audioClips"] as? [[String: Any]] ?? []).compactMap { PayloadValue.integer($0["track"]) })
        if !clipTracks.isDisjoint(with: selected) { warnings.append("Audio clips on the selected tracks are not part of a pattern render.") }
        let frames = musicalFrames(canonical, firstRow: request.firstRow, lastRow: request.lastRow, sampleRate: sampleRate)
        if let frames, Double(frames) > maximumMusicalSeconds * sampleRate {
            throw Failure(message: "The selection is longer than \(Int(maximumMusicalSeconds / 60)) minutes. Render a shorter range.")
        }
        return Plan(project: project, pattern: request.pattern, musicalFrames: frames, keptPlugins: kept.count, warnings: warnings)
    }

    // MARK: Tail

    enum TailDecision: Equatable {
        case render
        /// The output stayed below the threshold for the hold time: keep `frames`.
        case silent(frames: Int64)
        /// The maximum tail was reached: keep `frames` and fade the end.
        case maximum(frames: Int64)
    }

    /// When to stop. `rendered` output frames exist, the last one at or above the threshold
    /// is `lastLoud` (-1 for none). The musical part is `musicalFrames` long; once it has
    /// ended (`musicalEnded`), rendering stops after `holdFrames` quiet frames past
    /// max(musical end, last loud frame), or at musical end + `maximumTailFrames`.
    static func tailDecision(rendered: Int64, lastLoud: Int64, musicalFrames: Int64, musicalEnded: Bool,
                             holdFrames: Int64, maximumTailFrames: Int64) -> TailDecision {
        guard musicalEnded, rendered >= musicalFrames else { return .render }
        let end = max(musicalFrames, lastLoud + 1), cap = musicalFrames + maximumTailFrames
        // Sound past the cap: cut there. Otherwise wait for the hold (at most cap + hold).
        if end > cap { return .maximum(frames: cap) }
        return rendered - end >= holdFrames ? .silent(frames: end) : .render
    }

    static func linear(dB: Double) -> Float { Float(pow(10, dB / 20)) }

    /// Index of the last frame at or above `threshold` in either channel, or -1.
    static func lastLoud(_ left: UnsafeBufferPointer<Float>, _ right: UnsafeBufferPointer<Float>, threshold: Float) -> Int {
        var index = min(left.count, right.count) - 1
        while index >= 0 {
            if abs(left[index]) >= threshold || abs(right[index]) >= threshold { return index }
            index -= 1
        }
        return -1
    }

    struct Output {
        var channels: [[Float]]
        var peak: Float
        var gain: Float
    }

    /// Trim to `frames`, fade the last `fadeFrames` (a capped tail), mix to mono when
    /// asked and normalize the peak to −0.3 dBFS when asked.
    static func finish(left: [Float], right: [Float], frames: Int, fadeFrames: Int, channels: Int, normalize: Bool) -> Output {
        let count = max(0, min(frames, left.count, right.count))
        var l = Array(left.prefix(count)), r = Array(right.prefix(count))
        let fade = min(fadeFrames, count)
        if fade > 0 {
            for i in 0..<fade {
                let gain = Float(fade - i - 1) / Float(fade)
                l[count - fade + i] *= gain; r[count - fade + i] *= gain
            }
        }
        var result = channels == 1 ? [zip(l, r).map { ($0 + $1) * 0.5 }] : [l, r]
        var peak: Float = 0
        for channel in result { for value in channel { peak = max(peak, abs(value)) } }
        var gain: Float = 1
        if normalize, peak > 0 {
            gain = linear(dB: -0.3) / peak
            for c in result.indices { for i in result[c].indices { result[c][i] *= gain } }
            peak *= gain
        }
        return Output(channels: result, peak: peak, gain: gain)
    }
}

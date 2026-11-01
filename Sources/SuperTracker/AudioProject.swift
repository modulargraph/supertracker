import Foundation
import TrackerEngine

enum AudioProject {
    /// What was last sent to an engine, per pattern, so an edit re-sends only the
    /// patterns that changed and the engine shares the rest copy-on-write
    /// (st_engine_reuse_pattern). Valid only while the engine's pattern epoch is the
    /// one recorded here, so another writer or another engine never matches.
    private final class PatternSync {
        var epoch: UInt64 = 0
        var context = ""
        var patterns: [Int: (source: NSDictionary, derived: [Double])] = [:]
    }
    private static let patternSyncLock = NSLock()
    private static var patternSyncs: [OpaquePointer: PatternSync] = [:]
    /// Forget an engine's cache (call before destroying the engine).
    static func forgetPatternSync(_ engine: OpaquePointer) {
        patternSyncLock.lock(); patternSyncs.removeValue(forKey: engine); patternSyncLock.unlock()
    }
    /// Test/diagnostic counter: patterns reused by the most recent apply.
    private(set) static var lastReusedPatterns = 0

    /// The engine refused a published project (checked commit).
    struct Rejected: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    static func apply(_ project: [String: Any], to engine: OpaquePointer, clipAssets:[String:Int] = [:], sampleRate:Double = 48000) throws {
        let routingDefinition = try AudioRouting.parse(project:project)
        let hostAutomation = try routingAutomation(project)
        var completed = false
        patternSyncLock.lock()
        let sync = patternSyncs[engine] ?? PatternSync()
        patternSyncs.removeValue(forKey: engine)
        patternSyncLock.unlock()
        defer { if !completed { st_engine_cancel_update(engine) } }
        st_engine_begin_update(engine)
        st_engine_clear_project(engine)
        st_engine_clear_timing_map(engine)
        // Host events carry this numbering; the routing render table drops other ones.
        st_engine_set_host_automation_map(engine, hostAutomation.hostMap)
        let standardMixing = routingDefinition.enabled && routingDefinition.mixModel == .standard
        st_engine_set_host_mixing_tracks(engine,standardMixing ? 1 : 0,nil,0,1)
        st_engine_set_timing(engine, PayloadValue.number(project,"bpm",120), Int32(clamping:PayloadValue.integer(project,"linesPerBeat",4)), Float(PayloadValue.number(project,"swing",0)), Float(PayloadValue.number(project,"masterVolume",0.7)))
        let signature = project["timeSignature"] as? [String:Any] ?? [:]
        st_engine_set_metronome(engine,(project["metronome"] as? Bool ?? false) ? 1 : 0,Int32(clamping:PayloadValue.integer(signature,"numerator",4)),Int32(clamping:PayloadValue.integer(signature,"denominator",4)),Float(PayloadValue.number(project,"metronomeVolume",0.4)))
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.count <= ProjectLayout.trackSanityLimit, st_engine_set_track_count(engine, Int32(max(1,tracks.count))) != 0 else {
            throw Rejected(message:"A project supports at most \(ProjectLayout.trackSanityLimit) tracks.")
        }
        let ordinaryInstruments = min(ProjectLayout.instrumentSanityLimit, (project["instruments"] as? [Any])?.count ?? 0)
        guard st_engine_set_instrument_count(engine, Int32(max(1, ordinaryInstruments))) != 0 else {
            throw Rejected(message:"A project supports at most \(ProjectLayout.instrumentSanityLimit) instruments.")
        }
        // Track choke groups are scoped to the track's group (1 + its index), 0 for ungrouped tracks.
        let groupScopes = Dictionary((project["groups"] as? [[String: Any]] ?? []).enumerated().compactMap { entry in
            (entry.element["id"] as? String).map { ($0, entry.offset + 1) } }, uniquingKeysWith: { first, _ in first })
        for (index, item) in tracks.enumerated() {
            let choke = max(0, min(16, PayloadValue.integer(item, "chokeGroup", 0)))
            let mode = ["poly": 0, "mono": 1, "legato": 2][item["voiceMode"] as? String ?? ""] ?? -1
            if choke > 0 || mode >= 0 {
                let scope = (item["groupID"] as? String).flatMap { groupScopes[$0] } ?? 0
                guard st_engine_set_track_voice(engine, Int32(index), Int32(choke), Int32(min(scope, ProjectLayout.trackSanityLimit)), Int32(mode)) != 0 else {
                    throw Rejected(message: "The engine rejected a track's choke group or voice mode.")
                }
            }
            var track = st_default_track()
            track.volume = Float(PayloadValue.number(item,"volume",0.8)); track.pan = Float(PayloadValue.number(item,"pan",0))
            track.delaySend = Float(PayloadValue.number(item,"delaySend",0)); track.reverbSend = Float(PayloadValue.number(item,"reverbSend",0))
            track.mute = (item["mute"] as? Bool ?? false) ? 1 : 0; track.solo = (item["solo"] as? Bool ?? false) ? 1 : 0
            track.instrument = Int32(clamping:PayloadValue.integer(item,"instrument",0)); st_engine_set_track(engine,Int32(index),track)
        }
        let instruments = ProjectLayout.instruments(project)
        for (index, item) in instruments.enumerated() {
            var instrument = st_default_instrument()
            // "synth" (retired) and untyped instruments read as samplers; ProjectMigration converts them first.
            instrument.type = ["sample":1,"au":2,"midi":3][item["type"] as? String ?? "sample"] ?? 1
            instrument.mode = ["oneshot":0,"forward":1,"reverse":2,"pingpong":3,"slice":4,"wavetable":5,"granular":6][item["mode"] as? String ?? "oneshot"] ?? 0
            instrument.filterType = ["lowpass":0,"highpass":1,"bandpass":2][item["filterType"] as? String ?? "lowpass"] ?? 0
            instrument.rootNote = Int32(clamping:PayloadValue.integer(item,"rootNote",60)); instrument.sliceCount = Int32(clamping:PayloadValue.integer(item,"sliceCount",16)); instrument.bitDepth = Int32(clamping:PayloadValue.integer(item,"bitDepth",16))
            instrument.volume = Float(PayloadValue.number(item,"volume",0.7)); instrument.pan = Float(PayloadValue.number(item,"pan",0)); instrument.tune = Float(PayloadValue.number(item,"tune",0))
            instrument.start = Float(PayloadValue.number(item,"start",0)); instrument.end = Float(PayloadValue.number(item,"end",1)); instrument.loopStart = Float(PayloadValue.number(item,"loopStart",0)); instrument.loopEnd = Float(PayloadValue.number(item,"loopEnd",1))
            instrument.attack = Float(PayloadValue.number(item,"attack",0.005)); instrument.decay = Float(PayloadValue.number(item,"decay",0.2)); instrument.sustain = Float(PayloadValue.number(item,"sustain",0.7)); instrument.release = Float(PayloadValue.number(item,"release",0.2))
            instrument.cutoff = Float(PayloadValue.number(item,"cutoff",18000)); instrument.resonance = Float(PayloadValue.number(item,"resonance",0)); instrument.drive = Float(PayloadValue.number(item,"drive",0))
            instrument.lfoRate = Float(PayloadValue.number(item,"lfoRate",0)); instrument.lfoDepth = Float(PayloadValue.number(item,"lfoDepth",0)); instrument.grainSize = Float(PayloadValue.number(item,"grainSize",0.08)); instrument.grainPosition = Float(PayloadValue.number(item,"grainPosition",0.5))
            instrument.wavetablePosition = Float(PayloadValue.number(item,"wavetablePosition",0)); instrument.wavetableFrameSize = Int32(clamping:PayloadValue.integer(item,"wavetableFrameSize",0))
            instrument.grainDensity = Int32(clamping:PayloadValue.integer(item,"grainDensity",2)); instrument.grainSpray = Float(PayloadValue.number(item,"grainSpray",0)); instrument.grainPitchSpread = Float(PayloadValue.number(item,"grainPitchSpread",0))
            instrument.grainScan = Float(PayloadValue.number(item,"grainScan",0)); instrument.grainReverse = Float(PayloadValue.number(item,"grainReverse",0)); instrument.grainStereo = Float(PayloadValue.number(item,"grainStereo",0))
            instrument.voiceMode = ["poly":0,"mono":1,"legato":2][item["voiceMode"] as? String ?? "poly"] ?? 0
            instrument.chokeGroup = Int32(clamping:PayloadValue.integer(item,"chokeGroup",0))
            instrument.filterAttack = Float(PayloadValue.number(item,"filterAttack",0.005)); instrument.filterDecay = Float(PayloadValue.number(item,"filterDecay",0.2))
            instrument.filterSustain = Float(PayloadValue.number(item,"filterSustain",0)); instrument.filterRelease = Float(PayloadValue.number(item,"filterRelease",0.2)); instrument.filterEnvAmount = Float(PayloadValue.number(item,"filterEnvAmount",0))
            let lfoTargets:[String:Int32] = ["cutoff":0,"pitch":1,"volume":2,"pan":3,"position":4,"grainSize":5]
            let lfoShapes:[String:Int32] = ["sine":0,"triangle":1,"saw":2,"square":3,"random":4]
            instrument.lfoTarget = lfoTargets[item["lfoTarget"] as? String ?? "cutoff"] ?? 0
            instrument.lfoShape = lfoShapes[item["lfoShape"] as? String ?? "sine"] ?? 0
            instrument.lfo2Target = lfoTargets[item["lfo2Target"] as? String ?? "cutoff"] ?? 0
            instrument.lfo2Shape = lfoShapes[item["lfo2Shape"] as? String ?? "sine"] ?? 0
            instrument.lfo2Rate = Float(PayloadValue.number(item,"lfo2Rate",0)); instrument.lfo2Depth = Float(PayloadValue.number(item,"lfo2Depth",0))
            instrument.lfoSync = (item["lfoSync"] as? Bool ?? false) ? 1 : 0; instrument.lfo2Sync = (item["lfo2Sync"] as? Bool ?? false) ? 1 : 0
            instrument.lfoBeats = Float(PayloadValue.number(item,"lfoBeats",1)); instrument.lfo2Beats = Float(PayloadValue.number(item,"lfo2Beats",1))
            st_engine_set_instrument(engine,Int32(index),instrument)
            let slices = (item["slicePoints"] as? [Double] ?? []).map(Float.init)
            slices.withUnsafeBufferPointer { st_engine_set_slice_points(engine,Int32(index),$0.baseAddress,Int32(slices.count)) }
        }
        let locks = try AudioPluginStepFX(project:project,definition:routingDefinition,hostEntries:hostAutomation.entries)
        // Everything outside a pattern that changes how its cells and lanes resolve.
        let context = patternContext(project, tracks: tracks.count, routing: routingDefinition, standard: standardMixing)
        var derived: [Int: [Double]] = [:]
        for point in hostAutomation.points { derived[point.pattern, default: []] += [0, Double(point.destination), Double(point.address >> 32), Double(point.address & 0xffff_ffff), Double(point.interpolation), point.row, point.value] }
        for binding in locks.bindings { derived[binding.pattern, default: []] += [1, Double(binding.track), Double(binding.row), Double(binding.column), Double(binding.slot), Double(binding.destination), Double(binding.address >> 32), Double(binding.address & 0xffff_ffff), binding.value] }
        let reusable = sync.epoch != 0 && sync.epoch == st_engine_pattern_epoch(engine) && sync.context == context
        var reused = Set<Int>(), sent: [Int: (source: NSDictionary, derived: [Double])] = [:]
        for (index, pattern) in (project["patterns"] as? [[String: Any]] ?? []).prefix(256).enumerated() {
            let patternIndex = Int32(clamping:PayloadValue.integer(pattern,"id",index))
            let source = pattern as NSDictionary, key = derived[Int(patternIndex)] ?? []
            sent[Int(patternIndex)] = (source, key)
            if reusable, let previous = sync.patterns[Int(patternIndex)], previous.derived == key, previous.source.isEqual(source),
               st_engine_reuse_pattern(engine, patternIndex) != 0 {
                reused.insert(Int(patternIndex)); continue
            }
            st_engine_set_pattern(engine,patternIndex,Int32(clamping:PayloadValue.integer(pattern,"length",64)))
            let steps = pattern["steps"] as? [[Any]] ?? []
            for (track, rows) in steps.prefix(tracks.count).enumerated() {
                let extraColumns = noteColumns(tracks[track]) - 1
                for (row, value) in rows.prefix(256).enumerated() {
                    guard let data = value as? [String: Any] else { continue }
                    let step = step(data)
                    let chord = (data["chord"] as? [Int] ?? []).prefix(8).map { Int32(clamping:$0) }
                    chord.withUnsafeBufferPointer { pointer in st_engine_set_step(engine,patternIndex,Int32(track),Int32(row),step,pointer.baseAddress,Int32(chord.count)) }
                    // Extra note columns (step.columns[i] is note column i+1), up to the track's noteColumns.
                    guard extraColumns > 0, let columns = data["columns"] as? [Any] else { continue }
                    for (index, value) in columns.prefix(extraColumns).enumerated() {
                        guard let cell = value as? [String: Any] else { continue }
                        st_engine_set_step_column(engine,patternIndex,Int32(track),Int32(row),Int32(index + 1),columnStep(cell))
                    }
                }
            }
            for lane in pattern["automation"] as? [[String: Any]] ?? [] {
                let targetName = lane["target"] as? String ?? "volume"
                var laneTrack = PayloadValue.integer(lane,"track",0)
                if let owner = lane["owner"] as? String {
                    // Track mix belongs to the native row-lock scheduler even in
                    // standard mode; it emits timed controls to the common strip.
                    guard standardMixing, ProjectLayout.nativeAutomationRanges[targetName] != nil,
                          let index = routingDefinition.owner(owner)?.trackIndex else { continue }
                    laneTrack = index
                }
                // Unknown names (newer files, unsupported sends) are skipped, never retargeted to track volume.
                guard let (target, slot, address) = Self.nativeAutomationTarget(targetName) else { continue }
                for point in lane["points"] as? [[String: Any]] ?? [] {
                    st_engine_add_automation_point(engine,patternIndex,Int32(clamping:laneTrack),target,slot,address,(lane["interpolation"] as? String == "step") ? 1 : 0,PayloadValue.number(point,"row",0),PayloadValue.number(point,"value",0))
                }
            }
        }
        let sequence = (project["sequence"] as? [Int] ?? [0]).prefix(1024).map { Int32(clamping:$0) }
        sequence.withUnsafeBufferPointer { pointer in st_engine_set_sequence(engine,pointer.baseAddress,Int32(sequence.count),(project["loopSong"] as? Bool ?? false) ? 1 : 0,Int32(clamping:PayloadValue.integer(project,"activePattern",0))) }
        let delay = project["delay"] as? [String: Any] ?? [:], reverb = project["reverb"] as? [String: Any] ?? [:]
        st_engine_set_effects(engine,Float(PayloadValue.number(delay,"time",0.375)),Float(PayloadValue.number(delay,"feedback",0.35)),Float(PayloadValue.number(delay,"mix",0.3)),Float(PayloadValue.number(reverb,"size",0.7)),Float(PayloadValue.number(reverb,"damping",0.4)),Float(PayloadValue.number(reverb,"mix",0.25)))
        // Tempo-synced delay (quarter beats; 0 = free time), delay tone and reverb pre-delay.
        st_engine_set_send_effects(engine,max(0,PayloadValue.number(delay,"beats",0)),Float(PayloadValue.number(delay,"tone",1)),Float(PayloadValue.number(reverb,"predelay",0)))
        // Song section loop: inclusive sequence slots, honored only while Loop song is on
        // (exports and bounces turn loopSong off, so they always render the whole song).
        let songLoop = project["songLoop"] as? [String: Any]
        let loopsSection = (project["loopSong"] as? Bool ?? false) && songLoop != nil
        st_engine_set_song_loop(engine,loopsSection ? Int32(clamping:PayloadValue.integer(songLoop ?? [:],"start",-1)) : -1,loopsSection ? Int32(clamping:PayloadValue.integer(songLoop ?? [:],"end",-1)) : -1)
        st_engine_clear_audio_clips(engine)
        for clip in project["audioClips"] as? [[String:Any]] ?? [] {
            guard let path = clip["path"] as? String ?? clip["samplePath"] as? String, let slot = clipAssets[path] else { continue }
            st_engine_add_audio_clip(engine,Int32(slot),Int32(clamping:PayloadValue.integer(clip,"track",0)),PayloadValue.number(clip,"startBeat",0),PayloadValue.number(clip,"durationBeats",0),PayloadValue.number(clip,"sourceOffset",PayloadValue.number(clip,"offsetSeconds",0)),Float(PayloadValue.number(clip,"gain",1)),Float(PayloadValue.number(clip,"fadeIn",PayloadValue.number(clip,"fadeInSeconds",0.005))),Float(PayloadValue.number(clip,"fadeOut",PayloadValue.number(clip,"fadeOutSeconds",0.005))),(clip["muted"] as? Bool ?? clip["mute"] as? Bool ?? false) ? 1 : 0)
        }
        // Pattern-mode clips (Track Freeze pattern prints); assets come from the
        // same timeline-clip slots (see clipAssetEntries).
        for clip in project["patternClips"] as? [[String:Any]] ?? [] {
            guard let path = clip["path"] as? String, let head = clipAssets[path] else { continue }
            let loop = (clip["loopPath"] as? String).flatMap { clipAssets[$0] } ?? -1
            st_engine_add_pattern_clip(engine,Int32(head),Int32(loop),Int32(clamping:PayloadValue.integer(clip,"track",0)),Int32(clamping:PayloadValue.integer(clip,"pattern",0)),Float(PayloadValue.number(clip,"gain",1)))
        }
        // Song-loop tails (Track Freeze): a print's tail rings over the whole-song loop point.
        for clip in project["songTailClips"] as? [[String:Any]] ?? [] {
            guard let path = clip["path"] as? String, let slot = clipAssets[path] else { continue }
            st_engine_add_song_tail_clip(engine,Int32(slot),Int32(clamping:PayloadValue.integer(clip,"track",0)),Int64(max(0,PayloadValue.integer(clip,"startFrame",0))),Float(PayloadValue.number(clip,"gain",1)))
        }
        hostAutomation.apply(to:engine, skipping:reused)
        locks.apply(to:engine, skipping:reused)
        var message = [CChar](repeating:0,count:512)
        let accepted = message.withUnsafeMutableBufferPointer {
            st_engine_commit_update_checked(engine,$0.baseAddress,UInt32($0.count))
        }
        guard accepted != 0 else { throw Rejected(message:String(cString:message)) }
        completed = true
        sync.epoch = st_engine_pattern_epoch(engine); sync.context = context; sync.patterns = sent
        lastReusedPatterns = reused.count
        patternSyncLock.lock(); patternSyncs[engine] = sync; patternSyncLock.unlock()
    }

    /// Resolution context shared by every pattern: track count, routing owner indices
    /// and the mix model. Cheap: O(tracks).
    private static func patternContext(_ project: [String: Any], tracks: Int, routing: AudioRouting.Definition, standard: Bool) -> String {
        var parts = ["\(tracks)", standard ? "s" : "l"]
        for owner in routing.owners { parts.append("O\(owner.id)=\(owner.trackIndex ?? -1)") }
        return parts.joined(separator: "|")
    }

    /// Every file the engine must hold as a timeline-clip asset: song clips plus
    /// the head/loop files of pattern-mode clips.
    static func clipAssetEntries(_ project: [String: Any]) -> [[String: Any]] {
        var entries = project["audioClips"] as? [[String: Any]] ?? []
        for clip in project["patternClips"] as? [[String: Any]] ?? [] {
            for key in ["path", "loopPath"] { if let path = clip[key] as? String, !path.isEmpty { entries.append(["path": path]) } }
        }
        return entries
    }

    /// Desired slots define destination numbering even while a plug-in is missing
    /// or still loading. Live instance creation must never renumber other lanes.
    static let nativeAutomationTargets:[String:Int32] = ["volume":0,"pan":1,"cutoff":2,"resonance":3,"delay":4,"reverb":5,"pitch":6,"attack":8,"decay":9,"sustain":10,"release":11,"drive":12,"bitDepth":13,"lfoRate":14,"lfoDepth":15,"start":16,"end":17,"loopStart":18,"loopEnd":19,"grainSize":20,"grainPosition":21,"sliceCount":22,"instrumentVolume":23,"instrumentPan":24,"tune":25,"filterAttack":26,"filterDecay":27,"filterSustain":28,"filterRelease":29,"filterEnvAmount":30,"lfo2Rate":31,"lfo2Depth":32,"lfoBeats":33,"lfo2Beats":34,"wavetablePosition":35,"grainDensity":36,"grainSpray":37,"grainPitchSpread":38,"grainScan":39,"grainReverse":40,"grainStereo":41]
    /// Engine target, plug-in slot (-1 = instrument) and parameter address of a track-index lane; nil = not native.
    static func nativeAutomationTarget(_ name:String) -> (target:Int32, slot:Int32, address:UInt64)? {
        guard name.hasPrefix("plugin:") else { return nativeAutomationTargets[name].map { ($0,0,0) } }
        let components = name.split(separator: ":").map(String.init)
        if components.count == 4 { return (7, components[1] == "instrument" ? -1 : Int32(components[2]) ?? 0, UInt64(components[3]) ?? 0) }
        if components.count == 3 { return (7, Int32(components[1]) ?? 0, UInt64(components[2]) ?? 0) }
        return nil
    }
    static func routingAutomation(_ project:[String:Any]) throws -> AudioRoutingAutomation {
        let definition = try AudioRouting.parse(project:project)
        let layouts = try AudioRoutingAutomation.desiredPluginLayouts(project:project,definition:definition)
        let mapping = try AudioRoutingAutomation(project:project,definition:definition,plugins:layouts)
        // All host entry points already validate this adapter before accepting a
        // project. Invalid locks must fail here rather than turn into parameter0.
        _ = try AudioPluginStepFX(project: project, definition: definition, hostEntries: mapping.entries)
        return mapping
    }
    static func pluginStepFX(_ project: [String: Any]) throws -> AudioPluginStepFX {
        let definition = try AudioRouting.parse(project: project)
        let mapping = try routingAutomation(project)
        return try AudioPluginStepFX(project: project, definition: definition, hostEntries: mapping.entries)
    }
    static func step(_ data:[String:Any]) -> STStep {
        var step = st_default_step()
        step.note = Int32(clamping:PayloadValue.integer(data,"note",-3)); step.instrument = Int32(clamping:PayloadValue.integer(data,"instrument",-1))
        step.velocity = Int32(clamping:PayloadValue.integer(data,"velocity",100)); step.gate = Float(PayloadValue.number(data,"gate",0.8))
        step.fx1 = effect(data["fx1"] as? [String:Any]); step.fx2 = effect(data["fx2"] as? [String:Any])
        return step
    }
    /// A track's note columns, 1...ST_MAX_NOTE_COLUMNS (project key `noteColumns`, absent for 1).
    static func noteColumns(_ track: [String: Any]) -> Int {
        min(max(PayloadValue.integer(track,"noteColumns",1),1),Int(ST_MAX_NOTE_COLUMNS))
    }
    /// One extra note column cell {note, instrument?, velocity?, gate?}. Columns are newer than gate-less notes,
    /// so a missing gate is no gate (not the legacy 0.8 rows). Effects belong to column 0.
    static func columnStep(_ data:[String:Any]) -> STStep {
        var step = st_default_step()
        step.note = Int32(clamping:PayloadValue.integer(data,"note",-3)); step.instrument = Int32(clamping:PayloadValue.integer(data,"instrument",-1))
        step.velocity = Int32(clamping:PayloadValue.integer(data,"velocity",100)); step.gate = Float(PayloadValue.number(data,"gate",0))
        return step
    }
    static func effect(_ data: [String: Any]?) -> STStepFX {
        guard let data else { return STStepFX(type: 0, value: 0) }
        let mapping: [String: Int32] = ["volume":1,"pan":2,"pitch":3,"cutoff":4,"resonance":5,"delay":6,"reverb":7,"probability":8,"ratchet":9,"reverse":10,"offset":11,"glide":12,"microtiming":13,"arpeggio":14,"slice":15,"position":16]
        return STStepFX(type: mapping[data["type"] as? String ?? ""] ?? 0,value: Float(PayloadValue.number(data,"value",0)))
    }
}

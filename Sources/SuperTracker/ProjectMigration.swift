import Foundation

/// Brings older project documents to the current model. Runs where projects enter
/// the app (ProjectStore.read, and the audio host for payloads from an older UI),
/// so everything downstream sees only current fields. Idempotent and cheap when
/// there is nothing to migrate.
///
/// - Tempo maps (`timing`) are retired: the field is dropped and the project's base
///   tempo and meter are kept (taken from the first map events when missing).
/// - The built-in synth instrument is retired: oscillator synths become samplers in
///   Wavetable mode on the matching built-in single cycle (noise becomes a looping
///   noise sample); drum-model synths become one-shot samplers on a rendering of
///   that drum. Both are built-in sample references (BuiltinSamples), not project
///   media. Volume, pan, tuning, envelopes, filter, LFOs and voice settings are
///   shared with the sampler and carry over unchanged.
/// - Wavetable instruments saved before `wavetablePosition` existed used the grain
///   position as their table position.
/// - Kit lanes are retired: every kit becomes a track group with one member track per
///   Sound that has a source or hits (KitGroups.migrate).
enum ProjectMigration {
    static let drumModels = ["kick": 1, "snare": 2, "hihat": 3, "tom": 4, "perc": 5, "cymbal": 6]
    static let drumParameterKeys = ["tune", "decay", "tone", "impact", "noise", "attack", "drive", "character"]

    static func migrated(_ project: [String: Any]) -> [String: Any] { migratedWithNotes(project).project }

    /// The migrated project and what changed, for the message shown when it opens.
    static func migratedWithNotes(_ project: [String: Any]) -> (project: [String: Any], notes: [String]) {
        guard needsMigration(project) else { return (project, []) }
        var result = project
        if let timing = result.removeValue(forKey: "timing") as? [String: Any] {
            if result["bpm"] == nil, let first = (timing["tempos"] as? [[String: Any]])?.first, let bpm = first["bpm"] as? NSNumber {
                result["bpm"] = bpm.doubleValue
            }
            if result["timeSignature"] == nil, let first = (timing["meters"] as? [[String: Any]])?.first,
               let numerator = first["numerator"] as? NSNumber, let denominator = first["denominator"] as? NSNumber {
                result["timeSignature"] = ["numerator": numerator.intValue, "denominator": denominator.intValue]
            }
        }
        if let instruments = result["instruments"] as? [[String: Any]] {
            result["instruments"] = instruments.map(migratedInstrument)
        }
        if var kits = result["kits"] as? [[String: Any]] {
            for kit in kits.indices {
                guard var sounds = kits[kit]["sounds"] as? [[String: Any]] else { continue }
                for sound in sounds.indices {
                    if let instrument = sounds[sound]["instrument"] as? [String: Any] { sounds[sound]["instrument"] = migratedInstrument(instrument) }
                }
                kits[kit]["sounds"] = sounds
            }
            result["kits"] = kits
        }
        let kits = KitGroups.migrate(result)
        return (kits.project, kits.notes)
    }

    static func needsMigration(_ project: [String: Any]) -> Bool {
        if project["timing"] != nil || KitGroups.needsMigration(project) { return true }
        var instruments = project["instruments"] as? [[String: Any]] ?? []
        for kit in project["kits"] as? [[String: Any]] ?? [] {
            instruments += (kit["sounds"] as? [[String: Any]] ?? []).compactMap { $0["instrument"] as? [String: Any] }
        }
        return instruments.contains(where: instrumentNeedsMigration)
    }

    private static func instrumentNeedsMigration(_ instrument: [String: Any]) -> Bool {
        let type = instrument["type"] as? String
        if type == "synth" { return true }
        // Untyped instruments were synths unless they name a sample source.
        if type == nil {
            let hasSample = !(instrument["samplePath"] as? String ?? "").isEmpty || !(instrument["zones"] as? [Any] ?? []).isEmpty
            return !hasSample && instrument["plugin"] == nil && instrument["waveform"] != nil
        }
        return type == "sample" && instrument["mode"] as? String == "wavetable" && instrument["wavetablePosition"] == nil
    }

    static func migratedInstrument(_ instrument: [String: Any]) -> [String: Any] {
        guard instrumentNeedsMigration(instrument) else { return instrument }
        var result = instrument
        if result["type"] as? String == "sample" {
            result["wavetablePosition"] = result["grainPosition"] ?? 0.5
            return result
        }
        let drumModel = (instrument["drumModel"] as? String).flatMap { drumModels[$0] != nil ? $0 : nil }
        // `waveform` stays: it is inert on a sampler, and the current UI fills it in by default.
        result.removeValue(forKey: "drumModel")
        result.removeValue(forKey: "drumParameters")
        result["type"] = "sample"
        // The whole cycle is the region (synths ignored these; only stray values are reset).
        for (key, value) in ["start": 0.0, "end": 1.0, "loopStart": 0.0, "loopEnd": 1.0] where (result[key] as? NSNumber)?.doubleValue != value {
            result[key] = value
        }
        if let drumModel {
            let parameters = instrument["drumParameters"] as? [String: Any] ?? [:]
            let values = drumParameterKeys.map { key -> Float in
                guard let number = parameters[key] as? NSNumber, number.doubleValue.isFinite else { return .nan }
                return Float(max(0, min(1, number.doubleValue)))
            }
            result["samplePath"] = BuiltinSamples.drum(model: drumModel, parameters: values)
            result["mode"] = "oneshot"
            return result
        }
        let waveform = instrument["waveform"] as? String ?? "sine"
        result["samplePath"] = BuiltinWavetables.basicPath(waveform: waveform)
        if waveform == "noise" {
            result["mode"] = "forward"
        } else {
            result["mode"] = "wavetable"
            result["wavetablePosition"] = 0.0
            result["wavetableFrameSize"] = BuiltinWavetables.frameSize
        }
        return result
    }

    /// Sampler instrument on a built-in single cycle, as the MIDI importer and the
    /// agent API create for a requested waveform.
    static func wavetableInstrument(name: String, waveform: String, extra: [String: Any] = [:]) -> [String: Any] {
        var synth = extra
        synth["type"] = "synth"; synth["name"] = name; synth["waveform"] = waveform
        return migratedInstrument(synth)
    }
}

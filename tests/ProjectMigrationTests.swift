import Foundation

/// ProjectMigration, BuiltinWavetables/BuiltinSamples and WavetableFormat, without the
/// audio engine (drum rendering is injected). Writes only into a private support folder.
@main enum ProjectMigrationTests {
    struct Failure: Error { let message: String }
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !condition() { throw Failure(message: message) }
    }
    static func json(_ value: Any) throws -> Data { try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) }

    static func main() throws {
        let support = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-Migration-\(UUID().uuidString)", isDirectory: true)
        setenv("SUPERTRACKER_SUPPORT_DIR", support.path, 1)
        defer { try? FileManager.default.removeItem(at: support) }
        var groups = 0
        func pass(_ name: String) { groups += 1; print("PASS " + name) }

        // Tempo maps are dropped; the base tempo and meter stay (or come from the map's first events).
        let mapped: [String: Any] = ["version": 1, "tracks": [[:]], "instruments": [], "patterns": [["length": 4, "steps": [[Any]]()]],
            "timing": ["version": 1, "tempos": [["id": "a", "beat": 0, "bpm": 97.0], ["id": "b", "beat": 4, "bpm": 140.0]],
                       "meters": [["id": "m", "beat": 0, "numerator": 7, "denominator": 8]]]]
        let unmapped = ProjectMigration.migrated(mapped)
        try check(unmapped["timing"] == nil && (unmapped["bpm"] as? Double) == 97, "A tempo map without a base tempo keeps its first tempo")
        try check((unmapped["timeSignature"] as? [String: Int]) == ["numerator": 7, "denominator": 8], "A tempo map keeps its first meter")
        var based = mapped; based["bpm"] = 120.0; based["timeSignature"] = ["numerator": 3, "denominator": 4]
        let kept = ProjectMigration.migrated(based)
        try check(kept["bpm"] as? Double == 120 && (kept["timeSignature"] as? [String: Int]) == ["numerator": 3, "denominator": 4] && kept["timing"] == nil,
                  "The project's own base tempo and meter win over the retired map")
        pass("tempo maps are removed and the base tempo and meter are kept")

        // Oscillator synths become Wavetable samplers on the matching built-in cycle; shared settings carry over.
        let synth: [String: Any] = ["id": 3, "name": "Lead", "type": "synth", "waveform": "saw", "volume": 0.4, "pan": -0.25, "tune": 7.0,
            "attack": 0.01, "decay": 0.3, "sustain": 0.5, "release": 0.4, "cutoff": 900.0, "resonance": 0.3, "filterType": "bandpass",
            "filterEnvAmount": 12.0, "lfoTarget": "pitch", "lfoDepth": 0.2, "voiceMode": "mono", "chokeGroup": 2, "start": 0.3, "end": 0.6]
        let sampler = ProjectMigration.migratedInstrument(synth)
        try check(sampler["type"] as? String == "sample" && sampler["mode"] as? String == "wavetable", "A synth becomes a Wavetable sampler")
        try check(sampler["samplePath"] as? String == "builtin:wavetables/Basic/Saw.wav", "The waveform becomes its built-in cycle")
        try check(sampler["wavetablePosition"] as? Double == 0 && sampler["wavetableFrameSize"] as? Int == 2048, "Single cycles use frame 0 of a 2048-sample table")
        for key in ["id", "name", "volume", "pan", "tune", "attack", "decay", "sustain", "release", "cutoff", "resonance", "filterType",
                    "filterEnvAmount", "lfoTarget", "lfoDepth", "voiceMode", "chokeGroup"] {
            try check(try json([key: sampler[key]!]) == json([key: synth[key]!]), "\(key) carries over unchanged")
        }
        try check(sampler["start"] as? Double == 0 && sampler["end"] as? Double == 1, "The whole cycle is the region")
        for (waveform, file) in [("sine", "Sine"), ("triangle", "Triangle"), ("square", "Square")] {
            try check(ProjectMigration.migratedInstrument(["type": "synth", "waveform": waveform])["samplePath"] as? String == "builtin:wavetables/Basic/\(file).wav", "\(waveform) maps to its cycle")
        }
        let noise = ProjectMigration.migratedInstrument(["type": "synth", "waveform": "noise"])
        try check(noise["mode"] as? String == "forward" && noise["samplePath"] as? String == "builtin:wavetables/Basic/Noise.wav", "Noise becomes a looping noise sample")
        pass("oscillator synths become Wavetable samplers with their settings")

        // Drum-model synths become one-shot samplers on a rendering of that drum.
        let drum = ProjectMigration.migratedInstrument(["type": "synth", "drumModel": "snare", "rootNote": 38,
            "drumParameters": ["tune": 0.25, "decay": 0.5, "noise": 2.0]])
        let drumPath = drum["samplePath"] as? String ?? ""
        try check(drum["type"] as? String == "sample" && drum["mode"] as? String == "oneshot" && drum["rootNote"] as? Int == 38, "A drum synth becomes a one-shot sampler at its root note")
        try check(drumPath.hasPrefix("builtin:drums/snare/") && drumPath.count == "builtin:drums/snare/".count + 64 && drum["drumModel"] == nil && drum["drumParameters"] == nil,
                  "The drum reference encodes the model and all eight controls")
        var rendered: [(Int, [Float])] = []
        BuiltinSamples.drumRenderer = { model, parameters in rendered.append((model, parameters)); return [0.5, 0.25, 0] }
        let drumFile = try BuiltinSamples.resolve(drumPath)
        try check(rendered.count == 1 && rendered[0].0 == 2 && rendered[0].1[0] == 0.25 && rendered[0].1[1] == 0.5 && rendered[0].1[4] == 1
                  && rendered[0].1[2].isNaN, "The renderer receives the model, clamped controls and NaN for defaults")
        try check(FileManager.default.fileExists(atPath: drumFile) && (try BuiltinSamples.resolve(drumPath)) == drumFile && rendered.count == 1,
                  "A rendered drum is written once and reused")
        pass("drum-model synths become rendered one-shot samples")

        // Untyped instruments: synths only when they name a waveform and no source.
        try check(ProjectMigration.migratedInstrument(["samplePath": "/a.wav", "waveform": "sine"])["samplePath"] as? String == "/a.wav", "An untyped sampler keeps its sample")
        try check(ProjectMigration.migratedInstrument(["waveform": "square"])["mode"] as? String == "wavetable", "An untyped synth migrates")
        let legacyTable = ProjectMigration.migratedInstrument(["type": "sample", "mode": "wavetable", "samplePath": "/t.wav", "grainPosition": 0.7])
        try check(legacyTable["wavetablePosition"] as? Double == 0.7, "Older wavetable instruments take their position from the grain position")
        for instrument: [String: Any] in [["type": "sample", "samplePath": "/a.wav"], ["type": "midi"], ["type": "au", "plugin": ["id": "x"]],
                                         ["type": "sample", "mode": "wavetable", "wavetablePosition": 0.2]] {
            try check(try json(ProjectMigration.migratedInstrument(instrument)) == json(instrument), "Current instruments are untouched")
        }
        pass("untyped and current instruments")

        // Whole projects: instruments and kit Sounds; idempotent; current projects pass through unchanged.
        let project: [String: Any] = ["version": 1, "bpm": 120.0, "tracks": [["instrument": 0]], "patterns": [["length": 4, "steps": [[Any]]()]],
            "instruments": [synth, ["type": "sample", "samplePath": "/x.wav"]],
            "kits": [["id": "k", "busID": "b", "sounds": [["id": "s", "instrument": ["type": "synth", "drumModel": "kick"]], ["id": "t", "instrument": ["type": "sample"]]]]]]
        try check(ProjectMigration.needsMigration(project), "A project with synths needs migration")
        let migrated = ProjectMigration.migrated(project)
        // The kit became a group; its loaded Sound became a member track with an ordinary instrument.
        try check(migrated["kits"] == nil && (migrated["groups"] as? [[String: Any]])?.first?["id"] as? String == "k", "Kits become track groups")
        let member = (migrated["tracks"] as! [[String: Any]]).first { $0["groupID"] as? String == "k" }!
        let sound = (migrated["instruments"] as! [[String: Any]])[member["instrument"] as! Int]
        try check((migrated["instruments"] as! [[String: Any]])[0]["mode"] as? String == "wavetable" && (sound["samplePath"] as? String ?? "").hasPrefix("builtin:drums/kick/"),
                  "Project instruments and kit Sounds both migrate")
        try check(!ProjectMigration.needsMigration(migrated) && (try json(ProjectMigration.migrated(migrated))) == json(migrated), "Migration is idempotent")
        let current: [String: Any] = ["version": 1, "instruments": [["type": "sample", "samplePath": "/x.wav"]]]
        try check(!ProjectMigration.needsMigration(current), "A current project needs nothing")
        pass("projects, kit Sounds and idempotence")

        // Built-in tables: generated once, with a frame-size chunk that detection reads back.
        let tables = try BuiltinWavetables.list()
        try check(tables.count == BuiltinWavetables.catalogue().count && tables.contains { $0["name"] as? String == "Pulse Width" && $0["frames"] as? Int == 32 },
                  "The listing names every table with its frame count")
        for row in tables {
            let path = try BuiltinSamples.resolve(row["path"] as! String)
            let size = (try FileManager.default.attributesOfItem(atPath: path)[.size] as? NSNumber)?.intValue ?? 0
            if row["wavetable"] as? Bool == true {
                let frames = (row["frames"] as! Int) * 2048
                try check(WavetableFormat.declaredFrameSize(atPath: path) == 2048, "\(row["name"]!) declares its 2048-sample frames")
                try check(size > frames * 4, "\(row["name"]!) holds \(frames) float samples")
            } else {
                try check(WavetableFormat.declaredFrameSize(atPath: path) == nil, "Noise is not a wavetable")
            }
        }
        let saw = try BuiltinSamples.resolve(BuiltinWavetables.basicPath(waveform: "saw"))
        let modified = try FileManager.default.attributesOfItem(atPath: saw)[.modificationDate] as? Date
        _ = try BuiltinWavetables.installIfNeeded()
        try check(try FileManager.default.attributesOfItem(atPath: saw)[.modificationDate] as? Date == modified, "Installed tables are not rewritten")
        do { _ = try BuiltinSamples.resolve("builtin:wavetables/Basic/Missing.wav"); try check(false, "Unknown tables are rejected") } catch is BuiltinSamples.Failure {}
        pass("built-in tables are generated, labelled and reused")

        // Generated cycles are bounded, DC-free, normalised and morph.
        for table in BuiltinWavetables.catalogue() where table.isWavetable {
            for frame in table.frames {
                try check(frame.count == 2048 && frame.allSatisfy { $0.isFinite && abs($0) <= 0.9001 }, "\(table.name) frames are finite and within 0.9")
                try check(abs(frame.reduce(0, +) / 2048) < 0.05, "\(table.name) frames carry no DC offset")
            }
            try check(table.frames.contains { frame in frame.contains { abs($0) > 0.85 } }, "\(table.name) is normalised")
        }
        let morph = BuiltinWavetables.catalogue().first { $0.name == "Sine to Saw" }!
        try check(morph.frames.count == 32 && zip(morph.frames[0], morph.frames[31]).contains { abs($0 - $1) > 0.1 }, "A morph table changes across its frames")
        pass("generated cycles are bounded, DC-free, normalised and morph")

        // Frame-size detection.
        try check(WavetableFormat.frameSize(clmText: "<!>256 10000000 wavetable (x)") == 256 && WavetableFormat.frameSize(clmText: "<!>9 0") == nil
                  && WavetableFormat.frameSize(clmText: "2048") == nil, "clm text parsing")
        try check(WavetableFormat.detectFrameSize(declared: 512, frames: 4096) == 512 && WavetableFormat.detectFrameSize(declared: 512, frames: 100) == nil,
                  "A declared size wins when the file holds a frame")
        try check(WavetableFormat.detectFrameSize(declared: nil, frames: 2048 * 64) == 2048, "Whole multiples of 2048 are tables")
        try check(WavetableFormat.detectFrameSize(declared: nil, frames: 1024) == 1024 && WavetableFormat.detectFrameSize(declared: nil, frames: 44_100) == nil,
                  "A short power-of-two file is one cycle; other files use the sampler default")
        let plain = support.appendingPathComponent("plain.wav")
        try WavetableFormat.wavData(samples: [0, 0.5, -0.5, 0]).write(to: plain)
        try check(WavetableFormat.declaredFrameSize(atPath: plain.path) == nil && WavetableFormat.declaredFrameSize(atPath: "/nonexistent") == nil, "Files without the chunk declare nothing")
        pass("frame-size detection from metadata and size")

        // MIDI import placeholders use the same built-in cycles.
        let placeholder = ProjectMigration.wavetableInstrument(name: "GM Program 33", waveform: "saw", extra: ["volume": 0.5])
        try check(placeholder["type"] as? String == "sample" && placeholder["name"] as? String == "GM Program 33" && placeholder["volume"] as? Double == 0.5
                  && placeholder["samplePath"] as? String == "builtin:wavetables/Basic/Saw.wav", "Wavetable instrument factory")
        pass("wavetable instrument factory")
        print("Project migration: \(groups) groups passed")
    }
}

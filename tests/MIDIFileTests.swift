import Foundation

private struct TestFailure: Error, CustomStringConvertible {
    let description: String
}

private func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
    if try !condition() { throw TestFailure(description: message) }
}

private func near(_ actual: Any?, _ expected: Double, tolerance: Double = 0.0001) -> Bool {
    guard let value = actual as? NSNumber else { return false }
    return abs(value.doubleValue - expected) <= tolerance
}

@main
struct MIDIFileTests {
    private struct Event {
        let tick: Int
        let status: UInt8
        let data: [UInt8]
    }

    private struct LocatedStep {
        let pattern: Int
        let track: Int
        let row: Int
        let value: [String: Any]
        var column = 0
    }

    static func main() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-MIDI-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        try runningStatusAndZeroVelocity(directory)
        try typeOneMetadataAndRouting(directory)
        try tempoChangesPreserveElapsedTime(directory)
        try chordsAndMicrotiming(directory)
        try sustainPedalExtendsGate(directory)
        try coincidentNotesRetainDifferentGates(directory)
        try controllersCarryAcrossPatterns(directory)
        try sequenceExportAndRoundtrip(directory)
        try tempoMapExport(directory)
        try stickyRatchetExport(directory)
        try gatelessNotesExport(directory)
        try explicitControllerMappingExport(directory)
        try importCapacityWarnings(directory)
        try manyTrackExport(directory)
        try timelineBoundariesAndTrailingSilence(directory)
        try overlappingSamePitchNotesEndAtNextOnset(directory)
        try noteColumnsExportAndImport(directory)
        try malformedFiles(directory)
        print("PASS MIDI files: running status, note-off velocity zero, type 1 metadata, tempo changes, channel/program routing, chords, microtiming, sustain, independent gates, controller carry, sequence export, sticky ratchets, mapped CC export, capacity warnings, timeline boundaries, roundtrip, malformed input, same-pitch overlap")
    }

    private static func runningStatusAndZeroVelocity(_ directory: URL) throws {
        let data = file(format: 0, tracks: [[
            0x00, 0x90, 60, 100,
            0x78, 64, 90,
            0x78, 60, 0,
            0x78, 64, 0,
            0x00, 0xff, 0x2f, 0x00
        ]])
        let project = try load(data, named: "Running status", directory: directory)
        let notes = steps(project)
        try require(notes.count == 2, "Velocity-zero note-ons must end notes, not create notes")
        let low = try step(notes, note: 60)
        let high = try step(notes, note: 64)
        try require(low.row == 0 && high.row == 1, "Running status note timing must remain on the expected rows")
        try require(near(low.value["gate"], 2) && near(high.value["gate"], 2), "Running status note-off events must preserve two-row gates")
        try require(near(project["bpm"], 120) && near(project["linesPerBeat"], 4), "Files without a tempo event must use standard MIDI's 120 BPM default")
        try require(JSONSerialization.isValidJSONObject(project), "Imported projects must remain JSON serializable")
    }

    private static func typeOneMetadataAndRouting(_ directory: URL) throws {
        let conductor: [UInt8] = [
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20,
            0x00, 0xff, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08,
            0x00, 0xff, 0x2f, 0x00
        ]
        let instrument: [UInt8] = [
            0x00, 0xc2, 10,
            0x00, 0xb2, 7, 100,
            0x00, 0xb2, 10, 32,
            0x78, 0x92, 67, 104,
            0x81, 0x70, 0x82, 67, 64,
            0x00, 0xff, 0x2f, 0x00
        ]
        let project = try load(file(format: 1, tracks: [conductor, instrument]), named: "Type 1", directory: directory)
        let note = try step(steps(project), note: 67)
        try require(note.row == 1 && near(note.value["gate"], 2), "Type 1 note ticks must resolve against the conductor tempo")
        let instruments = project["instruments"] as? [[String: Any]] ?? []
        let routed = instruments.first { near($0["midiChannel"], 3) && near($0["midiProgram"], 10) }
        try require(routed != nil, "Imported instrument must retain one-based MIDI channel 3 and zero-based program 10")
        try require(routed?["type"] as? String == "sample" && (routed?["samplePath"] as? String ?? "").hasPrefix("builtin:"),
                    "Imported programs should have audible built-in wavetable placeholders")
        let metadata = project["midiImport"] as? [String: Any] ?? [:]
        try require(near(metadata["format"], 1) && near(metadata["ticksPerQuarter"], 480), "Original file timing metadata must be retained")
        let tempos = metadata["tempoChanges"] as? [[String: Any]] ?? []
        try require(tempos.contains { near($0["tick"], 0) && near($0["microsecondsPerQuarter"], 500_000) }, "Tempo metadata must be retained")
        let signatures = metadata["timeSignatures"] as? [[String: Any]] ?? []
        try require(signatures.contains { near($0["numerator"], 3) && near($0["denominator"], 4) }, "3/4 time-signature metadata must be retained")
        let programs = metadata["programChanges"] as? [[String: Any]] ?? []
        try require(programs.contains { near($0["channel"], 3) && near($0["program"], 10) }, "Program-change metadata must retain channel and program")
        let controllers = metadata["controllers"] as? [[String: Any]] ?? []
        try require(controllers.contains { near($0["controller"], 7) && near($0["value"], 100) && near($0["channel"], 3) }, "CC 7 must be retained")
        try require(controllers.contains { near($0["controller"], 10) && near($0["value"], 32) && near($0["channel"], 3) }, "CC 10 must be retained")
        try require(metadata["warnings"] is [String], "Import reports must expose warnings even when empty")
    }

    private static func chordsAndMicrotiming(_ directory: URL) throws {
        let project = try load(file(format: 0, tracks: [[
            0x1e, 0x90, 60, 100,
            0x00, 0x90, 64, 100,
            0x00, 0x90, 67, 100,
            0x78, 0x80, 60, 64,
            0x00, 0x80, 64, 64,
            0x00, 0x80, 67, 64,
            0x00, 0xff, 0x2f, 0x00
        ]]), named: "Microtimed chord", directory: directory)
        let notes = steps(project)
        try require(notes.count == 1, "Simultaneous notes on one channel should share a chord step")
        let chord = try step(notes, note: 60)
        try require(chord.row == 0, "Sub-row timing belongs on the original row")
        try require((chord.value["chord"] as? [Int]) == [0, 4, 7], "C major should retain its three chord intervals")
        let effects = [chord.value["fx1"], chord.value["fx2"]].compactMap { $0 as? [String: Any] }
        try require(effects.contains { $0["type"] as? String == "microtiming" && near($0["value"], 0.25) }, "Thirty ticks at 480 PPQN must retain quarter-row microtiming")
        try require(near(chord.value["gate"], 1 / 0.75), "Microtimed gate must account for the engine's shortened available row")
    }

    private static func tempoChangesPreserveElapsedTime(_ directory: URL) throws {
        let conductor: [UInt8] = [
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20,
            0x83, 0x60, 0xff, 0x51, 0x03, 0x0f, 0x42, 0x40,
            0x00, 0xff, 0x2f, 0x00
        ]
        let instrument: [UInt8] = [
            0x00, 0x90, 60, 100,
            0x83, 0x60, 0x80, 60, 64,
            0x00, 0x90, 64, 100,
            0x83, 0x60, 0x80, 64, 64,
            0x00, 0xff, 0x2f, 0x00
        ]
        let project = try load(file(format: 1, tracks: [conductor, instrument]), named: "Tempo change", directory: directory)
        let first = try step(steps(project), note: 60)
        let second = try step(steps(project), note: 64)
        try require(near(project["bpm"], 120), "Tempo flattening must retain the initial tempo")
        try require(first.row == 0 && near(first.value["gate"], 4), "The first quarter note at 120 BPM must span four rows")
        try require(second.row == 4 && near(second.value["gate"], 8), "The second quarter note at 60 BPM must preserve one second of elapsed playback")
        let metadata = project["midiImport"] as? [String: Any] ?? [:]
        try require((metadata["tempoChanges"] as? [[String: Any]])?.count == 2, "Both source tempo events must remain available in import metadata")
    }

    private static func sustainPedalExtendsGate(_ directory: URL) throws {
        let project = try load(file(format: 0, tracks: [[
            0x00, 0x90, 60, 100,
            0x00, 0xb0, 64, 127,
            0x78, 0x80, 60, 64,
            0x81, 0x70, 0xb0, 64, 0,
            0x00, 0x90, 64, 100,
            0x00, 0xb0, 64, 63,
            0x78, 0x80, 64, 64,
            0x00, 0xff, 0x2f, 0x00
        ]]), named: "Sustain pedal", directory: directory)
        let held = try step(steps(project), note: 60)
        let released = try step(steps(project), note: 64)
        try require(held.row == 0 && near(held.value["gate"], 3), "A sustained note must continue past note-off until the pedal is released")
        try require(released.row == 3 && near(released.value["gate"], 1), "CC64 values below 64 must leave subsequent note-off timing intact")
    }

    private static func coincidentNotesRetainDifferentGates(_ directory: URL) throws {
        let project = try load(file(format: 0, tracks: [[
            0x00, 0x90, 60, 100,
            0x00, 0x90, 64, 100,
            0x78, 0x80, 60, 64,
            0x78, 0x80, 64, 64,
            0x00, 0xff, 0x2f, 0x00
        ]]), named: "Independent gates", directory: directory)
        let notes = steps(project)
        let short = try step(notes, note: 60)
        let long = try step(notes, note: 64)
        try require(notes.count == 2 && short.track == long.track && short.column == 0 && long.column == 1, "Coincident notes with different note-offs must use independent note columns of one track")
        try require(near((project["tracks"] as? [[String: Any]])?.first?["noteColumns"], 2) && (project["tracks"] as? [[String: Any]])?.count == 1, "The track gets the note columns it needs")
        try require(short.row == 0 && long.row == 0, "Independent note gates may not shift coincident note onsets")
        try require(near(short.value["gate"], 1) && near(long.value["gate"], 2), "Each coincident note must retain its own duration")
    }

    private static func controllersCarryAcrossPatterns(_ directory: URL) throws {
        var track: [UInt8] = [0x00, 0x90, 60, 100, 0x78, 0x80, 60, 64, 0x00, 0xb0, 7, 32]
        track += variable(64 * 120) + [0x90, 64, 100]
        track += variable(120) + [0x80, 64, 64]
        track += variable(120) + [0xb0, 7, 100]
        track += variable(62 * 120) + [0x90, 67, 100]
        track += variable(120) + [0x80, 67, 64, 0x00, 0xff, 0x2f, 0x00]
        let project = try load(file(format: 0, tracks: [track]), named: "Controller carry", directory: directory)
        let patterns = project["patterns"] as? [[String: Any]] ?? []
        try require(patterns.count >= 3, "Controller carry fixture must span at least three patterns")
        func volumePoints(_ pattern: Int) -> [[String: Any]] {
            let automation = patterns[pattern]["automation"] as? [[String: Any]] ?? []
            return automation.first { $0["target"] as? String == "volume" }?["points"] as? [[String: Any]] ?? []
        }
        try require(volumePoints(0).contains { near($0["row"], 1) && near($0["value"], 32.0 / 127) }, "The original CC7 event must remain at its source row")
        try require(volumePoints(1).contains { near($0["row"], 0) && near($0["value"], 32.0 / 127) }, "The next pattern must start with the preceding pattern's final CC7 value")
        try require(volumePoints(1).contains { near($0["row"], 3) && near($0["value"], 100.0 / 127) }, "A later CC7 change must retain its local row after the carried value")
        try require(volumePoints(2).contains { near($0["row"], 0) && near($0["value"], 100.0 / 127) }, "A pattern without its own controllers must inherit the last controller state")
    }

    /// Gate 0 (no gate): a note lasts until the next note on its track, an OFF/CUT, or the song end.
    /// A missing gate is the legacy 0.8 rows.
    private static func gatelessNotesExport(_ directory: URL) throws {
        var rows: [Any] = Array(repeating: NSNull(), count: 8)
        rows[0] = ["note": 60, "velocity": 100, "gate": 0, "chord": [0, 4]]
        rows[2] = ["note": 62, "velocity": 100, "gate": 0, "fx1": ["type": "microtiming", "value": 0.5]]
        rows[3] = ["note": 64, "velocity": 100, "gate": 0, "fx1": ["type": "probability", "value": 0]]
        rows[4] = ["note": -1]
        rows[5] = ["note": 65, "velocity": 100]
        rows[6] = ["note": 67, "velocity": 100, "gate": 0]
        let project: [String: Any] = [
            "version": 1, "name": "Gateless", "bpm": 120.0, "linesPerBeat": 4,
            "tracks": [["id": 0, "name": "Lead", "instrument": 0]], "instruments": [["id": 0, "name": "Lead", "type": "synth"]],
            "patterns": [["id": 0, "name": "A", "length": 8, "steps": [rows], "automation": []]], "sequence": [0]
        ]
        let destination = directory.appendingPathComponent("gateless.mid")
        try MIDIFile.export(project, to: destination)
        let decoded = try decode(try Data(contentsOf: destination))
        let row = decoded.division / 4
        func span(_ pitch: UInt8) -> [Int] {
            decoded.events.filter { [0x80, 0x90].contains($0.status & 0xf0) && $0.data.count == 2 && $0.data[0] == pitch }.map { Int($0.tick) }
        }
        try require(span(60) == [0, 2 * row + row / 2] && span(64) == [0, 2 * row + row / 2], "A gateless chord lasts until the next (microtimed) note starts")
        try require(span(62) == [2 * row + row / 2, 4 * row], "A dropped probability note leaves the gateless note sounding until OFF")
        try require(span(65) == [5 * row, 5 * row + row * 4 / 5], "A step without a gate keeps the legacy 0.8-row gate")
        try require(span(67) == [6 * row, 8 * row], "A gateless note at the end lasts until the song end")
    }

    private static func stickyRatchetExport(_ directory: URL) throws {
        var rows: [Any] = Array(repeating: NSNull(), count: 9)
        rows[0] = ["note": 60, "gate": 0.5, "chord": [0, 7], "fx1": ["type": "ratchet", "value": 2]]
        rows[2] = ["note": 64, "gate": 0.5]
        rows[3] = ["note": -3, "fx1": ["type": "ratchet", "value": 4]]
        rows[4] = ["note": -3, "fx1": ["type": "pan", "value": 0.5]]
        rows[6] = ["note": -3, "fx1": ["type": "ratchet", "value": 2]]
        rows[7] = ["note": -1]
        let project: [String: Any] = ["bpm": 120, "linesPerBeat": 4,
            "tracks": [["instrument": 0]], "instruments": [["midiChannel": 1]],
            "patterns": [["id": 0, "length": 1, "steps": [Array(rows.prefix(1))]], ["id": 1, "length": 8, "steps": [Array(rows.dropFirst())]]], "sequence": [0, 1]]
        let destination = directory.appendingPathComponent("sticky-ratchet.mid")
        try MIDIFile.export(project, to: destination)
        let decoded = try decode(try Data(contentsOf: destination))
        let on = decoded.events.filter { $0.status & 0xf0 == 0x90 && $0.data[1] > 0 }
        let expected = [0, 0, 120, 120, 240, 240, 360, 360, 480, 720, 780, 840, 900, 1440, 1560]
        try require(on.map(\.tick) == expected, "Export must preserve sticky ratchets, a plain note stopping them, effect cancellation, FX-only restart and OFF")
        try require(on.prefix(8).map { $0.data[0] } == [60, 67, 60, 67, 60, 67, 60, 67], "Repeated rows retain the original chord")
        try require(on.suffix(7).allSatisfy { $0.data[0] == 64 }, "A later note replaces the held ratchet chord")
    }

    // Extra note columns are independent voices: a new note or OFF in one column leaves the other sounding. Import puts
    // overlapping notes into note columns (up to eight per track) and a different microtiming into another track.
    private static func noteColumnsExportAndImport(_ directory: URL) throws {
        var rows: [Any] = Array(repeating: NSNull(), count: 8)
        rows[0] = ["note": 60, "velocity": 100, "gate": 0, "columns": [["note": 64, "velocity": 90, "gate": 0], ["note": 67, "velocity": 80, "gate": 1]]]
        rows[2] = ["note": 62, "velocity": 100, "gate": 0, "fx1": ["type": "ratchet", "value": 2]]
        rows[3] = ["note": -3, "columns": [["note": -1]]]
        rows[4] = ["note": -1, "columns": [NSNull(), ["note": 72, "velocity": 100, "gate": 0], ["note": 76, "velocity": 100, "gate": 0]]]
        let project: [String: Any] = [
            "version": 1, "name": "Columns", "bpm": 120.0, "linesPerBeat": 4,
            "tracks": [["id": 0, "name": "Keys", "instrument": 0, "noteColumns": 3]], "instruments": [["id": 0, "name": "Keys", "type": "synth"]],
            "patterns": [["id": 0, "name": "A", "length": 8, "steps": [rows], "automation": []]], "sequence": [0]
        ]
        let destination = directory.appendingPathComponent("columns.mid")
        try MIDIFile.export(project, to: destination)
        let decoded = try decode(try Data(contentsOf: destination))
        let row = decoded.division / 4
        func span(_ pitch: UInt8) -> [Int] {
            decoded.events.filter { [0x80, 0x90].contains($0.status & 0xf0) && $0.data.count == 2 && $0.data[0] == pitch }.map { Int($0.tick) }
        }
        try require(span(60) == [0, 2 * row], "Column 0's note lasts until column 0's next note")
        try require(span(64) == [0, 3 * row], "Column 1's note ignores column 0's new note and ends at column 1's OFF")
        try require(span(67) == [0, row], "Column 2 keeps its own gate")
        try require(span(62).count == 4 && span(62).first == 2 * row, "Column 0's ratchet note repeats")
        try require(span(72) == [4 * row, 8 * row], "Column 2 plays on through column 0's OFF")
        try require(span(76).isEmpty, "A column beyond the track's note columns is not exported")

        let reimported = try MIDIFile.importProject(destination)
        let cells = steps(reimported)
        try require(cells.filter { $0.row == 0 }.count == 3 && Set(cells.filter { $0.row == 0 }.map(\.track)).count == 1, "Overlapping notes reimport into one track's note columns")
        try require(cells.contains { $0.row == 0 && $0.column == 2 && near($0.value["gate"], 1) }, "A reimported column keeps its gate")

        // Eight overlapping notes with different lengths fill one track's eight note columns; a ninth needs a second track.
        var track: [UInt8] = []
        for index in 0..<9 { track += [0x00, 0x90, UInt8(48 + index), 100] }
        for index in 0..<9 { track += variable(120) + [0x80, UInt8(48 + index), 64] }
        track += [0x00, 0xff, 0x2f, 0x00]
        let dense = try load(file(format: 0, tracks: [track]), named: "Nine voices", directory: directory)
        let tracks = dense["tracks"] as? [[String: Any]] ?? []
        try require(tracks.count == 2 && near(tracks[0]["noteColumns"], 8) && tracks[1]["noteColumns"] == nil, "Eight note columns per track, then another track")
        try require(steps(dense).count == 9, "Every note is imported")
    }

    /// Receivers pair note-offs with the most recent note-on, so a long note overlapping the same pitch on its channel
    /// must end where the next one begins (as live playback does); identical onsets collapse to one note.
    private static func overlappingSamePitchNotesEndAtNextOnset(_ directory: URL) throws {
        var rows: [Any] = Array(repeating: NSNull(), count: 8)
        rows[0] = ["note": 60, "velocity": 100, "gate": 4.0]
        rows[1] = ["note": 60, "velocity": 90, "gate": 1.0]
        rows[4] = ["note": 62, "velocity": 100, "gate": 2.0, "chord": [0, 0, 12]]
        let project: [String: Any] = [
            "version": 1, "name": "Overlap", "bpm": 120.0, "linesPerBeat": 4,
            "tracks": [["id": 0, "name": "Lead", "instrument": 0]], "instruments": [["id": 0, "name": "Lead", "type": "synth"]],
            "patterns": [["id": 0, "name": "A", "length": 8, "steps": [rows], "automation": []]], "sequence": [0]
        ]
        let destination = directory.appendingPathComponent("overlap.mid")
        let report = try MIDIFile.export(project, to: destination)
        let decoded = try decode(try Data(contentsOf: destination))
        let rowTicks = decoded.division / 4
        let notes = decoded.events.filter { [0x80, 0x90].contains($0.status & 0xf0) && $0.data.count == 2 }
        for pitch in [UInt8(60), 62, 74] {
            var sounding = false
            for event in notes where event.data[0] == pitch {
                let on = event.status & 0xf0 == 0x90 && event.data[1] > 0
                try require(on != sounding, "Pitch \(pitch) must alternate note-on and note-off (tick \(event.tick))")
                sounding = on
            }
            try require(!sounding, "Pitch \(pitch) must end")
        }
        let c = notes.filter { $0.data[0] == 60 }.map { ($0.tick, $0.status & 0xf0 == 0x90 && $0.data[1] > 0) }
        try require(c.map(\.0) == [0, rowTicks, rowTicks, 2 * rowTicks] && c.map(\.1) == [true, false, true, false], "The long note ends at the next onset; the second keeps its one-row gate")
        try require(near(report["noteCount"], 4), "Duplicate chord voices at one onset export as one note")
    }

    private static func sequenceExportAndRoundtrip(_ directory: URL) throws {
        var longSteps: [Any] = Array(repeating: NSNull(), count: 8)
        longSteps[3] = ["note": 60, "instrument": 0, "velocity": 100, "gate": 2.0, "chord": [0, 4, 7]]
        var shortSteps: [Any] = Array(repeating: NSNull(), count: 4)
        shortSteps[0] = ["note": 48, "instrument": 0, "velocity": 80, "gate": 1.0]
        let project: [String: Any] = [
            "version": 1, "name": "Sequence test", "bpm": 120.0, "linesPerBeat": 4,
            "tracks": [["id": 0, "name": "Keys", "instrument": 0, "volume": 0.8, "pan": 0.0]],
            "instruments": [["id": 0, "name": "Keys", "type": "synth", "midiChannel": 3, "midiProgram": 10]],
            "patterns": [
                ["id": 0, "name": "Chord", "length": 8, "steps": [longSteps], "automation": []],
                ["id": 1, "name": "Bass", "length": 4, "steps": [shortSteps], "automation": []]
            ],
            "sequence": [1, 0, 1]
        ]
        let destination = directory.appendingPathComponent("sequence.mid")
        let report = try MIDIFile.export(project, to: destination)
        try require(report["path"] as? String == destination.path, "Export report must identify its output file")
        try require(near(report["noteCount"], 5), "Export must count three chord voices and both repeated bass notes")
        try require(report["warnings"] is [String], "Export report must expose unsupported-feature warnings")

        let decoded = try decode(try Data(contentsOf: destination))
        let rowTicks = decoded.division / 4
        try require(rowTicks > 0, "Export must use a positive PPQN time division")
        try require(decoded.events.contains { $0.status == 0xff && $0.data == [0x51, 0x07, 0xa1, 0x20] }, "Export must include the 120 BPM tempo event")
        let ons = decoded.events.filter { $0.status & 0xf0 == 0x90 && $0.data.count == 2 && $0.data[1] > 0 }
        try require(ons.count == 5, "Emitted MIDI must contain five note-on events")
        try require(ons.allSatisfy { $0.status & 0x0f == 2 }, "Instrument MIDI channel 3 must become status channel index 2")
        try require(ons.filter { $0.data[0] == 48 }.map(\.tick).sorted() == [0, 12 * rowTicks], "Export must follow the sequence, including repeated patterns")
        let chord = ons.filter { [UInt8(60), 64, 67].contains($0.data[0]) }
        try require(chord.count == 3 && chord.allSatisfy { $0.tick == 7 * rowTicks }, "Chord row 3 must be offset by the preceding four-row pattern")
        let offs = decoded.events.filter { $0.status & 0xf0 == 0x80 || ($0.status & 0xf0 == 0x90 && $0.data.last == 0) }
        for pitch in [UInt8(60), 64, 67] {
            try require(offs.contains { $0.data.first == pitch && $0.tick == 9 * rowTicks }, "Every chord voice must retain its two-row gate")
        }
        try require(decoded.events.contains { $0.status == 0xc2 && $0.data == [10] }, "Export must preserve the instrument program")
        try require(decoded.events.contains { $0.status == 0xb2 && $0.data.first == 7 }, "Export must emit the track's basic volume controller")

        let imported = try MIDIFile.importProject(destination)
        let roundtripNotes = steps(imported)
        try require(near(imported["bpm"], 120), "Tempo must survive export/import")
        let importedChord = try step(roundtripNotes, note: 60)
        try require(importedChord.row == 7 && near(importedChord.value["gate"], 2), "Sequence position and chord duration must survive export/import")
        try require(importedChord.value["chord"] as? [Int] == [0, 4, 7], "Chord intervals must survive export/import")
        try require(roundtripNotes.filter { near($0.value["note"], 48) }.map(\.row).sorted() == [0, 12], "Repeated pattern instances must survive export/import")
    }

    // The project tempo map, not the legacy bpm mirror, decides when rows sound.
    private static func tempoMapExport(_ directory: URL) throws {
        var rows: [Any] = Array(repeating: NSNull(), count: 64)
        rows[32] = ["note": 60, "instrument": 0, "velocity": 100, "gate": 1.0]
        let project: [String: Any] = [
            "version": 1, "name": "Tempo map", "bpm": 120.0, "linesPerBeat": 4, "timeSignature": ["numerator": 4, "denominator": 4],
            "timing": ["version": 1,
                       "tempos": [["id": "a", "beat": 0, "bpm": 120.0], ["id": "b", "beat": 4, "bpm": 60.0], ["id": "late", "beat": 100, "bpm": 90.0]],
                       "meters": [["id": "m", "beat": 0, "numerator": 4, "denominator": 4], ["id": "n", "beat": 8, "numerator": 3, "denominator": 4]]],
            "tracks": [["id": 0, "name": "Keys", "instrument": 0]],
            "instruments": [["id": 0, "name": "Keys", "type": "synth"]],
            "patterns": [["id": 0, "name": "A", "length": 64, "steps": [rows], "automation": []]],
            "sequence": [0]
        ]
        let destination = directory.appendingPathComponent("tempo-map.mid")
        try MIDIFile.export(project, to: destination)
        let decoded = try decode(try Data(contentsOf: destination))
        let tempos = decoded.events.filter { $0.status == 0xff && $0.data.first == 0x51 }.map { [$0.tick] + $0.data.dropFirst().map(Int.init) }
        try require(tempos == [[0, 0x07, 0xa1, 0x20], [4 * decoded.division, 0x0f, 0x42, 0x40]], "Export must write every tempo change inside the song: \(tempos)")
        let meters = decoded.events.filter { $0.status == 0xff && $0.data.first == 0x58 }.map { [$0.tick, Int($0.data[1]), Int($0.data[2])] }
        try require(meters == [[0, 4, 2], [8 * decoded.division, 3, 2]], "Export must write every meter change: \(meters)")
        // 4 beats at 120 BPM + 4 beats at 60 BPM = 6 s, which is row 48 at the imported 120 BPM.
        try require(try step(steps(try MIDIFile.importProject(destination)), note: 60).row == 48, "Tempo-mapped note timing must survive export/import")
    }

    private static func malformedFiles(_ directory: URL) throws {
        let cases: [(String, Data)] = [
            ("Overlong VLQ", file(format: 0, tracks: [[0x81, 0x80, 0x80, 0x80, 0x00, 0xff, 0x2f, 0x00]])),
            ("Missing running status", file(format: 0, tracks: [[0x00, 60, 100, 0x00, 0xff, 0x2f, 0x00]])),
            ("Truncated chunk", Data(Array("MThd".utf8) + be32(6) + be16(0) + be16(1) + be16(480) + Array("MTrk".utf8) + be32(8) + [0x00, 0xff, 0x2f, 0x00])),
            ("SMPTE time division", file(format: 0, division: 0xe728, tracks: [[0x00, 0xff, 0x2f, 0x00]])),
            ("Truncated channel event", file(format: 0, tracks: [[0x00, 0x90, 60]])),
            ("Invalid header", Data([0, 1, 2, 3]))
        ]
        for (name, data) in cases {
            let url = directory.appendingPathComponent(name + ".mid")
            try data.write(to: url)
            var rejected = false
            do { _ = try MIDIFile.importProject(url) } catch { rejected = true }
            try require(rejected, "Malformed MIDI accepted: \(name)")
        }
    }

    private static func explicitControllerMappingExport(_ directory: URL) throws {
        var rows: [Any] = Array(repeating: NSNull(), count: 16)
        rows[0] = ["note": 60, "instrument": 0, "velocity": 100, "gate": 1.0]
        rows[12] = ["note": -3, "instrument": 0, "fx1": ["type": "cutoff", "value": 20.0]]
        let project: [String: Any] = [
            "version": 1, "name": "Mapped MIDI automation", "bpm": 120.0, "linesPerBeat": 4,
            "tracks": [["id": 0, "name": "External synth", "instrument": 0, "volume": 0.8, "pan": 0.0]],
            "instruments": [["id": 0, "name": "External synth", "type": "midi", "midiChannel": 5,
                             "midiCCMap": ["cutoff": 74], "midiCCRanges": ["cutoff": [20.0, 20_000.0]]]],
            "patterns": [["id": 0, "name": "Filter", "length": 16, "steps": [rows],
                          "automation": [["id": "cutoff", "track": 0, "target": "cutoff", "interpolation": "linear",
                                          "points": [["row": 0, "value": 20.0], ["row": 4, "value": 10_010.0], ["row": 8, "value": 20_000.0]]]]]],
            "sequence": [0]
        ]
        let destination = directory.appendingPathComponent("mapped-automation.mid")
        let report = try MIDIFile.export(project, to: destination)
        let decoded = try decode(try Data(contentsOf: destination))
        let rowTicks = decoded.division / 4
        let cutoff = decoded.events.filter { $0.status == 0xb4 && $0.data.first == 74 }
        try require(cutoff.contains { $0.tick == 0 && $0.data == [74, 0] }, "Mapped cutoff range minimum must export as CC74 value 0 on channel 5")
        try require(cutoff.contains { $0.tick == 4 * rowTicks && $0.data == [74, 64] }, "Mapped cutoff range midpoint must export as CC74 value 64")
        try require(cutoff.contains { $0.tick == 8 * rowTicks && $0.data == [74, 127] }, "Mapped cutoff range maximum must export as CC74 value 127")
        try require(cutoff.contains { $0.tick == 2 * rowTicks && $0.data == [74, 32] }, "Linear automation must include intermediate controller values")
        try require(cutoff.contains { $0.tick == 12 * rowTicks && $0.data == [74, 0] }, "Mapped step FX must export even on a row without a note")
        try require((report["warnings"] as? [String] ?? []).isEmpty, "Explicitly mapped automation and step FX should export without an unmapped-control warning")
    }

    /// More than 16 tracks: one SMF track each (type 1), default channels repeat, and the
    /// file reimports with every track.
    private static func manyTrackExport(_ directory: URL) throws {
        let count = 40
        let tracks: [[String: Any]] = (0..<count).map { ["id": $0, "name": "T\($0)", "instrument": 0, "volume": 0.8, "pan": 0.0] }
        let steps: [[Any]] = (0..<count).map { track in
            var rows: [Any] = Array(repeating: NSNull(), count: 16); rows[0] = ["note": 36 + track, "instrument": 0, "velocity": 100, "gate": 1.0]; return rows
        }
        let project: [String: Any] = ["version": 1, "name": "Forty tracks", "bpm": 120.0, "linesPerBeat": 4, "tracks": tracks,
            "instruments": [["id": 0, "name": "Synth", "type": "synth"]], "patterns": [["id": 0, "name": "A", "length": 16, "steps": steps]], "sequence": [0]]
        let destination = directory.appendingPathComponent("forty-tracks.mid")
        _ = try MIDIFile.export(project, to: destination)
        let bytes = [UInt8](try Data(contentsOf: destination))
        try require(Int(bytes[8]) << 8 | Int(bytes[9]) == 1 && Int(bytes[10]) << 8 | Int(bytes[11]) >= count, "Export writes a type-1 file with one MTrk per tracker track")
        let decoded = try decode(Data(bytes))
        let on = decoded.events.filter { $0.status & 0xf0 == 0x90 && $0.data[1] > 0 }
        try require(on.count == count && Set(on.map { Int($0.data[0]) }) == Set((0..<count).map { 36 + $0 }), "Every track's note is exported")
        try require(on.contains { $0.data[0] == 36 + 16 && $0.status == 0x90 } && on.contains { $0.data[0] == 36 + 39 && $0.status == 0x97 }, "Default channels repeat beyond 16 tracks")
        let roundtrip = try MIDIFile.importProject(destination)
        try require((roundtrip["tracks"] as? [[String: Any]])?.count == count, "All forty tracks reimport")
        var big = project; big["tracks"] = (0..<(MIDIFile.trackSanityLimit + 1)).map { ["id": $0, "name": "T", "instrument": 0] }
        var rejected = false
        do { _ = try MIDIFile.export(big, to: directory.appendingPathComponent("too-many.mid")) } catch { rejected = true }
        try require(rejected, "Absurd track counts are rejected by the sanity limit")
    }

    private static func importCapacityWarnings(_ directory: URL) throws {
        let sustainedTrack: [UInt8] = [0x00, 0x90, 60, 100] + variable(17 * 120) + [0x80, 60, 64, 0x00, 0xff, 0x2f, 0x00]
        let sustained = try load(file(format: 0, tracks: [sustainedTrack]), named: "Sustained note", directory: directory)
        try require(near(try step(steps(sustained), note: 60).value["gate"], 17), "Notes longer than one bar must preserve their full gate")
        let sustainedWarnings = (sustained["midiImport"] as? [String: Any])?["warnings"] as? [String] ?? []
        try require(!sustainedWarnings.contains { $0.contains("shortened") }, "A supported sustained note must not report truncation")
        let longTrack: [UInt8] = [0x00, 0x90, 60, 100] + variable(257 * 120) + [0x80, 60, 64, 0x00, 0xff, 0x2f, 0x00]
        let longProject = try load(file(format: 0, tracks: [longTrack]), named: "Long gate", directory: directory)
        let longNote = try step(steps(longProject), note: 60)
        let longMetadata = longProject["midiImport"] as? [String: Any] ?? [:]
        let longWarnings = longMetadata["warnings"] as? [String] ?? []
        try require(near(longNote.value["gate"], 256), "Imported notes beyond the engine gate limit must be bounded to 256 rows")
        try require(longWarnings.contains { $0.contains("256-row") && $0.contains("shortened") }, "Gate truncation must be disclosed in the import warnings")
        let longExport = directory.appendingPathComponent("Long gate roundtrip.mid")
        try MIDIFile.export(longProject, to: longExport)
        let longRoundtrip = try MIDIFile.importProject(longExport)
        try require(near(try step(steps(longRoundtrip), note: 60).value["gate"], 256), "MIDI export and reimport must preserve a 256-row gate")

        let sourceTracks: [[UInt8]] = (0..<17).map { index in
            [0x00, 0x90, UInt8(48 + index), 100, 0x78, 0x80, UInt8(48 + index), 64, 0x00, 0xff, 0x2f, 0x00]
        }
        let denseProject = try load(file(format: 1, tracks: sourceTracks), named: "Seventeen source tracks", directory: directory)
        let denseMetadata = denseProject["midiImport"] as? [String: Any] ?? [:]
        let denseWarnings = denseMetadata["warnings"] as? [String] ?? []
        // There is no 16-track ceiling: every source track becomes a tracker track.
        try require((denseProject["tracks"] as? [[String: Any]])?.count == 17, "Import keeps all seventeen source tracks")
        try require(steps(denseProject).count == 17 && near(denseMetadata["sourceNotes"], 17) && near(denseMetadata["importedNotes"], 17), "Import metadata counts every note")
        try require(!denseWarnings.contains { $0.contains("omitted") }, "Nothing is omitted for track capacity")
    }

    private static func timelineBoundariesAndTrailingSilence(_ directory: URL) throws {
        let boundaryTrack = variable(63 * 120) + [0x90, 60, 100, 0x78, 0x80, 60, 64, 0x00, 0xff, 0x2f, 0x00]
        let boundary = try load(file(format: 0, tracks: [boundaryTrack]), named: "Exact pattern boundary", directory: directory)
        try require((boundary["patterns"] as? [[String: Any]])?.count == 1, "A note ending exactly at row 64 must not create a second empty pattern")
        let finalNote = try step(steps(boundary), note: 60)
        try require(finalNote.row == 63 && near(finalNote.value["gate"], 1), "The last row of a pattern must retain its complete note duration")

        let silenceTrack: [UInt8] = [0x00, 0x90, 60, 100, 0x78, 0x80, 60, 64] + variable(127 * 120) + [0xff, 0x2f, 0x00]
        let silence = try load(file(format: 0, tracks: [silenceTrack]), named: "Trailing silence", directory: directory)
        try require((silence["patterns"] as? [[String: Any]])?.count == 2, "End-of-track timing must retain trailing silence through row 128")
        try require(silence["sequence"] as? [Int] == [0, 1], "Trailing silent patterns must remain part of the song sequence")
    }

    private static func step(_ notes: [LocatedStep], note: Int) throws -> LocatedStep {
        guard let result = notes.first(where: { near($0.value["note"], Double(note)) }) else {
            throw TestFailure(description: "Expected MIDI note \(note) in imported project")
        }
        return result
    }

    private static func steps(_ project: [String: Any]) -> [LocatedStep] {
        var result: [LocatedStep] = []
        for (patternIndex, pattern) in (project["patterns"] as? [[String: Any]] ?? []).enumerated() {
            for (track, rows) in (pattern["steps"] as? [[Any]] ?? []).enumerated() {
                for (row, item) in rows.enumerated() {
                    guard let value = item as? [String: Any] else { continue }
                    if (value["note"] as? Int ?? -1) >= 0 {
                        result.append(LocatedStep(pattern: patternIndex, track: track, row: row, value: value))
                    }
                    // Extra note columns (step.columns[i] is note column i + 1).
                    for (index, cell) in (value["columns"] as? [Any] ?? []).enumerated() {
                        if let cell = cell as? [String: Any], (cell["note"] as? Int ?? -1) >= 0 {
                            result.append(LocatedStep(pattern: patternIndex, track: track, row: row, value: cell, column: index + 1))
                        }
                    }
                }
            }
        }
        return result
    }

    private static func load(_ data: Data, named name: String, directory: URL) throws -> [String: Any] {
        let url = directory.appendingPathComponent(name + ".mid")
        try data.write(to: url)
        return try MIDIFile.importProject(url)
    }

    private static func be16(_ value: Int) -> [UInt8] { [UInt8((value >> 8) & 0xff), UInt8(value & 0xff)] }
    private static func be32(_ value: Int) -> [UInt8] { [UInt8((value >> 24) & 0xff), UInt8((value >> 16) & 0xff), UInt8((value >> 8) & 0xff), UInt8(value & 0xff)] }

    private static func variable(_ value: Int) -> [UInt8] {
        var remaining = value
        var bytes = [UInt8(remaining & 0x7f)]
        remaining >>= 7
        while remaining > 0 {
            bytes.insert(UInt8(remaining & 0x7f) | 0x80, at: 0)
            remaining >>= 7
        }
        return bytes
    }

    private static func file(format: Int, division: Int = 480, tracks: [[UInt8]]) -> Data {
        var bytes = Array("MThd".utf8) + be32(6) + be16(format) + be16(tracks.count) + be16(division)
        for track in tracks { bytes += Array("MTrk".utf8) + be32(track.count) + track }
        return Data(bytes)
    }

    // A small independent event reader verifies exported ticks without relying on the importer.
    private static func decode(_ data: Data) throws -> (division: Int, events: [Event]) {
        let bytes = [UInt8](data)
        var cursor = 0
        func take(_ count: Int) throws -> [UInt8] {
            guard count >= 0 && cursor + count <= bytes.count else { throw TestFailure(description: "Exported MIDI is truncated") }
            defer { cursor += count }
            return Array(bytes[cursor..<(cursor + count)])
        }
        func integer(_ count: Int) throws -> Int { try take(count).reduce(0) { ($0 << 8) | Int($1) } }
        func variable() throws -> Int {
            var value = 0
            for _ in 0..<4 {
                let byte = try integer(1)
                value = (value << 7) | (byte & 0x7f)
                if byte & 0x80 == 0 { return value }
            }
            throw TestFailure(description: "Export contains an invalid variable-length integer")
        }
        try require(try take(4) == Array("MThd".utf8), "Export must start with MThd")
        let headerSize = try integer(4)
        try require(headerSize >= 6, "Export must contain the six-byte MIDI header")
        _ = try integer(2)
        let trackCount = try integer(2)
        let division = try integer(2)
        _ = try take(headerSize - 6)
        var events: [Event] = []
        for _ in 0..<trackCount {
            try require(try take(4) == Array("MTrk".utf8), "Exported track must start with MTrk")
            let size = try integer(4)
            let end = cursor + size
            try require(end <= bytes.count, "Exported track size must stay in bounds")
            var tick = 0
            var running: UInt8 = 0
            while cursor < end {
                tick += try variable()
                let first = try take(1)[0]
                var status = first
                var payload: [UInt8] = []
                if first < 0x80 {
                    try require(running >= 0x80 && running < 0xf0, "Export uses undefined running status")
                    status = running
                    payload = [first]
                }
                if status == 0xff {
                    let type = try integer(1)
                    let length = try variable()
                    let metadata = try take(length)
                    if type == 0x51 || type == 0x58 { events.append(Event(tick: tick, status: status, data: [UInt8(type)] + metadata)) }
                    running = 0
                } else if status == 0xf0 || status == 0xf7 {
                    _ = try take(try variable())
                    running = 0
                } else {
                    try require(status >= 0x80 && status < 0xf0, "Export contains an unsupported system event")
                    running = status
                    let size = [UInt8(0xc0), 0xd0].contains(status & 0xf0) ? 1 : 2
                    payload += try take(size - payload.count)
                    events.append(Event(tick: tick, status: status, data: payload))
                }
            }
            try require(cursor == end, "Exported event may not cross its track boundary")
        }
        return (division, events)
    }
}

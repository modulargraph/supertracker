import Foundation

/// Dependency-free Standard MIDI File reader/writer (SMF 0 and 1, PPQN timing).
/// Format reference: https://midi.org/standard-midi-files-specification
/// Import deliberately reports the tracker representation's limits instead of dropping music silently.
enum MIDIFile {
    private static let maximumGate = 256.0
    enum Failure: LocalizedError {
        case invalid(String)
        var errorDescription: String? { if case .invalid(let message) = self { return message }; return nil }
    }

    private struct Reader {
        let bytes: [UInt8]
        var position: Int = 0
        var end: Int
        init(_ bytes: [UInt8], start: Int = 0, end: Int? = nil) { self.bytes = bytes; position = start; self.end = end ?? bytes.count }
        mutating func byte() throws -> UInt8 {
            guard position < end else { throw Failure.invalid("The MIDI file ends inside an event.") }
            defer { position += 1 }; return bytes[position]
        }
        mutating func take(_ count: Int) throws -> [UInt8] {
            guard count >= 0, count <= end - position else { throw Failure.invalid("A MIDI chunk or event exceeds its file bounds.") }
            defer { position += count }; return Array(bytes[position..<position + count])
        }
        mutating func skip(_ count: Int) throws {
            guard count >= 0, count <= end - position else { throw Failure.invalid("A MIDI chunk or event exceeds its file bounds.") }
            position += count
        }
        mutating func big(_ count: Int) throws -> Int {
            var value = 0
            for _ in 0..<count { value = (value << 8) | Int(try byte()) }
            return value
        }
        mutating func vlq() throws -> Int64 {
            var value: Int64 = 0
            for _ in 0..<4 {
                let b = try byte(); value = (value << 7) | Int64(b & 0x7f)
                if b & 0x80 == 0 { return value }
            }
            throw Failure.invalid("A MIDI variable-length quantity exceeds four bytes.")
        }
        mutating func dataByte() throws -> Int {
            let result = try byte()
            guard result < 0x80 else { throw Failure.invalid("A MIDI channel event contains a status byte where a data byte is required.") }
            return Int(result)
        }
    }

    private struct Event {
        var tick: Int64
        var track: Int
        var order: Int
        var status: Int
        var a: Int = 0
        var b: Int = 0
        var data: [UInt8] = []
    }
    private struct FileContents {
        var format: Int
        var division: Int
        var events: [Event]
        var trackNames: [Int: String]
        var endTick: Int64
        var warnings: [String]
    }
    private struct NoteKey: Hashable { let track: Int; let channel: Int; let note: Int }
    private struct Note {
        var track: Int
        var channel: Int
        var note: Int
        var velocity: Int
        var program: Int
        var bankMSB: Int
        var bankLSB: Int
        var start: Int64
        var end: Int64
    }
    private struct Tempo { var tick: Int64; var micros: Int; var seconds: Double }
    private struct ImportLane {
        var sourceTrack: Int
        var channel: Int
        var name: String
        var instrument: Int
        var steps: [Int: [String: Any]] = [:]
        var noteColumns = 1
    }
    private struct PacketKey: Hashable {
        var track: Int; var channel: Int; var tick: Int64; var end: Int64
        var velocity: Int; var program: Int; var msb: Int; var lsb: Int
    }

    static func importProject(_ url: URL) throws -> [String: Any] {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        guard let size = attributes[.size] as? NSNumber, size.int64Value <= 64 * 1024 * 1024 else {
            throw Failure.invalid("MIDI files must be smaller than 64 MiB.")
        }
        let parsed = try parse([UInt8](Data(contentsOf: url)))
        var warnings = parsed.warnings
        var counts: [String: Int] = [:]
        func warn(_ message: String, count: Int = 1) { counts[message, default: 0] += count }
        let tempos = tempoMap(parsed.events, division: parsed.division)
        let initialMicros = tempos[0].micros
        let sourceBPM = 60_000_000 / Double(initialMicros)
        let bpm = clamp(sourceBPM, 20, 400)
        if bpm != sourceBPM { warnings.append("The source tempo is outside 20–400 BPM; event timing was converted to \(Int(bpm)) BPM.") }
        if tempos.count > 1 { warnings.append("Tempo changes were converted to one tracker tempo while preserving event timing. The original tempo map remains in MIDI import metadata.") }
        let rowsPerSecond = bpm / 60 * 4
        func seconds(_ tick: Int64) -> Double {
            var low = 0, high = tempos.count
            while low + 1 < high { let mid = (low + high) / 2; if tempos[mid].tick <= tick { low = mid } else { high = mid } }
            let segment = tempos[low]
            return segment.seconds + Double(tick - segment.tick) * Double(segment.micros) / 1_000_000 / Double(parsed.division)
        }
        func rowAt(_ tick: Int64) -> Double { seconds(tick) * rowsPerSecond }
        var active: [NoteKey: [Note]] = [:]
        var held: [Int: [Note]] = [:]
        var sustain = Array(repeating: false, count: 16)
        var programs = Array(repeating: 0, count: 16)
        var bankMSB = Array(repeating: 0, count: 16)
        var bankLSB = Array(repeating: 0, count: 16)
        var notes: [Note] = []
        var controllerEvents: [Event] = []
        var programEvents: [[String: Any]] = []
        var signatures: [[String: Any]] = []
        func finish(_ note: Note, at tick: Int64) { var note = note; note.end = max(note.start + 1, tick); notes.append(note) }

        for event in parsed.events {
            if event.status == 0xff {
                if event.a == 0x58, event.data.count >= 4 {
                    signatures.append(["tick": event.tick, "numerator": Int(event.data[0]), "denominator": 1 << Int(event.data[1])])
                }
                continue
            }
            let channel = event.status & 15
            let key = NoteKey(track: event.track, channel: channel, note: event.a)
            switch event.status & 0xf0 {
            case 0x90 where event.b > 0:
                guard (active[key]?.count ?? 0) < 1024 else { throw Failure.invalid("The MIDI file contains more than 1,024 overlapping copies of one note.") }
                active[key, default: []].append(Note(track: event.track, channel: channel, note: event.a,
                    velocity: event.b, program: programs[channel], bankMSB: bankMSB[channel], bankLSB: bankLSB[channel], start: event.tick, end: event.tick))
            case 0x80, 0x90:
                if var pending = active[key], !pending.isEmpty {
                    let note = pending.removeFirst()
                    active[key] = pending.isEmpty ? nil : pending
                    if sustain[channel] { held[channel, default: []].append(note) } else { finish(note, at: event.tick) }
                } else { warn("Unmatched note-off events were ignored") }
            case 0xc0:
                programs[channel] = event.a
                programEvents.append(["tick": event.tick, "channel": channel + 1, "program": event.a])
            case 0xb0:
                controllerEvents.append(event)
                switch event.a {
                case 0: bankMSB[channel] = event.b
                case 32: bankLSB[channel] = event.b
                case 64:
                    sustain[channel] = event.b >= 64
                    if !sustain[channel] { for note in held.removeValue(forKey: channel) ?? [] { finish(note, at: event.tick) } }
                case 120, 123:
                    let keys = active.keys.filter { $0.channel == channel }
                    for key in keys { for note in active.removeValue(forKey: key) ?? [] { finish(note, at: event.tick) } }
                    for note in held.removeValue(forKey: channel) ?? [] { finish(note, at: event.tick) }
                default: break
                }
            case 0xe0: warn("Pitch-bend events remain unsupported by tracker MIDI import")
            case 0xa0, 0xd0: warn("Aftertouch events remain unsupported by tracker MIDI import")
            default: break
            }
        }
        for pending in active.values {
            for note in pending { finish(note, at: max(parsed.endTick, note.start + Int64(parsed.division))); warn("Notes without a note-off were ended at the file boundary") }
        }
        for pending in held.values { for note in pending { finish(note, at: max(parsed.endTick, note.start + 1)) } }
        notes.sort { $0.start == $1.start ? ($0.track == $1.track ? $0.note < $1.note : $0.track < $1.track) : $0.start < $1.start }
        guard !notes.isEmpty else { throw Failure.invalid("This MIDI file contains no note events to import.") }

        var instrumentIDs: [String: Int] = [:]
        var instruments: [[String: Any]] = []
        func instrument(for note: Note) -> Int {
            let key = "\(note.channel):\(note.program):\(note.bankMSB):\(note.bankLSB)"
            if let id = instrumentIDs[key] { return id }
            guard instruments.count < instrumentSanityLimit else { warn("Program changes beyond \(instrumentSanityLimit) instruments use the first instrument"); return 0 }
            let id = instruments.count
            instrumentIDs[key] = id
            // Audible placeholders: built-in wavetable samplers (noise for the GM drum channel).
            var placeholder = ProjectMigration.wavetableInstrument(name: note.channel == 9 ? "GM Drums" : "GM Program \(note.program + 1)",
                waveform: note.channel == 9 ? "noise" : (32...39).contains(note.program) ? "saw" : "triangle",
                extra: ["volume": 0.5, "pan": 0.0, "rootNote": 60, "tune": 0,
                        "attack": 0.003, "decay": 0.15, "sustain": 0.65, "release": 0.12,
                        "cutoff": 18000.0, "resonance": 0.0, "midiChannel": note.channel + 1,
                        "midiProgram": note.program, "midiBankMSB": note.bankMSB, "midiBankLSB": note.bankLSB])
            placeholder["id"] = id
            instruments.append(placeholder)
            return id
        }
        var groups: [PacketKey: [Note]] = [:]
        for note in notes {
            let key = PacketKey(track: note.track, channel: note.channel, tick: note.start, end: note.end,
                                velocity: note.velocity, program: note.program, msb: note.bankMSB, lsb: note.bankLSB)
            groups[key, default: []].append(note)
        }
        let packets = groups.values.sorted {
            let a = $0[0], b = $1[0]
            if a.start != b.start { return a.start < b.start }
            if a.track != b.track { return a.track < b.track }
            if a.channel != b.channel { return a.channel < b.channel }
            return a.note < b.note
        }
        var lanes: [ImportLane] = []
        let sourceEndRow = rowAt(parsed.endTick)
        if sourceEndRow > 65536 { warn("The source timeline extends past 65,536 rows and was truncated") }
        var highestRow = max(0, Int(ceil(min(65536, sourceEndRow))) - 1)
        var importedNotes = 0
        for packet in packets {
            let packet = packet.sorted { $0.note < $1.note }
            var cursor = 0
            while cursor < packet.count {
                let first = packet[cursor]
                var finish = min(packet.count, cursor + 8)
                while finish > cursor + 1 && packet[finish - 1].note - first.note > 96 { finish -= 1 }
                let chord = packet[cursor..<finish]
                cursor = finish
                let position = rowAt(first.start)
                guard position < 65536 else { warn("Notes beyond the 65,536-row import limit were omitted", count: chord.count); continue }
                let row = Int(position.rounded(.down))
                let rawMicro = max(0, position - Double(row))
                let micro = min(0.95, rawMicro)
                if rawMicro > 0.95001 { warn("Onsets in the final 5% of a row were moved slightly earlier to fit microtiming") }
                let duration = max(0.01, rowAt(first.end) - position)
                let rawGate = duration / (1 - micro)
                let gate = min(maximumGate, rawGate)
                if rawGate > maximumGate + 0.0001 { warn("Note durations longer than the engine's 256-row gate were shortened", count: chord.count) }
                let instrumentID = instrument(for: first)
                var laneID = lanes.firstIndex { $0.sourceTrack == first.track && $0.channel == first.channel && $0.steps[row] == nil }
                // Overlapping notes on a taken row go into free note columns of the same track (eight per track),
                // when they share the row's microtiming (row FX belong to column 0); otherwise into another lane.
                let rowMicro = micro > 0.000001 ? micro : 0
                func fitsColumns(_ lane: ImportLane) -> Bool {
                    guard lane.sourceTrack == first.track, lane.channel == first.channel, let taken = lane.steps[row] else { return false }
                    let used = (taken["columns"] as? [Any])?.count ?? 0
                    return used + chord.count <= 7 && abs(PayloadValue.number(taken["fx1"] as? [String: Any] ?? [:], "value", 0) - rowMicro) < 0.000001
                }
                if laneID == nil, let fit = lanes.firstIndex(where: fitsColumns) {
                    var taken = lanes[fit].steps[row] ?? [:]
                    var columns = taken["columns"] as? [Any] ?? []
                    for note in chord { columns.append(["note": note.note, "instrument": instrumentID, "velocity": note.velocity, "gate": gate] as [String: Any]) }
                    taken["columns"] = columns
                    lanes[fit].steps[row] = taken
                    lanes[fit].noteColumns = max(lanes[fit].noteColumns, columns.count + 1)
                    highestRow = max(highestRow, row, min(65535, Int(ceil(position + gate * (1 - micro))) - 1))
                    importedNotes += chord.count
                    continue
                }
                if laneID == nil && lanes.count < trackSanityLimit {
                    laneID = lanes.count
                    let label = parsed.trackNames[first.track] ?? "MIDI Channel \(first.channel + 1)"
                    let suffix = lanes.contains { $0.sourceTrack == first.track && $0.channel == first.channel } ? " · Voice \(lanes.count + 1)" : ""
                    lanes.append(ImportLane(sourceTrack: first.track, channel: first.channel, name: label + suffix, instrument: instrumentID))
                }
                guard let laneID else { warn("Notes requiring more than \(trackSanityLimit) tracker tracks were omitted", count: chord.count); continue }
                var step: [String: Any] = ["note": first.note, "instrument": instrumentID, "velocity": first.velocity, "gate": gate]
                if chord.count > 1 { step["chord"] = chord.map { $0.note - first.note } }
                if micro > 0.000001 { step["fx1"] = ["type": "microtiming", "value": micro] }
                lanes[laneID].steps[row] = step
                highestRow = max(highestRow, row, min(65535, Int(ceil(position + gate * (1 - micro))) - 1))
                importedNotes += chord.count
            }
        }
        guard !lanes.isEmpty else { throw Failure.invalid("All MIDI notes fall outside the tracker import limits.") }
        let patternLength = highestRow < 16384 ? 64 : 256
        let patternCount = min(256, max(1, (highestRow + patternLength) / patternLength))
        let tracks: [[String: Any]] = lanes.enumerated().map { index, lane in
            ["id": index, "name": lane.name, "color": ["#76d7cb", "#e8b86f", "#ab99ed", "#ee8997"][index % 4],
             "volume": 0.8, "pan": 0.0, "mute": false, "solo": false, "instrument": lane.instrument,
             "delaySend": 0.0, "reverbSend": 0.0, "plugins": [], "midiChannel": lane.channel + 1]
                .merging(lane.noteColumns > 1 ? ["noteColumns": lane.noteColumns] : [:]) { $1 }
        }
        var automation: [Int: [[String: Any]]] = [:]
        // CC7/CC10/CC91 have stable volume, pan and reverb meanings. Other CCs remain documented.
        let importedCCMap = [7: "volume", 10: "pan", 91: "reverb"]
        var lanePoints: [String: [[String: Any]]] = [:]
        for event in controllerEvents {
            guard let target = importedCCMap[event.a] else {
                if ![0, 32, 64, 120, 123].contains(event.a) { warn("Unmapped MIDI controller events were retained in metadata only") }
                continue
            }
            let position = rowAt(event.tick)
            guard position < Double(patternCount * patternLength) else { continue }
            let pattern = Int(position) / patternLength
            let row = position - Double(pattern * patternLength)
            let value = target == "pan" ? Double(event.b - 64) / (event.b < 64 ? 64 : 63) : Double(event.b) / 127
            for (track, lane) in lanes.enumerated() where lane.channel == event.status & 15 {
                lanePoints["\(pattern):\(track):\(target)", default: []].append(["row": row, "value": value])
            }
        }
        for (key, points) in lanePoints {
            let parts = key.split(separator: ":"); let pattern = Int(parts[0])!; let track = Int(parts[1])!; let target = String(parts[2])
            automation[pattern, default: []].append(["id": "midi-\(key)", "track": track, "target": target,
                "name": "MIDI \(target.capitalized)", "interpolation": "step", "points": points])
        }
        // Native tracker mix state resets at pattern boundaries; MIDI controllers remain in effect.
        // Carry the last controller value forward so an imported fader move survives a new pattern.
        var carried: [String: Double] = [:]
        for pattern in 0..<patternCount {
            var current = automation[pattern] ?? []
            for index in current.indices {
                let key = "\(PayloadValue.integer(current[index], "track", 0)):\(current[index]["target"] as? String ?? "volume")"
                var points = current[index]["points"] as? [[String: Any]] ?? []
                if let previous = carried[key], PayloadValue.number(points.first ?? [:], "row", 1) > 0 { points.insert(["row": 0.0, "value": previous], at: 0) }
                if let final = points.last { carried[key] = PayloadValue.number(final, "value", 0) }
                if points.count > 4096 { warn("Dense controller lanes were reduced to the engine's 4,096-point limit"); points = Array(points.prefix(4095)) + [points.last!] }
                current[index]["points"] = points
            }
            for (key, value) in carried {
                let parts = key.split(separator: ":"), track = Int(key.split(separator: ":")[0]) ?? 0
                let target = String(parts[1])
                if !current.contains(where: { PayloadValue.integer($0, "track", 0) == track && $0["target"] as? String == target }) {
                    current.append(["id": "midi-carry-\(pattern)-\(key)", "track": track, "target": target,
                                    "name": "MIDI \(target.capitalized)", "interpolation": "step", "points": [["row": 0.0, "value": value]]])
                }
            }
            automation[pattern] = current
        }
        var patterns: [[String: Any]] = []
        for pattern in 0..<patternCount {
            let steps: [[Any]] = lanes.map { lane in
                (0..<patternLength).map { row -> Any in lane.steps[pattern * patternLength + row] ?? NSNull() }
            }
            patterns.append(["id": pattern, "name": String(format: "MIDI %02d", pattern + 1), "length": patternLength,
                             "steps": steps, "automation": automation[pattern] ?? []])
        }
        if controllerEvents.count > 100_000 { warnings.append("Raw controller metadata is limited to the first 100,000 events.") }
        if programEvents.count > 100_000 { warnings.append("Raw program-change metadata is limited to the first 100,000 events.") }
        if signatures.count > 1 { warnings.append("Meter changes are retained in MIDI metadata; the tracker displays the initial time signature.") }
        for message in counts.keys.sorted() { warnings.append("\(message) (\(counts[message]!)).") }
        warnings.append("Imported instruments use native synth placeholders. Original MIDI channels, banks and programs are retained; choose samples or Audio Units for the intended sounds.")
        let metadata: [String: Any] = [
            "source": url.path, "format": parsed.format, "ticksPerQuarter": parsed.division, "warnings": warnings,
            "importedNotes": importedNotes, "sourceNotes": notes.count,
            "tempoChanges": tempos.map { ["tick": $0.tick, "microsecondsPerQuarter": $0.micros] },
            "timeSignatures": signatures, "programChanges": Array(programEvents.prefix(100_000)),
            "controllers": controllerEvents.prefix(100_000).map { ["tick": $0.tick, "channel": ($0.status & 15) + 1, "controller": $0.a, "value": $0.b] }
        ]
        let firstSignature = signatures.last(where: { ($0["tick"] as? Int64 ?? -1) == 0 }) ?? ["numerator": 4, "denominator": 4]
        return ["version": 1, "name": url.deletingPathExtension().lastPathComponent, "bpm": bpm, "linesPerBeat": 4,
                "swing": 0.0, "masterVolume": 0.7, "activePattern": 0, "tracks": tracks, "instruments": instruments,
                "patterns": patterns, "sequence": Array(0..<patternCount), "loopSong": false,
                "delay": ["time": 0.375, "feedback": 0.35, "mix": 0.3], "reverb": ["size": 0.7, "damping": 0.4, "mix": 0.25],
                "timeSignature": ["numerator": firstSignature["numerator"] ?? 4, "denominator": firstSignature["denominator"] ?? 4],
                "midiImport": metadata]
    }

    private static func parse(_ bytes: [UInt8]) throws -> FileContents {
        var reader = Reader(bytes)
        guard try reader.take(4) == Array("MThd".utf8) else { throw Failure.invalid("This is not a Standard MIDI File (MThd header missing).") }
        let headerLength = try reader.big(4)
        guard headerLength >= 6 else { throw Failure.invalid("The MIDI header is too short.") }
        let format = try reader.big(2), tracks = try reader.big(2), division = try reader.big(2)
        guard format == 0 || format == 1 else { throw Failure.invalid("Only synchronous SMF type 0 and type 1 MIDI files are supported; type 2 contains independent songs.") }
        guard (1...1024).contains(tracks), format != 0 || tracks == 1 else { throw Failure.invalid("The MIDI file declares an invalid track count.") }
        guard division & 0x8000 == 0 else { throw Failure.invalid("SMPTE-timed MIDI files are not supported. Export the source file with musical (PPQN) timing.") }
        guard division > 0 else { throw Failure.invalid("The MIDI timing division cannot be zero.") }
        try reader.skip(headerLength - 6)
        var events: [Event] = []
        var names: [Int: String] = [:]
        var trackIndex = 0
        var endTick: Int64 = 0
        var warnings: [String] = []
        var sysexCount = 0
        while trackIndex < tracks {
            let id = try reader.take(4), size = try reader.big(4)
            guard size <= reader.end - reader.position else { throw Failure.invalid("A MIDI track extends beyond the end of the file.") }
            guard id == Array("MTrk".utf8) else { try reader.skip(size); continue }
            var track = Reader(bytes, start: reader.position, end: reader.position + size)
            try reader.skip(size)
            var tick: Int64 = 0, running = 0
            var ended = false
            while track.position < track.end {
                tick += try track.vlq()
                guard tick <= 1_000_000_000_000 else { throw Failure.invalid("The MIDI timeline is too long to safely import.") }
                var status = Int(try track.byte())
                if status < 0x80 {
                    guard running >= 0x80 && running < 0xf0 else { throw Failure.invalid("MIDI running status appears without a preceding channel status.") }
                    track.position -= 1; status = running
                }
                if status < 0xf0 {
                    running = status
                    let a = try track.dataByte(), length = [0xc0, 0xd0].contains(status & 0xf0) ? 1 : 2
                    let b = length == 2 ? try track.dataByte() : 0
                    events.append(Event(tick: tick, track: trackIndex, order: events.count, status: status, a: a, b: b))
                } else if status == 0xff {
                    running = 0
                    let type = Int(try track.byte()), length = Int(try track.vlq())
                    if type == 0x2f {
                        guard length == 0 else { throw Failure.invalid("The MIDI end-of-track event has an invalid length.") }
                        ended = true; break
                    }
                    if type == 0x51 {
                        guard length == 3 else { throw Failure.invalid("The MIDI tempo event must contain three bytes.") }
                        let data = try track.take(length)
                        guard data.contains(where: { $0 != 0 }) else { throw Failure.invalid("The MIDI tempo cannot be zero.") }
                        events.append(Event(tick: tick, track: trackIndex, order: events.count, status: status, a: type, data: data))
                    } else if type == 0x58 {
                        guard length == 4 else { throw Failure.invalid("The MIDI time-signature event must contain four bytes.") }
                        let data = try track.take(length)
                        guard data[0] > 0, data[1] <= 7 else { throw Failure.invalid("The MIDI time signature is invalid.") }
                        events.append(Event(tick: tick, track: trackIndex, order: events.count, status: status, a: type, data: data))
                    } else if type == 0x03 {
                        let data = try track.take(min(length, 1024)); try track.skip(max(0, length - 1024))
                        names[trackIndex] = String(bytes: data, encoding: .utf8) ?? String(bytes: data, encoding: .isoLatin1) ?? "MIDI Track \(trackIndex + 1)"
                    } else { try track.skip(length) }
                } else if status == 0xf0 || status == 0xf7 {
                    running = 0; try track.skip(Int(try track.vlq())); sysexCount += 1
                } else { throw Failure.invalid("An unsupported system status (\(String(status, radix: 16))) appears in the MIDI file.") }
                guard events.count <= 1_000_000 else { throw Failure.invalid("The MIDI file exceeds the one-million-event safety limit.") }
            }
            if !ended { warnings.append("Track \(trackIndex + 1) has no end-of-track event; its chunk boundary was used.") }
            endTick = max(endTick, tick); trackIndex += 1
        }
        if sysexCount > 0 { warnings.append("\(sysexCount) SysEx events were skipped. Device patch dumps are not part of tracker instrument state.") }
        events.sort { $0.tick == $1.tick ? $0.order < $1.order : $0.tick < $1.tick }
        return FileContents(format: format, division: division, events: events, trackNames: names, endTick: endTick, warnings: warnings)
    }

    private static func tempoMap(_ events: [Event], division: Int) -> [Tempo] {
        var result = [Tempo(tick: 0, micros: 500_000, seconds: 0)]
        for event in events where event.status == 0xff && event.a == 0x51 {
            let value = event.data.reduce(0) { ($0 << 8) | Int($1) }
            let last = result[result.count - 1]
            let seconds = last.seconds + Double(event.tick - last.tick) * Double(last.micros) / 1_000_000 / Double(division)
            let segment = Tempo(tick: event.tick, micros: value, seconds: seconds)
            if event.tick == last.tick { result[result.count - 1] = segment }
            else if value != last.micros { result.append(segment) }
        }
        return result
    }

    private struct WriteEvent { var tick: Int64; var priority: Int; var order: Int; var bytes: [UInt8] }
    private struct OutputNote { var start: Double; var end: Double; var channel: Int; var pitch: Int; var velocity: Int; var column = 0 }

    /// Sanity bound for files (mirrors ST_TRACK_SANITY_LIMIT); not a design limit.
    static let trackSanityLimit = 1024
    /// Mirrors ProjectLayout.instrumentSanityLimit (this file also builds on its own).
    static let instrumentSanityLimit = 1024

    /// Exports the finite sequence once. Song looping and audio/plugin state are not MIDI data.
    @discardableResult static func export(_ project: [String: Any], to url: URL) throws -> [String: Any] {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        let instruments = project["instruments"] as? [[String: Any]] ?? []
        let patterns = project["patterns"] as? [[String: Any]] ?? []
        let sequence = project["sequence"] as? [Int] ?? [0]
        guard (1...trackSanityLimit).contains(tracks.count), instruments.count <= instrumentSanityLimit, (1...256).contains(patterns.count), sequence.count <= 4096 else {
            throw Failure.invalid("MIDI export requires a valid tracker project with 1–\(trackSanityLimit) tracks and up to 256 patterns.")
        }
        let bpm = PayloadValue.number(project, "bpm", 120), lpb = PayloadValue.number(project, "linesPerBeat", 4), swing = clamp(PayloadValue.number(project, "swing", 0), 0, 0.95)
        guard bpm.isFinite && (20...400).contains(bpm), lpb.isFinite && (1...32).contains(lpb) else { throw Failure.invalid("The project tempo or rows-per-beat is invalid.") }
        let ppqn = 960
        let ticksPerRow = Double(ppqn) / lpb
        var warnings = Set<String>()
        var byID: [Int: [String: Any]] = [:]
        for (index, pattern) in patterns.enumerated() { byID[PayloadValue.integer(pattern, "id", index)] = pattern }
        let signature = project["timeSignature"] as? [String: Any] ?? [:]
        let numerator = max(1, min(127, PayloadValue.integer(signature, "numerator", 4))), denominator = PayloadValue.integer(signature, "denominator", 4)
        let power = [1, 2, 4, 8, 16, 32, 64, 128].firstIndex(of: denominator) ?? 2
        func tempoEvent(_ tick: Int64, _ bpm: Double) -> WriteEvent {
            let micros = Int((60_000_000 / bpm).rounded())
            return WriteEvent(tick: tick, priority: 0, order: 0, bytes: [0xff, 0x51, 3, UInt8((micros >> 16) & 255), UInt8((micros >> 8) & 255), UInt8(micros & 255)])
        }
        func meterEvent(_ tick: Int64, _ numerator: Int, _ power: Int) -> WriteEvent {
            WriteEvent(tick: tick, priority: 0, order: 1, bytes: [0xff, 0x58, 4, UInt8(numerator), UInt8(power), 24, 8])
        }
        var conductor = [tempoEvent(0, bpm), meterEvent(0, numerator, power)]
        // A project tempo map (beats are quarter notes) is authoritative over the
        // legacy bpm/timeSignature mirror; the engine plays every change.
        if let timing = project["timing"] as? [String: Any] {
            let tempos = (timing["tempos"] as? [[String: Any]] ?? []).compactMap { row -> (Double, Double)? in
                let beat = PayloadValue.number(row, "beat", -1), value = PayloadValue.number(row, "bpm", 0)
                return beat >= 0 && (20...400).contains(value) ? (beat, value) : nil
            }.sorted { $0.0 < $1.0 }
            let meters = (timing["meters"] as? [[String: Any]] ?? []).compactMap { row -> (Double, Int, Int)? in
                let beat = PayloadValue.number(row, "beat", -1), top = PayloadValue.integer(row, "numerator", 0)
                guard beat >= 0, (1...127).contains(top), let exponent = [1, 2, 4, 8, 16, 32, 64, 128].firstIndex(of: PayloadValue.integer(row, "denominator", 0)) else { return nil }
                return (beat, top, exponent)
            }.sorted { $0.0 < $1.0 }
            if tempos.first?.0 == 0 { conductor.removeAll { $0.bytes[1] == 0x51 } }
            if meters.first?.0 == 0 { conductor.removeAll { $0.bytes[1] == 0x58 } }
            conductor += tempos.map { tempoEvent(Int64((Double(ppqn) * $0.0).rounded()), $0.1) }
            conductor += meters.map { meterEvent(Int64((Double(ppqn) * $0.0).rounded()), $0.1, $0.2) }
        }
        let anySolo = tracks.contains { $0["solo"] as? Bool == true }
        var outputTracks: [[UInt8]] = []
        var noteCount = 0
        var totalTicks: Int64 = 0
        for (trackIndex, track) in tracks.enumerated() {
            if track["mute"] as? Bool == true || anySolo && track["solo"] as? Bool != true { continue }
            var events: [WriteEvent] = []
            var notes: [OutputNote] = []
            var cursor = 0.0
            var rowCount = 0
            var lastProgram: [Int: String] = [:]
            var heldRatchet = 1
            var lastStep: [String: Any]? = nil
            // Extra note columns (step.columns) are independent voices of the track, as in the engine.
            let extraColumns = min(max(PayloadValue.integer(track, "noteColumns", 1), 1), 8) - 1
            func append(_ tick: Double, _ bytes: [UInt8], priority: Int = 1) {
                events.append(WriteEvent(tick: Int64(max(0, tick).rounded()), priority: priority, order: events.count, bytes: bytes))
            }
            func route(_ instrumentIndex: Int) -> (item: [String: Any], channel: Int) {
                let item = instruments.indices.contains(instrumentIndex) ? instruments[instrumentIndex] : [:]
                // Each tracker track stays its own SMF track; beyond 16 the default channels repeat.
                return (item, max(0, min(15, PayloadValue.integer(item, "midiChannel", PayloadValue.integer(track, "midiChannel", trackIndex % 16 + 1)) - 1)))
            }
            func controller(_ target: String, instrument: [String: Any]) -> Int? {
                if let explicit = (instrument["midiCCMap"] as? [String: Any])?[target] as? NSNumber, (0...127).contains(explicit.intValue) { return explicit.intValue }
                return ["volume": 7, "pan": 10, "reverb": 91][target]
            }
            func ccValue(_ target: String, _ value: Double, instrument: [String: Any]) -> UInt8 {
                let ranges = instrument["midiCCRanges"] as? [String: [Double]] ?? [:]
                let range = ranges[target]
                let normalized: Double
                if let range, range.count == 2, range[1] > range[0] { normalized = (value - range[0]) / (range[1] - range[0]) }
                else if target == "pan" { return UInt8(clamp((64 + value * (value < 0 ? 64 : 63)).rounded(), 0, 127)) }
                else { normalized = value }
                return UInt8(clamp((normalized * 127).rounded(), 0, 127))
            }
            func selectProgram(_ current: (item: [String: Any], channel: Int), at start: Double) {
                guard current.item["midiProgram"] != nil else { return }
                let channel = current.channel
                let program = max(0, min(127, PayloadValue.integer(current.item, "midiProgram", 0)))
                let msb = max(0, min(127, PayloadValue.integer(current.item, "midiBankMSB", 0))), lsb = max(0, min(127, PayloadValue.integer(current.item, "midiBankLSB", 0)))
                let state = "\(msb):\(lsb):\(program)"
                if lastProgram[channel] != state {
                    append(start, [UInt8(0xb0 | channel), 0, UInt8(msb)])
                    append(start, [UInt8(0xb0 | channel), 32, UInt8(lsb)])
                    append(start, [UInt8(0xc0 | channel), UInt8(program)])
                    lastProgram[channel] = state
                }
            }
            let defaultInstrument = PayloadValue.integer(track, "instrument", 0)
            let defaultRoute = route(defaultInstrument)
            for (target, value) in [("volume", PayloadValue.number(track, "volume", 0.8)), ("pan", PayloadValue.number(track, "pan", 0)), ("reverb", PayloadValue.number(track, "reverbSend", 0))] {
                if let cc = controller(target, instrument: defaultRoute.item) { append(0, [UInt8(0xb0 | defaultRoute.channel), UInt8(cc), ccValue(target, value, instrument: defaultRoute.item)]) }
            }
            for patternID in sequence {
                guard let pattern = byID[patternID] else { throw Failure.invalid("The song sequence refers to missing pattern \(patternID).") }
                let length = PayloadValue.integer(pattern, "length", 64)
                guard (1...256).contains(length) else { throw Failure.invalid("A MIDI export pattern has an invalid row count.") }
                rowCount += length
                guard rowCount <= 1_048_576 else { throw Failure.invalid("The MIDI export timeline exceeds its safety limit.") }
                let rowSteps = pattern["steps"] as? [[Any]] ?? []
                let steps = rowSteps.indices.contains(trackIndex) ? rowSteps[trackIndex] : []
                let patternStart = cursor
                var rowTimes = [cursor]
                for row in 0..<length { rowTimes.append(rowTimes.last! + ticksPerRow * (1 + (row % 2 == 0 ? swing : -swing))) }
                for row in 0..<length {
                    let source = steps.indices.contains(row) ? steps[row] as? [String: Any] : nil
                    var step = source ?? [:]
                    let effects = [step["fx1"], step["fx2"]].compactMap { $0 as? [String: Any] }
                    if extraColumns > 0, let columns = source?["columns"] as? [Any] {
                        // The row's note FX (column 0's FX) apply to every column; a sticky ratchet repeats column 0 only.
                        var micro = 0.0, ratchet = 1, probability = 1.0
                        // As in the engine, an OFF/CUT in column 0 carries no note FX for the other columns.
                        for fx in effects where ![-1, -2].contains(PayloadValue.integer(step, "note", -3)) {
                            let value = PayloadValue.number(fx, "value", 0)
                            switch fx["type"] as? String {
                            case "microtiming": micro = clamp(value, 0, 0.95)
                            case "ratchet": ratchet = Int(clamp(value, 1, 8))
                            case "probability": probability = clamp(value, 0, 1)
                            default: break
                            }
                        }
                        let start = rowTimes[row], width = rowTimes[row + 1] - start
                        for (index, raw) in columns.prefix(extraColumns).enumerated() {
                            guard let cell = raw as? [String: Any] else { continue }
                            let column = index + 1, pitch = PayloadValue.integer(cell, "note", -3)
                            if pitch == -1 || pitch == -2 {
                                for n in notes.indices where notes[n].column == column && notes[n].start < start && notes[n].end > start { notes[n].end = start }
                                continue
                            }
                            guard (0...127).contains(pitch), probability > 0 else { continue }
                            let selected = PayloadValue.integer(cell, "instrument", defaultInstrument)
                            let current = route(selected < 0 ? defaultInstrument : selected)
                            selectProgram(current, at: start)
                            let rawGate = PayloadValue.number(cell, "gate", 0)
                            let open = !(rawGate > 0) && ratchet <= 1, gate = rawGate > 0 ? clamp(rawGate, 0.01, maximumGate) : 1
                            let spacing = width * (1 - micro) / Double(ratchet), onset = start + width * micro
                            let velocity = max(0, min(127, PayloadValue.integer(cell, "velocity", 100)))
                            if velocity > 0 { for n in notes.indices where notes[n].column == column && notes[n].end == .infinity && notes[n].start < onset { notes[n].end = onset } }
                            for repetition in 0..<ratchet {
                                let begin = onset + Double(repetition) * spacing
                                if velocity > 0 { notes.append(OutputNote(start: begin, end: open ? .infinity : begin + max(1, spacing * gate), channel: current.channel, pitch: pitch, velocity: velocity, column: column)) }
                                else { for n in notes.indices where notes[n].column == column && notes[n].pitch == pitch && notes[n].channel == current.channel && notes[n].start < begin && notes[n].end > begin { notes[n].end = begin } }
                            }
                        }
                    }
                    let ratchetFX = effects.last { $0["type"] as? String == "ratchet" }
                    // As in the engine: a row's own Ratchet sets the rate; a new note or another effect row stops it.
                    if let ratchetFX { heldRatchet = Int(clamp(PayloadValue.number(ratchetFX, "value", 1), 1, 8)) }
                    else if effects.contains(where: { !($0["type"] as? String ?? "").isEmpty }) || PayloadValue.integer(step, "note", -3) >= 0 { heldRatchet = 1 }
                    let repeatOnly = source == nil || PayloadValue.integer(step, "note", -3) == -3 && ratchetFX != nil
                    if repeatOnly, heldRatchet > 1, let previous = lastStep {
                        step = previous
                        step["fx1"] = ["type": "ratchet", "value": heldRatchet]
                        step.removeValue(forKey: "fx2")
                    } else if source == nil { continue }
                    let start = rowTimes[row], width = rowTimes[row + 1] - start
                    let baseNote = PayloadValue.integer(step, "note", -3)
                    if baseNote == -1 || baseNote == -2 {
                        heldRatchet = 1; lastStep = nil
                        for index in notes.indices where notes[index].column == 0 && notes[index].start < start && notes[index].end > start { notes[index].end = start }
                        continue
                    }
                    let selected = PayloadValue.integer(step, "instrument", defaultInstrument)
                    let current = route(selected < 0 ? defaultInstrument : selected)
                    let channel = current.channel
                    var micro = 0.0, ratchet = heldRatchet, probability = 1.0
                    for fx in effects {
                        let target = fx["type"] as? String ?? "", value = PayloadValue.number(fx, "value", 0)
                        if target == "microtiming" { if !repeatOnly { micro = clamp(value, 0, 0.95) } }
                        else if target == "ratchet" { ratchet = Int(clamp(value, 1, 8)) }
                        else if target == "probability" { if !repeatOnly { probability = clamp(value, 0, 1) } }
                        else if let cc = controller(target, instrument: current.item) { append(start, [UInt8(0xb0 | channel), UInt8(cc), ccValue(target, value, instrument: current.item)]) }
                        else if !target.isEmpty { warnings.insert("Unmapped step FX (including sample processing and plugin controls) are not represented in MIDI.") }
                    }
                    guard baseNote >= 0, baseNote <= 127, probability > 0 else { continue }
                    if PayloadValue.integer(step, "velocity", 100) <= 0 { heldRatchet = 1; lastStep = nil }
                    else {
                        lastStep = step
                        lastStep?["instrument"] = selected < 0 ? defaultInstrument : selected
                    }
                    if probability < 1 { warnings.insert("Probabilistic steps were exported deterministically as notes; their random probability is not a MIDI event.") }
                    selectProgram(current, at: start)
                    let offsets = step["chord"] as? [Int] ?? [0]
                    if offsets.count > 8 { warnings.insert("Chord notes beyond the engine's eight-note limit were omitted.") }
                    // Gate 0 is no gate: the note lasts until the next note on this track, an OFF/CUT or the song end.
                    // Ratchet repeats of such a note each last one repeat spacing. A missing gate is the legacy 0.8.
                    let rawGate = PayloadValue.number(step, "gate", 0.8)
                    let open = !(rawGate > 0) && ratchet <= 1
                    let gate = rawGate > 0 ? clamp(rawGate, 0.01, maximumGate) : 1
                    let spacing = width * (1 - micro) / Double(ratchet)
                    let velocity = max(0, min(127, PayloadValue.integer(step, "velocity", 100)))
                    if velocity > 0 {
                        let onset = start + width * micro
                        for index in notes.indices where notes[index].column == 0 && notes[index].end == .infinity && notes[index].start < onset { notes[index].end = onset }
                    }
                    for repetition in 0..<ratchet {
                        for offset in (offsets.isEmpty ? [0] : offsets).prefix(8) {
                            let note = baseNote + max(-96, min(96, offset))
                            guard (0...127).contains(note) else { warnings.insert("Chord pitches outside the MIDI range 0–127 were omitted."); continue }
                            let begin = start + width * micro + Double(repetition) * spacing
                            if velocity > 0 { notes.append(OutputNote(start: begin, end: open ? .infinity : begin + max(1, spacing * gate), channel: channel, pitch: note, velocity: velocity)) }
                            else { for index in notes.indices where notes[index].column == 0 && notes[index].pitch == note && notes[index].channel == channel && notes[index].start < begin && notes[index].end > begin { notes[index].end = begin } }
                        }
                    }
                }
                for lane in pattern["automation"] as? [[String: Any]] ?? [] where PayloadValue.integer(lane, "track", 0) == trackIndex {
                    let target = lane["target"] as? String ?? ""
                    guard let cc = controller(target, instrument: defaultRoute.item) else { warnings.insert("Automation without a MIDI CC mapping was omitted from MIDI export."); continue }
                    let points = (lane["points"] as? [[String: Any]] ?? []).filter { PayloadValue.number($0, "row", -1) >= 0 && PayloadValue.number($0, "row", -1) <= Double(length) }.sorted { PayloadValue.number($0, "row", 0) < PayloadValue.number($1, "row", 0) }
                    func at(_ row: Double) -> Double {
                        let whole = min(length, max(0, Int(row)))
                        if whole == length { return rowTimes[length] }
                        return rowTimes[whole] + (row - Double(whole)) * (rowTimes[whole + 1] - rowTimes[whole])
                    }
                    for (index, point) in points.enumerated() {
                        let row = PayloadValue.number(point, "row", 0), value = PayloadValue.number(point, "value", 0)
                        append(at(row), [UInt8(0xb0 | defaultRoute.channel), UInt8(cc), ccValue(target, value, instrument: defaultRoute.item)])
                        if lane["interpolation"] as? String == "linear", index + 1 < points.count {
                            let nextRow = PayloadValue.number(points[index + 1], "row", row), nextValue = PayloadValue.number(points[index + 1], "value", value)
                            if nextRow > row {
                                var sample = (row * 4).rounded(.down) / 4 + 0.25
                                while sample < nextRow {
                                    let interpolated = value + (nextValue - value) * (sample - row) / (nextRow - row)
                                    append(at(sample), [UInt8(0xb0 | defaultRoute.channel), UInt8(cc), ccValue(target, interpolated, instrument: defaultRoute.item)])
                                    sample += 0.25
                                }
                            }
                        }
                    }
                }
                cursor = rowTimes.last ?? patternStart
                guard events.count + notes.count * 2 <= 2_000_000 else { throw Failure.invalid("MIDI export exceeds the two-million-event safety limit.") }
            }
            // The engine releases all notes when a finite song ends.
            // Receivers pair a note-off with the latest note-on, so a same-pitch note on one channel ends where the next
            // begins (as live playback does); notes sharing an onset tick become one.
            var emitted: [OutputNote] = []
            for note in notes.filter({ $0.start < cursor }).sorted(by: { ($0.channel, $0.pitch, $0.start.rounded()) < ($1.channel, $1.pitch, $1.start.rounded()) }) {
                if let last = emitted.last, last.channel == note.channel, last.pitch == note.pitch {
                    if last.start.rounded() == note.start.rounded() {
                        emitted[emitted.count - 1].end = max(last.end, note.end); emitted[emitted.count - 1].velocity = max(last.velocity, note.velocity); continue
                    }
                    emitted[emitted.count - 1].end = min(last.end, note.start)
                }
                emitted.append(note)
            }
            for note in emitted {
                append(note.start, [UInt8(0x90 | note.channel), UInt8(note.pitch), UInt8(note.velocity)], priority: 2)
                append(max(note.start + 1, min(cursor, note.end)), [UInt8(0x80 | note.channel), UInt8(note.pitch), 0], priority: 0)
            }
            noteCount += emitted.count
            let end = Int64(cursor.rounded())
            totalTicks = max(totalTicks, end)
            outputTracks.append(try encodeTrack(events, name: track["name"] as? String ?? "Track \(trackIndex + 1)", endTick: end))
        }
        if outputTracks.isEmpty { warnings.insert("All tracks were muted; the MIDI file contains only tempo and meter.") }
        // Changes after the arrangement's end would follow End of Track.
        conductor.removeAll { $0.tick > 0 && $0.tick >= totalTicks }
        conductor.append(WriteEvent(tick: totalTicks, priority: 3, order: 2, bytes: [0xff, 0x2f, 0]))
        let conductorBytes = try encodeTrack(conductor, name: project["name"] as? String ?? "SuperTracker", endTick: totalTicks, hasEnd: true)
        var bytes = Array("MThd".utf8) + bigEndian(6, count: 4) + bigEndian(1, count: 2) + bigEndian(outputTracks.count + 1, count: 2) + bigEndian(ppqn, count: 2)
        for track in [conductorBytes] + outputTracks { bytes += Array("MTrk".utf8) + bigEndian(track.count, count: 4) + track }
        guard bytes.count <= 64 * 1024 * 1024 else { throw Failure.invalid("The exported MIDI file exceeds 64 MiB.") }
        try Data(bytes).write(to: url, options: .atomic)
        return ["path": url.path, "noteCount": noteCount, "trackCount": outputTracks.count, "ticksPerQuarter": ppqn, "warnings": warnings.sorted()]
    }

    private static func encodeTrack(_ events: [WriteEvent], name: String, endTick: Int64, hasEnd: Bool = false) throws -> [UInt8] {
        let nameBytes = Array(name.utf8.prefix(1024))
        var bytes: [UInt8] = [0, 0xff, 3] + vlq(Int64(nameBytes.count)) + nameBytes
        let allEvents = hasEnd ? events : events + [WriteEvent(tick: endTick, priority: 3, order: events.count, bytes: [0xff, 0x2f, 0])]
        let sorted = allEvents.sorted { a, b in
            if a.tick != b.tick { return a.tick < b.tick }
            if a.priority != b.priority { return a.priority < b.priority }
            return a.order < b.order
        }
        var last: Int64 = 0
        for event in sorted {
            var delta = event.tick - last
            // A harmless sequencer-specific event bridges gaps wider than SMF's four-byte delta.
            while delta > 0x0fff_ffff { bytes += vlq(0x0fff_ffff) + [0xff, 0x7f, 0]; delta -= 0x0fff_ffff }
            guard delta >= 0 else { throw Failure.invalid("MIDI export produced a negative event delta.") }
            bytes += vlq(delta) + event.bytes; last = event.tick
        }
        return bytes
    }
    private static func vlq(_ value: Int64) -> [UInt8] {
        var value = max(0, value), bytes = [UInt8(value & 0x7f)]
        value >>= 7
        while value > 0 { bytes.insert(UInt8(value & 0x7f) | 0x80, at: 0); value >>= 7 }
        return bytes
    }
    private static func bigEndian(_ value: Int, count: Int) -> [UInt8] { (0..<count).reversed().map { UInt8((value >> ($0 * 8)) & 0xff) } }
    private static func clamp(_ value: Double, _ low: Double, _ high: Double) -> Double { min(high, max(low, value)) }
}

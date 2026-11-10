//! Standard MIDI File reader and conversion to a project.

use super::{clamp, MAX_EVENTS, MAX_FILE_BYTES, MAX_IMPORT_ROWS};
use crate::error::{Error, Result};
use crate::model::instrument::builtin_wavetable;
use crate::model::pattern::MAX_GATE;
use crate::model::*;
use crate::validate::{MAX_INSTRUMENTS, MAX_TRACKS};
use std::collections::BTreeMap;
use std::path::Path;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::invalid(message))
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    end: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8> {
        if self.position >= self.end {
            return invalid("The MIDI file ends inside an event.");
        }
        self.position += 1;
        Ok(self.bytes[self.position - 1])
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.end - self.position {
            return invalid("A MIDI chunk or event exceeds its file bounds.");
        }
        self.position += count;
        Ok(&self.bytes[self.position - count..self.position])
    }
    fn skip(&mut self, count: usize) -> Result<()> {
        self.take(count).map(|_| ())
    }
    fn big(&mut self, count: usize) -> Result<usize> {
        let mut value = 0usize;
        for _ in 0..count {
            value = (value << 8) | self.byte()? as usize;
        }
        Ok(value)
    }
    fn vlq(&mut self) -> Result<i64> {
        let mut value = 0i64;
        for _ in 0..4 {
            let byte = self.byte()?;
            value = (value << 7) | i64::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        invalid("A MIDI variable-length quantity exceeds four bytes.")
    }
    fn data_byte(&mut self) -> Result<i64> {
        let result = self.byte()?;
        if result >= 0x80 {
            return invalid("A MIDI channel event contains a status byte where a data byte is required.");
        }
        Ok(i64::from(result))
    }
}

#[derive(Debug, Clone)]
struct Event {
    tick: i64,
    track: usize,
    status: u8,
    a: i64,
    b: i64,
    data: Vec<u8>,
}

struct FileContents {
    format: usize,
    division: i64,
    events: Vec<Event>,
    track_names: BTreeMap<usize, String>,
    end_tick: i64,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Note {
    track: usize,
    channel: usize,
    note: i64,
    velocity: i64,
    program: i64,
    bank_msb: i64,
    bank_lsb: i64,
    start: i64,
    end: i64,
}

struct Tempo {
    tick: i64,
    micros: i64,
    seconds: f64,
}

struct Lane {
    source_track: usize,
    channel: usize,
    name: String,
    instrument: i64,
    steps: BTreeMap<usize, Step>,
    note_columns: i64,
}

fn parse(bytes: &[u8]) -> Result<FileContents> {
    let mut reader = Reader { bytes, position: 0, end: bytes.len() };
    if reader.take(4).ok() != Some(b"MThd") {
        return invalid("This is not a Standard MIDI File (MThd header missing).");
    }
    let header_length = reader.big(4)?;
    if header_length < 6 {
        return invalid("The MIDI header is too short.");
    }
    let (format, tracks, division) = (reader.big(2)?, reader.big(2)?, reader.big(2)?);
    if format > 1 {
        return invalid("Only synchronous SMF type 0 and type 1 MIDI files are supported; type 2 contains independent songs.");
    }
    if !(1..=1024).contains(&tracks) || (format == 0 && tracks != 1) {
        return invalid("The MIDI file declares an invalid track count.");
    }
    if division & 0x8000 != 0 {
        return invalid("SMPTE-timed MIDI files are not supported. Export the source file with musical (PPQN) timing.");
    }
    if division == 0 {
        return invalid("The MIDI timing division cannot be zero.");
    }
    reader.skip(header_length - 6)?;
    let mut events: Vec<Event> = Vec::new();
    let mut names = BTreeMap::new();
    let (mut track_index, mut end_tick, mut sysex_count) = (0usize, 0i64, 0usize);
    let mut warnings = Vec::new();
    while track_index < tracks {
        let id = reader.take(4)?;
        let size = reader.big(4)?;
        if size > reader.end - reader.position {
            return invalid("A MIDI track extends beyond the end of the file.");
        }
        if id != b"MTrk" {
            reader.skip(size)?;
            continue;
        }
        let mut track = Reader { bytes, position: reader.position, end: reader.position + size };
        reader.skip(size)?;
        let (mut tick, mut running, mut ended) = (0i64, 0u8, false);
        while track.position < track.end {
            tick += track.vlq()?;
            if tick > 1_000_000_000_000 {
                return invalid("The MIDI timeline is too long to safely import.");
            }
            let mut status = track.byte()?;
            if status < 0x80 {
                if !(0x80..0xf0).contains(&running) {
                    return invalid("MIDI running status appears without a preceding channel status.");
                }
                track.position -= 1;
                status = running;
            }
            if status < 0xf0 {
                running = status;
                let a = track.data_byte()?;
                let b = if matches!(status & 0xf0, 0xc0 | 0xd0) { 0 } else { track.data_byte()? };
                events.push(Event { tick, track: track_index, status, a, b, data: Vec::new() });
            } else if status == 0xff {
                running = 0;
                let kind = track.byte()?;
                let length = track.vlq()? as usize;
                match kind {
                    0x2f => {
                        if length != 0 {
                            return invalid("The MIDI end-of-track event has an invalid length.");
                        }
                        ended = true;
                        break;
                    }
                    0x51 => {
                        if length != 3 {
                            return invalid("The MIDI tempo event must contain three bytes.");
                        }
                        let data = track.take(length)?;
                        if data.iter().all(|byte| *byte == 0) {
                            return invalid("The MIDI tempo cannot be zero.");
                        }
                        events.push(Event { tick, track: track_index, status, a: i64::from(kind), b: 0, data: data.to_vec() });
                    }
                    0x58 => {
                        if length != 4 {
                            return invalid("The MIDI time-signature event must contain four bytes.");
                        }
                        let data = track.take(length)?;
                        if data[0] == 0 || data[1] > 7 {
                            return invalid("The MIDI time signature is invalid.");
                        }
                        events.push(Event { tick, track: track_index, status, a: i64::from(kind), b: 0, data: data.to_vec() });
                    }
                    0x03 => {
                        let data = track.take(length.min(1024))?;
                        track.skip(length.saturating_sub(1024))?;
                        // UTF-8, else Latin-1 (every byte is a character).
                        let name = String::from_utf8(data.to_vec()).unwrap_or_else(|_| data.iter().map(|byte| char::from(*byte)).collect());
                        names.insert(track_index, name);
                    }
                    _ => track.skip(length)?,
                }
            } else if status == 0xf0 || status == 0xf7 {
                running = 0;
                let length = track.vlq()? as usize;
                track.skip(length)?;
                sysex_count += 1;
            } else {
                return invalid(format!("An unsupported system status ({status:x}) appears in the MIDI file."));
            }
            if events.len() > MAX_EVENTS {
                return invalid("The MIDI file exceeds the one-million-event safety limit.");
            }
        }
        if !ended {
            warnings.push(format!("Track {} has no end-of-track event; its chunk boundary was used.", track_index + 1));
        }
        end_tick = end_tick.max(tick);
        track_index += 1;
    }
    if sysex_count > 0 {
        warnings.push(format!("{sysex_count} SysEx events were skipped. Device patch dumps are not part of tracker instrument state."));
    }
    // Stable: events at one tick keep their file order.
    events.sort_by_key(|event| event.tick);
    Ok(FileContents { format, division: division as i64, events, track_names: names, end_tick, warnings })
}

fn tempo_map(events: &[Event], division: i64) -> Vec<Tempo> {
    let mut result = vec![Tempo { tick: 0, micros: 500_000, seconds: 0.0 }];
    for event in events.iter().filter(|event| event.status == 0xff && event.a == 0x51) {
        let value = event.data.iter().fold(0i64, |value, byte| (value << 8) | i64::from(*byte));
        let last = &result[result.len() - 1];
        let seconds = last.seconds + (event.tick - last.tick) as f64 * last.micros as f64 / 1_000_000.0 / division as f64;
        let segment = Tempo { tick: event.tick, micros: value, seconds };
        if event.tick == last.tick {
            let index = result.len() - 1;
            result[index] = segment;
        } else if value != last.micros {
            result.push(segment);
        }
    }
    result
}

fn capitalized(target: &str) -> String {
    let mut characters = target.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

/// Audible placeholder: a sampler on a built-in single cycle (looping noise for the drum channel).
fn placeholder(id: usize, note: &Note) -> Instrument {
    let drums = note.channel == 9;
    let table = if drums {
        "Noise"
    } else if (32..=39).contains(&note.program) {
        "Saw"
    } else {
        "Triangle"
    };
    Instrument {
        id: Some(id as i64),
        name: Some(if drums { "GM Drums".to_string() } else { format!("GM Program {}", note.program + 1) }),
        kind: Some(InstrumentKind::Sample),
        sample_path: Some(builtin_wavetable("Basic", table)),
        mode: Some(if drums { SamplerMode::Forward } else { SamplerMode::Wavetable }),
        wavetable_position: (!drums).then_some(0.0),
        wavetable_frame_size: (!drums).then_some(2048),
        start: Some(0.0),
        end: Some(1.0),
        loop_start: Some(0.0),
        loop_end: Some(1.0),
        volume: Some(0.5),
        pan: Some(0.0),
        root_note: Some(60),
        tune: Some(0.0),
        attack: Some(0.003),
        decay: Some(0.15),
        sustain: Some(0.65),
        release: Some(0.12),
        cutoff: Some(18000.0),
        resonance: Some(0.0),
        midi_channel: Some(note.channel as i64 + 1),
        midi_program: Some(note.program),
        midi_bank_msb: Some(note.bank_msb),
        midi_bank_lsb: Some(note.bank_lsb),
        ..Instrument::default()
    }
}

/// Reads a MIDI file from disk and converts it; the project is named after the file.
pub fn import_file(path: &Path) -> Result<Project> {
    let size = std::fs::metadata(path).map_err(|error| Error::io("open", path, &error))?.len();
    if size > MAX_FILE_BYTES {
        return invalid("MIDI files must be smaller than 64 MiB.");
    }
    let bytes = std::fs::read(path).map_err(|error| Error::io("open", path, &error))?;
    let name = path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "MIDI import".to_string());
    import(&bytes, &name, &path.to_string_lossy())
}

/// Converts the bytes of a Standard MIDI File into a new project named `name`.
/// `source` is recorded in `midiImport.source` (the file's path).
///
/// The project uses the standard mixer layout of a new session (routing version 1, one mixer
/// channel per track) and validates. Fails with a readable message for files that are not SMF
/// type 0/1 with PPQN timing, are malformed, or contain no notes.
pub fn import(bytes: &[u8], name: &str, source: &str) -> Result<Project> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return invalid("MIDI files must be smaller than 64 MiB.");
    }
    let parsed = parse(bytes)?;
    let mut warnings = parsed.warnings.clone();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut warn = |message: &str, count: usize| *counts.entry(message.to_string()).or_insert(0) += count;
    let tempos = tempo_map(&parsed.events, parsed.division);
    let source_bpm = 60_000_000.0 / tempos[0].micros as f64;
    let bpm = clamp(source_bpm, 20.0, 400.0);
    if bpm != source_bpm {
        warnings.push(format!("The source tempo is outside 20–400 BPM; event timing was converted to {} BPM.", bpm as i64));
    }
    if tempos.len() > 1 {
        warnings.push("Tempo changes were converted to one tracker tempo while preserving event timing. The original tempo map remains in MIDI import metadata.".to_string());
    }
    let rows_per_second = bpm / 60.0 * 4.0;
    let row_at = |tick: i64| -> f64 {
        let index = tempos.partition_point(|tempo| tempo.tick <= tick).max(1) - 1;
        let segment = &tempos[index];
        (segment.seconds + (tick - segment.tick) as f64 * segment.micros as f64 / 1_000_000.0 / parsed.division as f64) * rows_per_second
    };

    let mut active: BTreeMap<(usize, usize, i64), Vec<Note>> = BTreeMap::new();
    let mut held: BTreeMap<usize, Vec<Note>> = BTreeMap::new();
    let mut sustain = [false; 16];
    let (mut programs, mut bank_msb, mut bank_lsb) = ([0i64; 16], [0i64; 16], [0i64; 16]);
    let mut notes: Vec<Note> = Vec::new();
    let mut controller_events: Vec<&Event> = Vec::new();
    let mut program_events: Vec<MidiProgramChange> = Vec::new();
    let mut signatures: Vec<MidiTimeSignature> = Vec::new();
    let finish = |notes: &mut Vec<Note>, mut note: Note, tick: i64| {
        note.end = tick.max(note.start + 1);
        notes.push(note);
    };
    for event in &parsed.events {
        if event.status == 0xff {
            if event.a == 0x58 && event.data.len() >= 4 {
                signatures.push(MidiTimeSignature {
                    tick: Some(event.tick),
                    numerator: Some(i64::from(event.data[0])),
                    denominator: Some(1 << event.data[1]),
                    ..MidiTimeSignature::default()
                });
            }
            continue;
        }
        let channel = (event.status & 15) as usize;
        let key = (event.track, channel, event.a);
        match event.status & 0xf0 {
            0x90 if event.b > 0 => {
                let pending = active.entry(key).or_default();
                if pending.len() >= 1024 {
                    return invalid("The MIDI file contains more than 1,024 overlapping copies of one note.");
                }
                pending.push(Note {
                    track: event.track,
                    channel,
                    note: event.a,
                    velocity: event.b,
                    program: programs[channel],
                    bank_msb: bank_msb[channel],
                    bank_lsb: bank_lsb[channel],
                    start: event.tick,
                    end: event.tick,
                });
            }
            0x80 | 0x90 => match active.get_mut(&key).filter(|pending| !pending.is_empty()) {
                Some(pending) => {
                    let note = pending.remove(0);
                    if sustain[channel] {
                        held.entry(channel).or_default().push(note);
                    } else {
                        finish(&mut notes, note, event.tick);
                    }
                }
                None => warn("Unmatched note-off events were ignored", 1),
            },
            0xc0 => {
                programs[channel] = event.a;
                program_events.push(MidiProgramChange { tick: Some(event.tick), channel: Some(channel as i64 + 1), program: Some(event.a), ..MidiProgramChange::default() });
            }
            0xb0 => {
                controller_events.push(event);
                match event.a {
                    0 => bank_msb[channel] = event.b,
                    32 => bank_lsb[channel] = event.b,
                    64 => {
                        sustain[channel] = event.b >= 64;
                        if !sustain[channel] {
                            for note in held.remove(&channel).unwrap_or_default() {
                                finish(&mut notes, note, event.tick);
                            }
                        }
                    }
                    120 | 123 => {
                        for (_, pending) in active.iter_mut().filter(|(key, _)| key.1 == channel) {
                            for note in pending.drain(..) {
                                finish(&mut notes, note, event.tick);
                            }
                        }
                        for note in held.remove(&channel).unwrap_or_default() {
                            finish(&mut notes, note, event.tick);
                        }
                    }
                    _ => {}
                }
            }
            0xe0 => warn("Pitch-bend events remain unsupported by tracker MIDI import", 1),
            0xa0 | 0xd0 => warn("Aftertouch events remain unsupported by tracker MIDI import", 1),
            _ => {}
        }
    }
    for note in active.into_values().flatten() {
        finish(&mut notes, note, parsed.end_tick.max(note.start + parsed.division));
        warn("Notes without a note-off were ended at the file boundary", 1);
    }
    for note in held.into_values().flatten() {
        finish(&mut notes, note, parsed.end_tick.max(note.start + 1));
    }
    notes.sort_by_key(|note| (note.start, note.track, note.note));
    if notes.is_empty() {
        return invalid("This MIDI file contains no note events to import.");
    }

    let mut instrument_ids: BTreeMap<(usize, i64, i64, i64), i64> = BTreeMap::new();
    let mut instruments: Vec<Instrument> = Vec::new();
    // Simultaneous notes of one source with equal end, velocity and program form one packet (a chord).
    let mut groups: BTreeMap<(i64, usize, usize, i64, i64, i64, i64, i64), Vec<Note>> = BTreeMap::new();
    for note in &notes {
        groups.entry((note.start, note.track, note.channel, note.end, note.velocity, note.program, note.bank_msb, note.bank_lsb)).or_default().push(*note);
    }
    let mut packets: Vec<Vec<Note>> = groups.into_values().collect();
    packets.sort_by_key(|packet| (packet[0].start, packet[0].track, packet[0].channel, packet[0].note));
    let mut lanes: Vec<Lane> = Vec::new();
    let source_end_row = row_at(parsed.end_tick);
    if source_end_row > MAX_IMPORT_ROWS as f64 {
        warn("The source timeline extends past 65,536 rows and was truncated", 1);
    }
    let mut highest_row = (source_end_row.min(MAX_IMPORT_ROWS as f64).ceil() as usize).max(1) - 1;
    let mut imported_notes = 0usize;
    for mut packet in packets {
        packet.sort_by_key(|note| note.note);
        let mut cursor = 0;
        while cursor < packet.len() {
            let first = packet[cursor];
            let mut finish_index = packet.len().min(cursor + 8);
            while finish_index > cursor + 1 && packet[finish_index - 1].note - first.note > 96 {
                finish_index -= 1;
            }
            let chord = &packet[cursor..finish_index];
            cursor = finish_index;
            let position = row_at(first.start);
            if position >= MAX_IMPORT_ROWS as f64 {
                warn("Notes beyond the 65,536-row import limit were omitted", chord.len());
                continue;
            }
            let row = position.floor() as usize;
            let raw_micro = (position - row as f64).max(0.0);
            let micro = raw_micro.min(0.95);
            if raw_micro > 0.95001 {
                warn("Onsets in the final 5% of a row were moved slightly earlier to fit microtiming", 1);
            }
            let duration = (row_at(first.end) - position).max(0.01);
            let raw_gate = duration / (1.0 - micro);
            let gate = raw_gate.min(MAX_GATE);
            if raw_gate > MAX_GATE + 0.0001 {
                warn("Note durations longer than the engine's 256-row gate were shortened", chord.len());
            }
            let key = (first.channel, first.program, first.bank_msb, first.bank_lsb);
            let instrument_id = match instrument_ids.get(&key) {
                Some(id) => *id,
                None if instruments.len() >= MAX_INSTRUMENTS => {
                    warn(&format!("Program changes beyond {MAX_INSTRUMENTS} instruments use the first instrument"), 1);
                    0
                }
                None => {
                    let id = instruments.len();
                    instruments.push(placeholder(id, &first));
                    instrument_ids.insert(key, id as i64);
                    id as i64
                }
            };
            let row_micro = if micro > 0.000001 { micro } else { 0.0 };
            let last_row = |gate: f64| (MAX_IMPORT_ROWS - 1).min(((position + gate * (1.0 - micro)).ceil() as usize).max(1) - 1);
            let same_source = |lane: &Lane| lane.source_track == first.track && lane.channel == first.channel;
            let mut lane_id = lanes.iter().position(|lane| same_source(lane) && !lane.steps.contains_key(&row));
            // Overlapping notes on a taken row go into free note columns of the same track (eight per
            // track) when they share the row's microtiming (row effects belong to column 0); otherwise
            // into another lane.
            if lane_id.is_none() {
                let fit = lanes.iter().position(|lane| {
                    let Some(taken) = lane.steps.get(&row).filter(|_| same_source(lane)) else { return false };
                    let used = taken.columns.as_ref().map_or(0, Vec::len);
                    let taken_micro = taken.fx1.as_ref().map_or(0.0, |effect| effect.value());
                    used + chord.len() <= 7 && (taken_micro - row_micro).abs() < 0.000001
                });
                if let Some(fit) = fit {
                    let lane = &mut lanes[fit];
                    let columns = lane.steps.get_mut(&row).map(|taken| taken.columns.get_or_insert_with(Vec::new));
                    if let Some(columns) = columns {
                        for note in chord {
                            columns.push(Some(ColumnCell { note: Some(note.note), instrument: Some(instrument_id), velocity: Some(note.velocity), gate: Some(gate), ..ColumnCell::default() }));
                        }
                        lane.note_columns = lane.note_columns.max(columns.len() as i64 + 1);
                    }
                    highest_row = highest_row.max(row).max(last_row(gate));
                    imported_notes += chord.len();
                    continue;
                }
            }
            if lane_id.is_none() && lanes.len() < MAX_TRACKS {
                lane_id = Some(lanes.len());
                let label = parsed.track_names.get(&first.track).cloned().unwrap_or_else(|| format!("MIDI Channel {}", first.channel + 1));
                let suffix = if lanes.iter().any(same_source) { format!(" · Voice {}", lanes.len() + 1) } else { String::new() };
                lanes.push(Lane { source_track: first.track, channel: first.channel, name: label + &suffix, instrument: instrument_id, steps: BTreeMap::new(), note_columns: 1 });
            }
            let Some(lane_id) = lane_id else {
                warn(&format!("Notes requiring more than {MAX_TRACKS} tracker tracks were omitted"), chord.len());
                continue;
            };
            let step = Step {
                note: Some(first.note),
                instrument: Some(instrument_id),
                velocity: Some(first.velocity),
                gate: Some(gate),
                chord: (chord.len() > 1).then(|| chord.iter().map(|note| note.note - first.note).collect()),
                fx1: (micro > 0.000001).then(|| StepEffect { kind: Some(StepEffectKind::Microtiming), value: Some(micro), ..StepEffect::default() }),
                ..Step::default()
            };
            lanes[lane_id].steps.insert(row, step);
            highest_row = highest_row.max(row).max(last_row(gate));
            imported_notes += chord.len();
        }
    }
    if lanes.is_empty() {
        return invalid("All MIDI notes fall outside the tracker import limits.");
    }
    let pattern_length: usize = if highest_row < 16384 { 64 } else { 256 };
    let pattern_count = ((highest_row + pattern_length) / pattern_length).clamp(1, 256);
    let colors = ["#76d7cb", "#e8b86f", "#ab99ed", "#ee8997"];
    let mut routing = Routing::standard();
    let tracks: Vec<Track> = lanes
        .iter()
        .enumerate()
        .map(|(index, lane)| {
            let routing_id = format!("midi-track-{}", index + 1);
            if let Some(outputs) = &mut routing.outputs {
                outputs.insert(format!("track:{routing_id}"), Some("master".to_string()));
            }
            Track {
                id: Some(index as i64),
                name: Some(lane.name.clone()),
                color: Some(colors[index % 4].to_string()),
                volume: Some(0.8),
                pan: Some(0.0),
                mute: Some(false),
                solo: Some(false),
                instrument: Some(lane.instrument),
                delay_send: Some(0.0),
                reverb_send: Some(0.0),
                routing_id: Some(routing_id),
                midi_channel: Some(lane.channel as i64 + 1),
                note_columns: (lane.note_columns > 1).then_some(lane.note_columns),
                ..Track::default()
            }
        })
        .collect();

    // CC 7, 10 and 91 have stable volume, pan and reverb meanings. Other controllers stay in metadata.
    let mut lane_points: BTreeMap<(usize, usize, &'static str), Vec<AutomationPoint>> = BTreeMap::new();
    for event in &controller_events {
        let target = match event.a {
            7 => "volume",
            10 => "pan",
            91 => "reverb",
            0 | 32 | 64 | 120 | 123 => continue,
            _ => {
                warn("Unmapped MIDI controller events were retained in metadata only", 1);
                continue;
            }
        };
        let position = row_at(event.tick);
        if position >= (pattern_count * pattern_length) as f64 {
            continue;
        }
        let pattern = position as usize / pattern_length;
        let row = position - (pattern * pattern_length) as f64;
        let value = if target == "pan" { (event.b - 64) as f64 / if event.b < 64 { 64.0 } else { 63.0 } } else { event.b as f64 / 127.0 };
        for (track, _) in lanes.iter().enumerate().filter(|(_, lane)| lane.channel == (event.status & 15) as usize) {
            lane_points.entry((pattern, track, target)).or_default().push(AutomationPoint { row: Some(row), value: Some(value), ..AutomationPoint::default() });
        }
    }
    let midi_lane = |id: String, track: usize, target: &str, points: Vec<AutomationPoint>| AutomationLane {
        id: Some(id),
        track: Some(track as i64),
        target: Some(target.to_string()),
        name: Some(format!("MIDI {}", capitalized(target))),
        interpolation: Some(Interpolation::Step),
        points: Some(points),
        ..AutomationLane::default()
    };
    let mut automation: BTreeMap<usize, Vec<AutomationLane>> = BTreeMap::new();
    for ((pattern, track, target), points) in lane_points {
        automation.entry(pattern).or_default().push(midi_lane(format!("midi-{pattern}:{track}:{target}"), track, target, points));
    }
    // Tracker mix state resets at pattern boundaries; MIDI controllers stay in effect. Carry the last
    // controller value forward so an imported fader move survives into the next pattern.
    let mut carried: BTreeMap<(usize, String), f64> = BTreeMap::new();
    let mut patterns: Vec<Pattern> = Vec::with_capacity(pattern_count);
    for pattern in 0..pattern_count {
        let mut current = automation.remove(&pattern).unwrap_or_default();
        for lane in &mut current {
            let key = (lane.track() as usize, lane.target_name().to_string());
            let points = lane.points.get_or_insert_with(Vec::new);
            if let Some(previous) = carried.get(&key) {
                if points.first().map_or(1.0, |point| point.row()) > 0.0 {
                    points.insert(0, AutomationPoint { row: Some(0.0), value: Some(*previous), ..AutomationPoint::default() });
                }
            }
            if let Some(last) = points.last() {
                carried.insert(key, last.value());
            }
            if points.len() > 4096 {
                warn("Dense controller lanes were reduced to the engine's 4,096-point limit", 1);
                let last = points[points.len() - 1].clone();
                points.truncate(4095);
                points.push(last);
            }
        }
        for ((track, target), value) in &carried {
            if !current.iter().any(|lane| lane.track() as usize == *track && lane.target_name() == target) {
                let point = AutomationPoint { row: Some(0.0), value: Some(*value), ..AutomationPoint::default() };
                current.push(midi_lane(format!("midi-carry-{pattern}-{track}:{target}"), *track, target, vec![point]));
            }
        }
        let steps = lanes.iter().map(|lane| (0..pattern_length).map(|row| lane.steps.get(&(pattern * pattern_length + row)).cloned()).collect()).collect();
        patterns.push(Pattern {
            id: Some(pattern as i64),
            name: Some(format!("MIDI {:02}", pattern + 1)),
            length: Some(pattern_length as i64),
            steps: Some(steps),
            automation: Some(current),
            ..Pattern::default()
        });
    }
    if controller_events.len() > 100_000 {
        warnings.push("Raw controller metadata is limited to the first 100,000 events.".to_string());
    }
    if program_events.len() > 100_000 {
        warnings.push("Raw program-change metadata is limited to the first 100,000 events.".to_string());
    }
    if signatures.len() > 1 {
        warnings.push("Meter changes are retained in MIDI metadata; the tracker displays the initial time signature.".to_string());
    }
    for (message, count) in &counts {
        warnings.push(format!("{message} ({count})."));
    }
    warnings.push("Imported instruments use built-in wavetable placeholders. Original MIDI channels, banks and programs are retained; choose samples or plug-ins for the intended sounds.".to_string());
    let first_signature = signatures.iter().rev().find(|signature| signature.tick == Some(0));
    let time_signature = TimeSignature {
        numerator: Some(first_signature.and_then(|signature| signature.numerator).unwrap_or(4)),
        denominator: Some(first_signature.and_then(|signature| signature.denominator).unwrap_or(4)),
        ..TimeSignature::default()
    };
    program_events.truncate(100_000);
    let metadata = MidiImportInfo {
        source: Some(source.to_string()),
        format: Some(parsed.format as i64),
        ticks_per_quarter: Some(parsed.division),
        warnings: Some(warnings),
        imported_notes: Some(imported_notes as i64),
        source_notes: Some(notes.len() as i64),
        tempo_changes: Some(tempos.iter().map(|tempo| MidiTempoChange { tick: Some(tempo.tick), microseconds_per_quarter: Some(tempo.micros), ..MidiTempoChange::default() }).collect()),
        time_signatures: Some(signatures),
        program_changes: Some(program_events),
        controllers: Some(
            controller_events
                .iter()
                .take(100_000)
                .map(|event| MidiControllerEvent {
                    tick: Some(event.tick),
                    channel: Some(i64::from(event.status & 15) + 1),
                    controller: Some(event.a),
                    value: Some(event.b),
                    ..MidiControllerEvent::default()
                })
                .collect(),
        ),
        ..MidiImportInfo::default()
    };
    Ok(Project {
        version: Some(1),
        name: Some(name.to_string()),
        bpm: Some(bpm),
        lines_per_beat: Some(4),
        swing: Some(0.0),
        master_volume: Some(0.7),
        active_pattern: Some(0),
        tracks: Some(tracks),
        instruments: Some(instruments),
        patterns: Some(patterns),
        sequence: Some((0..pattern_count as i64).collect()),
        loop_song: Some(false),
        delay: Some(Delay { time: Some(0.375), feedback: Some(0.35), mix: Some(0.3), ..Delay::default() }),
        reverb: Some(Reverb { size: Some(0.7), damping: Some(0.4), mix: Some(0.25), ..Reverb::default() }),
        time_signature: Some(time_signature),
        routing: Some(routing),
        audio_plugins: Some(Vec::new()),
        midi_import: Some(metadata),
        ..Project::default()
    })
}

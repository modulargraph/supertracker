//! Standard MIDI File writer.

use super::{clamp, EXPORT_PPQN, MAX_FILE_BYTES};
use crate::error::{Error, Result};
use crate::files::write_atomic;
use crate::model::pattern::{MAX_GATE, NOTE_CUT, NOTE_NONE, NOTE_OFF};
use crate::model::*;
use crate::validate::{MAX_INSTRUMENTS, MAX_PATTERNS, MAX_TRACKS};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// An exported MIDI file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiExport {
    /// The file's bytes (SMF type 1).
    pub bytes: Vec<u8>,
    /// Notes written.
    pub note_count: usize,
    /// Note tracks written (muted tracks are left out); the file has one more, the conductor.
    pub track_count: usize,
    /// Ticks per quarter note (960).
    pub ticks_per_quarter: u16,
    /// What the file does not represent, sorted.
    pub warnings: Vec<String>,
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::invalid(message))
}

struct WriteEvent {
    tick: i64,
    priority: u8,
    order: usize,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy)]
struct OutputNote {
    start: f64,
    end: f64,
    channel: u8,
    pitch: i64,
    velocity: i64,
    column: usize,
}

fn vlq(value: i64) -> Vec<u8> {
    let mut value = value.max(0);
    let mut bytes = vec![(value & 0x7f) as u8];
    value >>= 7;
    while value > 0 {
        bytes.insert(0, (value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    bytes
}

fn big_endian(value: usize, count: usize) -> Vec<u8> {
    (0..count).rev().map(|index| ((value >> (index * 8)) & 0xff) as u8).collect()
}

fn encode_track(mut events: Vec<WriteEvent>, name: &str, end_tick: i64, has_end: bool) -> Result<Vec<u8>> {
    let mut name_bytes = name.as_bytes().to_vec();
    name_bytes.truncate(1024);
    let mut bytes = vec![0, 0xff, 3];
    bytes.extend(vlq(name_bytes.len() as i64));
    bytes.extend(name_bytes);
    if !has_end {
        let order = events.len();
        events.push(WriteEvent { tick: end_tick, priority: 3, order, bytes: vec![0xff, 0x2f, 0] });
    }
    events.sort_by_key(|event| (event.tick, event.priority, event.order));
    let mut last = 0i64;
    for event in events {
        let mut delta = event.tick - last;
        if delta < 0 {
            return invalid("MIDI export produced a negative event delta.");
        }
        // A harmless sequencer-specific event bridges gaps wider than SMF's four-byte delta.
        while delta > 0x0fff_ffff {
            bytes.extend(vlq(0x0fff_ffff));
            bytes.extend([0xff, 0x7f, 0]);
            delta -= 0x0fff_ffff;
        }
        bytes.extend(vlq(delta));
        bytes.extend(event.bytes);
        last = event.tick;
    }
    Ok(bytes)
}

/// The channel and instrument a note is exported with.
struct Route<'a> {
    instrument: Option<&'a Instrument>,
    channel: u8,
}

struct TrackWriter<'a> {
    instruments: &'a [Instrument],
    track: &'a Track,
    track_index: usize,
    events: Vec<WriteEvent>,
    last_program: BTreeMap<u8, (i64, i64, i64)>,
}

impl<'a> TrackWriter<'a> {
    fn append(&mut self, tick: f64, bytes: Vec<u8>, priority: u8) {
        let order = self.events.len();
        self.events.push(WriteEvent { tick: tick.max(0.0).round() as i64, priority, order, bytes });
    }

    fn route(&self, instrument_index: i64) -> Route<'a> {
        let instrument = usize::try_from(instrument_index).ok().and_then(|index| self.instruments.get(index));
        // Each tracker track stays its own SMF track; beyond 16 the default channels repeat.
        let fallback = self.track.midi_channel.unwrap_or(self.track_index as i64 % 16 + 1);
        let channel = instrument.and_then(|instrument| instrument.midi_channel).unwrap_or(fallback);
        Route { instrument, channel: (channel - 1).clamp(0, 15) as u8 }
    }

    fn select_program(&mut self, route: &Route, start: f64) {
        let Some(instrument) = route.instrument else { return };
        let Some(program) = instrument.midi_program else { return };
        let state = (instrument.midi_bank_msb().clamp(0, 127), instrument.midi_bank_lsb().clamp(0, 127), program.clamp(0, 127));
        if self.last_program.get(&route.channel) != Some(&state) {
            self.append(start, vec![0xb0 | route.channel, 0, state.0 as u8], 1);
            self.append(start, vec![0xb0 | route.channel, 32, state.1 as u8], 1);
            self.append(start, vec![0xc0 | route.channel, state.2 as u8], 1);
            self.last_program.insert(route.channel, state);
        }
    }
}

/// The controller number for an automation target or step effect on this instrument.
fn controller(target: &str, instrument: Option<&Instrument>) -> Option<u8> {
    if let Some(explicit) = instrument.and_then(|instrument| instrument.midi_cc_map.as_ref()).and_then(|map| map.get(target)) {
        if (0..=127).contains(explicit) {
            return Some(*explicit as u8);
        }
    }
    match target {
        "volume" => Some(7),
        "pan" => Some(10),
        "reverb" => Some(91),
        _ => None,
    }
}

fn cc_value(target: &str, value: f64, instrument: Option<&Instrument>) -> u8 {
    let range = instrument.and_then(|instrument| instrument.midi_cc_ranges.as_ref()).and_then(|ranges| ranges.get(target));
    let normalized = match range {
        Some(range) if range.len() == 2 && range[1] > range[0] => (value - range[0]) / (range[1] - range[0]),
        _ if target == "pan" => return clamp((64.0 + value * if value < 0.0 { 64.0 } else { 63.0 }).round(), 0.0, 127.0) as u8,
        _ => value,
    };
    clamp((normalized * 127.0).round(), 0.0, 127.0) as u8
}

/// Exports the project's song (its sequence, once) as a Standard MIDI File.
///
/// Song looping, samples, plug-in state and unmapped automation are not MIDI data; the result's
/// warnings name what was left out.
pub fn export(project: &Project) -> Result<MidiExport> {
    let tracks = project.track_list();
    let instruments = project.instrument_list();
    let patterns = project.pattern_list();
    let sequence = project.song_sequence();
    if tracks.is_empty() || tracks.len() > MAX_TRACKS || instruments.len() > MAX_INSTRUMENTS || patterns.is_empty() || patterns.len() > MAX_PATTERNS || sequence.len() > 4096 {
        return invalid(format!("MIDI export requires a valid tracker project with 1–{MAX_TRACKS} tracks and up to {MAX_PATTERNS} patterns."));
    }
    let (bpm, lines_per_beat, swing) = (project.bpm(), project.lines_per_beat() as f64, clamp(project.swing(), 0.0, 0.95));
    if !bpm.is_finite() || !(20.0..=400.0).contains(&bpm) || !(1.0..=32.0).contains(&lines_per_beat) {
        return invalid("The project tempo or rows-per-beat is invalid.");
    }
    let ppqn = usize::from(EXPORT_PPQN);
    let ticks_per_row = ppqn as f64 / lines_per_beat;
    let mut warnings: BTreeSet<&'static str> = BTreeSet::new();
    let by_number: BTreeMap<i64, &Pattern> = patterns.iter().enumerate().map(|(position, pattern)| (pattern.number(position), pattern)).collect();
    let (numerator, denominator) = project.meter();
    let power = [1, 2, 4, 8, 16, 32, 64, 128].iter().position(|value| *value == denominator).unwrap_or(2);
    let micros = (60_000_000.0 / bpm).round() as usize;
    let mut conductor = vec![
        WriteEvent { tick: 0, priority: 0, order: 0, bytes: vec![0xff, 0x51, 3, (micros >> 16) as u8, (micros >> 8) as u8, micros as u8] },
        WriteEvent { tick: 0, priority: 0, order: 1, bytes: vec![0xff, 0x58, 4, numerator.clamp(1, 127) as u8, power as u8, 24, 8] },
    ];
    let any_solo = tracks.iter().any(|track| track.solo());
    let mut output_tracks: Vec<Vec<u8>> = Vec::new();
    let (mut note_count, mut total_ticks) = (0usize, 0i64);
    for (track_index, track) in tracks.iter().enumerate() {
        if track.mute() || (any_solo && !track.solo()) {
            continue;
        }
        let mut writer = TrackWriter { instruments, track, track_index, events: Vec::new(), last_program: BTreeMap::new() };
        let mut notes: Vec<OutputNote> = Vec::new();
        let (mut cursor, mut row_count) = (0.0f64, 0usize);
        let mut held_ratchet = 1usize;
        let mut last_step: Option<Step> = None;
        // Extra note columns (step.columns) are independent voices of the track, as in the engine.
        let extra_columns = track.note_column_count() - 1;
        let default_instrument = track.instrument();
        let default_route = writer.route(default_instrument);
        for (target, value) in [("volume", track.volume()), ("pan", track.pan()), ("reverb", track.reverb_send())] {
            if let Some(cc) = controller(target, default_route.instrument) {
                writer.append(0.0, vec![0xb0 | default_route.channel, cc, cc_value(target, value, default_route.instrument)], 1);
            }
        }
        for pattern_number in &sequence {
            let Some(pattern) = by_number.get(pattern_number) else {
                return invalid(format!("The song sequence refers to missing pattern {pattern_number}."));
            };
            let length = pattern.length();
            if !(1..=256).contains(&length) {
                return invalid("A MIDI export pattern has an invalid row count.");
            }
            let length = length as usize;
            row_count += length;
            if row_count > 1_048_576 {
                return invalid("The MIDI export timeline exceeds its safety limit.");
            }
            let mut row_times = vec![cursor];
            for row in 0..length {
                let last = row_times[row];
                row_times.push(last + ticks_per_row * (1.0 + if row % 2 == 0 { swing } else { -swing }));
            }
            for row in 0..length {
                let source = pattern.step(track_index, row);
                let source_note = source.map_or(NOTE_NONE, Step::note);
                let effects: Vec<&StepEffect> = source.map(|step| step.effects().collect()).unwrap_or_default();
                let effect_value = |effect: &StepEffect| effect.value();
                let (start, width) = (row_times[row], row_times[row + 1] - row_times[row]);
                if let Some(columns) = source.and_then(|step| step.columns.as_ref()).filter(|_| extra_columns > 0) {
                    // The row's note effects (column 0's) apply to every column; a sticky ratchet repeats column 0 only.
                    let (mut micro, mut ratchet, mut probability) = (0.0, 1usize, 1.0);
                    // As in the engine, an OFF/CUT in column 0 carries no note effects for the other columns.
                    if source_note != NOTE_OFF && source_note != NOTE_CUT {
                        for effect in &effects {
                            match effect.kind {
                                Some(StepEffectKind::Microtiming) => micro = clamp(effect_value(effect), 0.0, 0.95),
                                Some(StepEffectKind::Ratchet) => ratchet = clamp(effect_value(effect), 1.0, 8.0) as usize,
                                Some(StepEffectKind::Probability) => probability = clamp(effect_value(effect), 0.0, 1.0),
                                _ => {}
                            }
                        }
                    }
                    for (index, cell) in columns.iter().take(extra_columns).enumerate() {
                        let Some(cell) = cell else { continue };
                        let (column, pitch) = (index + 1, cell.note());
                        if pitch == NOTE_OFF || pitch == NOTE_CUT {
                            for note in notes.iter_mut().filter(|note| note.column == column && note.start < start && note.end > start) {
                                note.end = start;
                            }
                            continue;
                        }
                        if !(0..=127).contains(&pitch) || probability <= 0.0 {
                            continue;
                        }
                        let selected = cell.instrument.unwrap_or(default_instrument);
                        let route = writer.route(if selected < 0 { default_instrument } else { selected });
                        writer.select_program(&route, start);
                        let raw_gate = cell.gate();
                        let open = raw_gate <= 0.0 && ratchet <= 1;
                        let gate = if raw_gate > 0.0 { clamp(raw_gate, 0.01, MAX_GATE) } else { 1.0 };
                        let spacing = width * (1.0 - micro) / ratchet as f64;
                        let onset = start + width * micro;
                        let velocity = cell.velocity().clamp(0, 127);
                        if velocity > 0 {
                            for note in notes.iter_mut().filter(|note| note.column == column && note.end == f64::INFINITY && note.start < onset) {
                                note.end = onset;
                            }
                        }
                        for repetition in 0..ratchet {
                            let begin = onset + repetition as f64 * spacing;
                            if velocity > 0 {
                                let end = if open { f64::INFINITY } else { begin + (spacing * gate).max(1.0) };
                                notes.push(OutputNote { start: begin, end, channel: route.channel, pitch, velocity, column });
                            } else {
                                for note in notes.iter_mut().filter(|note| note.column == column && note.pitch == pitch && note.channel == route.channel && note.start < begin && note.end > begin) {
                                    note.end = begin;
                                }
                            }
                        }
                    }
                }
                let ratchet_effect = effects.iter().rev().find(|effect| effect.kind == Some(StepEffectKind::Ratchet));
                // As in the engine: a row's own Ratchet sets the rate; a new note or another effect row stops it.
                if let Some(effect) = ratchet_effect {
                    held_ratchet = clamp(effect_value(effect), 1.0, 8.0) as usize;
                } else if !effects.is_empty() || source_note >= 0 {
                    held_ratchet = 1;
                }
                let repeat_only = source.is_none() || (source_note == NOTE_NONE && ratchet_effect.is_some());
                let step: Step = match (&last_step, source) {
                    (Some(previous), _) if repeat_only && held_ratchet > 1 => previous.clone(),
                    (_, Some(source)) => source.clone(),
                    (_, None) => continue,
                };
                let base_note = step.note();
                if base_note == NOTE_OFF || base_note == NOTE_CUT {
                    held_ratchet = 1;
                    last_step = None;
                    for note in notes.iter_mut().filter(|note| note.column == 0 && note.start < start && note.end > start) {
                        note.end = start;
                    }
                    continue;
                }
                let selected = step.instrument.unwrap_or(default_instrument);
                let selected = if selected < 0 { default_instrument } else { selected };
                let route = writer.route(selected);
                let channel = route.channel;
                let (mut micro, mut ratchet, mut probability) = (0.0, held_ratchet, 1.0);
                for effect in &effects {
                    let value = effect_value(effect);
                    match effect.kind {
                        Some(StepEffectKind::Microtiming) => {
                            if !repeat_only {
                                micro = clamp(value, 0.0, 0.95);
                            }
                        }
                        Some(StepEffectKind::Ratchet) => ratchet = clamp(value, 1.0, 8.0) as usize,
                        Some(StepEffectKind::Probability) => {
                            if !repeat_only {
                                probability = clamp(value, 0.0, 1.0);
                            }
                        }
                        Some(kind) => match controller(kind.as_str(), route.instrument) {
                            Some(cc) => writer.append(start, vec![0xb0 | channel, cc, cc_value(kind.as_str(), value, route.instrument)], 1),
                            None => {
                                warnings.insert("Unmapped step FX (including sample processing and plugin controls) are not represented in MIDI.");
                            }
                        },
                        None => {}
                    }
                }
                if !(0..=127).contains(&base_note) || probability <= 0.0 {
                    continue;
                }
                if step.velocity() <= 0 {
                    held_ratchet = 1;
                    last_step = None;
                } else {
                    let mut remembered = step.clone();
                    remembered.instrument = Some(selected);
                    last_step = Some(remembered);
                }
                if probability < 1.0 {
                    warnings.insert("Probabilistic steps were exported deterministically as notes; their random probability is not a MIDI event.");
                }
                writer.select_program(&route, start);
                let offsets: Vec<i64> = match &step.chord {
                    Some(chord) if !chord.is_empty() => chord.clone(),
                    _ => vec![0],
                };
                if offsets.len() > 8 {
                    warnings.insert("Chord notes beyond the engine's eight-note limit were omitted.");
                }
                // Gate 0 is no gate: the note lasts until the next note on this track, an OFF/CUT or the song
                // end. Ratchet repeats of such a note each last one repeat spacing. A missing gate is the legacy 0.8.
                let raw_gate = step.gate();
                let open = raw_gate <= 0.0 && ratchet <= 1;
                let gate = if raw_gate > 0.0 { clamp(raw_gate, 0.01, MAX_GATE) } else { 1.0 };
                let spacing = width * (1.0 - micro) / ratchet as f64;
                let velocity = step.velocity().clamp(0, 127);
                if velocity > 0 {
                    let onset = start + width * micro;
                    for note in notes.iter_mut().filter(|note| note.column == 0 && note.end == f64::INFINITY && note.start < onset) {
                        note.end = onset;
                    }
                }
                for repetition in 0..ratchet {
                    for offset in offsets.iter().take(8) {
                        let pitch = base_note + (*offset).clamp(-96, 96);
                        if !(0..=127).contains(&pitch) {
                            warnings.insert("Chord pitches outside the MIDI range 0–127 were omitted.");
                            continue;
                        }
                        let begin = start + width * micro + repetition as f64 * spacing;
                        if velocity > 0 {
                            let end = if open { f64::INFINITY } else { begin + (spacing * gate).max(1.0) };
                            notes.push(OutputNote { start: begin, end, channel, pitch, velocity, column: 0 });
                        } else {
                            for note in notes.iter_mut().filter(|note| note.column == 0 && note.pitch == pitch && note.channel == channel && note.start < begin && note.end > begin) {
                                note.end = begin;
                            }
                        }
                    }
                }
            }
            // A lane belongs to the track by index, or by the track's mixer channel (`owner`).
            let track_owner = track.owner();
            let belongs = |lane: &&AutomationLane| match &lane.owner {
                Some(owner) => track_owner.as_ref() == Some(owner),
                None => lane.track() == track_index as i64,
            };
            for lane in pattern.automation.iter().flatten().filter(belongs) {
                let target = lane.target.as_deref().unwrap_or("");
                let Some(cc) = controller(target, default_route.instrument) else {
                    warnings.insert("Automation without a MIDI CC mapping was omitted from MIDI export.");
                    continue;
                };
                let mut points: Vec<(f64, f64)> = lane.points.iter().flatten().filter_map(|point| Some((point.row?, point.value()))).filter(|(row, _)| *row >= 0.0 && *row <= length as f64).collect();
                points.sort_by(|left, right| left.0.total_cmp(&right.0));
                let at = |row: f64| -> f64 {
                    let whole = (row as usize).min(length);
                    if whole == length {
                        return row_times[length];
                    }
                    row_times[whole] + (row - whole as f64) * (row_times[whole + 1] - row_times[whole])
                };
                let channel = default_route.channel;
                for (index, (row, value)) in points.iter().copied().enumerate() {
                    writer.append(at(row), vec![0xb0 | channel, cc, cc_value(target, value, default_route.instrument)], 1);
                    if lane.interpolation == Some(Interpolation::Linear) {
                        if let Some((next_row, next_value)) = points.get(index + 1).copied().filter(|(next_row, _)| *next_row > row) {
                            let mut sample = (row * 4.0).floor() / 4.0 + 0.25;
                            while sample < next_row {
                                let interpolated = value + (next_value - value) * (sample - row) / (next_row - row);
                                writer.append(at(sample), vec![0xb0 | channel, cc, cc_value(target, interpolated, default_route.instrument)], 1);
                                sample += 0.25;
                            }
                        }
                    }
                }
            }
            cursor = row_times[length];
            if writer.events.len() + notes.len() * 2 > 2_000_000 {
                return invalid("MIDI export exceeds the two-million-event safety limit.");
            }
        }
        // The engine releases all notes when a finite song ends. Receivers pair a note-off with the latest
        // note-on, so a same-pitch note on one channel ends where the next begins (as live playback does);
        // notes sharing an onset tick become one.
        let mut sounding: Vec<OutputNote> = notes.into_iter().filter(|note| note.start < cursor).collect();
        sounding.sort_by(|left, right| (left.channel, left.pitch).cmp(&(right.channel, right.pitch)).then(left.start.round().total_cmp(&right.start.round())));
        let mut emitted: Vec<OutputNote> = Vec::with_capacity(sounding.len());
        for note in sounding {
            if let Some(last) = emitted.last_mut().filter(|last| last.channel == note.channel && last.pitch == note.pitch) {
                if last.start.round() == note.start.round() {
                    last.end = last.end.max(note.end);
                    last.velocity = last.velocity.max(note.velocity);
                    continue;
                }
                last.end = last.end.min(note.start);
            }
            emitted.push(note);
        }
        for note in &emitted {
            writer.append(note.start, vec![0x90 | note.channel, note.pitch as u8, note.velocity as u8], 2);
            writer.append((note.start + 1.0).max(cursor.min(note.end)), vec![0x80 | note.channel, note.pitch as u8, 0], 0);
        }
        note_count += emitted.len();
        let end = cursor.round() as i64;
        total_ticks = total_ticks.max(end);
        output_tracks.push(encode_track(writer.events, &track.display_name(track_index), end, false)?);
    }
    if output_tracks.is_empty() {
        warnings.insert("All tracks were muted; the MIDI file contains only tempo and meter.");
    }
    conductor.push(WriteEvent { tick: total_ticks, priority: 3, order: 2, bytes: vec![0xff, 0x2f, 0] });
    let conductor_bytes = encode_track(conductor, project.name.as_deref().unwrap_or("SuperTracker"), total_ticks, true)?;
    let mut bytes = b"MThd".to_vec();
    bytes.extend(big_endian(6, 4));
    bytes.extend(big_endian(1, 2));
    bytes.extend(big_endian(output_tracks.len() + 1, 2));
    bytes.extend(big_endian(ppqn, 2));
    for track in std::iter::once(&conductor_bytes).chain(output_tracks.iter()) {
        bytes.extend(b"MTrk");
        bytes.extend(big_endian(track.len(), 4));
        bytes.extend(track);
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return invalid("The exported MIDI file exceeds 64 MiB.");
    }
    Ok(MidiExport { bytes, note_count, track_count: output_tracks.len(), ticks_per_quarter: EXPORT_PPQN, warnings: warnings.into_iter().map(str::to_string).collect() })
}

/// Exports to `path` (written atomically) and returns the export.
pub fn export_file(project: &Project, path: &Path) -> Result<MidiExport> {
    let result = export(project)?;
    write_atomic(path, &result.bytes)?;
    Ok(result)
}

//! Standard MIDI File import and export: the fixtures are built byte by byte here and exported
//! files are decoded by an independent reader below.

mod common;

use serde_json::{json, Value};
use st_project::midi::{export, import, EXPORT_PPQN};
use st_project::model::pattern::StepEffectKind;
use st_project::Project;

fn be(value: usize, count: usize) -> Vec<u8> {
    (0..count).rev().map(|index| (value >> (index * 8)) as u8).collect()
}

fn variable(value: usize) -> Vec<u8> {
    let mut remaining = value;
    let mut bytes = vec![(remaining & 0x7f) as u8];
    remaining >>= 7;
    while remaining > 0 {
        bytes.insert(0, (remaining & 0x7f) as u8 | 0x80);
        remaining >>= 7;
    }
    bytes
}

fn file_with(format: usize, division: usize, tracks: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"MThd".to_vec();
    bytes.extend(be(6, 4));
    bytes.extend(be(format, 2));
    bytes.extend(be(tracks.len(), 2));
    bytes.extend(be(division, 2));
    for track in tracks {
        bytes.extend(b"MTrk");
        bytes.extend(be(track.len(), 4));
        bytes.extend(track);
    }
    bytes
}

fn file(format: usize, tracks: &[Vec<u8>]) -> Vec<u8> {
    file_with(format, 480, tracks)
}

fn load(bytes: &[u8]) -> Project {
    let project = import(bytes, "Fixture", "/fixtures/Fixture.mid").unwrap();
    project.validate().unwrap_or_else(|error| panic!("an imported project must validate: {error}"));
    project
}

fn warnings(project: &Project) -> Vec<String> {
    project.midi_import.as_ref().unwrap().warnings.clone().unwrap()
}

/// A note of an imported project: where it is and what it holds.
#[derive(Debug, Clone, PartialEq)]
struct Located {
    pattern: usize,
    track: usize,
    row: usize,
    column: usize,
    note: i64,
    gate: f64,
}

fn notes(project: &Project) -> Vec<Located> {
    let mut result = Vec::new();
    for (pattern, item) in project.pattern_list().iter().enumerate() {
        for (track, rows) in item.steps.iter().flatten().enumerate() {
            for (row, step) in rows.iter().enumerate() {
                let Some(step) = step else { continue };
                if step.note() >= 0 {
                    result.push(Located { pattern, track, row, column: 0, note: step.note(), gate: step.gate() });
                }
                for (index, cell) in step.columns.iter().flatten().enumerate() {
                    if let Some(cell) = cell.as_ref().filter(|cell| cell.note() >= 0) {
                        result.push(Located { pattern, track, row, column: index + 1, note: cell.note(), gate: cell.gate() });
                    }
                }
            }
        }
    }
    result
}

fn note(project: &Project, pitch: i64) -> Located {
    notes(project).into_iter().find(|located| located.note == pitch).unwrap_or_else(|| panic!("note {pitch} was not imported"))
}

fn near(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= 0.0001
}

#[derive(Debug, Clone, PartialEq)]
struct Event {
    tick: usize,
    status: u8,
    /// Channel events: the data bytes. Meta events: the type, then the payload.
    data: Vec<u8>,
}

struct Decoded {
    format: usize,
    track_count: usize,
    division: usize,
    events: Vec<Event>,
}

/// An independent reader for exported files (no running status: the writer never uses it).
fn decode(bytes: &[u8]) -> Decoded {
    assert_eq!(&bytes[..4], b"MThd");
    let number = |range: std::ops::Range<usize>| bytes[range].iter().fold(0usize, |value, byte| (value << 8) | *byte as usize);
    let (format, track_count, division) = (number(8..10), number(10..12), number(12..14));
    let mut events = Vec::new();
    let mut position = 14;
    while position < bytes.len() {
        assert_eq!(&bytes[position..position + 4], b"MTrk");
        let end = position + 8 + number(position + 4..position + 8);
        position += 8;
        let mut tick = 0;
        while position < end {
            let mut delta = 0;
            loop {
                let byte = bytes[position];
                position += 1;
                delta = (delta << 7) | (byte & 0x7f) as usize;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            tick += delta;
            let status = bytes[position];
            position += 1;
            if status == 0xff {
                let kind = bytes[position];
                let length = bytes[position + 1] as usize;
                assert!(length < 0x80 || kind == 3, "long meta events are only track names");
                let (length, start) = if length < 0x80 { (length, position + 2) } else { (((length & 0x7f) << 7) | bytes[position + 2] as usize, position + 3) };
                let mut data = vec![kind];
                data.extend(&bytes[start..start + length]);
                position = start + length;
                events.push(Event { tick, status, data });
            } else {
                assert!(status >= 0x80, "the writer does not use running status");
                let length = if matches!(status & 0xf0, 0xc0 | 0xd0) { 1 } else { 2 };
                events.push(Event { tick, status, data: bytes[position..position + length].to_vec() });
                position += length;
            }
        }
        assert_eq!(position, end);
    }
    events.sort_by_key(|event| event.tick);
    Decoded { format, track_count, division, events }
}

/// The project of a test: `value` with what every project has and these tests leave out (the
/// format version, a routing table, a routing identity for each track).
fn project(mut value: Value) -> Project {
    value["version"] = json!(st_project::model::FORMAT_VERSION);
    if value.get("routing").is_none() {
        value["routing"] = json!({});
    }
    for (index, track) in value["tracks"].as_array_mut().into_iter().flatten().enumerate() {
        if track.get("routingID").is_none() {
            track["routingID"] = json!(format!("t{index}"));
        }
    }
    let project = Project::from_value(value).unwrap();
    project.validate().expect("a valid project");
    project
}

/// One track with `rows` as its steps in one pattern.
fn one_track(name: &str, rows: Vec<Value>, track: Value, instrument: Value) -> Project {
    project(json!({ "name": name, "bpm": 120.0, "linesPerBeat": 4, "tracks": [track], "instruments": [instrument],
        "patterns": [{ "id": 0, "name": "A", "length": rows.len(), "steps": [rows], "automation": [] }], "sequence": [0] }))
}

fn rows(length: usize, steps: &[(usize, Value)]) -> Vec<Value> {
    let mut rows = vec![Value::Null; length];
    for (row, step) in steps {
        rows[*row] = step.clone();
    }
    rows
}

/// Ticks of the note-on and note-off events of `pitch`, in order.
fn span(decoded: &Decoded, pitch: u8) -> Vec<usize> {
    decoded.events.iter().filter(|event| matches!(event.status & 0xf0, 0x80 | 0x90) && event.data[0] == pitch).map(|event| event.tick).collect()
}

fn note_ons(decoded: &Decoded) -> Vec<&Event> {
    decoded.events.iter().filter(|event| event.status & 0xf0 == 0x90 && event.data[1] > 0).collect()
}

const END: [u8; 4] = [0x00, 0xff, 0x2f, 0x00];

#[test]
fn running_status_and_zero_velocity() {
    let project = load(&file(0, &[[&[0x00, 0x90, 60, 100, 0x78, 64, 90, 0x78, 60, 0, 0x78, 64, 0][..], &END].concat()]));
    assert_eq!(notes(&project).len(), 2, "velocity-zero note-ons end notes, they do not create them");
    let (low, high) = (note(&project, 60), note(&project, 64));
    assert_eq!((low.row, high.row), (0, 1), "running status keeps note timing on the expected rows");
    assert!(near(low.gate, 2.0) && near(high.gate, 2.0), "running-status note-offs keep two-row gates");
    assert_eq!((project.bpm(), project.lines_per_beat()), (120.0, 4), "a file without a tempo event uses MIDI's 120 BPM default");
    assert_eq!(project.display_name(), "Fixture");
    assert!(project.routing.is_some() && project.track_list()[0].routing_id.is_some(), "an import is a project in the current layout");
}

#[test]
fn type_one_metadata_and_routing() {
    let conductor = [&[0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, 0x00, 0xff, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08][..], &END].concat();
    let instrument = [&[0x00, 0xc2, 10, 0x00, 0xb2, 7, 100, 0x00, 0xb2, 10, 32, 0x78, 0x92, 67, 104, 0x81, 0x70, 0x82, 67, 64][..], &END].concat();
    let project = load(&file(1, &[conductor, instrument]));
    let found = note(&project, 67);
    assert!(found.row == 1 && near(found.gate, 2.0), "type 1 note ticks resolve against the conductor tempo");
    let routed = project.instrument_list().iter().find(|instrument| instrument.midi_channel == Some(3) && instrument.midi_program == Some(10)).expect("channel 3, program 10 are retained");
    assert!(routed.sample_path.as_deref().unwrap().starts_with("builtin:"), "imported programs get audible built-in placeholders");
    assert_eq!(project.meter(), (3, 4), "the project shows the initial meter");
    let metadata = project.midi_import.as_ref().unwrap();
    assert_eq!((metadata.format, metadata.ticks_per_quarter, metadata.source.as_deref()), (Some(1), Some(480), Some("/fixtures/Fixture.mid")));
    assert!(metadata.tempo_changes.as_ref().unwrap().iter().any(|tempo| tempo.tick == Some(0) && tempo.microseconds_per_quarter == Some(500_000)));
    assert!(metadata.time_signatures.as_ref().unwrap().iter().any(|signature| signature.numerator == Some(3) && signature.denominator == Some(4)));
    assert!(metadata.program_changes.as_ref().unwrap().iter().any(|change| change.channel == Some(3) && change.program == Some(10)));
    let controllers = metadata.controllers.as_ref().unwrap();
    assert!(controllers.iter().any(|event| event.controller == Some(7) && event.value == Some(100) && event.channel == Some(3)));
    assert!(controllers.iter().any(|event| event.controller == Some(10) && event.value == Some(32) && event.channel == Some(3)));
    assert!(metadata.warnings.is_some(), "import reports expose their warnings");
    // CC 7 and 10 became volume and pan automation on the channel's track.
    let lanes = project.pattern_list()[0].automation.as_ref().unwrap();
    let pan = lanes.iter().find(|lane| lane.target_name() == "pan").unwrap();
    assert!(near(pan.points.as_ref().unwrap()[0].value(), -0.5) && lanes.iter().any(|lane| lane.target_name() == "volume"));
}

#[test]
fn chords_and_microtiming() {
    let project = load(&file(0, &[[&[0x1e, 0x90, 60, 100, 0x00, 0x90, 64, 100, 0x00, 0x90, 67, 100, 0x78, 0x80, 60, 64, 0x00, 0x80, 64, 64, 0x00, 0x80, 67, 64][..], &END].concat()]));
    assert_eq!(notes(&project).len(), 1, "simultaneous equal notes of one channel share a chord step");
    let step = project.pattern_list()[0].step(0, 0).expect("sub-row timing belongs on the original row");
    assert_eq!(step.chord, Some(vec![0, 4, 7]));
    let effect = step.fx1.as_ref().unwrap();
    assert!(effect.kind == Some(StepEffectKind::Microtiming) && near(effect.value(), 0.25), "thirty ticks at 480 PPQN are a quarter row");
    assert!(near(step.gate(), 1.0 / 0.75), "a microtimed gate accounts for the shortened row");
}

#[test]
fn tempo_changes_preserve_elapsed_time() {
    let conductor = [&[0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, 0x83, 0x60, 0xff, 0x51, 0x03, 0x0f, 0x42, 0x40][..], &END].concat();
    let instrument = [&[0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 64, 0x00, 0x90, 64, 100, 0x83, 0x60, 0x80, 64, 64][..], &END].concat();
    let project = load(&file(1, &[conductor, instrument]));
    let (first, second) = (note(&project, 60), note(&project, 64));
    assert_eq!(project.bpm(), 120.0, "flattening keeps the initial tempo");
    assert!(first.row == 0 && near(first.gate, 4.0), "a quarter note at 120 BPM spans four rows");
    assert!(second.row == 4 && near(second.gate, 8.0), "a quarter note at 60 BPM keeps its one second");
    assert_eq!(project.midi_import.as_ref().unwrap().tempo_changes.as_ref().unwrap().len(), 2);
    assert!(warnings(&project).iter().any(|warning| warning.starts_with("Tempo changes were converted to one tracker tempo")));
}

#[test]
fn sustain_pedal_extends_gate() {
    let project = load(&file(0, &[[
        &[0x00, 0x90, 60, 100, 0x00, 0xb0, 64, 127, 0x78, 0x80, 60, 64, 0x81, 0x70, 0xb0, 64, 0, 0x00, 0x90, 64, 100, 0x00, 0xb0, 64, 63, 0x78, 0x80, 64, 64][..],
        &END,
    ]
    .concat()]));
    let (held, released) = (note(&project, 60), note(&project, 64));
    assert!(held.row == 0 && near(held.gate, 3.0), "a sustained note continues until the pedal is released");
    assert!(released.row == 3 && near(released.gate, 1.0), "CC 64 below 64 leaves note-off timing intact");
}

#[test]
fn coincident_notes_retain_different_gates() {
    let project = load(&file(0, &[[&[0x00, 0x90, 60, 100, 0x00, 0x90, 64, 100, 0x78, 0x80, 60, 64, 0x78, 0x80, 64, 64][..], &END].concat()]));
    let (short, long) = (note(&project, 60), note(&project, 64));
    assert!(notes(&project).len() == 2 && short.track == long.track && short.column == 0 && long.column == 1, "different note-offs use note columns of one track");
    assert_eq!((project.track_list().len(), project.track_list()[0].note_columns), (1, Some(2)), "the track gets the note columns it needs");
    assert!(short.row == 0 && long.row == 0 && near(short.gate, 1.0) && near(long.gate, 2.0), "each note keeps its onset and its own duration");
}

#[test]
fn controllers_carry_across_patterns() {
    let mut track = vec![0x00, 0x90, 60, 100, 0x78, 0x80, 60, 64, 0x00, 0xb0, 7, 32];
    track.extend([variable(64 * 120), vec![0x90, 64, 100], variable(120), vec![0x80, 64, 64], variable(120), vec![0xb0, 7, 100]].concat());
    track.extend([variable(62 * 120), vec![0x90, 67, 100], variable(120), vec![0x80, 67, 64], END.to_vec()].concat());
    let project = load(&file(0, &[track]));
    assert!(project.pattern_list().len() >= 3);
    let volume = |pattern: usize| -> Vec<(f64, f64)> {
        let lanes = project.pattern_list()[pattern].automation.as_ref().unwrap();
        lanes.iter().find(|lane| lane.target_name() == "volume").map(|lane| lane.points.iter().flatten().map(|point| (point.row(), point.value())).collect()).unwrap_or_default()
    };
    let has = |pattern: usize, row: f64, value: f64| volume(pattern).iter().any(|(r, v)| near(*r, row) && near(*v, value));
    assert!(has(0, 1.0, 32.0 / 127.0), "the CC 7 event stays at its source row");
    assert!(has(1, 0.0, 32.0 / 127.0), "the next pattern starts with the previous pattern's last value");
    assert!(has(1, 3.0, 100.0 / 127.0), "a later change keeps its row after the carried value");
    assert!(has(2, 0.0, 100.0 / 127.0), "a pattern without controllers inherits the last state");
}

/// Gate 0 (no gate): a note lasts until the next note on its track, an OFF/CUT or the song end.
/// A missing gate is the default 0.8 rows.
#[test]
fn gateless_notes_export() {
    let steps = rows(8, &[
        (0, json!({ "note": 60, "velocity": 100, "gate": 0, "chord": [0, 4] })),
        (2, json!({ "note": 62, "velocity": 100, "gate": 0, "fx1": { "type": "microtiming", "value": 0.5 } })),
        (3, json!({ "note": 64, "velocity": 100, "gate": 0, "fx1": { "type": "probability", "value": 0 } })),
        (4, json!({ "note": -1 })),
        (5, json!({ "note": 65, "velocity": 100 })),
        (6, json!({ "note": 67, "velocity": 100, "gate": 0 })),
    ]);
    let decoded = decode(&export(&one_track("Gateless", steps, json!({ "id": 0, "name": "Lead", "instrument": 0 }), json!({ "id": 0, "name": "Lead", "type": "sample" }))).unwrap().bytes);
    let row = decoded.division / 4;
    assert_eq!((decoded.division, decoded.format), (usize::from(EXPORT_PPQN), 1));
    assert!(span(&decoded, 60) == [0, 2 * row + row / 2] && span(&decoded, 64) == [0, 2 * row + row / 2], "a gateless chord lasts until the next (microtimed) note starts");
    assert_eq!(span(&decoded, 62), [2 * row + row / 2, 4 * row], "a dropped probability note leaves the gateless note sounding until OFF");
    assert_eq!(span(&decoded, 65), [5 * row, 5 * row + row * 4 / 5], "a step without a gate has the default 0.8-row gate");
    assert_eq!(span(&decoded, 67), [6 * row, 8 * row], "a gateless note at the end lasts until the song end");
}

#[test]
fn sticky_ratchet_export() {
    let all = rows(9, &[
        (0, json!({ "note": 60, "gate": 0.5, "chord": [0, 7], "fx1": { "type": "ratchet", "value": 2 } })),
        (2, json!({ "note": 64, "gate": 0.5 })),
        (3, json!({ "note": -3, "fx1": { "type": "ratchet", "value": 4 } })),
        (4, json!({ "note": -3, "fx1": { "type": "pan", "value": 0.5 } })),
        (6, json!({ "note": -3, "fx1": { "type": "ratchet", "value": 2 } })),
        (7, json!({ "note": -1 })),
    ]);
    let song = project(json!({ "bpm": 120, "linesPerBeat": 4, "tracks": [{ "instrument": 0 }], "instruments": [{ "midiChannel": 1 }],
        "patterns": [{ "id": 0, "length": 1, "steps": [all[..1]] }, { "id": 1, "length": 8, "steps": [all[1..]] }], "sequence": [0, 1] }));
    let decoded = decode(&export(&song).unwrap().bytes);
    let on = note_ons(&decoded);
    assert_eq!(on.iter().map(|event| event.tick).collect::<Vec<_>>(), [0, 0, 120, 120, 240, 240, 360, 360, 480, 720, 780, 840, 900, 1440, 1560], "sticky ratchets, a plain note stopping them, effect cancellation, an effect-only restart and OFF");
    assert_eq!(on[..8].iter().map(|event| event.data[0]).collect::<Vec<_>>(), [60, 67, 60, 67, 60, 67, 60, 67], "repeated rows keep the original chord");
    assert!(on[8..].iter().all(|event| event.data[0] == 64), "a later note replaces the held ratchet chord");
}

/// Extra note columns are independent voices. Import puts overlapping notes into note columns (up
/// to eight per track) and needs another track beyond that.
#[test]
fn note_columns_export_and_import() {
    let steps = rows(8, &[
        (0, json!({ "note": 60, "velocity": 100, "gate": 0, "columns": [{ "note": 64, "velocity": 90, "gate": 0 }, { "note": 67, "velocity": 80, "gate": 1 }] })),
        (2, json!({ "note": 62, "velocity": 100, "gate": 0, "fx1": { "type": "ratchet", "value": 2 } })),
        (3, json!({ "note": -3, "columns": [{ "note": -1 }] })),
        (4, json!({ "note": -1, "columns": [null, { "note": 72, "velocity": 100, "gate": 0 }, { "note": 76, "velocity": 100, "gate": 0 }] })),
    ]);
    let exported = export(&one_track("Columns", steps, json!({ "id": 0, "name": "Keys", "instrument": 0, "noteColumns": 3 }), json!({ "id": 0, "name": "Keys", "type": "sample" }))).unwrap();
    let decoded = decode(&exported.bytes);
    let row = decoded.division / 4;
    assert_eq!(span(&decoded, 60), [0, 2 * row], "column 0's note lasts until column 0's next note");
    assert_eq!(span(&decoded, 64), [0, 3 * row], "column 1's note ignores column 0's new note and ends at column 1's OFF");
    assert_eq!(span(&decoded, 67), [0, row], "column 2 keeps its own gate");
    assert!(span(&decoded, 62).len() == 4 && span(&decoded, 62)[0] == 2 * row, "column 0's ratchet note repeats");
    assert_eq!(span(&decoded, 72), [4 * row, 8 * row], "column 2 plays on through column 0's OFF");
    assert!(span(&decoded, 76).is_empty(), "a column beyond the track's note columns is not exported");

    let reimported = load(&exported.bytes);
    let first_row: Vec<Located> = notes(&reimported).into_iter().filter(|located| located.row == 0).collect();
    assert!(first_row.len() == 3 && first_row.iter().all(|located| located.track == first_row[0].track), "overlapping notes reimport into one track's note columns");
    assert!(first_row.iter().any(|located| located.column == 2 && near(located.gate, 1.0)), "a reimported column keeps its gate");

    // Eight overlapping notes of different lengths fill one track's note columns; a ninth needs a second track.
    let mut track = Vec::new();
    for index in 0..9u8 {
        track.extend([0x00, 0x90, 48 + index, 100]);
    }
    for index in 0..9u8 {
        track.extend([variable(120), vec![0x80, 48 + index, 64]].concat());
    }
    track.extend(END);
    let dense = load(&file(0, &[track]));
    let tracks = dense.track_list();
    assert!(tracks.len() == 2 && tracks[0].note_columns == Some(8) && tracks[1].note_columns.is_none(), "eight note columns per track, then another track");
    assert_eq!(notes(&dense).len(), 9, "every note is imported");
}

/// Receivers pair note-offs with the latest note-on, so a long note overlapping the same pitch on
/// its channel ends where the next begins; identical onsets become one note.
#[test]
fn overlapping_same_pitch_notes_end_at_next_onset() {
    let steps = rows(8, &[
        (0, json!({ "note": 60, "velocity": 100, "gate": 4.0 })),
        (1, json!({ "note": 60, "velocity": 90, "gate": 1.0 })),
        (4, json!({ "note": 62, "velocity": 100, "gate": 2.0, "chord": [0, 0, 12] })),
    ]);
    let exported = export(&one_track("Overlap", steps, json!({ "id": 0, "name": "Lead", "instrument": 0 }), json!({ "id": 0, "name": "Lead", "type": "sample" }))).unwrap();
    let decoded = decode(&exported.bytes);
    let row = decoded.division / 4;
    for pitch in [60u8, 62, 74] {
        let mut sounding = false;
        for event in decoded.events.iter().filter(|event| matches!(event.status & 0xf0, 0x80 | 0x90) && event.data[0] == pitch) {
            let on = event.status & 0xf0 == 0x90 && event.data[1] > 0;
            assert_ne!(on, sounding, "pitch {pitch} alternates note-on and note-off (tick {})", event.tick);
            sounding = on;
        }
        assert!(!sounding, "pitch {pitch} ends");
    }
    assert_eq!(span(&decoded, 60), [0, row, row, 2 * row], "the long note ends at the next onset; the second keeps its one-row gate");
    assert_eq!(exported.note_count, 4, "duplicate chord voices at one onset export as one note");
}

#[test]
fn sequence_export_and_roundtrip() {
    let song = project(json!({
        "name": "Sequence test", "bpm": 120.0, "linesPerBeat": 4,
        "tracks": [{ "id": 0, "name": "Keys", "instrument": 0, "volume": 0.8, "pan": 0.0 }],
        "instruments": [{ "id": 0, "name": "Keys", "type": "sample", "midiChannel": 3, "midiProgram": 10 }],
        "patterns": [
            { "id": 0, "name": "Chord", "length": 8, "steps": [rows(8, &[(3, json!({ "note": 60, "instrument": 0, "velocity": 100, "gate": 2.0, "chord": [0, 4, 7] }))])], "automation": [] },
            { "id": 1, "name": "Bass", "length": 4, "steps": [rows(4, &[(0, json!({ "note": 48, "instrument": 0, "velocity": 80, "gate": 1.0 }))])], "automation": [] }
        ],
        "sequence": [1, 0, 1]
    }));
    let exported = export(&song).unwrap();
    assert_eq!((exported.note_count, exported.track_count, exported.ticks_per_quarter, exported.warnings.len()), (5, 1, 960, 0), "three chord voices and both repeated bass notes");
    let decoded = decode(&exported.bytes);
    let row = decoded.division / 4;
    assert_eq!(decoded.track_count, 2, "a conductor track and the note track");
    assert!(decoded.events.iter().any(|event| event.status == 0xff && event.data == [0x51, 0x07, 0xa1, 0x20]), "the 120 BPM tempo event");
    assert!(decoded.events.iter().any(|event| event.status == 0xff && event.data == [0x58, 4, 2, 24, 8]), "the 4/4 meter event");
    assert!(decoded.events.iter().any(|event| event.status == 0xff && event.data[0] == 3 && &event.data[1..] == b"Sequence test"), "the conductor is named after the project");
    let ons = note_ons(&decoded);
    assert!(ons.len() == 5 && ons.iter().all(|event| event.status & 0x0f == 2), "instrument MIDI channel 3 is status channel 2");
    assert_eq!(span(&decoded, 48).iter().step_by(2).copied().collect::<Vec<_>>(), [0, 12 * row], "the export follows the sequence, including repeats");
    for pitch in [60u8, 64, 67] {
        assert_eq!(span(&decoded, pitch), [7 * row, 9 * row], "chord row 3 follows the four-row pattern and keeps its two-row gate");
    }
    assert!(decoded.events.iter().any(|event| event.status == 0xc2 && event.data == [10]), "the instrument's program");
    assert!(decoded.events.iter().any(|event| event.status == 0xb2 && event.data[0] == 7), "the track's volume controller");

    let imported = load(&exported.bytes);
    assert_eq!(imported.bpm(), 120.0, "the tempo survives export and import");
    let chord = note(&imported, 60);
    assert!(chord.row == 7 && near(chord.gate, 2.0), "sequence position and chord duration survive");
    assert_eq!(imported.pattern_list()[0].step(chord.track, 7).unwrap().chord, Some(vec![0, 4, 7]));
    assert_eq!(notes(&imported).iter().filter(|located| located.note == 48).map(|located| located.row).collect::<Vec<_>>(), [0, 12]);
}

#[test]
fn explicit_controller_mapping_export() {
    let steps = rows(16, &[(0, json!({ "note": 60, "instrument": 0, "velocity": 100, "gate": 1.0 })), (12, json!({ "note": -3, "instrument": 0, "fx1": { "type": "cutoff", "value": 20.0 } }))]);
    let mut song = one_track("Mapped MIDI automation", steps, json!({ "id": 0, "name": "External synth", "instrument": 0, "volume": 0.8, "pan": 0.0 }),
        json!({ "id": 0, "name": "External synth", "type": "midi", "midiChannel": 5, "midiCCMap": { "cutoff": 74 }, "midiCCRanges": { "cutoff": [20.0, 20000.0] } })).to_value();
    song["patterns"][0]["automation"] = json!([{ "id": "cutoff", "track": 0, "target": "cutoff", "interpolation": "linear", "points": [{ "row": 0, "value": 20.0 }, { "row": 4, "value": 10010.0 }, { "row": 8, "value": 20000.0 }] }]);
    let exported = export(&project(song)).unwrap();
    let decoded = decode(&exported.bytes);
    let row = decoded.division / 4;
    let cutoff = |tick: usize, value: u8| decoded.events.iter().any(|event| event.status == 0xb4 && event.tick == tick && event.data == [74, value]);
    assert!(cutoff(0, 0), "the range minimum is CC 74 value 0 on channel 5");
    assert!(cutoff(4 * row, 64) && cutoff(8 * row, 127), "midpoint and maximum");
    assert!(cutoff(2 * row, 32), "linear automation includes intermediate values");
    assert!(cutoff(12 * row, 0), "a mapped step effect exports on a row without a note");
    assert!(exported.warnings.is_empty(), "mapped automation and step effects export without a warning: {:?}", exported.warnings);
}

#[test]
fn unmapped_content_is_reported_and_mute_solo_are_respected() {
    let lead = rows(4, &[(0, json!({ "note": 60, "gate": 1, "fx1": { "type": "cutoff", "value": 900 }, "fx2": { "type": "probability", "value": 0.5 } }))]);
    let mut song = json!({ "name": "Warnings", "bpm": 120, "tracks": [{ "name": "A", "routingID": "a" }, { "name": "B", "mute": true }, { "name": "C" }],
        "instruments": [{}], "patterns": [{ "id": 0, "length": 4, "steps": [lead.clone(), lead.clone(), lead],
            "automation": [{ "track": 0, "target": "cutoff", "points": [{ "row": 0, "value": 500 }] }, { "track": 0, "target": "volume", "interpolation": "step", "points": [{ "row": 2, "value": 1 }] }] }] });
    let exported = export(&project(song.clone())).unwrap();
    assert_eq!(exported.warnings, [
        "Automation without a MIDI CC mapping was omitted from MIDI export.",
        "Probabilistic steps were exported deterministically as notes; their random probability is not a MIDI event.",
        "Unmapped step FX (including sample processing and plugin controls) are not represented in MIDI.",
    ]);
    assert_eq!((exported.track_count, exported.note_count), (2, 2), "a muted track is left out");
    let decoded = decode(&exported.bytes);
    assert!(decoded.events.iter().any(|event| event.status == 0xb0 && event.tick == 480 && event.data == [7, 127]), "a mixer-channel lane of the track exports with it");
    song["tracks"][2]["solo"] = json!(true);
    assert_eq!(export(&project(song.clone())).unwrap().track_count, 1, "solo leaves only the soloed track");
    song["tracks"] = json!([{ "mute": true }]);
    song["patterns"][0]["steps"] = json!([[]]);
    let silent = export(&project(song)).unwrap();
    assert_eq!((silent.track_count, silent.warnings), (0, vec!["All tracks were muted; the MIDI file contains only tempo and meter.".to_string()]));
}

/// More than 16 tracks: one SMF track each, default channels repeat, and the file reimports whole.
#[test]
fn many_track_export() {
    let count = 40;
    let tracks: Vec<Value> = (0..count).map(|index| json!({ "id": index, "name": format!("T{index}"), "instrument": 0, "volume": 0.8, "pan": 0.0 })).collect();
    let steps: Vec<Vec<Value>> = (0..count).map(|track| rows(16, &[(0, json!({ "note": 36 + track, "instrument": 0, "velocity": 100, "gate": 1.0 }))])).collect();
    let mut song = json!({ "name": "Forty tracks", "bpm": 120.0, "linesPerBeat": 4, "tracks": tracks,
        "instruments": [{ "id": 0, "name": "Synth", "type": "sample", "midiChannel": null }], "patterns": [{ "id": 0, "name": "A", "length": 16, "steps": steps }], "sequence": [0] });
    let exported = export(&project(song.clone())).unwrap();
    let decoded = decode(&exported.bytes);
    assert!(decoded.format == 1 && decoded.track_count == count + 1, "a type 1 file with one MTrk per tracker track and a conductor");
    let on = note_ons(&decoded);
    assert_eq!(on.len(), count);
    assert!((0..count).all(|track| on.iter().any(|event| event.data[0] as usize == 36 + track)), "every track's note is exported");
    assert!(on.iter().any(|event| event.data[0] == 36 + 16 && event.status == 0x90) && on.iter().any(|event| event.data[0] == 36 + 39 && event.status == 0x97), "default channels repeat beyond 16 tracks");
    assert_eq!(load(&exported.bytes).track_list().len(), count, "all forty tracks reimport");
    // Too many tracks: not a project the host accepts, and not one that is exported.
    song["tracks"] = json!(vec![json!({ "instrument": 0 }); 1025]);
    assert_eq!(export(&Project::from_value(song).unwrap()).unwrap_err().to_string(), "MIDI export requires a valid tracker project with 1–1024 tracks and up to 256 patterns.");
}

#[test]
fn import_capacity_warnings() {
    let sustained = load(&file(0, &[[vec![0x00, 0x90, 60, 100], variable(17 * 120), vec![0x80, 60, 64], END.to_vec()].concat()]));
    assert!(near(note(&sustained, 60).gate, 17.0), "notes longer than a bar keep their full gate");
    assert!(!warnings(&sustained).iter().any(|warning| warning.contains("shortened")));
    let long = load(&file(0, &[[vec![0x00, 0x90, 60, 100], variable(257 * 120), vec![0x80, 60, 64], END.to_vec()].concat()]));
    assert!(near(note(&long, 60).gate, 256.0), "gates are bounded to the engine's 256 rows");
    assert!(warnings(&long).iter().any(|warning| warning.contains("256-row") && warning.contains("shortened")), "truncation is disclosed");
    let roundtrip = load(&export(&long).unwrap().bytes);
    assert!(near(note(&roundtrip, 60).gate, 256.0), "export and reimport keep a 256-row gate");

    let source_tracks: Vec<Vec<u8>> = (0..17u8).map(|index| [&[0x00, 0x90, 48 + index, 100, 0x78, 0x80, 48 + index, 64][..], &END].concat()).collect();
    let dense = load(&file(1, &source_tracks));
    let metadata = dense.midi_import.as_ref().unwrap();
    assert_eq!(dense.track_list().len(), 17, "there is no 16-track ceiling");
    assert_eq!((notes(&dense).len(), metadata.source_notes, metadata.imported_notes), (17, Some(17), Some(17)));
    assert!(!warnings(&dense).iter().any(|warning| warning.contains("omitted")));
    // What the tracker does not play is counted, not dropped silently.
    let bends = load(&file(0, &[[&[0x00, 0x90, 60, 100, 0x10, 0xe0, 0, 64, 0x10, 0xe0, 0, 80, 0x00, 0xd0, 5, 0x00, 0xb0, 1, 9, 0x40, 0x80, 60, 0][..], &END].concat()]));
    let found = warnings(&bends);
    for expected in ["Pitch-bend events remain unsupported by tracker MIDI import (2).", "Aftertouch events remain unsupported by tracker MIDI import (1).", "Unmapped MIDI controller events were retained in metadata only (1)."] {
        assert!(found.iter().any(|warning| warning == expected), "{expected} missing from {found:?}");
    }
}

#[test]
fn timeline_boundaries_and_trailing_silence() {
    let boundary = load(&file(0, &[[variable(63 * 120), vec![0x90, 60, 100, 0x78, 0x80, 60, 64], END.to_vec()].concat()]));
    assert_eq!(boundary.pattern_list().len(), 1, "a note ending exactly at row 64 does not create a second pattern");
    let last = note(&boundary, 60);
    assert!(last.row == 63 && near(last.gate, 1.0), "the last row keeps its whole duration");
    let silence = load(&file(0, &[[vec![0x00, 0x90, 60, 100, 0x78, 0x80, 60, 64], variable(127 * 120), vec![0xff, 0x2f, 0x00]].concat()]));
    assert_eq!(silence.pattern_list().len(), 2, "end-of-track timing keeps trailing silence through row 128");
    assert_eq!(silence.sequence, Some(vec![0, 1]));
}

#[test]
fn malformed_files() {
    let truncated_chunk = [b"MThd".to_vec(), be(6, 4), be(0, 2), be(1, 2), be(480, 2), b"MTrk".to_vec(), be(8, 4), END.to_vec()].concat();
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("overlong VLQ", file(0, &[[&[0x81, 0x80, 0x80, 0x80, 0x00][..], &END[1..]].concat()]), "A MIDI variable-length quantity exceeds four bytes."),
        ("missing running status", file(0, &[[&[0x00, 60, 100][..], &END].concat()]), "MIDI running status appears without a preceding channel status."),
        ("truncated chunk", truncated_chunk, "A MIDI track extends beyond the end of the file."),
        ("SMPTE time division", file_with(0, 0xe728, &[END.to_vec()]), "SMPTE-timed MIDI files are not supported. Export the source file with musical (PPQN) timing."),
        ("truncated channel event", file(0, &[vec![0x00, 0x90, 60]]), "The MIDI file ends inside an event."),
        ("invalid header", vec![0, 1, 2, 3], "This is not a Standard MIDI File (MThd header missing)."),
        ("type 2", file(2, &[END.to_vec()]), "Only synchronous SMF type 0 and type 1 MIDI files are supported; type 2 contains independent songs."),
        ("status in data", file(0, &[[&[0x00, 0x90, 0x90, 100][..], &END].concat()]), "A MIDI channel event contains a status byte where a data byte is required."),
        ("zero tempo", file(0, &[[&[0x00, 0xff, 0x51, 0x03, 0, 0, 0][..], &END].concat()]), "The MIDI tempo cannot be zero."),
        ("no notes", file(0, &[END.to_vec()]), "This MIDI file contains no note events to import."),
    ];
    for (name, bytes, message) in cases {
        assert_eq!(import(&bytes, name, "").unwrap_err().to_string(), message, "{name}");
    }
}

#[test]
fn files_on_disk() {
    let folder = common::TempDir::new("midi");
    let song = one_track("On disk", rows(4, &[(0, json!({ "note": 60, "gate": 1 }))]), json!({}), json!({}));
    let path = folder.join("Song name.mid");
    let exported = st_project::midi::export_file(&song, &path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), exported.bytes);
    let imported = st_project::midi::import_file(&path).unwrap();
    assert_eq!((imported.display_name(), imported.midi_import.as_ref().unwrap().source.as_deref()), ("Song name", Some(&*path.to_string_lossy())));
    assert!(st_project::midi::import_file(&folder.join("absent.mid")).unwrap_err().to_string().starts_with("Could not open absent.mid"));
}

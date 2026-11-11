//! The MIDI seam ([`st_audio::midi`]): what the render thread tells a MIDI output, checked with
//! a hook that writes everything down.

mod common;

use common::{engine, parse, ROW};
use serde_json::{json, Value};
use st_audio::midi::{MidiBlock, MidiOutput, MidiRender};
use st_audio::{AudioEngine, PlayOptions};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Block(MidiBlock),
    /// `(instrument, on, note, velocity, frame offset)` and the graph time of its block.
    Note(usize, bool, u8, u8, i32, u64),
    /// `(instrument, message, frame offset)`.
    Message(usize, [u8; 3], i32),
    AllNotesOff,
}

#[derive(Default)]
struct Log {
    seen: Mutex<Vec<Seen>>,
    hooks: AtomicUsize,
    dropped: AtomicUsize,
}

struct Hook {
    log: Arc<Log>,
    block: u64,
}

impl MidiRender for Hook {
    fn begin_block(&mut self, block: &MidiBlock) {
        self.block = block.time;
        self.log.seen.lock().unwrap().push(Seen::Block(*block));
    }
    fn note(&mut self, instrument: usize, on: bool, note: u8, velocity: u8, frame_offset: i32) -> bool {
        self.log.seen.lock().unwrap().push(Seen::Note(instrument, on, note, velocity, frame_offset, self.block));
        // Instrument 0 is the external MIDI instrument of the test project.
        instrument == 0
    }
    fn message(&mut self, instrument: usize, message: [u8; 3], frame_offset: i32) -> bool {
        self.log.seen.lock().unwrap().push(Seen::Message(instrument, message, frame_offset));
        instrument == 0
    }
    fn all_notes_off(&mut self) {
        self.log.seen.lock().unwrap().push(Seen::AllNotesOff);
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.log.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

struct Output(Arc<Log>);

impl MidiOutput for Output {
    fn render_hook(&self) -> Box<dyn MidiRender> {
        self.0.hooks.fetch_add(1, Ordering::SeqCst);
        Box::new(Hook { log: self.0.clone(), block: 0 })
    }
}

/// Track 0 plays an external MIDI instrument on rows 0 and 2; track 1 the built-in tone on row 0.
fn project() -> Value {
    let mut project = common::project(2);
    project["instruments"] = json!([
        { "id": 0, "type": "midi", "name": "Synth", "midiOutput": 7, "midiChannel": 2 },
        { "id": 1, "type": "sample", "samplePath": common::TONE, "mode": "forward", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.01 }
    ]);
    project["tracks"][1]["instrument"] = json!(1);
    let mut synth = vec![Value::Null; 16];
    synth[0] = json!({ "note": 60, "instrument": 0, "velocity": 100 });
    synth[2] = json!({ "note": 67, "instrument": 0, "velocity": 80 });
    let mut tone = vec![Value::Null; 16];
    tone[0] = json!({ "note": 57, "instrument": 1, "velocity": 100, "gate": 0 });
    project["patterns"][0]["steps"] = json!([synth, tone]);
    project
}

/// Renders `blocks` blocks of 512 frames through the engine's renderer, as a device would.
fn render(engine: &AudioEngine, blocks: usize) {
    let renderer = engine.renderer();
    let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
    for _ in 0..blocks {
        renderer.lock().unwrap().render(&mut left, &mut right);
    }
}

fn take(log: &Log) -> Vec<Seen> {
    std::mem::take(&mut *log.seen.lock().unwrap())
}

fn blocks(seen: &[Seen]) -> Vec<MidiBlock> {
    seen.iter().filter_map(|item| if let Seen::Block(block) = item { Some(*block) } else { None }).collect()
}

#[test]
fn the_render_thread_reports_blocks_notes_of_external_instruments_and_silence() {
    let log = Arc::new(Log::default());
    let engine = engine();
    engine.publish(&parse(&project())).unwrap();
    engine.set_midi_output(Some(Arc::new(Output(log.clone()))));
    assert_eq!(log.hooks.load(Ordering::SeqCst), 1, "the renderer in use gets a hook");

    // Stopped: every block is announced, with the project's tempo and a transport that stands still.
    render(&engine, 4);
    let seen = take(&log);
    let stopped = blocks(&seen);
    assert_eq!(seen.len(), 4, "nothing but blocks while stopped: {seen:?}");
    assert_eq!(stopped.iter().map(|block| (block.time, block.frames)).collect::<Vec<_>>(), [(0, 512), (512, 512), (1024, 512), (1536, 512)]);
    assert!(stopped.iter().all(|block| !block.playing && block.tempo == 120.0 && block.sample_rate == 48_000.0 && block.playback_delay.is_zero()), "{stopped:?}");

    // Playing: the beat at each block's first frame advances by the block's length, and the notes
    // of the external instrument arrive with their frame offset. The built-in instrument's do not.
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    render(&engine, 40);
    let seen = take(&log);
    let playing = blocks(&seen);
    assert!(playing.iter().all(|block| block.playing), "{playing:?}");
    let per_block = 512.0 / 48_000.0 * 120.0 / 60.0;
    for (index, block) in playing.iter().enumerate() {
        assert!((block.beat - index as f64 * per_block).abs() < 1e-6, "block {index} starts at beat {}, expected {}", block.beat, index as f64 * per_block);
    }
    let start = playing[0].time;
    let notes: Vec<(usize, bool, u8, u8, u64)> = seen
        .iter()
        .filter_map(|item| if let Seen::Note(instrument, on, note, velocity, offset, block) = item { Some((*instrument, *on, *note, *velocity, block + *offset as u64 - start)) } else { None })
        .collect();
    let ons: Vec<_> = notes.iter().filter(|note| note.1).collect();
    assert_eq!(ons, [&(0, true, 60, 100, 0), &(0, true, 67, 80, 2 * ROW as u64)], "note-ons at the frames of rows 0 and 2: {notes:?}");
    let offs: Vec<_> = notes.iter().filter(|note| !note.1).map(|note| (note.0, note.2)).collect();
    assert_eq!(offs, [(0, 60), (0, 67)], "each note is released: {notes:?}");
    assert!(notes.iter().all(|note| note.0 == 0), "the engine sounds its own instruments itself");
    assert!(!seen.contains(&Seen::AllNotesOff));

    // Stop and panic silence external instruments; an offline render reaches nothing.
    engine.stop();
    render(&engine, 2);
    assert_eq!(take(&log).iter().filter(|item| **item == Seen::AllNotesOff).count(), 1, "one all-notes-off when the transport stops");
    engine.panic();
    render(&engine, 2);
    assert_eq!(take(&log).iter().filter(|item| **item == Seen::AllNotesOff).count(), 1, "one all-notes-off on panic");
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    engine.render_offline(24_000, 512);
    assert_eq!(take(&log), [], "render_offline suspends the hook");
    render(&engine, 1);
    assert_eq!(blocks(&take(&log)).len(), 1, "and the hook is back afterwards");

    // Disconnecting drops the hook on the control thread.
    engine.set_midi_output(None);
    assert_eq!(log.dropped.load(Ordering::SeqCst), 1);
    render(&engine, 2);
    assert_eq!(take(&log), []);
}

#[test]
fn live_notes_and_channel_messages_of_an_external_instrument_leave_in_the_order_they_were_played() {
    let log = Arc::new(Log::default());
    let engine = engine();
    engine.publish(&parse(&project())).unwrap();
    engine.set_midi_output(Some(Arc::new(Output(log.clone()))));
    engine.note_on(0, 0, 60, 100);
    render(&engine, 1);
    take(&log);
    // Within one block: the key comes up, the pedal goes down, another key, a pitch bend, the
    // pedal comes up, the second key comes up. Every one of them is an event of the next
    // block, at its first frame, in this order: the pedal cannot catch the first note.
    engine.note_off(0, 60);
    assert!(engine.external_expression(0, [0xB3, 64, 127]), "a control change is taken (its channel is the instrument's to say)");
    engine.note_on(0, 0, 62, 90);
    assert!(engine.external_expression(0, [0xE0, 0, 96]));
    assert!(engine.external_expression(0, [0xB0, 64, 0]));
    engine.note_off(0, 62);
    assert!(!engine.external_expression(0, [0x90, 60, 100]), "notes are not channel messages");
    assert!(!engine.external_expression(-1, [0xB0, 64, 0]));
    render(&engine, 2);
    let seen: Vec<Seen> = take(&log).into_iter().filter(|item| !matches!(item, Seen::Block(_))).collect();
    let time = 512;
    assert_eq!(
        seen,
        [
            Seen::Note(0, false, 60, 0, 0, time),
            Seen::Message(0, [0xB0, 64, 127], 0),
            Seen::Note(0, true, 62, 90, 0, time),
            Seen::Message(0, [0xE0, 0, 96], 0),
            Seen::Message(0, [0xB0, 64, 0], 0),
            Seen::Note(0, false, 62, 0, 0, time),
        ]
    );
    // An offline render sends nothing to a device, and a message for an instrument that is
    // not external goes nowhere.
    engine.external_expression(1, [0xB0, 1, 64]);
    render(&engine, 1);
    assert_eq!(take(&log).into_iter().filter(|item| !matches!(item, Seen::Block(_))).collect::<Vec<_>>(), [Seen::Message(1, [0xB0, 1, 64], 0)], "the hook is asked, and answers that instrument 1 is not external");
    engine.set_midi_output(None);
}

#[test]
fn live_notes_know_whether_recording_is_armed_and_where_the_transport_is() {
    let engine = engine();
    engine.publish(&parse(&project())).unwrap();
    assert!(!engine.record_armed());
    engine.record_arm(true);
    assert!(engine.record_armed());
    let stopped = engine.transport_position();
    assert_eq!((stopped.pattern, stopped.row, stopped.playing), (0, 0, false));
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    engine.render_offline(3 * ROW + 100, 512);
    let playing = engine.transport_position();
    assert_eq!((playing.pattern, playing.row, playing.playing), (0, 3, true));
}

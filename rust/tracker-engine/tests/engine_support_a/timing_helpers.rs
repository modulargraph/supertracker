//! The fixtures and helpers of the former C++ program `engine_timing_tests.cpp`. Each function keeps the C++
//! helper's name (in snake case), statements and assertions.
//!
//! Standalone core regression suite: no plug-in, host graph or audio device. At 48 kHz the
//! two-tempo fixture maps beat b to 24000*b through b=.375, then to 48000*b-9000. All explicit
//! expectations in the suites are exact integers.

use super::tone::{load_test_tone, test_tone_instrument, TEST_SINE};
use super::{ControlHalf, Engine, RenderHalf};
use tracker_engine::clock::{ClockContext, ClockMeterEvent, ClockSpan, ClockTempoEvent, CLOCK_OK};
use tracker_engine::engine::*;

/// C++ `struct Note`: a note event with its absolute frame. Equality is the C++ `key()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub frame: u64,
    pub type_: i32,
    pub track: i32,
    pub instrument: i32,
    pub note: i32,
    pub velocity: i32,
}

/// C++ `struct Rendered`.
pub struct Rendered {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub notes: Vec<Note>,
}

impl Rendered {
    pub fn new(frames: usize) -> Rendered {
        Rendered { left: vec![0.0; frames], right: vec![0.0; frames], notes: Vec::new() }
    }
}

/// What the helpers need from either an [`Engine`] or the render half a test moved to its audio
/// thread.
pub trait RenderSide {
    fn pop(&mut self) -> Option<STEvent>;
    fn span(&mut self, frame: i64, requested: u32, latency: i64, out: &mut ClockSpan) -> i32;
    fn bus(&mut self, bus: i32, sample_time: u64, left: &mut [f32], right: &mut [f32]);
}

impl RenderSide for Engine {
    fn pop(&mut self) -> Option<STEvent> {
        self.pop_event()
    }
    fn span(&mut self, frame: i64, requested: u32, latency: i64, out: &mut ClockSpan) -> i32 {
        self.clock_span(frame, requested, latency, out)
    }
    fn bus(&mut self, bus: i32, sample_time: u64, left: &mut [f32], right: &mut [f32]) {
        self.render_bus_into(bus, sample_time, left, right);
    }
}

impl RenderSide for RenderHalf<'_> {
    fn pop(&mut self) -> Option<STEvent> {
        self.pop_event()
    }
    fn span(&mut self, frame: i64, requested: u32, latency: i64, out: &mut ClockSpan) -> i32 {
        self.clock_span(frame, requested, latency, out)
    }
    fn bus(&mut self, bus: i32, sample_time: u64, left: &mut [f32], right: &mut [f32]) {
        self.render_bus_into(bus, sample_time, left, right);
    }
}

/// C++ `drainNotes(engine,start,notes)`.
pub fn drain_notes(engine: &mut impl RenderSide, start: u64, notes: &mut Vec<Note>) {
    while let Some(event) = engine.pop() {
        assert!(event.frame_offset >= 0);
        if event.type_ == ST_EVENT_NOTE_ON || event.type_ == ST_EVENT_NOTE_OFF {
            notes.push(Note {
                frame: start + event.frame_offset as u64,
                type_: event.type_,
                track: event.track,
                instrument: event.instrument,
                note: event.note,
                velocity: event.velocity,
            });
        }
    }
}

/// C++ `render(engine,frames,block)` (default block 127): direct render in blocks, collecting
/// the note events of each block.
pub fn render(engine: &mut Engine, frames: usize, block: usize) -> Rendered {
    let mut result = Rendered::new(frames);
    let mut at = 0usize;
    while at < frames {
        let count = block.min(frames - at);
        engine.render_into(&mut result.left[at..at + count], &mut result.right[at..at + count]);
        drain_notes(engine, at as u64, &mut result.notes);
        at += block;
    }
    for at in 0..frames {
        assert!(result.left[at].is_finite() && result.right[at].is_finite());
    }
    assert!(engine.status().dropped_commands == 0);
    assert!(engine.status().dropped_events == 0);
    result
}

/// C++ `flush(engine)`: apply queued commands without rendering audio and discard the events.
pub fn flush(engine: &mut Engine) {
    engine.render_nothing();
    let mut ignored = Vec::new();
    drain_notes(engine, 0, &mut ignored);
}

/// C++ `noteFrames(notes,type)`.
pub fn note_frames(notes: &[Note], type_: i32) -> Vec<u64> {
    notes.iter().filter(|note| note.type_ == type_).map(|note| note.frame).collect()
}

/// C++ `expectFrames(notes,type,expected)` (which printed both lists and aborted).
#[track_caller]
pub fn expect_frames(notes: &[Note], type_: i32, expected: &[u64]) {
    let actual = note_frames(notes, type_);
    assert!(actual == expected, "Event {type_} frames: expected {expected:?}; actual {actual:?}");
}

/// C++ `setMap(engine,revision)` (default revision 1): 120 BPM in 4/4, then 60 BPM in 7/8 from
/// beat 0.375.
pub fn set_map(engine: &mut Engine, revision: u64) {
    let tempos = [ClockTempoEvent { beat: 0.0, bpm: 120.0 }, ClockTempoEvent { beat: 0.375, bpm: 60.0 }];
    let meters = [
        ClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 },
        ClockMeterEvent { beat: 0.375, numerator: 7, denominator: 8 },
    ];
    // C++ also asserted that the error buffer stayed empty; `Ok(())` carries no text.
    assert!(engine.set_timing_map(&tempos, &meters, revision) == Ok(()));
}

/// C++ `beginFixture(engine,type,rows,swing)` (defaults: 4 rows, no swing): an open update with
/// three raw host-mixed tracks, one pattern and the two-tempo map. `type < 0` is the sustained
/// sine test tone.
pub fn begin_fixture(engine: &mut Engine, type_: i32, rows: i32, swing: f32) {
    engine.begin_update();
    engine.clear_project();
    engine.set_track_count(3);
    engine.set_timing(120.0, 4, swing, 1.0);
    engine.set_host_mixing(1, 7);
    engine.set_pattern(0, rows);
    let sequence = [0];
    engine.set_sequence(&sequence, 0, 0);
    let mut instrument = if type_ < 0 { test_tone_instrument(TEST_SINE) } else { default_instrument() };
    if type_ >= 0 {
        instrument.type_ = type_;
    } else {
        load_test_tone(engine, 0, TEST_SINE);
    }
    instrument.attack = 0.0;
    instrument.decay = 0.0;
    instrument.sustain = 1.0;
    instrument.release = 0.005;
    instrument.volume = 0.2;
    engine.set_instrument(0, instrument);
    set_map(engine, 1);
}

/// C++ `putNote(engine,row,note,gate,track,instrument)` (defaults: track 0, instrument 0).
pub fn put_note_on(engine: &mut Engine, row: i32, note: i32, gate: f32, track: i32, instrument: i32) {
    let mut step = default_step();
    step.note = note;
    step.instrument = instrument;
    step.velocity = 100;
    step.gate = gate;
    engine.set_step(0, track, row, step, &[]);
}

/// C++ `putNote(engine,row,note,gate)`.
pub fn put_note(engine: &mut Engine, row: i32, note: i32, gate: f32) {
    put_note_on(engine, row, note, gate, 0, 0);
}

/// C++ `commit(engine)` (which printed the error and aborted).
#[track_caller]
pub fn commit(engine: &mut Engine) {
    if let Err(error) = engine.commit_update_checked() {
        panic!("Timing fixture commit failed: {error}");
    }
}

/// C++ `commit(engine)` on the control half of an engine whose render half runs on another
/// thread.
#[track_caller]
pub fn commit_control(control: &mut ControlHalf<'_>) {
    if let Err(error) = control.commit_update_checked() {
        panic!("Timing fixture commit failed: {error}");
    }
}

/// C++ `start(engine,songMode)` (default Song mode).
#[track_caller]
pub fn start(engine: &mut Engine, song_mode: i32) {
    commit(engine);
    engine.play(song_mode);
}

/// C++ `clockSpan`: the query, counted as a render-thread call.
pub fn clock_span(engine: &mut impl RenderSide, frame: i64, requested: u32, latency: i64, span: &mut ClockSpan) -> i32 {
    engine.span(frame, requested, latency, span)
}

/// C++ `preparedSpan(engine,frame,requested,latency)` (defaults: 64 frames, no latency).
#[track_caller]
pub fn prepared_span_at(engine: &mut impl RenderSide, frame: i64, requested: u32, latency: i64) -> ClockSpan {
    let mut span = ClockSpan::default();
    assert!(clock_span(engine, frame, requested, latency, &mut span) == CLOCK_OK);
    span
}

/// C++ `preparedSpan(engine,frame)`.
#[track_caller]
pub fn prepared_span(engine: &mut impl RenderSide, frame: i64) -> ClockSpan {
    prepared_span_at(engine, frame, 64, 0)
}

/// A span whose every field holds the C++ test's `memset(...,0x5a,...)` pattern.
fn sentinel_span() -> ClockSpan {
    const I64: i64 = 0x5a5a_5a5a_5a5a_5a5a;
    const U64: u64 = 0x5a5a_5a5a_5a5a_5a5a;
    const I32: i32 = 0x5a5a_5a5a;
    const U32: u32 = 0x5a5a_5a5a;
    let f64_ = f64::from_bits(U64);
    ClockSpan {
        context: ClockContext {
            revision: U64,
            epoch: U64,
            graph_frame: I64,
            content_graph_frame: I64,
            song_frame: I64,
            loop_iteration: I64,
            bar: I64,
            sample_rate: f64_,
            beat: f64_,
            bpm: f64_,
            bar_start_beat: f64_,
            bar_end_beat: f64_,
            numerator: I32,
            denominator: I32,
            flags: I32,
            next_boundary_frame: I64,
            next_boundary_kinds: U32,
            loop_start_beat: f64_,
            loop_end_beat: f64_,
            next_quarter_beat_frame: I64,
            next_quarter_grid_frame: I64,
        },
        frames: U32,
    }
}

/// C++ `expectClockError(engine,frame,latency,expected)`: the query fails with `expected` and
/// leaves the caller's span untouched.
#[track_caller]
pub fn expect_clock_error(engine: &mut impl RenderSide, frame: i64, latency: i64, expected: i32) {
    let mut untouched = sentinel_span();
    let before = untouched;
    assert!(clock_span(engine, frame, 64, latency, &mut untouched) == expected);
    assert!(before == untouched);
}

/// C++ `prepareBus(engine,startFrame,frames)`: pull bus 0 so that its clock history exists.
pub fn prepare_bus(engine: &mut impl RenderSide, start_frame: u64, frames: u32) {
    let mut left = vec![0.0f32; frames as usize];
    let mut right = vec![0.0f32; frames as usize];
    engine.bus(0, start_frame, &mut left, &mut right);
}

/// C++ `contextKey`: every field of the context, in the C++ order.
#[allow(clippy::type_complexity)]
pub fn context_key(
    c: &ClockContext,
) -> ((u64, u64, i64, i64, i64, i64, i64), (f64, f64, f64, f64, f64), (i32, i32, i32, i64, u32), (f64, f64, i64, i64)) {
    (
        (c.revision, c.epoch, c.graph_frame, c.content_graph_frame, c.song_frame, c.loop_iteration, c.bar),
        (c.sample_rate, c.beat, c.bpm, c.bar_start_beat, c.bar_end_beat),
        (c.numerator, c.denominator, c.flags, c.next_boundary_frame, c.next_boundary_kinds),
        (c.loop_start_beat, c.loop_end_beat, c.next_quarter_beat_frame, c.next_quarter_grid_frame),
    )
}

/// C++ `statusKey`: the status fields a read-only query must not change.
#[allow(clippy::type_complexity)]
pub fn status_key(
    s: &STStatus,
) -> ((i32, i32, i32, i32, i32, f32, f32), (f64, f64, u64, u64, i32), (f64, f64, f64, i64), (i32, i32, u64, i32)) {
    (
        (s.playing, s.pattern, s.row, s.sequence, s.track_count, s.peak_l, s.peak_r),
        (s.position, s.seconds, s.dropped_commands, s.dropped_events, s.song_mode),
        (s.beat, s.bpm, s.bar_start_beat, s.bar),
        (s.numerator, s.denominator, s.timing_revision, s.recording),
    )
}

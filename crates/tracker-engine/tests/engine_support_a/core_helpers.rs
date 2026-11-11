//! The fixtures and helpers that more than one of the engine core suites uses.

use super::tone::{load_test_tone, test_tone_instrument, TEST_SINE};
use super::Engine;
use std::f64::consts::PI;
use tracker_engine::engine::*;

/// The instrument type that makes `configure` set instrument 0 to a sustained sine (sampler in
/// Wavetable mode on one cycle).
pub const TONE: i32 = -1;

/// A stereo buffer.
#[derive(Clone)]
pub struct Audio {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl Audio {
    pub fn new(n: usize) -> Audio {
        Audio { l: vec![0.0; n], r: vec![0.0; n] }
    }

    /// Mean energy of frames `start..end`; an `end` of zero means the end of the buffer.
    pub fn energy_in(&self, start: usize, end: usize) -> f64 {
        let end = if end == 0 { self.l.len() } else { end };
        let mut sum = 0.0f64;
        for i in start..end {
            assert!(self.l[i].is_finite() && self.r[i].is_finite());
            sum += self.l[i] as f64 * self.l[i] as f64 + self.r[i] as f64 * self.r[i] as f64;
        }
        sum / (end - start) as f64
    }

    /// Mean energy of the whole buffer.
    pub fn energy(&self) -> f64 {
        self.energy_in(0, 0)
    }
}

/// A render of the buffer's length, as a render-thread call (see `Engine::render_into`).
pub fn render(e: &mut Engine, a: &mut Audio) {
    e.render_into(&mut a.l, &mut a.r);
}

/// One bus of the block at `time`, as long as `a` (at most 4096 frames): the block is rendered
/// when it follows the previous one and read again when it is the previous one.
pub fn bus_render(e: &mut Engine, bus: i32, time: u64, a: &mut Audio) {
    e.render_bus_into(bus, time, &mut a.l, &mut a.r);
}

/// Renders the length of `outs[0]` from `time` in blocks of at most 4096 frames and copies bus
/// `buses[n]` of each block into `outs[n]`. Events stay queued; their frame offsets count from
/// the start of their own block.
pub fn bus_render_many(e: &mut Engine, buses: &[i32], time: u64, outs: &mut [&mut Audio]) {
    let frames = outs[0].l.len();
    let mut at = 0;
    while at < frames {
        let count = 4096.min(frames - at);
        for (bus, out) in buses.iter().zip(outs.iter_mut()) {
            e.render_bus_into(*bus, time + at as u64, &mut out.l[at..at + count], &mut out.r[at..at + count]);
        }
        at += count;
    }
}

/// Two tracks at 120 BPM, 4 lines per beat, master 0.7. `TONE` (-1)
/// makes instrument 0 the sine test tone; any other value is the instrument type.
pub fn configure(e: &mut Engine, type_: i32) {
    e.begin_update();
    e.set_track_count(2);
    e.set_timing(120.0, 4, 0.0);
    let mut i = if type_ < 0 { test_tone_instrument(TEST_SINE) } else { default_instrument() };
    if type_ >= 0 {
        i.type_ = type_;
    } else {
        load_test_tone(e, 0, TEST_SINE);
    }
    i.attack = 0.0;
    i.decay = 0.001;
    i.sustain = 1.0;
    i.release = 0.01;
    i.volume = 0.5;
    e.set_instrument(0, i);
    e.commit_update();
}

/// An event with its absolute frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timed {
    pub frame: u64,
    pub e: STEvent,
}

/// Renders `frames` in blocks of `block` frames and returns the events with their frames.
pub fn render_events_block(e: &mut Engine, frames: usize, block: usize) -> Vec<Timed> {
    let mut result = Vec::new();
    let mut at = 0usize;
    while at < frames {
        let mut a = Audio::new(block.min(frames - at));
        render(e, &mut a);
        while let Some(v) = e.pop_event() {
            result.push(Timed { frame: at as u64 + v.frame_offset as u64, e: v });
        }
        at += block;
    }
    result
}

/// `render_events_block` in blocks of 256 frames.
pub fn render_events(e: &mut Engine, frames: usize) -> Vec<Timed> {
    render_events_block(e, frames, 256)
}

/// Only note-on and note-off events.
pub fn note_events(mut events: Vec<Timed>) -> Vec<Timed> {
    events.retain(|e| e.e.type_ == ST_EVENT_NOTE_ON || e.e.type_ == ST_EVENT_NOTE_OFF);
    events
}

/// The last value per frame of one native parameter.
pub fn parameter_changes_at(events: &[Timed], target: i32, track: i32, address: u64) -> Vec<Timed> {
    let mut result: Vec<Timed> = Vec::new();
    for event in events {
        if event.e.type_ == ST_EVENT_PARAMETER
            && event.e.target == target
            && event.e.track == track
            && event.e.address == address
        {
            match result.last_mut() {
                Some(last) if last.frame == event.frame => *last = *event,
                _ => result.push(*event),
            }
        }
    }
    result
}

/// `parameter_changes_at` for track 0, address 0.
pub fn parameter_changes(events: &[Timed], target: i32) -> Vec<Timed> {
    parameter_changes_at(events, target, 0, 0)
}

/// Only host-parameter events.
pub fn host_events(mut events: Vec<Timed>) -> Vec<Timed> {
    events.retain(|e| e.e.type_ == ST_EVENT_HOST_PARAMETER);
    events
}

/// Note 69 on a fresh engine with `instrument`, rendered in blocks.
pub fn modulation_audio(instrument: STInstrument, frames: usize, block: usize) -> Audio {
    let mut e = Engine::new();
    configure(&mut e, instrument.type_);
    e.begin_update();
    e.set_instrument(0, instrument);
    if instrument.type_ == ST_INSTRUMENT_SAMPLE {
        let mut sample = vec![0.0f32; 4096];
        for (n, value) in sample.iter_mut().enumerate().take(2048) {
            *value = (2.0 * PI * n as f64 / 2048.0).sin() as f32 * 0.4;
        }
        e.set_sample(0, &sample, 48000.0);
    }
    e.commit_update();
    e.note_on(0, 0, 69, 127);
    let mut result = Audio::new(frames);
    let mut n = 0usize;
    while n < frames {
        let count = block.min(frames - n);
        e.render_into(&mut result.l[n..n + count], &mut result.r[n..n + count]);
        n += block;
    }
    result
}

/// The sine test tone with an instant attack, full sustain and a 2-second release.
pub fn modulation_instrument() -> STInstrument {
    let mut p = test_tone_instrument(TEST_SINE);
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.release = 2.0;
    p.volume = 0.3;
    p
}

/// A forward sampler whose LFO 1 (slot 0) or LFO 2 is a
/// tempo-synced square on the pan.
pub fn synced_pan_instrument(slot: i32) -> STInstrument {
    let mut p = modulation_instrument();
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.mode = ST_MODE_FORWARD;
    if slot == 0 {
        p.lfo_sync = 1;
        p.lfo_depth = 1.0;
        p.lfo_target = ST_MOD_PAN;
        p.lfo_shape = ST_LFO_SQUARE;
    } else {
        p.lfo2_sync = 1;
        p.lfo2_depth = 1.0;
        p.lfo2_target = ST_MOD_PAN;
        p.lfo2_shape = ST_LFO_SQUARE;
    }
    p
}

/// `p` on a constant sample at `bpm`, with note 60 held.
pub fn configure_synced_lfo(e: &mut Engine, p: STInstrument, bpm: f64) {
    configure(e, ST_INSTRUMENT_SAMPLE);
    let sample = vec![0.2f32; 2048];
    e.begin_update();
    e.set_timing(bpm, 4, 0.0);
    e.set_instrument(0, p);
    e.set_sample(0, &sample, 48000.0);
    e.commit_update();
    e.note_on(0, 0, 60, 127);
}

/// The left channel starts at `expected` (within a frame)
/// while the right channel is silent there.
pub fn assert_pan_switch(a: &Audio, expected: usize) {
    let mut first = 0usize;
    while first < a.l.len() && a.l[first] == 0.0 {
        first += 1;
    }
    assert!((first as i64 - expected as i64).abs() <= 1);
    assert!(a.r[first] == 0.0 && a.l[first] > 0.0001f32);
    for n in first..(first + 100).min(a.l.len()) {
        assert!(a.r[n] == 0.0);
    }
}

/// Rows 0..15 each carry note 60+row; one row is 6000
/// frames at 120 BPM, 4 lines per beat.
pub fn configure_loop_music(e: &mut Engine, swing: f32) {
    configure(e, ST_INSTRUMENT_AU);
    e.begin_update();
    e.set_timing(120.0, 4, swing);
    e.set_pattern(0, 16);
    for r in 0..16 {
        let mut step = default_step();
        step.note = 60 + r;
        step.gate = 0.5;
        e.set_step(0, 0, r, step, &[]);
    }
    let sequence = [0];
    e.set_sequence(&sequence, 0, 0);
    e.commit_update();
}

/// The note-on events of a render of `frames`.
pub fn note_ons(e: &mut Engine, frames: usize) -> Vec<Timed> {
    let mut on = Vec::new();
    for v in render_events(e, frames) {
        if v.e.type_ == ST_EVENT_NOTE_ON {
            on.push(v);
        }
    }
    on
}

/// The first note-ons are exactly `expected` (frame, note).
pub fn assert_notes(on: &[Timed], expected: &[(u64, i32)]) {
    assert!(on.len() >= expected.len());
    for (n, &(frame, note)) in expected.iter().enumerate() {
        assert!(on[n].frame == frame);
        assert!(on[n].e.note == note);
    }
}

/// Pattern 0 = 16 rows (notes 60+row), pattern 1 = 4 rows
/// (notes 80+row); 6000 frames per row.
pub fn configure_queue_music(e: &mut Engine, order: &[i32]) {
    configure_loop_music(e, 0.0);
    e.begin_update();
    e.set_pattern(1, 4);
    for r in 0..4 {
        let mut step = default_step();
        step.note = 80 + r;
        step.gate = 0.5;
        e.set_step(1, 0, r, step, &[]);
    }
    e.set_sequence(order, 0, 0);
    e.commit_update();
}

/// A step effect.
pub fn fx(type_: i32, value: f32) -> STStepFX {
    STStepFX { type_, value }
}

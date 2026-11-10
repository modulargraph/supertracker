//! The fixtures and helpers of `tests/engine_tests.cpp` that more than one suite uses. Each
//! function keeps the C++ helper's name (in snake case), statements and assertions.

use super::tone::{load_test_tone, test_tone_instrument, TEST_SINE};
use super::Engine;
use std::f64::consts::PI;
use tracker_engine::clock::{STClockMeterEvent, STClockTempoEvent};
use tracker_engine::engine::*;

/// `configure(e)` without a type argument (C++ default `type=-1`): instrument 0 is a sustained
/// sine (sampler in Wavetable mode on one cycle).
pub const TONE: i32 = -1;

/// C++ `struct Audio`.
#[derive(Clone)]
pub struct Audio {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl Audio {
    pub fn new(n: usize) -> Audio {
        Audio { l: vec![0.0; n], r: vec![0.0; n] }
    }

    /// C++ `energy(start,end)`; an `end` of zero means the whole buffer.
    pub fn energy_in(&self, start: usize, end: usize) -> f64 {
        let end = if end == 0 { self.l.len() } else { end };
        let mut sum = 0.0f64;
        for i in start..end {
            assert!(self.l[i].is_finite() && self.r[i].is_finite());
            sum += self.l[i] as f64 * self.l[i] as f64 + self.r[i] as f64 * self.r[i] as f64;
        }
        sum / (end - start) as f64
    }

    /// C++ `energy()`.
    pub fn energy(&self) -> f64 {
        self.energy_in(0, 0)
    }
}

/// C++ `render(e,a)`: one direct render call of the buffer's length, as a render-thread call.
pub fn render(e: &mut Engine, a: &mut Audio) {
    e.render_into(&mut a.l, &mut a.r);
}

/// C++ `busRender(e,bus,time,a)`.
pub fn bus_render(e: &mut Engine, bus: i32, time: u64, a: &mut Audio) {
    e.render_bus_into(bus, time, &mut a.l, &mut a.r);
}

/// C++ `configure(e,type)`: two tracks at 120 BPM, 4 lines per beat, master 0.7. `TONE` (-1)
/// makes instrument 0 the sine test tone; any other value is the instrument type.
pub fn configure(e: &mut Engine, type_: i32) {
    e.begin_update();
    e.set_track_count(2);
    e.set_timing(120.0, 4, 0.0, 0.7);
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

/// C++ `struct Timed`: an event with its absolute frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timed {
    pub frame: u64,
    pub e: STEvent,
}

/// C++ `renderEvents(e,frames,block)`.
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

/// C++ `renderEvents(e,frames)` with its default block of 256 frames.
pub fn render_events(e: &mut Engine, frames: usize) -> Vec<Timed> {
    render_events_block(e, frames, 256)
}

/// C++ `noteEvents`: only note-on and note-off events.
pub fn note_events(mut events: Vec<Timed>) -> Vec<Timed> {
    events.retain(|e| e.e.type_ == ST_EVENT_NOTE_ON || e.e.type_ == ST_EVENT_NOTE_OFF);
    events
}

/// C++ `parameterChanges(events,target,track,address)`: the last value per frame of one native
/// parameter.
pub fn parameter_changes_at(events: &[Timed], target: i32, track: i32, address: u64) -> Vec<Timed> {
    let mut result: Vec<Timed> = Vec::new();
    for event in events {
        if event.e.type_ == ST_EVENT_PARAMETER && event.e.target == target && event.e.track == track && event.e.address == address {
            match result.last_mut() {
                Some(last) if last.frame == event.frame => *last = *event,
                _ => result.push(*event),
            }
        }
    }
    result
}

/// C++ `parameterChanges(events,target)` (track 0, address 0).
pub fn parameter_changes(events: &[Timed], target: i32) -> Vec<Timed> {
    parameter_changes_at(events, target, 0, 0)
}

/// C++ `hostEvents`: only host-parameter events.
pub fn host_events(mut events: Vec<Timed>) -> Vec<Timed> {
    events.retain(|e| e.e.type_ == ST_EVENT_HOST_PARAMETER);
    events
}

/// C++ `modulationAudio(instrument,frames,block)` (defaults: 48000 frames, blocks of 512): note
/// 69 on a fresh engine with `instrument`, rendered in blocks.
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

/// C++ `modulationInstrument`.
pub fn modulation_instrument() -> STInstrument {
    let mut p = test_tone_instrument(TEST_SINE);
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.release = 2.0;
    p.volume = 0.3;
    p
}

/// C++ `syncedPanInstrument(slot)`: a forward sampler whose LFO 1 (slot 0) or LFO 2 is a
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

/// C++ `configureSyncedLfo(e,p,bpm)` (default 120 BPM).
pub fn configure_synced_lfo(e: &mut Engine, p: STInstrument, bpm: f64) {
    configure(e, ST_INSTRUMENT_SAMPLE);
    let sample = vec![0.2f32; 2048];
    e.begin_update();
    e.set_timing(bpm, 4, 0.0, 0.7);
    e.set_instrument(0, p);
    e.set_sample(0, &sample, 48000.0);
    e.commit_update();
    e.note_on(0, 0, 60, 127);
}

/// C++ `assertPanSwitch(a,expected)`: the left channel starts at `expected` (within a frame)
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

/// C++ `configureCheckpointMusic`.
pub fn configure_checkpoint_music(e: &mut Engine) {
    configure(e, ST_INSTRUMENT_MIDI);
    e.begin_update();
    e.set_timing(123.0, 4, 0.2, 0.7);
    e.set_pattern(0, 4);
    e.set_pattern(1, 3);
    let sequence = [0, 1];
    e.set_sequence(&sequence, 0, 1);
    for p in 0..2 {
        for r in 0..(if p != 0 { 3 } else { 4 }) {
            let mut step = default_step();
            step.note = 60 + p * 12 + r;
            step.gate = 8.0;
            e.set_step(p, 0, r, step, &[]);
        }
    }
    e.add_automation_point(1, 0, ST_TARGET_VOLUME, 0, 0, 1, 0.0, 0.25);
    e.add_automation_point(1, 0, ST_TARGET_VOLUME, 0, 0, 1, 2.0, 0.75);
    e.commit_update();
}

/// C++ `temporaryExport`: what an offline export does to the transport between a capture and a
/// restore.
pub fn temporary_export(e: &mut Engine) {
    e.seek(0, 0, 0);
    e.play(1);
    let mut a = Audio::new(2048);
    render(e, &mut a);
    e.stop();
    render(e, &mut a);
    e.panic();
    e.seek(0, 0, 0);
}

/// C++ `installTestTiming(e,meterChange)`: 120 BPM, 60 BPM from beat 0.375; with `meter_change`
/// also 5/8 from there. Revision 17.
pub fn install_test_timing(e: &mut Engine, meter_change: bool) {
    let tempos = [STClockTempoEvent { beat: 0.0, bpm: 120.0 }, STClockTempoEvent { beat: 0.375, bpm: 60.0 }];
    let meters = [
        STClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 },
        STClockMeterEvent { beat: 0.375, numerator: 5, denominator: 8 },
    ];
    assert!(e.set_timing_map(&tempos, &meters[..if meter_change { 2 } else { 1 }], 17).is_ok());
}

/// C++ `configureLoopMusic(e,swing)`: rows 0..15 each carry note 60+row; one row is 6000
/// frames at 120 BPM, 4 lines per beat.
pub fn configure_loop_music(e: &mut Engine, swing: f32) {
    configure(e, ST_INSTRUMENT_AU);
    e.begin_update();
    e.set_timing(120.0, 4, swing, 0.7);
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

/// C++ `noteOns(e,frames)`.
pub fn note_ons(e: &mut Engine, frames: usize) -> Vec<Timed> {
    let mut on = Vec::new();
    for v in render_events(e, frames) {
        if v.e.type_ == ST_EVENT_NOTE_ON {
            on.push(v);
        }
    }
    on
}

/// C++ `assertNotes(on,{{frame,note},...})`: the first note-ons are exactly `expected`.
pub fn assert_notes(on: &[Timed], expected: &[(u64, i32)]) {
    assert!(on.len() >= expected.len());
    for (n, &(frame, note)) in expected.iter().enumerate() {
        assert!(on[n].frame == frame);
        assert!(on[n].e.note == note);
    }
}

/// C++ `configureQueueMusic(e,order)`: pattern 0 = 16 rows (notes 60+row), pattern 1 = 4 rows
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

/// A step effect, C++ `{type,value}`.
pub fn fx(type_: i32, value: f32) -> STStepFX {
    STStepFX { type_, value }
}

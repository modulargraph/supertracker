//! Port of the former C++ test program `engine_timing_tests.cpp`, part 1 of 2: mapped playback (rows and gates across
//! a tempo change, exact post-tempo onsets, microtiming and ratchets, repeated odd patterns and
//! the song loop, pattern origin and loop), live map edits, clip placement and source time,
//! exact PCM equivalence across block sizes and overlapping bus pulls, and draft publication
//! with the atomic rejection of invalid timing maps.
//!
//! One `#[test]` per C++ suite (`testFooBar` is `test_foo_bar`), calling the engine's safe Rust
//! API through `engine_support_a::Engine`. Every C++ `assert` is one `assert!` here and every
//! `expectFrames` one `expect_frames`, with the same inputs and expected values; every test
//! ends with the C++ program's final "zero render/query allocations" check.
//!
//! At 48 kHz the two-tempo fixture maps beat b to 24000*b through b=.375, then to 48000*b-9000.
//! All explicit expectations below are exact integers.
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod engine_support_a;

use engine_support_a::timing_helpers::*;
use engine_support_a::tone::*;
use engine_support_a::{assert_zero_render_allocations, Engine};
use tracker_engine::clock::{ClockMeterEvent, ClockTempoEvent, CLOCK_MAX_EVENTS};
use tracker_engine::engine::*;

fn tempo(beat: f64, bpm: f64) -> ClockTempoEvent {
    ClockTempoEvent { beat, bpm }
}

fn meter(beat: f64, numerator: i32, denominator: i32) -> ClockMeterEvent {
    ClockMeterEvent { beat, numerator, denominator }
}

#[test]
fn test_rows_and_gate_across_tempo() {
    for type_ in [ST_INSTRUMENT_AU, ST_INSTRUMENT_MIDI] {
        let mut engine = Engine::new();
        begin_fixture(&mut engine, type_, 4, 0.0);
        for row in 0..4 {
            put_note(&mut engine, row, 60 + row, if row != 0 { 0.5 } else { 2.0 });
        }
        start(&mut engine, 1);
        assert!(engine.duration(1) == 39000.0 / 48000.0);
        let result = render(&mut engine, 39001, 127);
        expect_frames(&result.notes, ST_EVENT_NOTE_ON, &[0, 6000, 15000, 27000]);
        expect_frames(&result.notes, ST_EVENT_NOTE_OFF, &[9000, 15000, 21000, 33000]);
        assert!(engine.status().playing == 0);
        let long_off = result.notes.iter().find(|note| note.type_ == ST_EVENT_NOTE_OFF && note.note == 60);
        assert!(long_off.is_some_and(|note| note.frame == 15000));
    }
    assert_zero_render_allocations();
}

#[test]
fn test_exact_post_tempo_row_onsets() {
    let onsets: Vec<u64> = vec![0, 6000, 12000, 24000, 36000, 48000, 60000, 64000];
    for type_ in [ST_INSTRUMENT_MIDI, ST_INSTRUMENT_SAMPLE] {
        let mut engine = Engine::new();
        begin_fixture(&mut engine, type_, 4, 0.0);
        let tempos = [tempo(0.0, 120.0), tempo(0.5, 60.0), tempo(1.5, 180.0)];
        let meters = [meter(0.0, 4, 4), meter(0.75, 3, 4)];
        assert!(engine.set_timing_map(&tempos, &meters, 42).is_ok());
        engine.set_pattern(1, 4);
        let sequence = [0, 1];
        engine.set_sequence(&sequence, 0, 0);
        for pattern in 0..2 {
            for row in 0..4 {
                let mut step = default_step();
                step.note = 60;
                step.instrument = 0;
                step.velocity = 100;
                step.gate = 0.5;
                engine.set_step(pattern, 0, row, step, &[]);
            }
        }
        if type_ == ST_INSTRUMENT_SAMPLE {
            let sample = [0.5f32; 64];
            assert!(engine.set_sample(0, &sample, 48000.0));
        }
        start(&mut engine, 1);
        assert!(engine.duration(1) == 68000.0 / 48000.0);
        let result = render(&mut engine, 68001, 4097);
        if type_ == ST_INSTRUMENT_MIDI {
            expect_frames(&result.notes, ST_EVENT_NOTE_ON, &onsets);
            expect_frames(&result.notes, ST_EVENT_NOTE_OFF, &[3000, 9000, 18000, 30000, 42000, 54000, 62000, 66000]);
        } else {
            for &onset in &onsets {
                assert!(result.left[onset as usize].abs() > 1e-5f32);
                if onset != 0 {
                    assert!(result.left[onset as usize - 1] == 0.0);
                }
            }
        }
        assert!(engine.status().playing == 0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_microtiming_and_ratchet_across_tempo() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 4, 0.0);
    let mut step = default_step();
    step.gate = 2.0;
    step.fx1 = STStepFX { type_: ST_FX_MICROTIMING, value: 0.25 };
    step.fx2 = STStepFX { type_: ST_FX_RATCHET, value: 2.0 };
    engine.set_step(0, 0, 1, step, &[]);
    // Isolate the first row's mapped gates; ratchets now persist across empty rows.
    let mut stop_repeat = default_step();
    stop_repeat.note = -3;
    stop_repeat.fx1 = STStepFX { type_: ST_FX_RATCHET, value: 1.0 };
    engine.set_step(0, 0, 2, stop_repeat, &[]);
    start(&mut engine, 1);
    let result = render(&mut engine, 20000, 7);
    // Row one starts at beat .25. Its remaining .1875 beats are divided in two;
    // note ends independently integrate .1875 beats from each ratchet onset. The overlapping
    // repeat of the same external pitch first ends the older note (MIDI has no per-voice ownership).
    expect_frames(&result.notes, ST_EVENT_NOTE_ON, &[7500, 10500]);
    expect_frames(&result.notes, ST_EVENT_NOTE_OFF, &[10500, 19500]);
    assert_zero_render_allocations();
}

#[test]
fn test_repeated_odd_pattern_and_song_loop() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_AU, 3, 0.25);
    for row in 0..3 {
        put_note(&mut engine, row, 60 + row, 0.5);
    }
    let sequence = [0, 0];
    engine.set_sequence(&sequence, 1, 0);
    start(&mut engine, 1);
    assert!(engine.duration(1) == 69000.0 / 48000.0);
    let result = render(&mut engine, 138001, 512);
    expect_frames(
        &result.notes,
        ST_EVENT_NOTE_ON,
        &[0, 7500, 15000, 30000, 45000, 54000, 69000, 76500, 84000, 99000, 114000, 123000, 138000],
    );
    expect_frames(
        &result.notes,
        ST_EVENT_NOTE_OFF,
        &[3750, 10500, 22500, 37500, 49500, 61500, 72750, 79500, 91500, 106500, 118500, 130500],
    );
    assert!(engine.status().playing != 0);
    assert_zero_render_allocations();
}

#[test]
fn test_pattern_origin_and_loop() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 2, 0.0);
    put_note(&mut engine, 0, 60, 0.5);
    put_note(&mut engine, 1, 62, 0.5);
    assert!(engine.set_pattern_timing_origin(0.25));
    start(&mut engine, 0);
    assert!(engine.duration(0) == 21000.0 / 48000.0);
    let result = render(&mut engine, 42001, 7);
    expect_frames(&result.notes, ST_EVENT_NOTE_ON, &[0, 9000, 21000, 30000, 42000]);
    expect_frames(&result.notes, ST_EVENT_NOTE_OFF, &[3000, 15000, 24000, 36000]);
    assert!(engine.status().song_mode == 0 && engine.status().playing != 0);
    assert_zero_render_allocations();
}

#[test]
fn test_live_map_edit_preserves_musical_deadlines() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 4, 0.0);
    put_note(&mut engine, 0, 60, 3.0);
    let mut delayed = default_step();
    delayed.note = 64;
    delayed.gate = 1.0;
    delayed.fx1 = STStepFX { type_: ST_FX_MICROTIMING, value: 0.5 };
    engine.set_step(0, 0, 1, delayed, &[]);
    start(&mut engine, 1);
    let before = render(&mut engine, 7500, 127);
    expect_frames(&before.notes, ST_EVENT_NOTE_ON, &[0]);
    expect_frames(&before.notes, ST_EVENT_NOTE_OFF, &[]);

    // Old cursor beat .3125 is exactly frame7500. The replacement maps that
    // same beat to frame6750; graph time continues from its current position.
    // The already-scheduled onset at beat.375 maps to7500, its off at.5 to9000,
    // and the existing three-row note's off at.75 to12000.
    let tempos = [tempo(0.0, 120.0), tempo(0.25, 240.0)];
    let meters = [meter(0.0, 4, 4)];
    engine.begin_update();
    assert!(engine.set_timing_map(&tempos, &meters, 2).is_ok());
    commit(&mut engine);
    let after = render(&mut engine, 6000, 7);
    expect_frames(&after.notes, ST_EVENT_NOTE_ON, &[750]);
    expect_frames(&after.notes, ST_EVENT_NOTE_OFF, &[2250, 5250]);
    assert!(after.notes.last().unwrap().note == 60);
    assert_zero_render_allocations();
}

#[test]
fn test_clip_placement_and_source_time() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, -1, 4, 0.0);
    let mut sample = vec![0.0f32; 65536];
    for n in 0..sample.len() {
        sample[n] = n as f32 / 65536.0;
    }
    assert!(engine.set_audio_asset(0, &sample, 48000.0));
    engine.add_audio_clip(0, 0, 0.25, 0.5, 0.125, 1.0, 0.0, 0.0, 0);
    start(&mut engine, 1);
    let result = render(&mut engine, 39001, 127);
    for frame in 0..result.left.len() {
        // Native clip source PCM stays in seconds while the musical clip end
        // follows the tempo map. Its .125-second source offset is 6000 samples.
        let expected: f32 = if (6000..27000).contains(&frame) { frame as f32 / 65536.0 } else { 0.0 };
        assert!((result.left[frame] - expected).abs() < 1e-6f32);
        assert!((result.right[frame] - expected).abs() < 1e-6f32);
    }
    assert!(engine.status().playing == 0);

    // Seeking to row two starts at beat .5/frame15000: 9000 samples into clip,
    // plus the 6000-source-frame offset, and exactly12000 samples remain.
    engine.seek(0, 2, 0);
    engine.play(1);
    let sought = render(&mut engine, 12001, 512);
    for frame in 0..sought.left.len() {
        let expected: f32 = if frame < 12000 { (frame + 15000) as f32 / 65536.0 } else { 0.0 };
        assert!((sought.left[frame] - expected).abs() < 1e-6f32);
    }
    assert_zero_render_allocations();
}

/// C++ `configureMixedAudio`: a synced-LFO saw, a sampler and an external instrument on three
/// tracks of a swung three-row pattern that plays twice and loops; playing.
fn configure_mixed_audio(engine: &mut Engine) {
    begin_fixture(engine, -1, 3, 0.25);
    let mut synth = test_tone_instrument(TEST_SAW);
    load_test_tone(engine, 0, TEST_SAW);
    synth.attack = 0.003;
    synth.decay = 0.007;
    synth.sustain = 0.7;
    synth.release = 0.013;
    synth.volume = 0.15;
    synth.cutoff = 2500.0;
    synth.lfo_sync = 1;
    synth.lfo_beats = 0.5;
    synth.lfo_depth = 0.1;
    synth.lfo_target = ST_MOD_PITCH;
    engine.set_instrument(0, synth);
    let mut sampler = synth;
    sampler.type_ = ST_INSTRUMENT_SAMPLE;
    sampler.mode = ST_MODE_FORWARD;
    sampler.pan = 0.3;
    sampler.lfo_target = ST_MOD_VOLUME;
    engine.set_instrument(1, sampler);
    let mut external = default_instrument();
    external.type_ = ST_INSTRUMENT_AU;
    engine.set_instrument(2, external);
    let mut sample = vec![0.0f32; 128];
    for n in 0..sample.len() {
        sample[n] = ((n % 32) as i32 - 16) as f32 / 32.0;
    }
    assert!(engine.set_sample(1, &sample, 48000.0));
    for row in 0..3 {
        put_note_on(engine, row, 60 + row * 2, 0.75, 0, 0);
        put_note_on(engine, row, 60 + row, 1.5, 1, 1);
        put_note_on(engine, row, 72 + row, 0.5, 2, 2);
    }
    let sequence = [0, 0];
    engine.set_sequence(&sequence, 1, 0);
    start(engine, 1);
}

#[test]
fn test_render_block_equivalence() {
    let mut reference = Engine::new();
    configure_mixed_audio(&mut reference);
    let expected = render(&mut reference, 72001, 1);
    assert!(expected.left.iter().any(|value| value.abs() > 0.001f32));
    for block in [7usize, 127, 512, 4096] {
        let mut engine = Engine::new();
        configure_mixed_audio(&mut engine);
        let actual = render(&mut engine, expected.left.len(), block);
        assert!(actual.left == expected.left && actual.right == expected.right && actual.notes == expected.notes);
        assert!(engine.status().seconds == reference.status().seconds);
    }
    assert_zero_render_allocations();
}

/// C++ `struct BusRendered`.
struct BusRendered {
    first: Rendered,
    second: Rendered,
}

/// C++ `renderBuses(engine,frames,overlap)`: buses 0 and 1 pulled in 4096-frame steps; with
/// `overlap` every bus-0 pull asks for one frame more than the step.
fn render_buses(engine: &mut Engine, frames: usize, overlap: bool) -> BusRendered {
    let mut result = BusRendered { first: Rendered::new(frames), second: Rendered::new(frames) };
    let (mut left, mut right) = (vec![0.0f32; 4097], vec![0.0f32; 4097]);
    let mut at = 0usize;
    while at < frames {
        let count = 4096usize.min(frames - at);
        let pull = (count + if overlap { 1 } else { 0 }).min(frames - at);
        engine.render_bus_into(0, at as u64, &mut left[..pull], &mut right[..pull]);
        result.first.left[at..at + count].copy_from_slice(&left[..count]);
        result.first.right[at..at + count].copy_from_slice(&right[..count]);
        drain_notes(engine, at as u64, &mut result.first.notes);
        engine.render_bus_into(
            1,
            at as u64,
            &mut result.second.left[at..at + count],
            &mut result.second.right[at..at + count],
        );
        drain_notes(engine, at as u64, &mut result.second.notes);
        at += 4096;
    }
    assert!(result.second.notes.is_empty());
    assert!(engine.status().dropped_events == 0);
    result
}

#[test]
fn test_overlapping_bus_pulls() {
    let (mut reference, mut overlapping) = (Engine::new(), Engine::new());
    configure_mixed_audio(&mut reference);
    configure_mixed_audio(&mut overlapping);
    let expected = render_buses(&mut reference, 72001, false);
    let actual = render_buses(&mut overlapping, 72001, true);
    let audible = |value: &f32| value.abs() > 0.001f32;
    assert!(expected.first.left.iter().any(audible));
    assert!(expected.second.left.iter().any(audible));
    assert!(actual.first.left == expected.first.left && actual.first.right == expected.first.right);
    assert!(actual.second.left == expected.second.left && actual.second.right == expected.second.right);
    assert!(actual.first.notes == expected.first.notes);
    assert!(overlapping.status().seconds == reference.status().seconds);
    assert_zero_render_allocations();
}

#[test]
fn test_draft_publication_and_invalid_atomicity() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 4, 0.0);
    for row in 0..4 {
        put_note(&mut engine, row, 60 + row, 0.5);
    }
    commit(&mut engine);
    flush(&mut engine);

    engine.begin_update();
    {
        let flat = vec![tempo(0.0, 240.0)];
        let flat_meter = vec![meter(0.0, 4, 4)];
        assert!(engine.set_timing_map(&flat, &flat_meter, 2).is_ok());
        // The setter must copy caller arrays, and only commit publishes the draft. (C++
        // overwrote its arrays with an invalid map after the call; a Rust caller cannot change
        // a list the engine still borrows, so here the lists are freed instead.)
    }
    engine.play(1);
    let before_commit = render(&mut engine, 15001, 127);
    expect_frames(&before_commit.notes, ST_EVENT_NOTE_ON, &[0, 6000, 15000]);
    engine.stop();
    flush(&mut engine);

    let valid_tempo = [tempo(0.0, 120.0)];
    let valid_meter = [meter(0.0, 4, 4)];
    // C++ `rejects(tempos,tempoCount,meters,meterCount)`: the map is refused with a message.
    // (The C++ check that the message was NUL-terminated inside its 128-byte buffer has no Rust
    // counterpart.)
    let rejects = |engine: &mut Engine, tempos: &[ClockTempoEvent], meters: &[ClockMeterEvent]| {
        let result = engine.set_timing_map(tempos, meters, 3);
        assert!(result.is_err());
        assert!(result.is_err_and(|error| !error.is_empty()));
    };
    // C++ passed a null tempo pointer, then a count of zero: both are "no tempo events" here.
    rejects(&mut engine, &[], &valid_meter);
    rejects(&mut engine, &valid_tempo[..0], &valid_meter);
    // C++ passed a count of CLOCK_MAX_EVENTS+1 with a one-event array; a slice has to hold
    // that many events, so they are valid ones and only their number is wrong.
    let too_many_tempos: Vec<ClockTempoEvent> = (0..CLOCK_MAX_EVENTS + 1).map(|n| tempo(n as f64, 120.0)).collect();
    rejects(&mut engine, &too_many_tempos, &valid_meter);
    rejects(&mut engine, &valid_tempo, &[]);
    rejects(&mut engine, &valid_tempo, &valid_meter[..0]);
    let too_many_meters: Vec<ClockMeterEvent> =
        (0..CLOCK_MAX_EVENTS + 1).map(|n| meter(n as f64 * 4.0, 4, 4)).collect();
    rejects(&mut engine, &valid_tempo, &too_many_meters);
    for bad in [tempo(0.25, 120.0), tempo(0.0, 19.0), tempo(0.0, 401.0), tempo(0.0, f64::NAN), tempo(f64::INFINITY, 120.0)] {
        rejects(&mut engine, &[bad], &valid_meter);
    }
    let duplicate = [tempo(0.0, 120.0), tempo(0.0, 60.0)];
    let descending = [tempo(0.0, 120.0), tempo(1.0, 60.0), tempo(0.5, 120.0)];
    let collapsed = [tempo(0.0, 120.0), tempo(0.5000000001, 60.0), tempo(0.5000000002, 90.0)];
    rejects(&mut engine, &duplicate, &valid_meter);
    rejects(&mut engine, &descending, &valid_meter);
    rejects(&mut engine, &collapsed, &valid_meter);
    for bad in [meter(0.0, 0, 4), meter(0.0, 4, 3), meter(0.25, 4, 4)] {
        rejects(&mut engine, &valid_tempo, &[bad]);
    }
    assert!(!engine.set_pattern_timing_origin(-0.25));
    assert!(!engine.set_pattern_timing_origin(f64::NAN));

    commit(&mut engine);
    engine.seek(0, 0, 0);
    engine.play(1);
    let after_commit = render(&mut engine, 9001, 127);
    expect_frames(&after_commit.notes, ST_EVENT_NOTE_ON, &[0, 3000, 6000, 9000]);
    engine.stop();
    flush(&mut engine);

    engine.begin_update();
    engine.clear_timing_map();
    commit(&mut engine);
    engine.seek(0, 0, 0);
    engine.play(1);
    let cleared = render(&mut engine, 18001, 127);
    expect_frames(&cleared.notes, ST_EVENT_NOTE_ON, &[0, 6000, 12000, 18000]);
    assert_zero_render_allocations();
}

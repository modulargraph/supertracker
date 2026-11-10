//! Port of the former C++ test program `engine_instrument_count_tests.cpp`: instruments are sized per project, like
//! tracks: more than 64 instruments, bounded indices.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use tracker_engine::engine::*;

/// C++ `energy`: one render call of `frames`, mean square of the left channel.
fn energy(e: &mut Engine, frames: usize) -> f64 {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    e.render(&mut l, &mut r);
    l.iter().map(|&x| x as f64 * x as f64).sum::<f64>() / frames as f64
}

#[test]
fn native_instrument_table_holds_exactly_the_projects_instruments() {
    assert!(
        instrument_capacity(64) == 64
            && instrument_capacity(ST_INSTRUMENT_SANITY_LIMIT) == ST_INSTRUMENT_SANITY_LIMIT
    );
}

/// 300 ordinary instruments: instrument 250 plays, a step can name it, a track can use it.
#[test]
fn three_hundred_instruments_steps_tracks_and_live_notes_reach_instrument_250() {
    let mut e = Engine::new();
    e.begin_update();
    assert!(e.set_track_count(2) && e.set_instrument_count(300));
    e.set_timing(120.0, 4, 0.0, 1.0);
    let mut track = default_track();
    track.instrument = 250;
    track.volume = 1.0;
    e.set_track(0, track);
    let mut p = test_tone_instrument(TEST_SINE);
    p.attack = 0.0;
    p.sustain = 1.0;
    p.volume = 0.5;
    e.set_instrument(250, p);
    load_test_tone(&mut e, 250, TEST_SINE);
    e.set_pattern(0, 4);
    let mut step = default_step();
    step.note = 69;
    step.instrument = 250;
    step.gate = 4.0;
    e.set_step(0, 0, 0, step, &[]);
    e.commit_update();
    e.play(0);
    assert!(energy(&mut e, 4800) > 0.01);
    e.stop();
    e.panic();
    energy(&mut e, 512);
    e.note_on(1, 250, 69, 100);
    assert!(energy(&mut e, 4800) > 0.01);
    // Out-of-range indices are rejected, not wrapped onto another instrument.
    e.panic();
    energy(&mut e, 512);
    e.note_on(1, 300, 69, 100);
    assert!(energy(&mut e, 4800) == 0.0);
    assert!(!e.set_sample_cycle(300, 0) && e.set_sample_cycle(299, 0));
}

/// Shrinking keeps the remaining instruments and their samples; the count is bounded.
#[test]
fn shrinking_keeps_remaining_instruments_and_rejects_removed_ones() {
    let mut e = Engine::new();
    e.begin_update();
    assert!(e.set_instrument_count(200));
    let mut p = test_tone_instrument(TEST_SINE);
    p.attack = 0.0;
    p.sustain = 1.0;
    p.volume = 0.5;
    e.set_instrument(5, p);
    load_test_tone(&mut e, 5, TEST_SINE);
    e.commit_update();
    e.begin_update();
    assert!(e.set_instrument_count(10));
    assert!(!e.set_instrument_count(0) && !e.set_instrument_count(ST_INSTRUMENT_SANITY_LIMIT + 1));
    e.commit_update();
    e.note_on(0, 5, 69, 100);
    assert!(energy(&mut e, 4800) > 0.01);
    e.panic();
    energy(&mut e, 512);
    e.note_on(0, 150, 69, 100);
    assert!(energy(&mut e, 4800) == 0.0);
}

//! Port of the former C++ test program `engine_parameter_bounds_tests.cpp`: native finite-but-out-of-range inputs must
//! saturate before float->int conversion.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::engine::Engine;
use tracker_engine::engine::*;

struct Audio {
    l: Vec<f32>,
    r: Vec<f32>,
}

fn scene(test: i32, value: f32) -> Audio {
    let mut e = Engine::new();
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(240.0, 8, 0.0, 1.0);
    let mut p = default_instrument();
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.mode = if test == 1 || test == 2 || test == 3 { ST_MODE_SLICE } else { ST_MODE_FORWARD };
    p.slice_count = 8;
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.release = 0.001;
    p.volume = 0.2;
    let pcm: Vec<f32> =
        (0..32768).map(|n| (0.4 * (n as f64 * 0.07).sin() + 0.2 * (n as f64 * 0.019).cos()) as f32).collect();
    assert!(e.set_sample(0, &pcm, None, 48000.0));
    e.set_instrument(0, p);
    let mut step = default_step();
    step.note = 72;
    step.instrument = 0;
    step.gate = 2.0;
    if test < 2 {
        let target = if test == 0 { ST_TARGET_BIT_DEPTH } else { ST_TARGET_SLICE_COUNT };
        e.add_automation_point(0, 0, target, 0, 0, 1, 0.0, value as f64);
    } else {
        let fx = if test == 2 || test == 3 {
            ST_FX_SLICE
        } else if test == 4 {
            ST_FX_RATCHET
        } else {
            ST_FX_ARPEGGIO
        };
        step.fx1 = STStepFX { type_: fx, value };
    }
    if test != 2 {
        e.set_step(0, 0, 0, step, &[]);
    }
    e.commit_update();
    if test == 2 {
        e.preview_step(0, step, &[], 0.2);
    } else {
        e.play(0);
    }
    let mut out = Audio { l: vec![0.0; 12000], r: vec![0.0; 12000] };
    let mut n = 0;
    while n < out.l.len() {
        let count = 127.min(out.l.len() - n);
        e.render(&mut out.l[n..n + count], &mut out.r[n..n + count]);
        n += 127;
    }
    out
}

/// One iteration of the C++ `main` loop: `test` saturates finite extremes before integer
/// conversion.
fn saturates(test: i32) {
    let lo: f32 = if test == 0 {
        2.0
    } else if test == 5 {
        0.0
    } else {
        1.0
    };
    let hi: f32 = if test == 0 {
        16.0
    } else if test == 1 || test == 2 || test == 3 {
        128.0
    } else if test == 4 {
        8.0
    } else {
        255.0
    };
    for value in [-f32::MAX, -1e20f32, 1e20f32, f32::MAX, hi + 0.75f32, lo - 0.75f32] {
        let actual = scene(test, value);
        // std::max(lo, std::min(hi, value))
        let limited = if hi < value { hi } else { value };
        let limited = if lo < limited { limited } else { lo };
        let bounded = scene(test, limited);
        assert!(actual.l == bounded.l && actual.r == bounded.r);
    }
}

#[test]
fn bit_depth_automation_saturates_finite_extremes() {
    saturates(0);
}

#[test]
fn slice_count_automation_saturates_finite_extremes() {
    saturates(1);
}

#[test]
fn preview_slice_fx_saturates_finite_extremes() {
    saturates(2);
}

#[test]
fn sequenced_slice_fx_saturates_finite_extremes() {
    saturates(3);
}

#[test]
fn ratchet_fx_saturates_finite_extremes() {
    saturates(4);
}

#[test]
fn arpeggio_fx_saturates_finite_extremes() {
    saturates(5);
}

//! Port of the former C++ test program `engine_dsp_regressions.cpp`: behaviour preserved by render-loop optimizations.
//! Every render runs as the render thread; each group ends with the C++ program's "zero render
//! allocations" assertion for its own thread.
#![cfg(feature = "engine")]

mod engine_support_b;
#[allow(unused_imports)]
use engine_support_b::{both_views, host_view_only, mix_view, mix_view_only};

use engine_support_b::alloc::{realtime, render_allocations};
use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use tracker_engine::engine::*;

struct Audio {
    l: Vec<f32>,
    r: Vec<f32>,
}

fn render(e: &mut Engine, n: usize) -> Audio {
    let mut a = Audio { l: vec![0.0; n], r: vec![0.0; n] };
    realtime(|| {
        let mut p = 0;
        while p < n {
            let count = 137.min(n - p);
            e.render(&mut a.l[p..p + count], &mut a.r[p..p + count]);
            p += 137;
        }
    });
    a
}

fn instrument() -> STInstrument {
    let mut p = test_tone_instrument(TEST_SAW);
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.release = 0.01;
    p.volume = 0.2;
    p
}

fn setup(e: &mut Engine, p: STInstrument) {
    e.begin_update();
    e.set_track_count(1);
    if mix_view() {
        e.set_host_mixing(true, 1);
    }
    e.set_instrument(0, p);
    load_test_tone(e, 0, TEST_SAW);
    e.commit_update();
    e.note_on(0, 0, 60, 127);
}

/// "PASS zero-depth phase and random continuity: both LFOs, all five shapes"
#[test]
fn zero_depth_continuity() {
    both_views(|| {
    for slot in [0, 1] {
        for shape in [ST_LFO_SINE, ST_LFO_TRIANGLE, ST_LFO_SAW, ST_LFO_SQUARE, ST_LFO_RANDOM] {
            let (mut continuous, mut inaudible) = (Engine::new(), Engine::new());
            let mut a = instrument();
            if slot == 0 {
                a.lfo_rate = 41.3;
                a.lfo_depth = 0.7;
                a.lfo_shape = shape;
                a.lfo_target = ST_MOD_VOLUME;
            } else {
                a.lfo2_rate = 41.3;
                a.lfo2_depth = 0.7;
                a.lfo2_shape = shape;
                a.lfo2_target = ST_MOD_VOLUME;
            }
            let mut b = a;
            if slot == 0 {
                b.lfo_depth = 0.0;
            } else {
                b.lfo2_depth = 0.0;
            }
            setup(&mut continuous, a);
            setup(&mut inaudible, b);
            render(&mut continuous, 12347);
            render(&mut inaudible, 12347);
            inaudible.begin_update();
            inaudible.set_instrument(0, a);
            inaudible.commit_update();
            let (expected, actual) = (render(&mut continuous, 8192), render(&mut inaudible, 8192));
            assert!(expected.l == actual.l && expected.r == actual.r);
        }
    }
    assert!(render_allocations() == 0);
    });
}

/// "PASS automated region boundaries isolate excluded PCM in all applicable modes"
#[test]
fn region_automation_bounds() {
    both_views(|| {
    // Both samples contain the same selected silence, but only one has loud excluded material.
    // Moving each selection edge with automation must never borrow that excluded material.
    for mode in
        [ST_MODE_ONESHOT, ST_MODE_FORWARD, ST_MODE_REVERSE, ST_MODE_PINGPONG, ST_MODE_GRANULAR, ST_MODE_WAVETABLE]
    {
        let mut e = Engine::new();
        let mut p = instrument();
        p.type_ = ST_INSTRUMENT_SAMPLE;
        p.mode = mode;
        p.start = 0.25;
        p.end = 0.75;
        p.loop_start = 0.25;
        p.loop_end = 0.75;
        p.tune = -12.0;
        p.grain_position = 0.001;
        let mut pcm = vec![0.8f32; 4096];
        pcm[1024..3072].fill(0.0);
        e.begin_update();
        e.set_track_count(1);
        e.set_timing(240.0, 16, 0.0, 1.0);
        if mix_view() {
            e.set_host_mixing(true, 1);
        }
        e.set_instrument(0, p);
        assert!(e.set_sample(0, &pcm, None, 48000.0));
        e.add_automation_point(0, 0, ST_TARGET_START, 0, 0, 0, 0.0, 0.25);
        e.add_automation_point(0, 0, ST_TARGET_START, 0, 0, 0, 2.0, 0.375);
        e.add_automation_point(0, 0, ST_TARGET_END, 0, 0, 0, 0.0, 0.75);
        e.add_automation_point(0, 0, ST_TARGET_END, 0, 0, 0, 2.0, 0.625);
        e.commit_update();
        e.play(0);
        e.note_on(0, 0, 60, 127);
        let audio = render(&mut e, 8000);
        assert!(audio.l.iter().all(|&x| x == 0.0));
        assert!(audio.l == audio.r);
    }
    assert!(render_allocations() == 0);
    });
}

/// A sustained looping note survives automation that moves its region: the loop follows the
/// moved start/end, as at note start. Left at its original points outside the region, the next
/// wrap ended the voice (silence).
#[test]
fn region_automation_keeps_looping_voices() {
    both_views(|| {
    for mode in [ST_MODE_FORWARD, ST_MODE_REVERSE, ST_MODE_PINGPONG] {
        for target in [ST_TARGET_START, ST_TARGET_END] {
            let mut e = Engine::new();
            let mut p = instrument();
            p.type_ = ST_INSTRUMENT_SAMPLE;
            p.mode = mode;
            let pcm = vec![0.5f32; 4800];
            e.begin_update();
            e.set_track_count(1);
            e.set_timing(120.0, 4, 0.0, 1.0);
            if mix_view() {
                e.set_host_mixing(true, 1);
            }
            e.set_instrument(0, p);
            assert!(e.set_sample(0, &pcm, None, 48000.0));
            // Stepped: the whole sample on row 0, a quarter trimmed from row 1 (frame 6000) on.
            e.add_automation_point(0, 0, target, 0, 0, 1, 0.0, if target == ST_TARGET_START { 0.0 } else { 1.0 });
            e.add_automation_point(0, 0, target, 0, 0, 1, 1.0, if target == ST_TARGET_START { 0.25 } else { 0.75 });
            e.commit_update();
            e.play(0);
            e.note_on(0, 0, 60, 127);
            let audio = render(&mut e, 48000);
            assert!(audio.l[24000..].iter().all(|&x| x.abs() > 0.01f32));
        }
    }
    assert!(render_allocations() == 0);
    });
}

/// A start automated past the (un-automated) end leaves a one-frame region at the start, never a
/// negative one: Wavetable mode converted that negative size to an unsigned frame count and read
/// PCM outside the selection.
#[test]
fn region_automation_start_past_end() {
    both_views(|| {
    let mut e = Engine::new();
    let mut p = instrument();
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.mode = ST_MODE_WAVETABLE;
    p.start = 0.25;
    p.end = 0.5;
    p.wavetable_frame_size = 64;
    p.wavetable_position = 0.5;
    let mut pcm = vec![0.0f32; 4096];
    pcm[..1024].fill(0.8);
    pcm[4096 - 512..].fill(0.8);
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0, 1.0);
    if mix_view() {
        e.set_host_mixing(true, 1);
    }
    e.set_instrument(0, p);
    assert!(e.set_sample(0, &pcm, None, 48000.0));
    e.add_automation_point(0, 0, ST_TARGET_START, 0, 0, 1, 0.0, 0.25);
    e.add_automation_point(0, 0, ST_TARGET_START, 0, 0, 1, 1.0, 0.7);
    e.commit_update();
    e.play(0);
    e.note_on(0, 0, 60, 127);
    let audio = render(&mut e, 12000);
    assert!(audio.l.iter().all(|&x| x == 0.0));
    assert!(audio.l == audio.r);
    assert!(render_allocations() == 0);
    });
}

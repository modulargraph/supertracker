//! Reverse playback: polyphony, loop crossfades, repeated reversals, release tails, block
//! invariance, reverse start and lead-in turnarounds.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::engine::Engine;
use std::f64::consts::PI;
use tracker_engine::engine::*;

#[allow(clippy::too_many_arguments)]
fn render(
    rapid: bool,
    voices: usize,
    block: usize,
    reverse_start: bool,
    release_tail: bool,
    one_shot: bool,
    zero_cross: bool,
) -> Vec<f32> {
    let mut e = Engine::new();
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(if rapid { 999.0 } else { 120.0 }, if rapid { 32 } else { 4 }, 0.0);
    e.set_pattern(0, 16);
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_SAMPLE;
    instrument.mode = if reverse_start || one_shot { ST_MODE_ONESHOT } else { ST_MODE_FORWARD };
    if one_shot {
        instrument.end = 0.606;
    }
    instrument.root_note = 60;
    instrument.attack = 0.0;
    instrument.decay = 0.0;
    instrument.sustain = 1.0;
    instrument.release = 0.5;
    instrument.volume = 0.25;
    instrument.cutoff = 22000.0;
    instrument.loop_start = 0.2;
    instrument.loop_end = if rapid { 0.411 } else { 0.605 };
    if zero_cross {
        instrument.loop_start = 0.0;
        instrument.loop_end = 1.0;
    }
    e.set_instrument(0, instrument);
    let mut sample = vec![0.8f32; 10000];
    if !reverse_start {
        sample[..2000].fill(-0.8);
    }
    if zero_cross {
        for (i, value) in sample.iter_mut().enumerate() {
            *value = (0.8f32 as f64 * (2.0 * PI * i as f64 / 480.0).sin()) as f32;
        }
    }
    assert!(e.set_sample(0, &sample, None, 48000.0));
    let mut note = default_step();
    note.note = 60;
    note.velocity = 127;
    note.gate = if release_tail { 0.8 } else { 256.0 };
    let chord = [0, 0, 0];
    if reverse_start {
        note.fx1 = STStepFX { type_: ST_FX_REVERSE, value: 1.0 };
    } else if rapid {
        note.fx1 = STStepFX { type_: ST_FX_OFFSET, value: 0.4 };
    }
    e.set_step(0, 0, 0, note, &chord[..voices]);
    if !reverse_start {
        for row in 1..16 {
            let mut command = default_step();
            command.note = -3;
            command.fx1 = STStepFX { type_: ST_FX_REVERSE, value: (row % 2) as f32 };
            e.set_step(0, 0, row, command, &[]);
        }
    }
    e.commit_update();
    e.play(0);
    let frames: usize = if reverse_start {
        200
    } else if rapid {
        1400
    } else {
        13000
    };
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut at = 0;
    // The engine renders at most 4096 frames per block: a longer pull is consecutive blocks.
    while at < frames {
        let n = block.min(4096).min(frames - at);
        e.render_bus(0, at as u64, &mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    l
}

fn continuity(rapid: bool, tail: bool, one_shot: bool, zero_cross: bool) {
    let single = render(rapid, 1, 257, false, tail, one_shot, zero_cross);
    let poly = render(rapid, 3, 257, false, tail, one_shot, zero_cross);
    let small = render(rapid, 3, 1, false, tail, one_shot, zero_cross);
    let large = render(rapid, 3, 8192, false, tail, one_shot, zero_cross);
    assert!(poly == small && poly == large);
    for i in 0..single.len() {
        assert!(poly[i].is_finite());
        assert!((poly[i] - 3.0f32 * single[i]).abs() < 1e-5f32);
    }
    let row_frames: f64 = if rapid { 48000.0 * 60.0 / 999.0 / 32.0 } else { 6000.0 };
    let mut maximum = 0.0f32;
    for row in 1..(if rapid { 15 } else { 3 }) {
        let start = (row_frames * row as f64).ceil() as i32;
        for i in start..(poly.len() as i32).min(start + 80) {
            let i = i as usize;
            maximum = maximum.max((poly[i] - poly[i - 1]).abs());
        }
    }
    println!(
        "reverse {}{}: max adjacent PCM delta {:.6} (3 voices)",
        if rapid { "rapid" } else { "loop-crossfade" },
        if tail {
            " release tails"
        } else if one_shot {
            " one-shot fade"
        } else if zero_cross {
            " zero crossing"
        } else {
            ""
        },
        maximum
    );
    assert!(maximum < 0.035f32);
    assert!(single.iter().copied().fold(f32::NEG_INFINITY, f32::max) > 0.1f32);
}

/// Turning around in a loop sample's lead-in (before the loop in forward mode, after it in
/// reverse mode) must not jump into the loop: the voice runs back through the lead-in to the
/// region edge and fades out there.
fn lead_in_turnaround(reverse_mode: bool) {
    let mut e = Engine::new();
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0);
    e.set_pattern(0, 16);
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_SAMPLE;
    instrument.mode = if reverse_mode { ST_MODE_REVERSE } else { ST_MODE_FORWARD };
    instrument.loop_start = if reverse_mode { 0.0 } else { 0.5 };
    instrument.loop_end = if reverse_mode { 0.5 } else { 1.0 };
    instrument.root_note = 60;
    instrument.attack = 0.0;
    instrument.decay = 0.0;
    instrument.sustain = 1.0;
    instrument.release = 0.001;
    instrument.volume = 0.5;
    instrument.cutoff = 22000.0;
    e.set_instrument(0, instrument);
    // Lead-in: a cosine (at a peak on the region edges, so the edge needs its fade); loop: a
    // constant .8 that a jump would land on.
    let mut sample = vec![0.0f32; 48000];
    for (i, value) in sample.iter_mut().enumerate() {
        let lead = if reverse_mode { i >= 24000 } else { i < 24000 };
        *value = if lead { (0.8f32 as f64 * (2.0 * PI * i as f64 / 480.0).cos()) as f32 } else { 0.8 };
    }
    assert!(e.set_sample(0, &sample, None, 48000.0));
    let mut note = default_step();
    note.note = 60;
    note.gate = 32.0;
    e.set_step(0, 0, 0, note, &[]);
    let mut command = default_step();
    command.note = -3;
    command.fx1 = STStepFX { type_: ST_FX_REVERSE, value: if reverse_mode { 0.0 } else { 1.0 } };
    e.set_step(0, 0, 1, command, &[]);
    e.commit_update();
    e.play(0);
    let (mut l, mut r) = (vec![0.0f32; 16000], vec![0.0f32; 16000]);
    let mut at = 0;
    while at < 16000 {
        let n = 257.min(16000 - at);
        e.render(&mut l[at..at + n], &mut r[at..at + n]);
        at += 257;
    }
    let (mut normal, mut turn, mut end) = (0.0f32, 0.0f32, 0.0f32);
    for i in 3000..5900 {
        normal = normal.max((l[i] - l[i - 1]).abs());
    }
    for i in 5990..6400 {
        turn = turn.max((l[i] - l[i - 1]).abs());
    }
    for i in 11000..12400 {
        end = end.max((l[i] - l[i - 1]).abs());
    }
    // Still the lead-in sine after the turn (sign changes), silent once the region edge is
    // reached ~6000 frames later.
    let mut crossings = 0;
    for i in 6101..11000 {
        crossings += ((l[i - 1] < 0.0) != (l[i] < 0.0)) as i32;
    }
    let mut tail = 0.0f32;
    for value in &l[12200..16000] {
        tail = tail.max(value.abs());
    }
    println!(
        "reverse {} lead-in turnaround: turn delta {:.4}, edge delta {:.4} (normal {:.4}), {} crossings",
        if reverse_mode { "reverse-mode" } else { "forward-mode" },
        turn,
        end,
        normal,
        crossings
    );
    assert!(turn < normal * 1.5f32 && end < normal * 1.5f32 && crossings > 15 && tail < 1e-4f32);
}

#[test]
fn lead_in_turnaround_forward_mode() {
    lead_in_turnaround(false);
}

#[test]
fn lead_in_turnaround_reverse_mode() {
    lead_in_turnaround(true);
}

#[test]
fn continuity_loop_crossfade() {
    continuity(false, false, false, false);
}

#[test]
fn continuity_rapid() {
    continuity(true, false, false, false);
}

#[test]
fn continuity_release_tails() {
    continuity(false, true, false, false);
}

#[test]
fn continuity_one_shot_fade() {
    continuity(false, false, true, false);
}

#[test]
fn continuity_zero_crossing() {
    continuity(false, false, false, true);
}

#[test]
fn reverse_start() {
    let onset = render(false, 1, 1, true, false, false, false);
    assert!(onset[0].abs() < 0.01f32);
    assert!(onset[70] > 0.15f32);
}

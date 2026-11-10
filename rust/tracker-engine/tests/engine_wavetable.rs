//! Port of `tests/engine_wavetable_tests.cpp`: sampler Wavetable mode: position parameter,
//! automation, step FX, frame size and smoothing.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::engine::Engine;
use std::f64::consts::PI;
use tracker_engine::engine::*;

/// `frames` cycles of `width` samples; frame k is a sine of amplitude .2*(k+1).
fn table(frames: i32, width: i32) -> Vec<f32> {
    let mut pcm = vec![0.0f32; (frames * width) as usize];
    for k in 0..frames {
        for i in 0..width {
            pcm[(k * width + i) as usize] = 0.2f32 * (k + 1) as f32 * (2.0 * PI * i as f64 / width as f64).sin() as f32;
        }
    }
    pcm
}

fn wavetable(position: f32) -> STInstrument {
    let mut p = default_instrument();
    p.mode = ST_MODE_WAVETABLE;
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.release = 0.01;
    p.volume = 1.0;
    p.cutoff = 22000.0;
    p.wavetable_position = position;
    p
}

fn setup(e: &mut Engine, p: STInstrument, pcm: &[f32], rows: i32) {
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0, 1.0);
    e.set_host_mixing(true, 1);
    let mut track = default_track();
    track.volume = 1.0;
    track.delay_send = 0.0;
    track.reverb_send = 0.0;
    e.set_track(0, track);
    e.set_instrument(0, p);
    assert!(e.set_sample(0, pcm, None, 48000.0));
    e.set_pattern(0, rows);
    e.commit_update();
}

fn render(e: &mut Engine, frames: usize) -> Vec<f32> {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut at = 0;
    while at < frames {
        let count = 256.min(frames - at);
        e.render(&mut l[at..at + count], &mut r[at..at + count]);
        at += 256;
    }
    l
}

/// Peak amplitude over a window (440 Hz, so any 120-sample window holds a full cycle).
fn peak(x: &[f32], from: usize, to: usize) -> f64 {
    x[from..to].iter().fold(0.0f64, |p, &v| p.max(v.abs() as f64))
}

fn level(x: &[f32], from: usize, to: usize) -> f64 {
    peak(x, from, to) / 0.5f64.sqrt()
}

/// The Position parameter selects frames; between frames it crossfades.
#[test]
fn position_selects_and_crossfades_frames() {
    for position in [0.0f32, 1.0f32 / 3.0, 0.5, 1.0] {
        let mut e = Engine::new();
        setup(&mut e, wavetable(position), &table(4, 2048), 4);
        e.note_on(0, 0, 69, 127);
        let out = render(&mut e, 9600);
        let expected = 0.2 * (1.0f32 + 3.0f32 * position) as f64;
        let got = peak(&out, 4800, 9600);
        println!("position {position:.3} -> peak {got:.3} (expected {expected:.3})");
        assert!((got - expected).abs() < 0.01);
    }
}

/// The instrument's frame size overrides the default 2048; the sample's registered cycle is used
/// when it is 0.
#[test]
fn explicit_and_registered_frame_sizes() {
    let pcm = table(8, 256);
    // Half volume keeps the loudest frame (1.6) clear of the output's limits.
    let mut explicit_size = Engine::new();
    let mut p = wavetable(1.0);
    p.volume = 0.5;
    p.wavetable_frame_size = 256;
    setup(&mut explicit_size, p, &pcm, 4);
    explicit_size.note_on(0, 0, 69, 127);
    assert!((peak(&render(&mut explicit_size, 9600), 4800, 9600) - 0.8).abs() < 0.01);
    let mut half = wavetable(1.0);
    half.volume = 0.5;
    let mut registered = Engine::new();
    setup(&mut registered, half, &pcm, 4);
    assert!(registered.set_sample_cycle(0, 256));
    registered.note_on(0, 0, 69, 127);
    assert!((peak(&render(&mut registered, 9600), 4800, 9600) - 0.8).abs() < 0.01);
    assert!(
        !registered.set_sample_cycle(0, 8) && !registered.set_sample_cycle(0, 9000) && !registered.set_sample_cycle(-1, 256)
    );
    // Replacing the sample clears the registered size: the default 2048-sample read then spans
    // all eight cycles.
    let again = pcm.clone();
    assert!(registered.set_sample(0, &again, None, 48000.0));
    registered.panic();
    render(&mut registered, 256);
    registered.note_on(0, 0, 69, 127);
    let default_read = render(&mut registered, 9600);
    let mut crossings = 0;
    for i in 4801..9600 {
        if default_read[i - 1] <= 0.0 && default_read[i] > 0.0 {
            crossings += 1;
        }
    }
    assert!(crossings > 300); // eight cycles per 2048-sample read instead of one at 440 Hz
}

/// Automation lanes move the position for sounding voices.
#[test]
fn position_automation() {
    let mut e = Engine::new();
    setup(&mut e, wavetable(0.0), &table(4, 2048), 16);
    e.begin_update();
    e.add_automation_point(0, 0, ST_TARGET_WAVETABLE_POSITION, 0, 0, 1, 0.0, 0.0);
    e.add_automation_point(0, 0, ST_TARGET_WAVETABLE_POSITION, 0, 0, 1, 8.0, 1.0);
    let mut step = default_step();
    step.note = 69;
    step.velocity = 127;
    step.gate = 16.0;
    e.set_step(0, 0, 0, step, &[]);
    e.commit_update();
    e.play(0);
    let out = render(&mut e, 60000);
    assert!(level(&out, 1000, 2000) < 0.3 && level(&out, 50000, 52000) > 0.75);
}

/// Step FX "position" sets the track's position for its voices (value 0..1).
#[test]
fn step_fx_position() {
    let mut e = Engine::new();
    setup(&mut e, wavetable(0.0), &table(4, 2048), 4);
    e.begin_update();
    let mut step = default_step();
    step.note = 69;
    step.velocity = 127;
    step.gate = 4.0;
    e.set_step(0, 0, 0, step, &[]);
    let mut fx = default_step();
    fx.note = -3;
    fx.fx1 = STStepFX { type_: ST_FX_POSITION, value: 1.0 };
    e.set_step(0, 0, 2, fx, &[]);
    e.commit_update();
    e.play(0);
    let out = render(&mut e, 24000);
    // Rows are 6000 frames at 120 BPM / 4 LPB.
    assert!((peak(&out, 1000, 6000) - 0.2).abs() < 0.02 && (peak(&out, 13000, 18000) - 0.8).abs() < 0.02);
}

/// Position jumps slew over 5 ms instead of clicking: no sample-to-sample step beyond the
/// waveform's own slope.
#[test]
fn position_changes_are_smoothed() {
    let mut e = Engine::new();
    setup(&mut e, wavetable(0.0), &table(2, 2048), 4);
    e.note_on(0, 0, 33, 127); // 55 Hz: the waveform itself moves slowly
    render(&mut e, 4800);
    e.begin_update();
    e.set_instrument(0, wavetable(1.0));
    e.commit_update();
    let out = render(&mut e, 4800);
    let mut largest = 0.0f64;
    for i in 1..out.len() {
        largest = largest.max((out[i] - out[i - 1]).abs() as f64);
    }
    let slope = 0.4 * 2.0 * PI * 55.0 / 48000.0; // steepest step of the louder frame
    assert!(largest < slope * 1.5 + 0.2 / 240.0);
    assert!((peak(&out, 2400, 4800) - 0.4).abs() < 0.01);
}

/// Invalid values are clamped or ignored.
#[test]
fn position_clamps_to_1_and_an_invalid_frame_size_reads_as_automatic() {
    let mut e = Engine::new();
    let mut p = wavetable(7.0);
    p.wavetable_frame_size = 3;
    setup(&mut e, p, &table(4, 2048), 4);
    e.note_on(0, 0, 69, 127);
    let out = render(&mut e, 9600);
    assert!((peak(&out, 4800, 9600) - 0.8).abs() < 0.01);
}

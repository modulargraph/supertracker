//! Port of `tests/sampler_modes_tests.cpp`: behavioural PCM checks for the Polyend-style sample
//! play modes. A ramp sample encodes the read position in its value, so the rendered output
//! reveals where (and in which direction) the voice is reading.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::engine::Engine;
use std::f64::consts::PI;
use tracker_engine::engine::*;

const RATE: i32 = 48000;
const VOLUME: f32 = 0.5;

fn sampler(mode: i32) -> STInstrument {
    let mut i = default_instrument();
    i.type_ = ST_INSTRUMENT_SAMPLE;
    i.mode = mode;
    i.attack = 0.0;
    i.decay = 0.0;
    i.sustain = 1.0;
    i.release = 0.005;
    i.volume = VOLUME;
    i.cutoff = 20000.0;
    i.root_note = 60;
    i
}

fn render(pcm: &[f32], instrument: STInstrument, frames: usize, note: i32, slices: &[f32]) -> Vec<f32> {
    let mut e = Engine::new();
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0, 1.0);
    let mut track = default_track();
    track.volume = 1.0;
    track.pan = 0.0;
    e.set_track(0, track);
    e.set_instrument(0, instrument);
    assert!(e.set_sample(0, pcm, None, RATE as f64));
    if !slices.is_empty() {
        e.set_slice_points(0, slices);
    }
    e.commit_update();
    e.note_on(0, 0, note, 127);
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut at = 0;
    while at < frames {
        let count = 256.min(frames - at);
        e.render(&mut l[at..at + count], &mut r[at..at + count]);
        at += 256;
    }
    for n in 0..frames {
        assert!(l[n].is_finite());
        assert!((l[n] - r[n]).abs() < 1e-6f32);
    }
    l
}

fn ramp(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 / n as f32).collect()
}

fn sine(n: usize, period: f64, amplitude: f32, phase: f64) -> Vec<f32> {
    (0..n).map(|i| amplitude * (2.0 * PI * (i as f64 / period + phase)).sin() as f32).collect()
}

/// Read position (in source frames) recovered from a ramp render.
fn position(out: f32, n: usize) -> f64 {
    out as f64 / VOLUME as f64 * n as f64
}

fn max_step(a: &[f32], from: usize, to: usize) -> f32 {
    let mut worst = 0.0f32;
    for i in from.max(1)..to {
        worst = worst.max((a[i] - a[i - 1]).abs());
    }
    worst
}

fn rms(a: &[f32], from: usize, to: usize) -> f64 {
    let s: f64 = a[from..to].iter().map(|&x| x as f64 * x as f64).sum();
    (s / (to - from) as f64).sqrt()
}

#[test]
fn oneshot() {
    let n: usize = 4800;
    let pcm = ramp(n);
    let out = render(&pcm, sampler(ST_MODE_ONESHOT), 8000, 60, &[]);
    // Reads forward at one source frame per output frame.
    for i in (200..4000).step_by(100) {
        assert!((position(out[i], n) - i as f64).abs() < 3.0);
    }
    for value in &out[n + 10..] {
        assert!(*value == 0.0);
    }
    // Ends with a short fade instead of dropping from 0.5 to silence.
    let end_step = max_step(&out, 4000, 6000);
    println!("one-shot: end step {end_step:.4}");
    assert!(end_step < 0.02f32);
    // An octave up reads twice as fast.
    let fast = render(&pcm, sampler(ST_MODE_ONESHOT), 3000, 72, &[]);
    for i in (200..2000).step_by(100) {
        assert!((position(fast[i], n) - 2.0 * i as f64).abs() < 4.0);
    }
    for value in &fast[n / 2 + 10..] {
        assert!(*value == 0.0);
    }
}

#[test]
fn forward() {
    let n: usize = 4800;
    let pcm = ramp(n);
    let mut inst = sampler(ST_MODE_FORWARD);
    inst.loop_start = 0.5;
    inst.loop_end = 0.75;
    let out = render(&pcm, inst, 48000, 60, &[]);
    let (lo, hi) = (n as f64 * 0.5, n as f64 * 0.75);
    let width = hi - lo;
    // First pass covers the pre-loop region, then playback stays inside the loop
    // (the seam crossfade reads at most a few ms before the loop start).
    assert!((position(out[1000], n) - 1000.0).abs() < 3.0);
    let (mut minimum, mut maximum) = (1e9f64, -1e9f64);
    for value in &out[4000..] {
        minimum = minimum.min(position(*value, n));
        maximum = maximum.max(position(*value, n));
    }
    println!("forward: loop window {minimum:.1}..{maximum:.1} (loop {lo:.0}..{hi:.0})");
    assert!(minimum > lo - 250.0 && maximum < hi + 2.0);
    // Periodic with the loop length.
    for i in 20000..30000 {
        assert!((out[i] - out[i + width as usize]).abs() < 1e-5f32);
    }
}

#[test]
fn reverse() {
    let n: usize = 4800;
    let pcm = ramp(n);
    let mut inst = sampler(ST_MODE_REVERSE);
    inst.loop_start = 0.25;
    inst.loop_end = 0.5;
    let out = render(&pcm, inst, 48000, 60, &[]);
    let (lo, hi) = (n as f64 * 0.25, n as f64 * 0.5);
    let width = hi - lo;
    // Starts at the sample end and reads backwards.
    assert!(position(out[100], n) > (n - 110) as f64 && position(out[100], n) < (n - 90) as f64);
    assert!(position(out[1000], n) < position(out[900], n));
    let (mut minimum, mut maximum) = (1e9f64, -1e9f64);
    for value in &out[4000..] {
        minimum = minimum.min(position(*value, n));
        maximum = maximum.max(position(*value, n));
    }
    println!("reverse: loop window {minimum:.1}..{maximum:.1} (loop {lo:.0}..{hi:.0})");
    assert!(minimum > lo - 2.0 && maximum < hi + 250.0);
    for i in 20000..30000 {
        assert!((out[i] - out[i + width as usize]).abs() < 1e-5f32);
    }
    // Mostly decreasing inside the loop body.
    let (mut falling, mut rising) = (0, 0);
    for i in 20001..30000 {
        if out[i] < out[i - 1] {
            falling += 1;
        } else if out[i] > out[i - 1] {
            rising += 1;
        }
    }
    assert!(falling > rising * 3); // the rest is the seam crossfade on this ramp
}

#[test]
fn pingpong() {
    let n: usize = 4800;
    let pcm = ramp(n);
    let mut inst = sampler(ST_MODE_PINGPONG);
    inst.loop_start = 0.5;
    inst.loop_end = 0.75;
    let out = render(&pcm, inst, 48000, 60, &[]);
    let (lo, hi) = (n as f64 * 0.5, n as f64 * 0.75);
    let (mut minimum, mut maximum) = (1e9f64, -1e9f64);
    for value in &out[4000..] {
        minimum = minimum.min(position(*value, n));
        maximum = maximum.max(position(*value, n));
    }
    println!("ping-pong: window {minimum:.1}..{maximum:.1} (loop {lo:.0}..{hi:.0})");
    assert!(minimum > lo - 2.0 && maximum < hi + 1.0);
    // Continuous turnarounds: no jumps, both directions occur.
    assert!(max_step(&out, 4000, out.len()) < 2.5f32 / n as f32 * VOLUME);
    let (mut falling, mut rising) = (0, 0);
    for i in 4001..out.len() {
        if out[i] < out[i - 1] {
            falling += 1;
        } else if out[i] > out[i - 1] {
            rising += 1;
        }
    }
    assert!(falling > 10000 && rising > 10000);
}

#[test]
fn seam() {
    // Loop points deliberately away from zero crossings of a 100-frame sine.
    let n: usize = 9600;
    let pcm = sine(n, 100.0, 0.5, 0.0);
    let mut inst = sampler(ST_MODE_FORWARD);
    inst.loop_start = 0.3013;
    inst.loop_end = 0.7771;
    let mut out = render(&pcm, inst, 48000, 60, &[]);
    let natural = (2.0 * PI / 100.0 * 0.5 * VOLUME as f64) as f32;
    let mut step = max_step(&out, 100, out.len());
    println!("forward seam: max step {step:.4} (natural {natural:.4})");
    assert!(step < natural * 1.6f32);
    // Reverse loop seam too.
    inst.mode = ST_MODE_REVERSE;
    out = render(&pcm, inst, 48000, 60, &[]);
    step = max_step(&out, 100, out.len());
    println!("reverse seam: max step {step:.4}");
    assert!(step < natural * 1.6f32);
    // Loop that ends before the sample end and starts at the sample start uses the post-loop
    // audio.
    inst.mode = ST_MODE_FORWARD;
    inst.loop_start = 0.0;
    inst.loop_end = 0.6137;
    out = render(&pcm, inst, 48000, 60, &[]);
    step = max_step(&out, 100, out.len());
    println!("forward seam (post-loop crossfade): max step {step:.4}");
    assert!(step < natural * 1.6f32);
}

#[test]
fn interpolation() {
    // A 32-frame sine played an octave down must match a 64-frame sine at root.
    let n: usize = 32 * 400;
    let low = render(&sine(n, 32.0, 0.5, 0.0), sampler(ST_MODE_ONESHOT), 12000, 48, &[]);
    let reference = render(&sine(n * 2, 64.0, 0.5, 0.0), sampler(ST_MODE_ONESHOT), 12000, 60, &[]);
    let diff: Vec<f32> = (0..12000).map(|i| low[i] - reference[i]).collect();
    let error = rms(&diff, 1000, 11000) / rms(&reference, 1000, 11000);
    println!("interpolation: relative RMS error {error:.6} at half speed");
    assert!(error < 2e-4);
}

#[test]
fn slices() {
    let n: usize = 4800;
    let pcm = ramp(n);
    let mut inst = sampler(ST_MODE_SLICE);
    inst.slice_count = 4;
    for slice in 0..4 {
        let out = render(&pcm, inst, 3000, 60 + slice, &[]);
        assert!((position(out[100], n) - (slice * 1200 + 100) as f64).abs() < 3.0);
        for value in &out[1210..] {
            assert!(*value == 0.0);
        }
        assert!(max_step(&out, 2, out.len()) < 0.02f32);
    }
    // Manual markers.
    let out = render(&pcm, inst, 3000, 61, &[0.0, 0.1, 0.6]);
    assert!((position(out[100], n) - (480 + 100) as f64).abs() < 3.0);
    for value in &out[2410..] {
        assert!(*value == 0.0);
    }
}

fn rising_crossings(a: &[f32], from: usize, to: usize) -> usize {
    (from + 1..to).filter(|&i| a[i - 1] <= 0.0 && a[i] > 0.0).count()
}

#[test]
fn wavetable() {
    // Four 2048-frame single cycles with increasing amplitude.
    let mut pcm = vec![0.0f32; 2048 * 4];
    for frame in 0..4 {
        for i in 0..2048 {
            pcm[frame * 2048 + i] = 0.2f32 * (frame + 1) as f32 * (2.0 * PI * i as f64 / 2048.0).sin() as f32;
        }
    }
    let mut inst = sampler(ST_MODE_WAVETABLE);
    let mut level = [0.0f64; 5];
    for k in 0..=4 {
        inst.wavetable_position =
            if k == 4 { 1.0f32 } else { k as f32 / 4.0 + 0.0625f32 * (if k == 1 { 1.0 } else { 0.0 }) };
        let out = render(&pcm, inst, 48000, 69, &[]);
        let crossings = rising_crossings(&out, 0, 48000);
        level[k] = rms(&out, 1000, 47000) / VOLUME as f64 * 2.0f64.sqrt();
        // Every position is a clean 440 Hz sine: fit sin/cos and measure the residual.
        let (mut s, mut c, mut total) = (0.0f64, 0.0f64, 0.0f64);
        for i in 1000..47000 {
            let w = 2.0 * PI * 440.0 * i as f64 / RATE as f64;
            s += out[i] as f64 * w.sin();
            c += out[i] as f64 * w.cos();
            total += out[i] as f64 * out[i] as f64;
        }
        let fitted = 2.0 * (s * s + c * c) / 46000.0;
        let residual = (total - fitted).max(0.0) / total;
        println!(
            "wavetable: position {:.2} -> {} Hz, amplitude {:.3}, non-sine residual {:.5}",
            inst.wavetable_position, crossings, level[k], residual
        );
        assert!((439..=441).contains(&crossings));
        assert!(residual < 0.002);
    }
    assert!((level[0] - 0.2).abs() < 0.02 && (level[4] - 0.8).abs() < 0.02);
    for k in 1..=4 {
        assert!(level[k] > level[k - 1] + 0.05);
    }
    // Halfway between frames 1 and 2 blends them (no phase-misaligned window).
    assert!((level[2] - 0.5).abs() < 0.02);
}

#[test]
fn granular() {
    let n: usize = 48000;
    let pcm = ramp(n);
    let mut inst = sampler(ST_MODE_GRANULAR);
    let mut spread = |place: f32, size: f32| -> (f64, f64) {
        inst.grain_position = place;
        inst.grain_size = size;
        let out = render(&pcm, inst, 24000, 60, &[]);
        let (mut sum, mut lo, mut hi) = (0.0f64, 1e9f64, -1e9f64);
        let mut count = 0usize;
        for value in &out[4000..] {
            let p = position(*value, n);
            sum += p;
            count += 1;
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (sum / count as f64, hi - lo)
    };
    let (m1, _w1) = spread(0.25, 0.05);
    let (m2, w2) = spread(0.75, 0.05);
    let (_m3, w3) = spread(0.75, 0.2);
    println!(
        "granular: mean {:.0}/{:.0} (expect {:.0}/{:.0}), span {:.0} -> {:.0} with larger grains",
        m1,
        m2,
        n as f64 * 0.25,
        n as f64 * 0.75,
        w2,
        w3
    );
    assert!((m1 - n as f64 * 0.25).abs() < n as f64 * 0.02 && (m2 - n as f64 * 0.75).abs() < n as f64 * 0.02);
    assert!(w2 < n as f64 * 0.06 && w3 > w2 * 2.5);
}

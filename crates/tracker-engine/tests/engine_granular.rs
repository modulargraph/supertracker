//! Port of the former C++ test program `engine_granular_tests.cpp`: sampler Granular parameters, live instrument
//! parameters and the voice playhead snapshot.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations, reset_render_allocations};
use engine_support_b::engine::Engine;
use std::collections::BTreeSet;
use std::f64::consts::PI;
use tracker_engine::engine::*;

const N: usize = 48000;

fn sampler(mode: i32) -> STInstrument {
    let mut p = default_instrument();
    p.mode = mode;
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.release = 0.005;
    p.volume = 1.0;
    p.cutoff = 22000.0;
    p
}

fn sine_hz(frames: usize, hz: f64) -> Vec<f32> {
    (0..frames).map(|i| (0.5 * (2.0 * PI * hz * i as f64 / 48000.0).sin()) as f32).collect()
}

/// C++ `sine(frames = N, hz = 441)`
fn sine_frames(frames: usize) -> Vec<f32> {
    sine_hz(frames, 441.0)
}

fn sine() -> Vec<f32> {
    sine_frames(N)
}

fn noise_seeded(frames: usize, mut seed: u32) -> Vec<f32> {
    let mut pcm = vec![0.0f32; frames];
    for x in &mut pcm {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *x = (seed as i32 as f64 / 2147483648.0 * 0.5) as f32;
    }
    pcm
}

/// C++ `noise(frames = N)`: seed 7.
fn noise() -> Vec<f32> {
    noise_seeded(N, 7)
}

fn setup(e: &mut Engine, p: STInstrument, pcm: &[f32], rows: i32) {
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0, 1.0);
    let mut track = default_track();
    track.volume = 1.0;
    track.delay_send = 0.0;
    track.reverb_send = 0.0;
    e.set_track(0, track);
    e.set_instrument(0, p);
    assert!(e.set_sample(0, pcm, None, 48000.0));
    e.set_pattern(0, rows);
}

fn publish(e: &mut Engine) {
    e.commit_update();
}

struct Out {
    l: Vec<f32>,
    r: Vec<f32>,
}

fn render_into(e: &mut Engine, o: &mut Out, block: usize) {
    let frames = o.l.len();
    let mut at = 0;
    while at < frames {
        let count = block.min(frames - at);
        e.render(&mut o.l[at..at + count], &mut o.r[at..at + count]);
        at += block;
    }
}

fn render(e: &mut Engine, frames: usize) -> Out {
    let mut o = Out { l: vec![0.0; frames], r: vec![0.0; frames] };
    render_into(e, &mut o, 256);
    o
}

fn rms(x: &[f32], from: usize, to: usize) -> f64 {
    let s: f64 = x[from..to].iter().map(|&v| v as f64 * v as f64).sum();
    (s / (to - from) as f64).sqrt()
}

fn positions(e: &Engine) -> Vec<STVoicePosition> {
    let mut v = vec![STVoicePosition::default(); ST_VOICE_POSITION_CAPACITY];
    let (count, _serial) = e.voice_positions(&mut v);
    v.truncate(count);
    v
}

#[test]
fn granular_parameters() {
    // grainDensity 0 (projects saved before it existed) reads as the historical 2; out-of-range
    // values are clamped.
    {
        let (mut a, mut b) = (Engine::new(), Engine::new());
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_density = 0;
        setup(&mut a, p, &sine(), 16);
        publish(&mut a);
        p.grain_density = 2;
        setup(&mut b, p, &sine(), 16);
        publish(&mut b);
        a.note_on(0, 0, 60, 100);
        b.note_on(0, 0, 60, 100);
        let (x, y) = (render(&mut a, 9600), render(&mut b, 9600));
        for i in 0..x.l.len() {
            assert!(x.l[i] == y.l[i]);
        }
    }
    // Density: more overlapping grains keep the level (normalized) and stay smooth.
    let mut levels = [0.0f64; 3];
    let mut index = 0;
    for density in [1, 2, 6] {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_density = density;
        p.grain_size = 0.05;
        setup(&mut e, p, &noise(), 16);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.note_on(0, 0, 60, 100);
        let o = render(&mut e, 24000);
        levels[index] = rms(&o.l, 4800, 24000);
        index += 1;
        let snap = positions(&e);
        assert!(snap.len() == 1 && snap[0].grain_count == density);
        println!("density {} -> rms {:.3}", density, levels[index - 1]);
    }
    assert!(levels[1] > 0.1 && (levels[2] / levels[1] - 1.0).abs() < 0.3);
    println!("PASS density (0 = historical 2, overlap normalized)");

    // Scan 1x moves the read position through the region at the original speed; 0 freezes it; -1
    // runs backwards.
    for scan in [0.0f32, 1.0, -1.0] {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_scan = scan;
        p.grain_position = 0.5;
        setup(&mut e, p, &sine_frames(N), 16);
        publish(&mut e);
        e.watch_voice_positions(-1);
        e.note_on(0, 0, 72, 100);
        render(&mut e, 256);
        let first = positions(&e)[0].position;
        render(&mut e, 9600);
        let snap = positions(&e);
        let moved = snap[0].position - first;
        println!("scan {:.0} -> moved {:.0} frames, direction {}", scan, moved, snap[0].direction);
        assert!((moved - (scan * 9600.0f32) as f64).abs() < 2.0);
        assert!(
            snap[0].direction
                == (if scan > 0.0 {
                    1
                } else if scan < 0.0 {
                    -1
                } else {
                    0
                })
        );
    }
    println!("PASS scan (pitch independent: the note is an octave up, the scan is still 1x)");

    // Spray spreads grain positions around the scan point; without it every grain reads near the
    // centre.
    for spray in [0.0f32, 0.1f32] {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_spray = spray;
        p.grain_density = 8;
        p.grain_size = 0.02;
        setup(&mut e, p, &sine(), 16);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.note_on(0, 0, 60, 100);
        let mut spread = 0.0f64;
        for _block in 0..60 {
            render(&mut e, 256);
            for v in positions(&e) {
                for k in 0..v.grain_count as usize {
                    spread = spread.max((v.grain_positions[k] - v.position).abs());
                }
            }
        }
        println!("spray {spray:.2} s -> max grain offset {spread:.0} frames");
        if spray == 0.0 {
            assert!(spread <= 0.02 * 48000.0 * 0.5 + 2.0);
        } else {
            assert!(spread > 2000.0 && spread <= 0.02 * 48000.0 * 0.5 + 0.1 * 48000.0 + 2.0);
        }
    }
    println!("PASS spray");

    // Reverse 1: every grain reads backwards (its read position falls while its window advances).
    {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_reverse = 1.0;
        p.grain_density = 1;
        p.grain_size = 0.2;
        setup(&mut e, p, &sine(), 16);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.note_on(0, 0, 60, 100);
        render(&mut e, 512);
        let mut a = positions(&e)[0].grain_positions[0];
        render(&mut e, 256);
        let mut b = positions(&e)[0].grain_positions[0];
        assert!(b < a && ((a - b) - 256.0).abs() < 2.0);
        p.grain_reverse = 0.0;
        let mut f = Engine::new();
        setup(&mut f, p, &sine(), 16);
        publish(&mut f);
        f.watch_voice_positions(0);
        f.note_on(0, 0, 60, 100);
        render(&mut f, 512);
        a = positions(&f)[0].grain_positions[0];
        render(&mut f, 256);
        b = positions(&f)[0].grain_positions[0];
        assert!(b > a);
    }
    println!("PASS reverse probability");

    // Pitch spread changes grain pitch (output differs, stays finite); stereo spread makes a mono
    // sample stereo.
    {
        let (mut a, mut b) = (Engine::new(), Engine::new());
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_density = 4;
        setup(&mut a, p, &sine(), 16);
        publish(&mut a);
        p.grain_pitch_spread = 12.0;
        p.grain_stereo = 1.0;
        setup(&mut b, p, &sine(), 16);
        publish(&mut b);
        a.note_on(0, 0, 60, 100);
        b.note_on(0, 0, 60, 100);
        let (x, y) = (render(&mut a, 24000), render(&mut b, 24000));
        let (mut diff, mut side, mut side_a) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..x.l.len() {
            assert!(y.l[i].is_finite() && y.r[i].is_finite());
            diff += (x.l[i] - y.l[i]).abs() as f64;
            side += (y.l[i] - y.r[i]).abs() as f64;
            side_a += (x.l[i] - x.r[i]).abs() as f64;
        }
        assert!(diff > 100.0 && side > 100.0 && side_a < 1e-3);
    }
    println!("PASS pitch spread and stereo spread");

    // Automation targets, the LFO grain-size destination and POS step effect reach granular
    // voices.
    {
        let mut e = Engine::new();
        let p = sampler(ST_MODE_GRANULAR);
        setup(&mut e, p, &sine(), 4);
        let mut step = default_step();
        step.note = 60;
        step.instrument = 0;
        step.velocity = 100;
        step.gate = 4.0;
        step.fx1 = STStepFX { type_: ST_FX_POSITION, value: 0.25 };
        e.set_step(0, 0, 0, step, &[]);
        e.add_automation_point(0, 0, ST_TARGET_GRAIN_SCAN, -1, 0, 0, 0.0, 2.0);
        e.add_automation_point(0, 0, ST_TARGET_GRAIN_DENSITY, -1, 0, 0, 0.0, 5.0);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.play(0);
        render(&mut e, 512);
        let a = positions(&e);
        assert!(a.len() == 1 && a[0].grain_count == 5 && a[0].direction == 1);
        let start = a[0].position;
        assert!((start - (0.25 * (N - 1) as f64 + 2.0 * 512.0)).abs() < 600.0);
        render(&mut e, 2400);
        let b = positions(&e);
        assert!(((b[0].position - start) - 2.0 * 2400.0).abs() < 4.0);
    }
    {
        let (mut a, mut b) = (Engine::new(), Engine::new());
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_size = 0.02;
        setup(&mut a, p, &sine(), 16);
        publish(&mut a);
        p.lfo_target = ST_MOD_GRAIN_SIZE;
        p.lfo_depth = 1.0;
        p.lfo_rate = 3.0;
        setup(&mut b, p, &sine(), 16);
        publish(&mut b);
        a.watch_voice_positions(0);
        b.watch_voice_positions(0);
        a.note_on(0, 0, 60, 100);
        b.note_on(0, 0, 60, 100);
        let mut spans = BTreeSet::new();
        for _ in 0..40 {
            render(&mut a, 256);
            render(&mut b, 256);
            spans.insert((positions(&b)[0].window_end - positions(&b)[0].position).round() as i64);
            assert!((positions(&a)[0].window_end - positions(&a)[0].position).round() as i64 == 480);
        }
        assert!(spans.len() > 10);
    }
    println!("PASS grain automation targets, LFO grain size and POS step effect");
}

#[test]
fn live_positions() {
    // Forward loop: the playhead advances (+1) and wraps inside the loop.
    {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_FORWARD);
        p.loop_start = 0.5;
        p.loop_end = 0.6;
        setup(&mut e, p, &sine(), 16);
        publish(&mut e);
        assert!(positions(&e).is_empty());
        e.note_on(0, 0, 60, 100);
        render(&mut e, 1024);
        assert!(positions(&e).is_empty()); // not watched yet
        e.watch_voice_positions(1);
        render(&mut e, 256);
        assert!(positions(&e).is_empty()); // another instrument
        e.watch_voice_positions(0);
        render(&mut e, 256);
        let a = positions(&e);
        assert!(a.len() == 1 && a[0].direction == 1 && a[0].mode == ST_MODE_FORWARD && a[0].sample_frames == N as f64);
        assert!(
            (a[0].position - 1536.0).abs() < 2.0 && a[0].instrument == 0 && a[0].note == 60 && a[0].level > 0.99f32
        );
        render(&mut e, 48000 - 1536 + 2400);
        let b = positions(&e);
        assert!(b[0].position >= 0.5 * N as f64 && b[0].position < 0.6 * N as f64);
        e.watch_voice_positions(-2);
        render(&mut e, 256);
        assert!(positions(&e).is_empty());
    }
    // Backward loop: starts at the end, runs down (-1).
    {
        let mut e = Engine::new();
        let p = sampler(ST_MODE_REVERSE);
        setup(&mut e, p, &sine(), 16);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.note_on(0, 0, 60, 100);
        render(&mut e, 256);
        let a = positions(&e);
        render(&mut e, 256);
        let b = positions(&e);
        assert!(
            a[0].direction == -1
                && b[0].position < a[0].position
                && (a[0].position - b[0].position - 256.0).abs() < 2.0
                && a[0].position > (N - 300) as f64
        );
    }
    // Ping-pong bounces: both directions appear and the head stays inside the loop.
    {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_PINGPONG);
        p.loop_start = 0.1;
        p.loop_end = 0.2;
        setup(&mut e, p, &sine(), 16);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.note_on(0, 0, 84, 100);
        let mut dirs = BTreeSet::new();
        render(&mut e, 6000);
        for _ in 0..100 {
            render(&mut e, 256);
            let v = positions(&e);
            dirs.insert(v[0].direction);
            assert!(v[0].position >= 0.1 * N as f64 - 1.0 && v[0].position <= 0.2 * N as f64);
        }
        assert!(dirs.contains(&1) && dirs.contains(&-1));
    }
    // Slice: the slice the note picks.
    {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_SLICE);
        p.slice_count = 4;
        setup(&mut e, p, &sine(), 16);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.note_on(0, 0, 62, 100);
        render(&mut e, 512);
        let v = positions(&e);
        assert!(
            v.len() == 1 && v[0].position >= 0.5 * N as f64 && v[0].position < 0.75 * N as f64 && v[0].direction == 1
        );
    }
    // Wavetable: the current frame region.
    {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_WAVETABLE);
        p.wavetable_position = 1.0;
        p.wavetable_frame_size = 2048;
        setup(&mut e, p, &sine_frames(8 * 2048), 16);
        publish(&mut e);
        e.watch_voice_positions(0);
        e.note_on(0, 0, 60, 100);
        render(&mut e, 2048);
        let v = positions(&e);
        assert!(v.len() == 1 && v[0].window_start == (7 * 2048) as f64 && v[0].window_end == (8 * 2048) as f64);
    }
    // Granular: grains around the scan centre; a released voice is flagged; polyphony reports
    // every voice.
    {
        let mut e = Engine::new();
        let mut p = sampler(ST_MODE_GRANULAR);
        p.grain_density = 3;
        p.release = 0.5;
        setup(&mut e, p, &sine(), 16);
        publish(&mut e);
        e.watch_voice_positions(-1);
        e.note_on(0, 0, 60, 100);
        e.note_on(0, 0, 67, 100);
        render(&mut e, 1024);
        let mut v = positions(&e);
        assert!(
            v.len() == 2
                && v[0].grain_count == 3
                && v[0].window_start < v[0].position
                && v[0].window_end > v[0].position
        );
        for k in 0..3 {
            assert!(
                v[0].grain_positions[k] >= v[0].window_start - 1.0
                    && v[0].grain_positions[k] <= v[0].window_end + 1.0
                    && v[0].grain_levels[k] >= 0.0
                    && v[0].grain_levels[k] <= 1.0
            );
        }
        e.note_off(0, -1);
        render(&mut e, 256);
        v = positions(&e);
        assert!(v.len() == 2 && (v[0].flags & ST_VOICE_POSITION_RELEASING) != 0);
    }
    println!("PASS voice positions: forward, backward, ping-pong, slice, wavetable, granular, watch filter");
}

fn high_band(x: &[f32], from: usize, to: usize) -> f64 {
    let mut s = 0.0f64;
    for i in from + 1..to {
        let d = x[i] as f64 - x[i - 1] as f64;
        s += d * d;
    }
    (s / (to - from) as f64).sqrt()
}

#[test]
fn live_parameters() {
    // A held note follows volume and cutoff edits within a couple of blocks, without clicks and
    // without a publish.
    let noise = noise_seeded(N, 1);
    let mut e = Engine::new();
    let p = sampler(ST_MODE_FORWARD);
    setup(&mut e, p, &noise, 16);
    publish(&mut e);
    e.note_on(0, 0, 60, 100);
    let before = render(&mut e, 4800);
    let (level, bright) = (rms(&before.l, 2400, 4800), high_band(&before.l, 2400, 4800));
    assert!(e.set_instrument_parameter(0, ST_TARGET_INSTRUMENT_VOLUME, 0.25));
    assert!(e.set_instrument_parameter(0, ST_TARGET_CUTOFF, 400.0));
    assert!(!e.set_instrument_parameter(0, ST_TARGET_VOLUME, 1.0)); // track volume is not an instrument parameter
    assert!(!e.set_instrument_parameter(0, 99, 1.0));
    let mut after = Out { l: vec![0.0; 9600], r: vec![0.0; 9600] };
    reset_render_allocations();
    realtime(|| render_into(&mut e, &mut after, 256));
    assert!(render_allocations() == 0);
    let (level2, bright2) = (rms(&after.l, 4800, 9600), high_band(&after.l, 4800, 9600));
    println!("live: level {level:.3} -> {level2:.3}, high band {bright:.3} -> {bright2:.3}");
    assert!(level2 < level * 0.45 && bright2 < bright * 0.2);
    // The first blocks already move (smoothing, not a step) and nothing jumps beyond the source's
    // own steps.
    assert!(rms(&after.l, 256, 512) < level * 0.98);
    // Notes started before the next publication use the live value; a publication replaces it.
    e.note_off(0, -1);
    render(&mut e, 4800);
    e.note_on(0, 0, 60, 100);
    let next = render(&mut e, 4800);
    assert!(rms(&next.l, 2400, 4800) < level * 0.45);
    e.note_off(0, -1);
    render(&mut e, 4800);
    setup(&mut e, p, &noise, 16);
    publish(&mut e);
    render(&mut e, 256);
    e.note_on(0, 0, 60, 100);
    let republished = render(&mut e, 4800);
    assert!(rms(&republished.l, 2400, 4800) > level * 0.8);
    // Granular parameters are live too.
    let mut g = Engine::new();
    let q = sampler(ST_MODE_GRANULAR);
    setup(&mut g, q, &sine(), 16);
    publish(&mut g);
    g.watch_voice_positions(0);
    g.note_on(0, 0, 60, 100);
    render(&mut g, 512);
    assert!(positions(&g)[0].grain_count == 2);
    g.set_instrument_parameter(0, ST_TARGET_GRAIN_DENSITY, 7.0);
    g.set_instrument_parameter(0, ST_TARGET_GRAIN_POSITION, 0.9);
    render(&mut g, 512);
    let v = positions(&g);
    assert!(v[0].grain_count == 7 && (v[0].position - 0.9 * (N - 1) as f64).abs() < 2.0);
    println!("PASS live instrument parameters (held notes, next notes, cleared by publication, no allocation)");
}

#[test]
fn no_allocations() {
    let mut e = Engine::new();
    let mut p = sampler(ST_MODE_GRANULAR);
    p.grain_density = 8;
    p.grain_spray = 0.2;
    p.grain_pitch_spread = 7.0;
    p.grain_scan = 0.5;
    p.grain_reverse = 0.5;
    p.grain_stereo = 1.0;
    setup(&mut e, p, &sine(), 16);
    publish(&mut e);
    e.watch_voice_positions(-1);
    for n in 0..16 {
        e.note_on(0, 0, 48 + n, 100);
    }
    render(&mut e, 256);
    let mut out = Out { l: vec![0.0; 48000], r: vec![0.0; 48000] };
    let mut snap = vec![STVoicePosition::default(); ST_VOICE_POSITION_CAPACITY];
    reset_render_allocations();
    let count = realtime(|| {
        render_into(&mut e, &mut out, 128);
        e.voice_positions(&mut snap).0
    });
    assert!(render_allocations() == 0);
    assert!(count == 16);
    println!("PASS granular rendering and snapshots allocate nothing");
}

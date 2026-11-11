//! Port of the former C++ test program `engine_render_budget_tests.cpp`: representative projects rendered the way the
//! host pulls them (every channel bus plus the return, per small quantum, host mixing), timed in
//! thread CPU against generous budgets, with zero render allocations. Budgets are a coarse
//! regression guard that holds under a parallel test run, not a device deadline.
//!
//! The budgets are those of the C++ program (which was built `-O2`); they are enforced in
//! optimized builds only. An unoptimized (`debug_assertions`) build still renders every scenario,
//! asserts zero render allocations and the PCM hashes, and prints its CPU share.
//!
//! The C++ `--bench` mode (20 seconds per scenario, numbers and hashes, no budgets) is the
//! ignored test `bench`.
#![cfg(feature = "engine")]

mod engine_support_b;
#[allow(unused_imports)]
use engine_support_b::{both_views, host_view_only, mix_view, mix_view_only};

use engine_support_b::alloc::{realtime, render_allocations, reset_render_allocations};
use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use engine_support_b::{hash_f32, thread_cpu_seconds, FNV_OFFSET};
use tracker_engine::engine::*;

const RATE: f64 = 48000.0;

fn commit(e: &mut Engine) {
    if let Err(error) = e.commit_update_checked() {
        panic!("commit: {error}");
    }
}

fn sample(e: &mut Engine, instrument: i32, frames: usize, seed: i32) {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut s: u32 = (seed * 7919 + 1) as u32;
    for i in 0..frames {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        let noise = (s as i32) as f32 / 2147483647.0f32;
        let decay = (-(i as f64) / frames as f64 * 5.0).exp() as f32;
        let tone = (i as f64 * 2.0 * std::f64::consts::PI * (60 + seed * 40) as f64 / RATE).sin() as f32;
        l[i] = (tone * 0.7f32 + noise * 0.3f32) * decay * 0.5f32;
        r[i] = (tone * 0.6f32 + noise * 0.4f32) * decay * 0.5f32;
    }
    assert!(e.set_sample(instrument, &l, Some(&r), RATE));
}

fn step(e: &mut Engine, track: i32, row: i32, note: i32, gate: f32, chord: &[i32]) {
    let mut s = default_step();
    s.note = note;
    s.velocity = 100;
    s.gate = gate;
    e.set_step(0, track, row, s, chord);
}

/// The user's session: eight empty template tracks, a 16-pad kit of one-shot sampler tracks
/// (60 ms .. 1.6 s, a drum pattern), two wavetable pads playing chords and a looping sampler
/// bass.
fn template_kit(e: &mut Engine) {
    e.begin_update();
    assert!(e.set_track_count(27));
    e.set_instrument_count(32);
    e.set_timing(120.0, 4, 0.0, 0.8);
    e.set_pattern(0, 64);
    for pad in 0..16 {
        let mut p = default_instrument();
        p.mode = ST_MODE_ONESHOT;
        p.volume = 0.5;
        p.attack = 0.0;
        p.decay = 0.0;
        p.sustain = 1.0;
        p.release = 0.05;
        e.set_instrument(pad, p);
        sample(e, pad, (RATE * (0.06 + 0.1 * pad as f64)) as usize, pad);
        let mut t = default_track();
        t.instrument = pad;
        e.set_track(8 + pad, t);
        e.set_track_voice(8 + pad, 0, 1, 1);
        for row in 0..64 {
            let hit = if pad == 0 {
                row % 4 == 0
            } else if pad == 1 {
                row % 8 == 4
            } else if pad == 2 {
                row % 2 == 0
            } else if pad == 3 {
                row % 16 == 14
            } else {
                row % 16 == (pad * 5) % 16
            };
            if hit {
                step(e, 8 + pad, row, 60, 1.0, &[]);
            }
        }
    }
    for synth in 0..3 {
        let i = 16 + synth;
        let mut p = test_tone_instrument(TEST_SAW);
        p.volume = 0.3;
        if synth < 2 {
            p.cutoff = 4000.0;
            p.resonance = 0.2;
            p.attack = 0.01;
            p.decay = 0.3;
            p.sustain = 0.7;
            p.release = 0.3;
            p.lfo_rate = 0.5;
            p.lfo_depth = 0.2;
            p.lfo_target = ST_MOD_POSITION;
            p.wavetable_position = 0.3;
            e.set_instrument(i, p);
            load_test_tone(e, i, TEST_SAW);
        } else {
            p.mode = ST_MODE_FORWARD;
            p.loop_start = 0.1;
            p.loop_end = 0.9;
            p.decay = 0.2;
            p.sustain = 0.8;
            p.release = 0.1;
            e.set_instrument(i, p);
            sample(e, i, RATE as usize, 40);
        }
        let mut t = default_track();
        t.instrument = i;
        e.set_track(24 + synth, t);
        let chord = [0, 4, 7];
        for row in (0..64).step_by(16) {
            if synth < 2 {
                step(e, 24 + synth, row, 48 + synth * 7, 12.0, &chord);
            } else {
                step(e, 24 + synth, row, 36, 12.0, &[]);
            }
        }
    }
    commit(e);
}

/// Sixty-four sampler tracks playing a note every other row.
fn many_tracks(e: &mut Engine) {
    e.begin_update();
    assert!(e.set_track_count(64));
    e.set_instrument_count(64);
    e.set_timing(120.0, 4, 0.0, 0.8);
    e.set_pattern(0, 64);
    for t in 0..64 {
        let mut p = default_instrument();
        p.mode = if t % 2 != 0 { ST_MODE_ONESHOT } else { ST_MODE_FORWARD };
        p.volume = 0.1;
        p.attack = 0.0;
        p.decay = 0.1;
        p.sustain = 0.8;
        p.release = 0.05;
        e.set_instrument(t, p);
        let frames = (if t % 2 != 0 { RATE * (0.06 + 0.1 * (t % 16) as f64) } else { RATE }) as usize;
        sample(e, t, frames, t % 16);
        let mut track = default_track();
        track.instrument = t;
        track.pan = (t % 5 - 2) as f32 * 0.2f32;
        e.set_track(t, track);
        let mut row = t % 2;
        while row < 64 {
            step(e, t, row, 48 + (row + t) % 24, 1.5, &[]);
            row += 2;
        }
    }
    commit(e);
}

struct Outcome {
    percent: f64,
    hash: u64,
}

fn run(build: fn(&mut Engine), playing: bool, tracks: i32, block: usize, seconds: f64) -> Outcome {
    // One engine (at 48 kHz, `RATE`): the run is timed.
    let mut e = Engine::alone();
    build(&mut e);
    if playing {
        e.play(0);
    }
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let mut hash = FNV_OFFSET;
    let mut at: u64 = 0;
    let total: u64 = (seconds * RATE) as u64 / block as u64 * block as u64;
    let mut cpu = 0.0f64;
    let mut done: u64 = 0;
    while done < total {
        realtime(|| {
            let t0 = thread_cpu_seconds();
            e.render_block(at, block as u32);
            for c in -1..tracks {
                e.render_channel(c, at, &mut l, &mut r);
                for n in 0..block {
                    for v in [l[n], r[n]] {
                        hash = hash_f32(hash, v);
                    }
                }
            }
            while e.pop_event().is_some() {}
            cpu += thread_cpu_seconds() - t0;
        });
        done += block as u64;
        at += block as u64;
    }
    drop(e);
    Outcome { percent: cpu / (total as f64 / RATE) * 100.0, hash }
}

struct Case {
    name: &'static str,
    build: fn(&mut Engine),
    playing: bool,
    tracks: i32,
    budget: f64,
    /// PCM hashes of the 6-second run in 128- and 512-frame blocks, as rendered by the Rust
    /// engine on x86-64 Linux when these tests were ported (they equal the C++ engine's).
    pcm: [u64; 2],
}

// Budgets are 4-13x the measured cost on an M4 core (1.2%, 0.4%, 7%): they catch an
// order-of-magnitude regression, never scheduling noise from a loaded machine.
const CASES: [Case; 3] = [
    Case {
        name: "template-kit-playing",
        build: template_kit,
        playing: true,
        tracks: 27,
        budget: 10.0,
        pcm: [0xe0df736ea3152284, 0x2771f605f2f53bf8],
    },
    Case {
        name: "template-kit-stopped",
        build: template_kit,
        playing: false,
        tracks: 27,
        budget: 5.0,
        pcm: [0x411662941a3e2383, 0x1b97e38001038383],
    },
    Case {
        name: "sixty-four-tracks",
        build: many_tracks,
        playing: true,
        tracks: 64,
        budget: 40.0,
        pcm: [0x64552246f1cd4fe1, 0xcf5186b565f35cb7],
    },
];

/// The PCM hashes depend on the platform's `sin`/`exp` (the samples are generated here) and on
/// the target's floating-point code generation, so they are pinned for the platform they were
/// recorded on only.
const HASHES_APPLY: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

/// The C++ program measured its scenarios one after the other; the tests of this binary run on
/// parallel threads, so the timed scenarios take turns instead of timing each other's cache and
/// core contention.
static TIMED: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn scenario(c: &Case) {
    let _turn = TIMED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for (index, block) in [128usize, 512].into_iter().enumerate() {
        reset_render_allocations();
        let result = run(c.build, c.playing, c.tracks, block, 6.0);
        println!(
            "{:<22} block {:3}: engine {:.3}% of real time (budget {:.0}%), PCM {:016x}",
            c.name, block, result.percent, c.budget, result.hash
        );
        assert!(render_allocations() == 0);
        if cfg!(not(debug_assertions)) {
            assert!(result.percent <= c.budget, "FAIL {} exceeded its render budget", c.name);
        }
        if HASHES_APPLY {
            assert!(
                result.hash == c.pcm[index],
                "{} block {}: PCM {:016x}, recorded {:016x}",
                c.name,
                block,
                result.hash,
                c.pcm[index]
            );
        }
    }
}

#[test]
fn template_kit_playing() {
    host_view_only(|| {
    scenario(&CASES[0]);
    });
}

#[test]
fn template_kit_stopped() {
    host_view_only(|| {
    scenario(&CASES[1]);
    });
}

#[test]
fn sixty_four_tracks() {
    host_view_only(|| {
    scenario(&CASES[2]);
    });
}

/// C++ `--bench [seconds]`: prints the numbers and PCM hashes without enforcing budgets. The
/// duration comes from `ENGINE_RENDER_BUDGET_SECONDS` (default 20). Run with
/// `cargo test --release --test engine_render_budget -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark (C++ --bench): prints CPU share and PCM hashes for 20-second renders"]
fn bench() {
    host_view_only(|| {
    let _turn = TIMED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let seconds = std::env::var("ENGINE_RENDER_BUDGET_SECONDS").ok().and_then(|text| text.parse().ok()).unwrap_or(20.0);
    for c in &CASES {
        for block in [128usize, 512] {
            reset_render_allocations();
            let result = run(c.build, c.playing, c.tracks, block, seconds);
            println!(
                "{:<22} block {:3}: engine {:.3}% of real time (budget {:.0}%), PCM {:016x}",
                c.name, block, result.percent, c.budget, result.hash
            );
            assert!(render_allocations() == 0);
        }
    }
    println!("Engine render budget: all scenarios within budget with zero render allocations.");
    });
}

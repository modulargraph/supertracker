//! An independent deterministic DSP benchmark with no absolute timing pass threshold (CPU time
//! distinguishes engine work from competing builds / OS scheduling).
//!
//! * `benchmark` (ignored) renders every scene and prints a table (CPU and wall time, PCM hash,
//!   RMS). Run it with
//!   `cargo test --release --test engine_benchmark -- --ignored --nocapture`.
//! * One test per scene renders it, asserts that the output is finite, bounded and audible and
//!   that the render allocated nothing, and compares the PCM hash of the buses with the recorded
//!   value, so a refactor that changes a single output bit is caught.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations, reset_render_allocations};
use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use engine_support_b::{hash_f32, process_cpu_seconds, FNV_OFFSET};
use std::time::Instant;
use tracker_engine::engine::*;

const FRAMES: usize = 96000;
const BLOCK: usize = 256;

struct Scene {
    name: &'static str,
    voices: i32,
    /// A sampler mode, or -1 for a saw test tone (sampler in Wavetable mode).
    mode: i32,
    modulated: bool,
    /// PCM hash of the buses (every track's bus in turn, then the return bus), recorded on
    /// x86-64 Linux.
    buses: u64,
}

const SCENES: [Scene; 12] = [
    Scene { name: "saw", voices: 16, mode: -1, modulated: false, buses: 0x6eeb8d5ce52dc807 },
    Scene { name: "saw", voices: 32, mode: -1, modulated: false, buses: 0x1c5107e33004bc89 },
    Scene { name: "saw", voices: 64, mode: -1, modulated: false, buses: 0x35c7314fd0de79b9 },
    Scene { name: "saw", voices: 256, mode: -1, modulated: false, buses: 0xb9c26f1fe5cf23a1 },
    Scene { name: "sample-loop", voices: 16, mode: ST_MODE_FORWARD, modulated: false, buses: 0xc0f1325108d97c81 },
    Scene { name: "sample-loop", voices: 32, mode: ST_MODE_FORWARD, modulated: false, buses: 0xd8d69af726a6dd89 },
    Scene { name: "sample-loop", voices: 64, mode: ST_MODE_FORWARD, modulated: false, buses: 0x0f9a83ec295dc651 },
    Scene { name: "sample-loop", voices: 256, mode: ST_MODE_FORWARD, modulated: false, buses: 0x91bfffd90cac81ff },
    Scene { name: "saw-modulated", voices: 64, mode: -1, modulated: true, buses: 0xb7531957f4cd4ecd },
    Scene { name: "saw-modulated", voices: 256, mode: -1, modulated: true, buses: 0xf482785d66066c4f },
    Scene { name: "sample-mod", voices: 64, mode: ST_MODE_FORWARD, modulated: true, buses: 0xa175cf1ca23cefe1 },
    Scene { name: "granular-mod", voices: 64, mode: ST_MODE_GRANULAR, modulated: true, buses: 0x3ac1a2f9f9484235 },
];

/// The hashes depend on the platform's `sin`/`cos` (the sample is generated here) and on the
/// target's floating-point code generation; they are pinned for the platform they were recorded
/// on.
const HASHES_APPLY: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

struct Outcome {
    cpu_seconds: f64,
    wall_seconds: f64,
    hash: u64,
    rms: f64,
}

fn scene(s: &Scene) -> Outcome {
    let mut e = Engine::new();
    e.begin_update();
    e.set_track_count(16);
    e.set_timing(120.0, 4, 0.0);
    let mut p = default_instrument();
    p.volume = 0.12f32 / (s.voices as f32).sqrt();
    p.attack = 0.0;
    p.decay = 0.02;
    p.sustain = 1.0;
    p.release = 0.01;
    // mode < 0: a saw test tone (sampler in Wavetable mode).
    if s.mode < 0 {
        p.mode = ST_MODE_WAVETABLE;
        load_test_tone(&mut e, 0, TEST_SAW);
    }
    if s.mode >= 0 {
        p.type_ = ST_INSTRUMENT_SAMPLE;
        p.mode = s.mode;
        let sample: Vec<f32> =
            (0..16384).map(|n| ((n as f64 * 0.02).sin() * 0.6 + (n as f64 * 0.013).cos() * 0.2) as f32).collect();
        assert!(e.set_sample(0, &sample, None, 44100.0));
    }
    if s.modulated {
        p.cutoff = 1600.0;
        p.resonance = 0.5;
        p.drive = 0.15;
        p.lfo_rate = 1.7;
        p.lfo_depth = 0.3;
        p.lfo_target = ST_MOD_CUTOFF;
        p.lfo2_rate = 3.1;
        p.lfo2_depth = 0.07;
        p.lfo2_target = ST_MOD_PITCH;
        p.filter_env_amount = 12.0;
    }
    e.set_instrument(0, p);
    for n in 0..16 {
        let mut t = default_track();
        t.pan = (n % 3 - 1) as f32 * 0.15f32;
        t.delay_send = 0.1;
        t.reverb_send = 0.1;
        e.set_track(n, t);
    }
    e.commit_update();
    for n in 0..s.voices {
        e.note_on(n % 16, 0, 36 + n % 36, 80);
    }
    // What a host reads of each block: the bus of each of the 16 tracks, then the return bus.
    let mut buses: Vec<(Vec<f32>, Vec<f32>)> = (0..=16).map(|_| (vec![0.0f32; FRAMES], vec![0.0f32; FRAMES])).collect();
    let wall = Instant::now();
    let cpu = process_cpu_seconds();
    reset_render_allocations();
    realtime(|| {
        let mut offset = 0;
        while offset < FRAMES {
            let count = BLOCK.min(FRAMES - offset);
            for (bus, (left, right)) in buses.iter_mut().enumerate() {
                let channel = if bus < 16 { bus as i32 } else { -1 };
                e.render_channel(channel, offset as u64, &mut left[offset..offset + count], &mut right[offset..offset + count]);
            }
            offset += BLOCK;
        }
    });
    let cpu_seconds = process_cpu_seconds() - cpu;
    let wall_seconds = wall.elapsed().as_secs_f64();
    let mut energy = 0.0f64;
    let mut hash = FNV_OFFSET;
    for (left, right) in &buses {
        for n in 0..FRAMES {
            for value in [left[n], right[n]] {
                assert!(value.is_finite());
                assert!(value.abs() <= 1.0);
                energy += (value * value) as f64;
                hash = hash_f32(hash, value);
            }
        }
    }
    assert!(energy > 0.0);
    assert!(render_allocations() == 0);
    Outcome { cpu_seconds, wall_seconds, hash, rms: (energy / (FRAMES * 2 * buses.len()) as f64).sqrt() }
}

/// One scene of the regression set: the scene's assertions plus the recorded PCM hash.
fn regression(index: usize) {
    let s = &SCENES[index];
    let result = scene(s);
    println!("{:<14} {:3} voices | PCM {:016x} | RMS {:.7}", s.name, s.voices, result.hash, result.rms);
    if HASHES_APPLY {
        assert!(
            result.hash == s.buses,
            "{} {} voices: PCM {:016x}, recorded {:016x}",
            s.name,
            s.voices,
            result.hash,
            s.buses
        );
    }
}

/// Renders every scene and prints the table.
#[test]
#[ignore = "benchmark: prints CPU/wall time, PCM hash and RMS of every scene"]
fn benchmark() {
    for s in &SCENES {
        let result = scene(s);
        println!(
            "{:<14} {:3} voices | CPU {:.4}s {:6.2}x | wall {:.4}s {:6.2}x | PCM {:016x} | RMS {:.7}",
            s.name,
            s.voices,
            result.cpu_seconds,
            2.0 / result.cpu_seconds,
            result.wall_seconds,
            2.0 / result.wall_seconds,
            result.hash,
            result.rms
        );
    }
    println!("All benchmark renders finite, audible, bounded and allocation-free.");
}

macro_rules! regressions {
    ($($name:ident: $index:expr;)*) => {
        $(
            #[test]
            fn $name() {
                regression($index);
            }
        )*
    };
}

regressions! {
    saw_16_voices: 0;
    saw_32_voices: 1;
    saw_64_voices: 2;
    saw_256_voices: 3;
    sample_loop_16_voices: 4;
    sample_loop_32_voices: 5;
    sample_loop_64_voices: 6;
    sample_loop_256_voices: 7;
    saw_modulated_64_voices: 8;
    saw_modulated_256_voices: 9;
    sample_mod_64_voices: 10;
    granular_mod_64_voices: 11;
}

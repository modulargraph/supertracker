//! Port of the former C++ test program `engine_dsp_benchmark.cpp`: the offline engine performance and bit-exact
//! differential fixture. No devices or plug-ins.
//!
//! * `benchmark` (ignored) is the C++ program: it renders every scenario and prints the same JSON
//!   line per scenario (timings, PCM hash, render allocations). The repetition count
//!   and the scenario filter (`argv[1]`, `argv[2]`) are `ENGINE_DSP_BENCHMARK_REPETITIONS` and
//!   `ENGINE_DSP_BENCHMARK_SCENARIO`. Run it with
//!   `cargo test --release --test engine_dsp_benchmark -- --ignored --nocapture`.
//! * One test per scenario renders it with the program's own assertions (finite, bounded,
//!   audible, zero render allocations) and compares the PCM hash with the recorded value.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations, reset_render_allocations};
use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use engine_support_b::{hash_f32, process_cpu_seconds, FNV_OFFSET};
use std::time::Instant;
use tracker_engine::engine::*;

const SAMPLE_RATE: f64 = 48000.0;
const FRAMES: usize = 96000;
const BLOCK: usize = 256;

struct Scenario {
    name: &'static str,
    voices: i32,
    /// A sampler mode, or -1 for a saw test tone (sampler in Wavetable mode).
    mode: i32,
    modulated: bool,
    /// Region automation lanes, and instrument edits committed while the scene renders.
    edits: bool,
    /// The PCM hash of the buses (per block: every track's bus in turn, then the return bus),
    /// recorded on x86-64 Linux.
    buses: u64,
}

const fn scenario(name: &'static str, voices: i32, mode: i32, modulated: bool, edits: bool, buses: u64) -> Scenario {
    Scenario { name, voices, mode, modulated, edits, buses }
}

const SCENARIOS: [Scenario; 11] = [
    scenario("saw-16", 16, -1, false, false, 0x478a6782c56b66ef),
    scenario("saw-64", 64, -1, false, false, 0xb13fe802672a8dd9),
    scenario("saw-256", 256, -1, false, false, 0x16b0f828e3fc78a1),
    scenario("loop-64", 64, ST_MODE_FORWARD, false, false, 0x9b174747dda3dfc5),
    scenario("loop-256", 256, ST_MODE_FORWARD, false, false, 0x4548d96cf82b85b5),
    scenario("saw-mod-64", 64, -1, true, false, 0xfbcf5d7a165b780d),
    scenario("saw-mod-256", 256, -1, true, false, 0xc8835654ac7a5267),
    scenario("granular-mod-64", 64, ST_MODE_GRANULAR, true, false, 0xcb768348bbae8ac1),
    scenario("wavetable-mod-64", 64, ST_MODE_WAVETABLE, true, false, 0x83752c424f18602e),
    scenario("slice-mod-64", 64, ST_MODE_SLICE, true, false, 0x2c5a75740e8091f3),
    scenario("region-edits-64", 64, ST_MODE_FORWARD, false, true, 0xc985dcb5779e1bd9),
];

/// The hashes depend on the platform's `sin`/`cos` (the sample is generated here) and on the
/// target's floating-point code generation; they are pinned for the platform they were recorded
/// on.
const HASHES_APPLY: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

struct Outcome {
    cpu_ms: f64,
    wall_ms: f64,
    block_median_us: f64,
    block_p95_us: f64,
    pcm: u64,
    allocations: usize,
}

fn scene(scenario: &Scenario) -> Outcome {
    let mut e = Engine::with_rate(SAMPLE_RATE);
    e.begin_update();
    e.set_track_count(16);
    e.set_timing(127.0, 4, 0.17, 0.7);
    let mut p = default_instrument();
    p.volume = 0.12f32 / (scenario.voices as f32).sqrt();
    p.attack = 0.0;
    p.decay = 0.02;
    p.sustain = 1.0;
    p.release = 0.01;
    if scenario.mode < 0 {
        p.mode = ST_MODE_WAVETABLE;
        load_test_tone(&mut e, 0, TEST_SAW);
    }
    if scenario.mode >= 0 {
        p.type_ = ST_INSTRUMENT_SAMPLE;
        p.mode = scenario.mode;
        p.start = 0.073;
        p.end = 0.917;
        p.loop_start = 0.18;
        p.loop_end = 0.79;
        p.slice_count = 2;
        let (mut left, mut right) = (vec![0.0f32; 32768], vec![0.0f32; 32768]);
        for n in 0..left.len() {
            let x = n as f64;
            left[n] = ((x * 0.02).sin() * 0.6 + (x * 0.013).cos() * 0.2) as f32;
            right[n] = ((x * 0.017).sin() * 0.4 - (x * 0.023).cos() * 0.3) as f32;
        }
        assert!(e.set_sample(0, &left, Some(&right), 44100.0));
    }
    if scenario.modulated {
        p.cutoff = 1600.0;
        p.resonance = 0.5;
        p.drive = 0.15;
        p.bit_depth = 11;
        p.lfo_rate = 1.7;
        p.lfo_depth = 0.3;
        p.lfo_target = ST_MOD_CUTOFF;
        p.lfo2_rate = 3.1;
        p.lfo2_depth = 0.07;
        p.lfo2_target = ST_MOD_PITCH;
        p.filter_env_amount = 12.0;
    }
    if scenario.edits {
        p.lfo_rate = 2.37;
        p.lfo_depth = 0.0;
        p.lfo2_rate = 3.11;
        p.lfo2_depth = 0.0;
        p.lfo2_shape = ST_LFO_RANDOM;
        p.lfo2_target = ST_MOD_POSITION;
    }
    e.set_instrument(0, p);
    for n in 0..16 {
        let mut t = default_track();
        t.pan = (n % 3 - 1) as f32 * 0.15f32;
        t.delay_send = 0.1;
        t.reverb_send = 0.1;
        e.set_track(n, t);
    }
    e.set_pattern(0, 64);
    e.set_sequence(&[0], true, 0);
    if scenario.edits {
        for track in 0..16 {
            e.add_automation_point(0, track, ST_TARGET_START, 0, 0, 0, 0.0, 0.073);
            e.add_automation_point(0, track, ST_TARGET_START, 0, 0, 0, 6.0, 0.11);
            e.add_automation_point(0, track, ST_TARGET_END, 0, 0, 0, 0.0, 0.917);
            e.add_automation_point(0, track, ST_TARGET_END, 0, 0, 0, 6.0, 0.87);
        }
    }
    e.commit_update();
    e.play(0);
    for n in 0..scenario.voices {
        e.note_on(n % 16, 0, 36 + n % 36, 80);
    }
    let (mut left, mut right) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
    let mut block_us: Vec<f64> = Vec::with_capacity(FRAMES.div_ceil(BLOCK));
    let mut pcm = FNV_OFFSET;
    let mut energy = 0.0f64;
    reset_render_allocations();
    let cpu = process_cpu_seconds();
    let start = Instant::now();
    let mut offset = 0usize;
    while offset < FRAMES {
        if scenario.mode == ST_MODE_SLICE && offset > 0 && offset.is_multiple_of(8192) {
            for n in 0..scenario.voices {
                e.note_on(n % 16, 0, 36 + n % 36, 80);
            }
        }
        if scenario.edits && offset == 32768 {
            p.lfo_depth = 0.2;
            p.lfo2_depth = 0.15;
            p.bit_depth = 9;
            e.begin_update();
            e.set_instrument(0, p);
            e.commit_update();
        }
        if scenario.edits && offset == 65536 {
            p.lfo_depth = 0.0;
            p.lfo2_depth = 0.0;
            p.pan = 0.63;
            p.bit_depth = 16;
            e.begin_update();
            e.set_instrument(0, p);
            e.commit_update();
        }
        let count = BLOCK.min(FRAMES - offset);
        let begin = Instant::now();
        realtime(|| e.render_block(offset as u64, count as u32));
        block_us.push(begin.elapsed().as_secs_f64() * 1e6);
        // What a host reads of the block: the bus of each of the 16 tracks, then the return bus.
        for channel in (0..16).chain([-1]) {
            realtime(|| e.render_channel(channel, offset as u64, &mut left[..count], &mut right[..count]));
            for n in 0..count {
                for value in [left[n], right[n]] {
                    assert!(value.is_finite());
                    assert!(value.abs() <= 1.0);
                    energy += (value * value) as f64;
                    pcm = hash_f32(pcm, value);
                }
            }
        }
        while e.pop_event().is_some() {}
        offset += BLOCK;
    }
    let cpu_ms = (process_cpu_seconds() - cpu) * 1000.0;
    let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert!(energy > 0.0);
    assert!(render_allocations() == 0);
    block_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Outcome {
        cpu_ms,
        wall_ms,
        block_median_us: block_us[block_us.len() / 2],
        block_p95_us: block_us[(block_us.len() as f64 * 0.95) as usize],
        pcm,
        allocations: render_allocations(),
    }
}

/// One scenario of the regression set: the program's assertions plus the recorded hashes.
fn regression(name: &str) {
    let scenario = SCENARIOS.iter().find(|scenario| scenario.name == name).expect("scenario");
    let result = scene(scenario);
    println!("{{\"scenario\":\"{}\",\"pcm\":\"{:016x}\"}}", scenario.name, result.pcm);
    if HASHES_APPLY {
        assert!(result.pcm == scenario.buses, "{}: PCM {:016x}, recorded PCM {:016x}", scenario.name, result.pcm, scenario.buses);
    }
}

/// The C++ program's `main`.
#[test]
#[ignore = "benchmark: prints one JSON line of timings and hashes per scenario"]
fn benchmark() {
    let repetitions: i32 = std::env::var("ENGINE_DSP_BENCHMARK_REPETITIONS")
        .ok()
        .and_then(|text| text.parse().ok())
        .map_or(1, |count: i32| count.max(1));
    let only = std::env::var("ENGINE_DSP_BENCHMARK_SCENARIO").ok();
    for repetition in 0..repetitions {
        for s in &SCENARIOS {
            if only.as_deref().is_some_and(|name| name != s.name) {
                continue;
            }
            let r = scene(s);
            println!(
                "{{\"scenario\":\"{}\",\"repetition\":{},\"cpu_ms\":{:.6},\"wall_ms\":{:.6},\"block_median_us\":{:.6},\"block_p95_us\":{:.6},\"pcm\":\"{:016x}\",\"render_allocations\":{}}}",
                s.name, repetition, r.cpu_ms, r.wall_ms, r.block_median_us, r.block_p95_us, r.pcm, r.allocations
            );
        }
    }
}

macro_rules! regressions {
    ($($name:ident: $scenario:expr;)*) => {
        $(
            #[test]
            fn $name() {
                regression($scenario);
            }
        )*
    };
}

regressions! {
    saw_16: "saw-16";
    saw_64: "saw-64";
    saw_256: "saw-256";
    loop_64: "loop-64";
    loop_256: "loop-256";
    saw_mod_64: "saw-mod-64";
    saw_mod_256: "saw-mod-256";
    granular_mod_64: "granular-mod-64";
    wavetable_mod_64: "wavetable-mod-64";
    slice_mod_64: "slice-mod-64";
    region_edits_64: "region-edits-64";
}

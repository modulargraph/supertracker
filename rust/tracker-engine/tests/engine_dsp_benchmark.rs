//! Port of `tests/engine_dsp_benchmark.cpp`: the offline engine performance and bit-exact
//! differential fixture. No devices or plug-ins.
//!
//! * `benchmark` (ignored) is the C++ program: it renders every scenario and prints the same JSON
//!   line per scenario (timings, PCM hash, clock hash, render allocations). The repetition count
//!   and the scenario filter (`argv[1]`, `argv[2]`) are `ENGINE_DSP_BENCHMARK_REPETITIONS` and
//!   `ENGINE_DSP_BENCHMARK_SCENARIO`. Run it with
//!   `cargo test --release --test engine_dsp_benchmark -- --ignored --nocapture`.
//! * One test per scenario renders it with the program's own assertions (finite, bounded,
//!   audible, clock spans available, zero render allocations) and compares the PCM and clock
//!   hashes with the values recorded when the tests were ported.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations, reset_render_allocations};
use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use engine_support_b::{hash_f32, hash_f64, hash_word, process_cpu_seconds, FNV_OFFSET};
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
    mapped: bool,
    host: bool,
    edits: bool,
    /// PCM and clock hashes rendered by the Rust engine on x86-64 Linux when the tests were
    /// ported (equal to the C++ engine's on the same machine).
    pcm: u64,
    clock: u64,
}

const fn scenario(name: &'static str, voices: i32, mode: i32, flags: [bool; 4], pcm: u64, clock: u64) -> Scenario {
    Scenario { name, voices, mode, modulated: flags[0], mapped: flags[1], host: flags[2], edits: flags[3], pcm, clock }
}

const NO: [bool; 4] = [false, false, false, false];
const MODULATED: [bool; 4] = [true, false, false, false];

const SCENARIOS: [Scenario; 13] = [
    scenario("saw-16", 16, -1, NO, 0x7927989f7fec8293, 0x14650fb0739d0383),
    scenario("saw-64", 64, -1, NO, 0xabfd0e767e7d8128, 0x14650fb0739d0383),
    scenario("saw-256", 256, -1, NO, 0x373dad267ef310f2, 0x14650fb0739d0383),
    scenario("loop-64", 64, ST_MODE_FORWARD, NO, 0xe34c1b45ad81c309, 0x14650fb0739d0383),
    scenario("loop-256", 256, ST_MODE_FORWARD, NO, 0xb86002dce385d67a, 0x14650fb0739d0383),
    scenario("saw-mod-64", 64, -1, MODULATED, 0x87eeecdf8a8b9d2a, 0x14650fb0739d0383),
    scenario("saw-mod-256", 256, -1, MODULATED, 0x4efe7a3d83a303d5, 0x14650fb0739d0383),
    scenario("granular-mod-64", 64, ST_MODE_GRANULAR, MODULATED, 0x22554a584ba7ed5f, 0x14650fb0739d0383),
    scenario("wavetable-mod-64", 64, ST_MODE_WAVETABLE, MODULATED, 0x57af8fdf2672b5f7, 0x14650fb0739d0383),
    scenario("slice-mod-64", 64, ST_MODE_SLICE, MODULATED, 0x468f93cf60f61227, 0x14650fb0739d0383),
    scenario("mapped-host-16", 16, -1, [false, true, true, false], 0xce6d2072e77b9eef, 0x945348bc0bda79b3),
    scenario(
        "mapped-host-loop-64",
        64,
        ST_MODE_FORWARD,
        [true, true, true, false],
        0x62bdf0d81869a8aa,
        0x945348bc0bda79b3,
    ),
    scenario(
        "automated-region-lfo-64",
        64,
        ST_MODE_FORWARD,
        [false, true, false, true],
        0x96d754cfcba9c72e,
        0x14650fb0739d0383,
    ),
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
    clock: u64,
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
    e.set_send_effects(0.375, 0.55, 0.035);
    e.set_pattern(0, 64);
    e.set_sequence(&[0], true, 0);
    if scenario.mapped {
        let tempos = [
            STClockTempoEvent { beat: 0.0, bpm: 127.0 },
            STClockTempoEvent { beat: 1.0, bpm: 181.0 },
            STClockTempoEvent { beat: 3.0, bpm: 93.0 },
            STClockTempoEvent { beat: 7.0, bpm: 149.0 },
        ];
        let meters = [
            STClockMeterEvent { beat: 0.0, numerator: 7, denominator: 8 },
            STClockMeterEvent { beat: 7.0, numerator: 4, denominator: 4 },
        ];
        assert!(e.set_timing_map(&tempos, &meters, 15).is_ok());
    }
    if scenario.host {
        e.set_host_mixing(true, 0xffff);
    }
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
    let (mut bus_l, mut bus_r) = (vec![[0.0f32; BLOCK]; 16], vec![[0.0f32; BLOCK]; 16]);
    let mut block_us: Vec<f64> = Vec::with_capacity(FRAMES.div_ceil(BLOCK));
    let mut pcm = FNV_OFFSET;
    let mut clock = pcm;
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
        realtime(|| {
            if scenario.host {
                for bus in 0..16 {
                    e.render_bus(bus as i32, offset as u64, &mut bus_l[bus][..count], &mut bus_r[bus][..count]);
                }
            } else {
                e.render(&mut left[..count], &mut right[..count]);
            }
        });
        block_us.push(begin.elapsed().as_secs_f64() * 1e6);
        if scenario.host {
            let mut span = STClockSpan::default();
            assert!(e.clock_span(offset as i64, count as u32, 0, &mut span) == ST_CLOCK_OK);
            clock = hash_f64(
                hash_f64(hash_f64(clock, span.context.beat), span.context.bpm),
                span.context.next_boundary_frame as f64,
            );
            clock = hash_word(clock, span.frames);
        }
        for bus in 0..(if scenario.host { 16 } else { 1 }) {
            for n in 0..count {
                let pair = if scenario.host { [bus_l[bus][n], bus_r[bus][n]] } else { [left[n], right[n]] };
                for value in pair {
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
        clock,
        allocations: render_allocations(),
    }
}

/// One scenario of the regression set: the program's assertions plus the recorded hashes.
fn regression(name: &str) {
    let scenario = SCENARIOS.iter().find(|scenario| scenario.name == name).expect("scenario");
    let result = scene(scenario);
    println!(
        "{{\"scenario\":\"{}\",\"pcm\":\"{:016x}\",\"clock\":\"{:016x}\"}}",
        scenario.name, result.pcm, result.clock
    );
    if HASHES_APPLY {
        assert!(
            result.pcm == scenario.pcm && result.clock == scenario.clock,
            "{}: PCM {:016x} clock {:016x}, recorded PCM {:016x} clock {:016x}",
            scenario.name,
            result.pcm,
            result.clock,
            scenario.pcm,
            scenario.clock
        );
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
                "{{\"scenario\":\"{}\",\"repetition\":{},\"cpu_ms\":{:.6},\"wall_ms\":{:.6},\"block_median_us\":{:.6},\"block_p95_us\":{:.6},\"pcm\":\"{:016x}\",\"clock\":\"{:016x}\",\"render_allocations\":{}}}",
                s.name, repetition, r.cpu_ms, r.wall_ms, r.block_median_us, r.block_p95_us, r.pcm, r.clock, r.allocations
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
    mapped_host_16: "mapped-host-16";
    mapped_host_loop_64: "mapped-host-loop-64";
    automated_region_lfo_64: "automated-region-lfo-64";
}

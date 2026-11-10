//! Port of the former C++ test program `builtin_effects_perf_tests.cpp`: the built-in effect CPU budget check and
//! benchmark.
//!
//! * `budget`: the program's default mode (budget assertions).
//! * `bench` (ignored): `--bench`, the full per-effect table with more trials and no assertions.
//! * `dump` (ignored): `--dump DIR`, every case's output (drum loop and impulse response) as raw
//!   float32 stereo for before/after sound comparisons; the directory is taken from
//!   `BUILTIN_EFFECTS_PERF_DUMP_DIR`.
//!
//! Every catalogue effect is measured at 48 kHz in 128-frame blocks with drum-like input
//! (decaying noise/kick bursts and silence), at default parameters and at a typical Maschine-kit
//! setting.
//!
//! The ceilings are those of the C++ program (built `-O2`) and are enforced in optimized builds
//! only; an unoptimized (`debug_assertions`) build runs every case once, prints the numbers and
//! still asserts the functional half of the silence-gating check (exact silence, resumes).
//!
//! The effect handle is `effects::Effect` behind `engine_support_b::fx`.
#![cfg(feature = "effects")]
// `s % 4 == 0` next to `s % 8 == 4`: the C++ drum pattern, spelled the same way.
#![allow(clippy::manual_is_multiple_of)]

mod engine_support_b;

use engine_support_b::fx::{Fx, Sidechain};
use std::f64::consts::PI;
use std::time::Instant;
use tracker_engine::effects::normalized;
use tracker_engine::effects::catalogue;

const RATE: f64 = 48000.0;
const BLOCK: usize = 128;

type Setting = (&'static str, f64);

#[derive(Clone)]
struct Case {
    effect: &'static str,
    label: &'static str,
    settings: Vec<Setting>,
}

fn case(effect: &'static str, label: &'static str, settings: &[Setting]) -> Case {
    Case { effect, label, settings: settings.to_vec() }
}

/// Defaults for every effect come from the catalogue; these are extra "typical" cases: what a
/// Maschine kit import actually instantiates.
fn typical_cases() -> Vec<Case> {
    vec![
        case(
            "compressor",
            "sidechain-ish",
            &[("threshold", -24.0), ("ratio", 6.0), ("attack", 1.0), ("release", 80.0)],
        ),
        case("eq", "boosts", &[("lowgain", 4.0), ("lowmidgain", -3.0), ("highmidgain", 2.0), ("highgain", 3.0)]),
        case("filter", "resonant lp", &[("cutoff", 1200.0), ("resonance", 60.0), ("amount", 2.0)]),
        case("cabinet", "mix 60", &[("mix", 60.0), ("distance", 40.0)]),
        case("freqshifter", "+200 Hz", &[("coarse", 200.0), ("mix", 50.0)]),
        case("ice", "ice 85", &[("ice", 85.0)]),
        case("reverb", "hall 2.5s", &[("mode", 1.0), ("time", 2.5), ("size", 60.0), ("mix", 25.0)]),
        case("reverb", "plate mode", &[("mode", 2.0), ("mix", 30.0)]),
        case("reverblegacy", "shatter", &[("room", 3.0), ("mix", 30.0)]),
        case("reverblegacy", "freeze", &[("freeze", 1.0), ("mix", 50.0)]),
        case("plate", "mix 25", &[("mix", 25.0), ("decay", 3.0)]),
        case("metaverb", "size 80", &[("size", 80.0), ("mix", 40.0)]),
        case("graindelay", "pitch +12", &[("pitch", 12.0), ("mix", 50.0), ("density", 70.0)]),
        case("grainstretch", "on", &[("on", 1.0), ("mix", 100.0)]),
        case("resochord", "string", &[("mode", 1.0), ("mix", 50.0)]),
        case("distortion", "analog", &[("mode", 1.0), ("drive", 24.0)]),
        case("lofi", "4 kHz 8 bit", &[("sample_rate", 4000.0), ("bits", 8.0), ("smooth", 40.0)]),
        case("saturator", "classic drive", &[("drive", 70.0), ("input", 6.0)]),
        case("saturator", "tape", &[("mode", 1.0), ("drive", 60.0)]),
        case("saturator", "tube", &[("mode", 2.0), ("drive", 60.0), ("charge", 50.0)]),
        case("saturator", "mix 0", &[("mix", 0.0)]),
        case("performfilter", "engaged", &[("engage", 1.0), ("position", 0.4), ("resonance", 0.6)]),
        case("performflanger", "engaged", &[("engage", 1.0)]),
        case("burstecho", "engaged", &[("engage", 1.0)]),
        case("resoecho", "engaged", &[("engage", 1.0)]),
        case("ring", "engaged", &[("engage", 1.0)]),
        case("stutter", "engaged", &[("engage", 1.0)]),
        case("tremolo", "engaged", &[("engage", 1.0), ("blend", 0.5), ("vibrato_mode", 1.0)]),
        case("scratcher", "engaged", &[("engage", 1.0), ("frequency_shift", 300.0)]),
    ]
}

fn all_cases() -> Vec<Case> {
    let typical = typical_cases();
    let mut cases = Vec::new();
    for d in catalogue() {
        let id: &'static str = d.info.id().to_str().expect("effect id");
        cases.push(case(id, "default", &[]));
        for c in &typical {
            if c.effect == id {
                cases.push(c.clone());
            }
        }
    }
    cases
}

/// Deterministic drum loop: 120 BPM sixteenths, kick/snare/hat bursts that decay, plus the second
/// half of every fourth bar in digital silence.
fn drum_loop(frames: usize, channel: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    let mut random: u32 = 0x1234567u32.wrapping_add(channel.wrapping_mul(977));
    let step = (RATE * 0.125) as usize;
    let bar = step * 16;
    for (i, sample) in out.iter_mut().enumerate() {
        let (in_bar, bar_index) = (i % bar, i / bar);
        let s = in_bar / step;
        let t = (in_bar % step) as f64 / RATE;
        if bar_index % 4 == 3 && s >= 8 {
            continue;
        }
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let noise = random as f64 / 2147483648.0 - 1.0;
        let mut v = 0.0f64;
        if s % 4 == 0 {
            v += 0.7 * (2.0 * PI * (45.0 * t + 3.0 * (1.0 - (-t * 30.0).exp()))).sin() * (-t * 18.0).exp();
        }
        if s % 8 == 4 {
            v += 0.4 * noise * (-t * 25.0).exp() + 0.2 * (2.0 * PI * 190.0 * t).sin() * (-t * 30.0).exp();
        }
        v += 0.12 * noise * (-t * 90.0).exp();
        *sample = v as f32;
    }
    out
}

struct Instance {
    fx: Fx,
}

impl Instance {
    fn new(c: &Case) -> Instance {
        let Some(mut fx) = Fx::create(c.effect, RATE, BLOCK as u32) else { panic!("cannot create {}", c.effect) };
        fx.set_tempo(120.0);
        for (parameter, value) in &c.settings {
            let mut found = false;
            for (j, info) in fx.info.params.iter().enumerate() {
                if info.id().to_bytes() == parameter.as_bytes() {
                    fx.set_parameter(j as u32, normalized(info, *value));
                    found = true;
                }
            }
            if !found {
                panic!("{} has no parameter {}", c.effect, parameter);
            }
        }
        fx.reset();
        Instance { fx }
    }

    fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let mut o = 0;
        while o < l.len() {
            let count = BLOCK.min(l.len() - o);
            self.fx.process(&mut l[o..o + count], &mut r[o..o + count], Sidechain::None);
            o += BLOCK;
        }
    }
}

/// Percent of realtime for one instance: median over trials of the time to render the input,
/// each trial on a fresh instance (warmed one bar).
fn measure(c: &Case, in_l: &[f32], in_r: &[f32], trials: usize) -> f64 {
    let mut results = Vec::new();
    let (mut l, mut r) = (vec![0.0f32; in_l.len()], vec![0.0f32; in_r.len()]);
    for _ in 0..trials {
        let mut instance = Instance::new(c);
        let (mut wl, mut wr) = (in_l[..BLOCK * 64].to_vec(), in_r[..BLOCK * 64].to_vec());
        instance.render(&mut wl, &mut wr);
        l.copy_from_slice(in_l);
        r.copy_from_slice(in_r);
        let start = Instant::now();
        instance.render(&mut l, &mut r);
        let elapsed = start.elapsed().as_secs_f64();
        results.push(100.0 * elapsed / (in_l.len() as f64 / RATE));
    }
    results.sort_by(|a, b| a.partial_cmp(b).unwrap());
    results[results.len() / 2]
}

fn name(c: &Case) -> String {
    format!("{} [{}]", c.effect, c.label)
}

fn write_raw(path: &std::path::Path, l: &[f32], r: &[f32]) {
    let mut bytes = Vec::with_capacity(l.len() * 8);
    for i in 0..l.len() {
        bytes.extend_from_slice(&l[i].to_ne_bytes());
        bytes.extend_from_slice(&r[i].to_ne_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

fn file_name(c: &Case) -> String {
    format!("{}_{}", c.effect, c.label)
        .chars()
        .map(|ch| {
            if ch == '+' {
                'p'
            } else if !ch.is_ascii_alphanumeric() && ch != '_' {
                '-'
            } else {
                ch
            }
        })
        .collect()
}

/// C++ `bench(assertBudget)`; returns the number of failed budget checks.
fn run_bench(assert_budget: bool) -> usize {
    // Timing an unoptimized build against the ceilings says nothing: it runs each case once.
    let enforce = assert_budget && cfg!(not(debug_assertions));
    let seconds = if assert_budget { 2.0 } else { 8.0 };
    let trials = if !enforce && assert_budget {
        1
    } else if assert_budget {
        5
    } else {
        9
    };
    let (in_l, in_r) = (drum_loop((RATE * seconds) as usize, 0), drum_loop((RATE * seconds) as usize, 1));
    let mut failures = 0;
    let mut total = 0.0f64;
    // Generous ceiling: 3% per instance is ~10x the most expensive effect as measured on an
    // Apple M4 (and leaves room for parallel test jobs), so the check only trips on a real
    // regression.
    let ceiling = 3.0f64;
    for c in all_cases() {
        let mut percent = measure(&c, &in_l, &in_r, trials);
        if enforce && percent > ceiling {
            // Re-measure once before failing: timer noise on a loaded machine.
            percent = percent.min(measure(&c, &in_l, &in_r, trials));
        }
        total += percent;
        let over = enforce && percent > ceiling;
        failures += over as usize;
        println!("PERF {:<34} {:7.3} % realtime{}", name(&c), percent, if over { "  OVER BUDGET" } else { "" });
    }
    println!("PERF total {total:.3} % realtime over all cases");

    // Silence gating: after the tail time has elapsed with silent input, a long-tail effect must
    // cost close to nothing (absolute, and relative to the same effect processing audio in this
    // run); a non-silent block must resume processing immediately.
    {
        let reverb = case("reverb", "default", &[]);
        let active = measure(&reverb, &in_l, &in_r, trials);
        let mut instance = Instance::new(&reverb);
        let mut l = vec![0.0f32; (RATE * 0.5) as usize];
        let mut r = l.clone();
        l[0] = 0.5;
        r[0] = 0.5;
        instance.render(&mut l, &mut r);
        let mut silent = vec![0.0f32; (RATE * (instance.fx.tail_seconds() + 2.0)) as usize];
        let mut sr = silent.clone();
        instance.render(&mut silent, &mut sr);
        let mut exact = true;
        for i in silent.len() - RATE as usize..silent.len() {
            exact = exact && silent[i] == 0.0 && sr[i] == 0.0;
        }
        let mut quiet = vec![0.0f32; (RATE * 4.0) as usize];
        let mut qr = quiet.clone();
        let start = Instant::now();
        for _ in 0..5 {
            quiet.fill(0.0);
            qr.fill(0.0);
            instance.render(&mut quiet, &mut qr);
        }
        let percent = 100.0 * start.elapsed().as_secs_f64() / (5.0 * 4.0);
        let (mut hit, mut hr) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
        hit[3] = 0.5;
        hr[3] = 0.5;
        instance.fx.process(&mut hit, &mut hr, Sidechain::None);
        let mut resumed = false;
        for value in &hit[3..] {
            resumed = resumed || *value != 0.0;
        }
        println!(
            "PERF gated silent reverb {:.4} % realtime (active {:.4} %), exact silence {}, resumes {}",
            percent,
            active,
            if exact { "yes" } else { "NO" },
            if resumed { "yes" } else { "NO" }
        );
        let too_slow = enforce && (percent > 0.2 || percent > active * 0.25);
        if assert_budget && (too_slow || !exact || !resumed) {
            eprintln!("FAIL silence gating");
            failures += 1;
        }
    }
    if failures != 0 {
        eprintln!("FAIL {failures} built-in effect budget checks");
    }
    failures
}

/// The program's default mode: budget assertions.
#[test]
fn budget() {
    assert!(run_bench(true) == 0);
}

/// `--bench`: the full per-effect table with more trials; nothing is asserted.
#[test]
#[ignore = "benchmark (C++ --bench): prints the per-effect CPU table"]
fn bench() {
    run_bench(false);
}

/// `--dump DIR`: writes every case's output (drum loop and impulse response) as raw float32
/// stereo into `$BUILTIN_EFFECTS_PERF_DUMP_DIR`.
#[test]
#[ignore = "C++ --dump DIR: set BUILTIN_EFFECTS_PERF_DUMP_DIR"]
fn dump() {
    let dir = std::env::var("BUILTIN_EFFECTS_PERF_DUMP_DIR").expect("set BUILTIN_EFFECTS_PERF_DUMP_DIR to a directory");
    let dir = std::path::Path::new(&dir);
    let (looped, tail) = ((RATE * 8.0) as usize, (RATE * 4.0) as usize);
    let (mut in_l, mut in_r) = (drum_loop(looped, 0), drum_loop(looped, 1));
    in_l.resize(looped + tail, 0.0);
    in_r.resize(looped + tail, 0.0);
    for c in all_cases() {
        {
            let mut instance = Instance::new(&c);
            let (mut l, mut r) = (in_l.clone(), in_r.clone());
            instance.render(&mut l, &mut r);
            write_raw(&dir.join(format!("{}.loop.f32", file_name(&c))), &l, &r);
        }
        {
            let mut instance = Instance::new(&c);
            let mut l = vec![0.0f32; (RATE * 8.0) as usize];
            let mut r = l.clone();
            l[0] = 0.5;
            r[0] = 0.5;
            instance.render(&mut l, &mut r);
            write_raw(&dir.join(format!("{}.ir.f32", file_name(&c))), &l, &r);
        }
        println!("{}", file_name(&c));
    }
}

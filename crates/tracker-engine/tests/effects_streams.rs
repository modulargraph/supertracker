//! Randomized streams through the effect handle (`effects::Effect`), meant to run in debug
//! builds (overflow and bounds checks on) so that any panic in a render path fails the test.
//!
//! The scenarios have random parameters, automation mid-stream, bypass toggles, resets, tempo
//! changes, block sizes from 0 to beyond the block limit, optional sidechains (both, one side, or
//! the main input itself) and silence / impulse / noise / sine / hostile (nonfinite, denormal,
//! huge) inputs.
// The index loops read as the scenario generator is specified.
#![allow(clippy::needless_range_loop)]

use std::ffi::CStr;
use tracker_engine::effects::common::EFFECT_SIDECHAIN;
use tracker_engine::effects::{catalogue, drums, find, Effect, Sidechain};

struct Rng {
    s: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        let mut rng = Rng {
            s: seed
                .wrapping_mul(0x9E3779B97F4A7C15)
                .wrapping_add(0x1234567),
        };
        rng.next();
        rng.next();
        rng
    }
    fn next(&mut self) -> u64 {
        self.s ^= self.s << 13;
        self.s ^= self.s >> 7;
        self.s ^= self.s << 17;
        self.s
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / 9007199254740992.0
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

fn hash_name(text: &[u8]) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for &b in text {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

#[derive(Clone, Copy, PartialEq)]
enum Input {
    Silence,
    Impulse,
    Noise,
    Sine,
    Hostile,
}

const INPUTS: [Input; 5] = [
    Input::Silence,
    Input::Impulse,
    Input::Noise,
    Input::Sine,
    Input::Hostile,
];

fn fill_input(kind: Input, sr: f64, rng: &mut Rng, l: &mut [f32], r: &mut [f32]) {
    let n = l.len();
    let pi = std::f64::consts::PI;
    match kind {
        Input::Silence => {}
        Input::Impulse => {
            l[0] = 1.0;
            r[0] = 0.5;
            for _ in 0..12 {
                let at = rng.below(n as u32) as usize;
                l[at] = (rng.unit() * 2.0 - 1.0) as f32;
                let offset = rng.below(3) as usize;
                r[(at + offset) % n] = (rng.unit() * 2.0 - 1.0) as f32;
            }
        }
        Input::Noise => {
            // Bursts of white noise separated by digital silence (exercises tails and gating).
            let period = ((sr * 0.5) as usize).max(1);
            for i in 0..n {
                let on = (i % period) < period * 3 / 5 && i < n * 2 / 3;
                let a = rng.unit() - 0.5;
                let b = rng.unit() - 0.5;
                if on {
                    l[i] = a as f32;
                    r[i] = (0.7 * b + 0.3 * a) as f32;
                }
            }
        }
        Input::Sine => {
            let f0 = 55.0 * 2.0f64.powf(rng.unit() * 6.0);
            let f1 = 55.0 * 2.0f64.powf(rng.unit() * 7.0);
            let mut phase = 0.0f64;
            for i in 0..n {
                let t = i as f64 / n as f64;
                let f = f0 + (f1 - f0) * t;
                phase += f / sr;
                let env = if i < n * 3 / 4 { 0.5 } else { 0.0 };
                l[i] = (env * (2.0 * pi * phase).sin()) as f32;
                r[i] = (env * (2.0 * pi * phase * 1.01 + 0.3).sin()) as f32;
            }
        }
        Input::Hostile => {
            // Loud material with nonfinite, denormal and huge samples sprinkled in.
            for i in 0..n {
                l[i] = ((rng.unit() * 2.0 - 1.0) * 3.0 * (i as f64 * 0.0007).sin()) as f32;
                r[i] = ((i as f64 * 0.013).sin() * 2.5 + (rng.unit() - 0.5) * 0.1) as f32;
                match rng.below(4000) {
                    0 => l[i] = f32::INFINITY,
                    1 => r[i] = f32::NAN,
                    2 => l[i] = 1e-40,
                    3 => r[i] = -3e7,
                    4 => l[i] = f32::NEG_INFINITY,
                    5 => r[i] = 1e-31,
                    roll if roll < 40 => {
                        l[i] = 0.0;
                        r[i] = 0.0;
                    }
                    _ => {}
                }
            }
            for i in n / 2..n * 5 / 8 {
                l[i] = 0.0;
                r[i] = 0.0;
            }
        }
    }
}

/// Runs one scenario and returns an FNV-1a hash of the rendered output bits.
fn run_effect(index: u32, sr: f64, seed: u32, input: Input, seconds: f64) -> u64 {
    let descriptor = catalogue().nth(index as usize).expect("a catalogue index");
    let info = &descriptor.info;
    let id: &CStr = info.id();
    let mut rng = Rng::new(
        hash_name(id.to_bytes()) ^ ((seed as u64) << 32) ^ sr as u64 ^ ((input as u64) << 20),
    );
    const CAPACITIES: [u32; 7] = [1, 16, 64, 127, 256, 512, 4096];
    let max_frames = CAPACITIES[rng.below(7) as usize];
    let fx = Effect::create(id.to_str().expect("an ASCII id"), sr, max_frames);
    assert!(fx.is_some(), "cannot create {id:?}");
    let mut fx = fx.unwrap();
    let parameter_count = info.parameter_count;
    assert!(fx.tail_seconds().is_finite());
    let randomize = |fx: &Effect, rng: &mut Rng, probability: f64| {
        for p in 0..parameter_count {
            if rng.chance(probability) {
                let mut v = rng.unit();
                match rng.below(10) {
                    0 => v = 0.0,
                    1 => v = 1.0,
                    2 => v = 0.5,
                    _ => {}
                }
                fx.set_parameter(p, v);
            }
        }
    };
    if seed != 0 {
        randomize(&fx, &mut rng, 0.8);
        fx.set_tempo(20.0 + rng.unit() * 280.0);
    }
    if seed % 3 == 2 {
        fx.reset();
    }
    assert!(fx.tail_seconds().is_finite());
    let n = (sr * seconds) as usize;
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    fill_input(input, sr, &mut rng, &mut l, &mut r);
    let sidechain_capable = (info.flags & EFFECT_SIDECHAIN) != 0;
    // 0 none, 1 both, 2 left only, 3 right only, 4 the main input itself.
    let sidechain_mode = if sidechain_capable {
        rng.below(5)
    } else if rng.chance(0.15) {
        1
    } else {
        0
    };
    let (mut scl, mut scr) = (Vec::new(), Vec::new());
    if (1..=3).contains(&sidechain_mode) {
        scl = vec![0.0f32; n];
        scr = vec![0.0f32; n];
        let mut side = Rng::new(rng.next());
        let kind = INPUTS[side.below(5) as usize];
        fill_input(kind, sr, &mut side, &mut scl, &mut scr);
    }
    let mut offset = 0usize;
    let mut bypass_blocks = 0i32;
    let mut block_style = rng.below(4);
    while offset < n {
        let mut frames = match block_style {
            0 => 128,
            1 => 1 + rng.below(700),
            2 => 1 + rng.below(32),
            _ => 1 + rng.below(5000),
        };
        if rng.chance(0.002) {
            block_style = rng.below(4);
        }
        if rng.chance(0.01) {
            frames = 0;
        }
        let frames = (frames as usize).min(n - offset);
        // Control events between blocks.
        let event = rng.below(1000);
        if event < 25 {
            let p = rng.below(parameter_count);
            fx.set_parameter(p, rng.unit());
        } else if event < 30 {
            randomize(&fx, &mut rng, 0.5);
        } else if event < 36 {
            fx.set_tempo(20.0 + rng.unit() * 979.0);
        } else if event < 39 {
            fx.reset();
        } else if event < 43 && bypass_blocks == 0 {
            fx.set_bypass(true);
            bypass_blocks = 1 + rng.below(6) as i32;
        } else if event < 46 {
            for p in 0..parameter_count {
                let parameter = &descriptor.params[p as usize];
                fx.set_parameter(p, parameter.normalized(parameter.default_value));
            }
        } else if event < 50 {
            // toggle-like parameters flip between their extremes
            let p = rng.below(parameter_count);
            let value = if rng.chance(0.5) { 1.0 } else { 0.0 };
            fx.set_parameter(p, value);
        }
        if event == 999 {
            assert!(fx.tail_seconds().is_finite());
        }
        let range = offset..offset + frames;
        let sidechain = match sidechain_mode {
            1 => Sidechain::Stereo(&scl[range.clone()], &scr[range.clone()]),
            2 => Sidechain::Mono(&scl[range.clone()]),
            3 => Sidechain::Mono(&scr[range.clone()]),
            4 => Sidechain::MainInput,
            _ => Sidechain::None,
        };
        let bypassed = bypass_blocks > 0;
        let ok = fx.process(&mut l[range.clone()], &mut r[range.clone()], sidechain);
        assert!(ok, "process failed for {id:?}");
        if !bypassed {
            // With the effect active the output is always finite and bounded.
            for i in range {
                assert!(
                    l[i].is_finite()
                        && r[i].is_finite()
                        && l[i].abs() <= 1e6
                        && r[i].abs() <= 1e6,
                    "{id:?} output at frame {i}"
                );
            }
        }
        if bypass_blocks > 0 {
            bypass_blocks -= 1;
            if bypass_blocks == 0 {
                fx.set_bypass(false);
            }
        }
        offset += frames;
    }
    assert!(fx.tail_seconds().is_finite());
    drop(fx);
    let mut h: u64 = 1469598103934665603;
    for v in l.iter().chain(r.iter()) {
        h ^= v.to_bits() as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

#[test]
fn random_streams_do_not_panic() {
    let count = catalogue().count() as u32;
    assert_eq!(count, 34);
    let mut combined: u64 = 0;
    let mut runs = 0;
    for index in 0..count {
        for sr in [44100.0, 96000.0] {
            for seed in 0..3u32 {
                for input in INPUTS {
                    combined = combined.rotate_left(7) ^ run_effect(index, sr, seed, input, 0.2);
                    runs += 1;
                }
            }
        }
    }
    // The same hash in debug and release builds shows optimization does not change the output.
    println!("random streams: {runs} runs, output hash {combined:016x}");
}

/// Unknown ids, arguments out of range and buffers that do not fit are rejected.
#[test]
fn invalid_arguments_are_rejected() {
    let mut l = [0.0f32; 8];
    let mut r = [0.0f32; 8];
    assert!(find(c"").is_none());
    assert!(Effect::create("", 48000.0, 256).is_none());
    assert!(Effect::create("no such effect", 48000.0, 256).is_none());
    assert!(Effect::create("eq", f64::NAN, 256).is_none());
    assert!(Effect::create("eq", 7999.0, 256).is_none());
    assert!(Effect::create("eq", 384001.0, 256).is_none());
    assert!(Effect::create("eq", 48000.0, 0).is_none());
    assert!(Effect::create("eq", 48000.0, 1048577).is_none());
    let fx = Effect::create("eq", 48000.0, 4);
    assert!(fx.is_some());
    let mut fx = fx.unwrap();
    assert!(fx.process(&mut l[..0], &mut r[..0], Sidechain::None));
    // Channels of different lengths, and a key shorter than the block, are refused untouched.
    l.fill(0.25);
    r.fill(0.25);
    assert!(!fx.process(&mut l[..4], &mut r, Sidechain::None));
    assert!(!fx.process(&mut l, &mut r, Sidechain::Mono(&[0.0; 4])));
    assert!(!fx.process(&mut l, &mut r, Sidechain::Stereo(&[0.0; 8], &[0.0; 7])));
    assert!(l == [0.25; 8] && r == [0.25; 8]);
    assert!(fx.process(&mut l, &mut r, Sidechain::None));
    assert!(!fx.set_parameter(99, 0.5));
    assert!(!fx.set_parameter(0, f64::INFINITY));
    assert_eq!(fx.parameter(99), 0.0);
    drop(fx);
    assert_eq!(drums::render(1, &[], 48000.0, &mut l), 8);
    assert_eq!(drums::render(1, &[], f64::NAN, &mut l), 0);
    assert_eq!(drums::render(7, &[], 48000.0, &mut l), 0);
    assert_eq!(drums::render(1, &[], 48000.0, &mut []), 0);
}

//! Offline audio-property tests of the spatial and delay processors, exercised directly through
//! `effects::spatial` / `effects::delay`: 362 checks.
// The tests index their buffers in plain loops.
#![allow(clippy::needless_range_loop)]

mod effects_support;

use effects_support::{allocations, descriptor, energy, Audio, Checks, Effect};
use std::collections::HashSet;
use std::time::Instant;
use tracker_engine::effects::common::{Descriptor, Processor, PI};
use tracker_engine::effects::{delay, spatial};

fn find(id: &str) -> &'static Descriptor {
    if let Some(d) = spatial::descriptors()
        .iter()
        .find(|d| d.info.id().to_bytes() == id.as_bytes())
    {
        return d;
    }
    descriptor(delay::descriptors(), id)
}

fn make(id: &str, sr: f64) -> Effect {
    Effect::with_defaults(find(id), sr)
}

/// Renders in blocks of `block` frames; allocations in `process` are counted.
fn render(p: &mut dyn Processor, a: &mut Audio, block: usize) {
    effects_support::render(p, a, block);
}

fn render_default(p: &mut dyn Processor, a: &mut Audio) {
    render(p, a, 127);
}

fn first_signal(a: &[f32]) -> usize {
    a.iter()
        .position(|v| (*v as f64).abs() > 1.0e-6)
        .unwrap_or(a.len())
}

fn dft_power(a: &[f32], frequency: f64, sr: f64, first: usize, count: usize) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    let w = PI * 2.0 * frequency / sr;
    let mut k = 0usize;
    while k < count && k + first < a.len() {
        let window = 0.5 - 0.5 * (PI * 2.0 * k as f64 / (count - 1) as f64).cos();
        re += a[first + k] as f64 * window * (w * k as f64).cos();
        im += a[first + k] as f64 * window * (w * k as f64).sin();
        k += 1;
    }
    re * re + im * im
}

fn peak_hz(a: &[f32], sr: f64, low: f64, high: f64) -> f64 {
    let (mut power, mut best) = (-1.0f64, 0.0f64);
    let mut f = low;
    while f <= high {
        let e = dft_power(a, f, sr, (sr * 0.03) as usize, (sr * 0.5) as usize);
        if e > power {
            power = e;
            best = f;
        }
        f += 0.25;
    }
    best
}

fn break_like(sr: f64, seconds: f64) -> Audio {
    let mut a = Audio::new((sr * seconds) as usize);
    let mut state: u32 = 173;
    for i in 0..a.l.len() {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        let t = i as f64 / sr;
        let phase = t % 0.25;
        let env = (-phase * 40.0).exp();
        let noise = (state as f64 / 2147483648.0 - 1.0) * 0.12 * (-(t % 0.125) * 100.0).exp();
        let kick =
            (2.0 * PI * (50.0 * phase + 2.0 * (1.0 - (-phase * 35.0).exp()))).sin() * env * 0.3;
        let note = (t * 2.0 * PI * 220.0).sin() * 0.08;
        a.l[i] = (kick + noise + note) as f32;
        a.r[i] = (kick - noise * 0.6 + note * 0.8) as f32;
    }
    a
}

const IDS: [&str; 10] = [
    "ice",
    "metaverb",
    "reflex",
    "reverblegacy",
    "reverb",
    "plate",
    "beatdelay",
    "graindelay",
    "grainstretch",
    "resochord",
];

fn catalogue(c: &mut Checks) {
    let mut seen = HashSet::new();
    for id in IDS {
        let d = find(id);
        c.require(
            seen.insert(d.info.id().to_bytes().to_vec()),
            "unique effect IDs",
        );
        c.require(
            d.info.parameter_count > 0 && d.info.parameter_count <= 16,
            "parameters fit two eight-control pages",
        );
        let mut params = HashSet::new();
        for p in d.params.iter().take(d.info.parameter_count as usize) {
            c.require(
                params.insert(p.id().to_bytes().to_vec()),
                "unique parameter IDs",
            );
            c.require(
                p.minimum <= p.default_value && p.default_value <= p.maximum,
                "defaults in range",
            );
            if (p.flags & 1) != 0 {
                c.require(p.minimum > 0.0, "log parameter positive range");
            }
            if p.label_count() != 0 {
                c.require(
                    p.label_count() == (p.maximum - p.minimum + 1.0) as u32,
                    "all discrete choices labelled",
                );
            }
        }
    }
}

fn invariants(c: &mut Checks) {
    for sr in [44100.0f64, 48000.0, 96000.0] {
        for id in IDS {
            let mut p = make(id, sr);
            let original = break_like(sr, 0.23);
            let mut dry = original.clone();
            p.set("mix", 0.0);
            if id == "grainstretch" {
                p.set("on", 1.0);
            }
            render_default(p.dsp.as_mut(), &mut dry);
            c.require(
                dry.l == original.l && dry.r == original.r,
                "zero Mix is exact dry",
            );
            p = make(id, sr);
            if id == "grainstretch" {
                p.set("on", 1.0);
            }
            let mut a = break_like(sr, 0.37);
            let mut b = a.clone();
            render(p.dsp.as_mut(), &mut a, 127);
            p.dsp.reset();
            render(p.dsp.as_mut(), &mut b, 239);
            c.require(
                a.l == b.l && a.r == b.r,
                "reset determinism and block-size invariance",
            );
            for edge in 0..2 {
                p = make(id, sr);
                let d = find(id);
                for k in 0..d.info.parameter_count as usize {
                    p.dsp.core_mut().p[k] = if edge != 0 {
                        d.params[k].maximum
                    } else {
                        d.params[k].minimum
                    };
                }
                let mut audio = break_like(sr, 0.55);
                audio.resize((sr * 3.0) as usize);
                render(p.dsp.as_mut(), &mut audio, 311);
                let mut peak = 0.0f64;
                for i in 0..audio.l.len() {
                    if !audio.l[i].is_finite() || !audio.r[i].is_finite() {
                        c.require(false, "finite range edge and feedback tail");
                    }
                    peak = peak.max(audio.l[i].abs().max(audio.r[i].abs()) as f64);
                }
                c.require(peak < 20.0, "bounded extreme settings");
            }
        }
    }
    c.require(allocations() == 0, "zero render allocations");
}

fn delay_timing(c: &mut Checks) {
    for sr in [44100.0f64, 48000.0, 96000.0] {
        for tempo in [60.0f64, 120.0, 240.0] {
            let mut p = make("beatdelay", sr);
            p.set_tempo(tempo);
            p.set("mix", 100.0);
            p.set("feedback", 0.0);
            let mut a = Audio::new((sr * 2.0) as usize);
            a.l[0] = 0.5;
            render_default(p.dsp.as_mut(), &mut a);
            c.require(
                (first_signal(&a.l) as f64 - sr * 60.0 / tempo).abs() <= 1.0,
                "Beat Delay follows project tempo at every sample rate",
            );
            c.require(
                energy(&a.r, 0, 0) == 0.0,
                "uncrossed mono delay stays on input channel",
            );
        }
    }
    let mut p = make("beatdelay", 48000.0);
    p.set("mix", 100.0);
    p.set("feedback", 70.0);
    p.set("crossover", 100.0);
    let mut a = Audio::new(96000);
    a.l[0] = 0.5;
    render_default(p.dsp.as_mut(), &mut a);
    c.require(
        energy(&a.l, 23000, 25000) > 0.1,
        "ping-pong first echo left",
    );
    c.require(
        energy(&a.r, 47000, 49000) > 0.02,
        "ping-pong second echo right",
    );
    c.require(
        energy(&a.r, 0, 47000) == 0.0,
        "crossover acts on feedback not first input",
    );
}

fn tail_metadata(c: &mut Checks) {
    let mut p = make("beatdelay", 48000.0);
    p.set("feedback", 0.0);
    c.require(
        (p.tail_now(120.0) - 0.5).abs() < 1e-9,
        "delay tail available before first render",
    );
    p.set("time", 31.0);
    p.set("unit", 4.0);
    p.set("offset", 100.0);
    p.set("feedback", 97.0);
    let longest = p.tail_now(20.0);
    c.require(
        longest > 19000.0 && longest <= p.dsp.tail(),
        "high-feedback long-delay tail is not falsely reported as180seconds",
    );
    for id in IDS {
        let mut fx = make(id, 48000.0);
        let desc = find(id);
        for k in 0..desc.info.parameter_count as usize {
            fx.dsp.core_mut().p[k] = desc.params[k].maximum;
        }
        let estimate = fx.tail_now(20.0);
        c.require(
            estimate.is_finite() && estimate > 0.0 && estimate <= fx.dsp.tail(),
            "maximum parameter tails fit declared upper bound",
        );
    }
    let mut reflex = make("reflex", 48000.0);
    reflex.set("size", 100.0);
    c.require(
        reflex.tail_now(120.0) > 50.0,
        "resonant Reflex maximum tail is preserved",
    );
    let mut grain = make("graindelay", 48000.0);
    grain.set("size", 500.0);
    grain.set("space", 2000.0);
    grain.set("density", 100.0);
    c.require(
        grain.tail_now(120.0) > 200.0,
        "dense long grain cloud tail is preserved",
    );
    let (slower, faster) = (p.tail_now(60.0), p.tail_now(120.0));
    c.require(
        (slower - 2.0 * faster).abs() < 1e-8,
        "tail metadata follows target tempo without rendering",
    );
}

fn delay_boundary_timing(c: &mut Checks) {
    let mut p = make("beatdelay", 8000.0);
    p.set_tempo(20.0);
    p.set("time", 31.0);
    p.set("unit", 4.0);
    p.set("offset", 100.0);
    p.set("feedback", 0.0);
    p.set("mix", 100.0);
    let mut a = Audio::new(408010);
    a.l[0] = 0.5;
    render_default(p.dsp.as_mut(), &mut a);
    c.require(
        first_signal(&a.l) == 408000,
        "longest supported delay is not truncated by its buffer",
    );
    let mut p = make("beatdelay", 48000.0);
    p.set("time", 1.0);
    p.set("unit", 5.0);
    p.set("mix", 100.0);
    p.set("feedback", 0.0);
    let mut triplet = Audio::new(10000);
    triplet.l[0] = 0.5;
    render_default(p.dsp.as_mut(), &mut triplet);
    c.require(
        first_signal(&triplet.l) == 4000,
        "sixteenth-triplet is exactly one sixth of a quarter-note beat",
    );
}

fn reverbs(c: &mut Checks) {
    for id in [
        "ice",
        "metaverb",
        "reflex",
        "reverblegacy",
        "reverb",
        "plate",
    ] {
        let mut p = make(id, 48000.0);
        p.set("mix", 100.0);
        let mut a = Audio::new(192000);
        a.l[0] = 0.5;
        a.r[0] = 0.5;
        render_default(p.dsp.as_mut(), &mut a);
        c.require(
            energy(&a.l, 4800, 0) > 1.0e-7,
            "reverb impulse has an audible tail",
        );
        c.require(a.l != a.r, "reverb generates stereo decorrelation");
    }
    let mut p = make("reverb", 48000.0);
    p.set("mix", 100.0);
    p.set("predelay", 100.0);
    p.set("modulation", 0.0);
    let mut a = Audio::new(24000);
    a.l[0] = 1.0;
    a.r[0] = 1.0;
    render_default(p.dsp.as_mut(), &mut a);
    c.require(
        first_signal(&a.l) >= 4800,
        "100ms predelay never emits early wet audio",
    );
    let (mut short_reverb, mut long_reverb) = (make("reverb", 48000.0), make("reverb", 48000.0));
    short_reverb.set("mix", 100.0);
    long_reverb.set("mix", 100.0);
    short_reverb.set("time", 0.5);
    long_reverb.set("time", 8.0);
    let (mut sh, mut lo) = (Audio::new(144000), Audio::new(144000));
    sh.l[0] = 0.5;
    sh.r[0] = 0.5;
    lo.l[0] = 0.5;
    lo.r[0] = 0.5;
    render_default(short_reverb.dsp.as_mut(), &mut sh);
    render_default(long_reverb.dsp.as_mut(), &mut lo);
    c.require(
        energy(&lo.l, 48000, 0) > energy(&sh.l, 48000, 0) * 100.0,
        "Reverb Time changes measured tail decay",
    );
    let mut legacy = make("reverblegacy", 48000.0);
    legacy.set("mix", 0.0);
    legacy.set("freeze", 1.0);
    let mut frozen = break_like(48000.0, 0.1);
    render_default(legacy.dsp.as_mut(), &mut frozen);
    c.require(
        energy(&frozen.l, 0, 0) == 0.0 && energy(&frozen.r, 0, 0) == 0.0,
        "legacy Freeze mutes dry signal",
    );
    let (mut ice_low, mut ice_high) = (make("ice", 48000.0), make("ice", 48000.0));
    ice_low.set("mix", 100.0);
    ice_high.set("mix", 100.0);
    ice_low.set("ice", 0.0);
    ice_high.set("ice", 100.0);
    let (mut il, mut ih) = (Audio::new(192000), Audio::new(192000));
    il.l[0] = 0.5;
    il.r[0] = 0.5;
    ih.l[0] = 0.5;
    ih.r[0] = 0.5;
    render_default(ice_low.dsp.as_mut(), &mut il);
    render_default(ice_high.dsp.as_mut(), &mut ih);
    c.require(
        energy(&ih.l, 96000, 0) > energy(&il.l, 96000, 0) * 20.0,
        "high Ice lengthens excited resonant tails",
    );
    // NI reference sweeps showed Color moves the resonant band from bass to treble.
    for color in [0.0f64, 100.0] {
        let mut filter = make("ice", 48000.0);
        filter.set("mix", 100.0);
        filter.set("ice", 100.0);
        filter.set("color", color);
        let mut impulse = Audio::new(96000);
        impulse.l[0] = 0.5;
        impulse.r[0] = 0.5;
        render_default(filter.dsp.as_mut(), &mut impulse);
        let (mut bass, mut treble) = (0.0f64, 0.0f64);
        let mut f = 30.0f64;
        while f < 150.0 {
            bass += dft_power(&impulse.l, f, 48000.0, 12000, 12000);
            f += 3.0;
        }
        let mut f = 7500.0f64;
        while f < 9500.0 {
            treble += dft_power(&impulse.l, f, 48000.0, 12000, 12000);
            f += 25.0;
        }
        c.require(
            if color == 0.0 {
                bass > treble * 100.0
            } else {
                treble > bass * 100.0
            },
            "Ice Color moves metallic resonance across the spectrum",
        );
    }
    let mut measured = make("ice", 48000.0);
    measured.set("color", 75.0);
    measured.set("ice", 75.0);
    measured.set("mix", 100.0);
    let mut input = break_like(48000.0, 2.0);
    let input_energy = energy(&input.l, 0, 0);
    input.resize(384000);
    let mut wet = input.clone();
    render_default(measured.dsp.as_mut(), &mut wet);
    let ratio = (energy(&wet.l, 0, 96000) / input_energy).sqrt();
    c.require(
        ratio > 0.025 && ratio < 0.10,
        "Ice wet level remains in the measured reference range",
    );
    measured.set("ice", 100.0);
    measured.dsp.reset();
    wet = input.clone();
    render_default(measured.dsp.as_mut(), &mut wet);
    c.require(
        energy(&wet.l, 336000, 384000) < energy(&wet.l, 48000, 96000) * 0.001,
        "maximum Ice decays instead of forcing unconditional self-oscillation",
    );
}

fn resonator(c: &mut Checks) {
    for tune in [0.0f64, 12.0] {
        let mut p = make("resochord", 48000.0);
        p.set("mode", 1.0);
        p.set("spread", 0.0);
        p.set("tune", tune);
        p.set("feedback", 99.0);
        p.set("brightness", 100.0);
        p.set("decay", 10.0);
        p.set("mix", 100.0);
        let mut a = Audio::new(48000);
        a.l[0] = 1.0;
        a.r[0] = 1.0;
        render_default(p.dsp.as_mut(), &mut a);
        let expected = 130.81278265 * 2.0f64.powf(tune / 12.0);
        let peak = peak_hz(&a.l, 48000.0, expected * 0.94, expected * 1.04);
        c.require(
            (peak - expected).abs() < expected * 0.015,
            "Resochord comb fundamental follows semitone tuning",
        );
    }
}

fn grains(c: &mut Checks) {
    let mut p = make("graindelay", 48000.0);
    p.set("mix", 100.0);
    p.set("pitch", 12.0);
    p.set("jitter", 0.0);
    p.set("space", 0.0);
    p.set("size", 100.0);
    p.set("mod", 0.0);
    p.set("stereo", 0.0);
    p.set("density", 0.0);
    let mut a = Audio::new(96000);
    for i in 0..a.l.len() {
        let v = ((i as f64 * 2.0 * PI * 220.0 / 48000.0).sin() * 0.2) as f32;
        a.l[i] = v;
        a.r[i] = v;
    }
    render_default(p.dsp.as_mut(), &mut a);
    let shifted = dft_power(&a.l, 440.0, 48000.0, 48000, 48000);
    let original = dft_power(&a.l, 220.0, 48000.0, 48000, 48000);
    c.require(
        shifted > original * 100.0,
        "Grain Delay octave pitch shift preserves expected tone",
    );
    let mut p = make("grainstretch", 48000.0);
    p.set("on", 1.0);
    p.set("mix", 100.0);
    p.set("loop", 8.0);
    p.set("stretch", 50.0);
    let mut capture = break_like(48000.0, 4.1);
    render_default(p.dsp.as_mut(), &mut capture);
    let mut tail = Audio::new(48000);
    render_default(p.dsp.as_mut(), &mut tail);
    c.require(
        energy(&tail.l, 0, 0) > 0.01,
        "Grain Stretch retains captured audio after incoming audio stops",
    );
    p.set("on", 0.0);
    let mut bypass = Audio::new(256);
    bypass.l[0] = 0.37;
    bypass.r[0] = -0.29;
    render_default(p.dsp.as_mut(), &mut bypass);
    c.require(
        bypass.l[0] == 0.37f32 && bypass.r[0] == -0.29f32,
        "Grain Stretch off monitors input",
    );
    p.set("on", 1.0);
    let mut recapture = Audio::new(48000);
    render_default(p.dsp.as_mut(), &mut recapture);
    c.require(
        energy(&recapture.l, 0, 0) == 0.0,
        "retrigger captures silence without leaking old recording",
    );
}

fn benchmark() {
    for id in IDS {
        let mut p = make(id, 48000.0);
        if id == "grainstretch" {
            p.set("on", 1.0);
        }
        let mut a = break_like(48000.0, 2.0);
        let start = Instant::now();
        render(p.dsp.as_mut(), &mut a, 256);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        println!("{{\"effect\":\"{id}\",\"audio_seconds\":2,\"render_ms\":{ms:.3},\"realtime_cpu_percent\":{:.3}}}", ms / 20.0);
    }
}

#[test]
fn spatial_and_delay() {
    let mut c = Checks::default();
    catalogue(&mut c);
    invariants(&mut c);
    delay_timing(&mut c);
    tail_metadata(&mut c);
    delay_boundary_timing(&mut c);
    reverbs(&mut c);
    resonator(&mut c);
    grains(&mut c);
    benchmark();
    c.require(allocations() == 0, "all test rendering is allocation-free");
    println!(
        "PASS: {} spatial/delay checks, zero render allocations",
        c.count
    );
    assert_eq!(
        c.count, 362,
        "the number of checks in this file"
    );
}

//! Port of `tests/effects_distortion_tests.cpp`: Distortion, Lofi and Saturator exercised
//! directly through `effects::distortion` (no C API), including the "no allocation in
//! process/reset" check. Same checks and tolerances as the C++ program, whose `require` is
//! called 148742 times.

mod effects_support;

use effects_support::{allocations, counted, descriptor, energy, squared_difference as difference, Audio, Checks, Effect};
use std::collections::BTreeSet;
use tracker_engine::effects::common::{PARAMETER_LOG, PI};
use tracker_engine::effects::distortion;

const SAMPLE_RATE: f64 = 48000.0;

fn effect_at(id: &str, sr: f64) -> Effect {
    // Like the C++ fixture, this relies on the constructors applying the parameter defaults.
    Effect::raw(descriptor(distortion::descriptors(), id), sr)
}

fn effect(id: &str) -> Effect {
    effect_at(id, SAMPLE_RATE)
}

fn tone_with(frames: usize, hz: f64, amplitude: f64) -> Audio {
    let mut audio = Audio::new(frames);
    for i in 0..frames {
        let v = (amplitude * (2.0 * PI * hz * i as f64 / SAMPLE_RATE).sin()) as f32;
        audio.l[i] = v;
        audio.r[i] = v;
    }
    audio
}

fn tone(frames: usize) -> Audio {
    tone_with(frames, 375.0, 0.25)
}

fn varied(frames: usize) -> Audio {
    let mut audio = Audio::new(frames);
    let mut seed: u32 = 1234567;
    for i in 0..frames {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        audio.l[i] = ((seed as f64 / 4294967296.0 - 0.5) * 0.6) as f32;
        audio.r[i] = (0.3 * (i as f64 * 0.023).sin() + 0.1 * (i as f64 * 0.139).sin()) as f32;
    }
    audio
}

/// Renders in blocks of `chunk` frames (0 = one block); allocations in `process` are counted.
fn render(effect: &mut Effect, audio: &mut Audio, chunk: usize) {
    effects_support::render(effect.dsp.as_mut(), audio, chunk);
}

fn bin_amplitude(audio: &[f32], begin: usize, hz: f64) -> f64 {
    let (mut real, mut imaginary) = (0.0f64, 0.0f64);
    for i in begin..audio.len() {
        let phase = 2.0 * PI * hz * i as f64 / SAMPLE_RATE;
        real += audio[i] as f64 * phase.cos();
        imaginary += audio[i] as f64 * phase.sin();
    }
    2.0 * real.hypot(imaginary) / (audio.len() - begin) as f64
}

const IDS: [&str; 3] = ["distortion", "lofi", "saturator"];

fn check_catalogue(c: &mut Checks) {
    let all = distortion::descriptors();
    c.require(all.len() == 3, "three distortion-family descriptors");
    let counts = [16u32, 8, 11];
    for (i, d) in all.iter().enumerate() {
        c.require(d.info.id().to_bytes() == IDS[i].as_bytes(), "stable processor ID/order");
        c.require(d.info.parameter_count == counts[i], "parameter page counts");
        let mut ids = BTreeSet::new();
        for p in d.params.iter().take(d.info.parameter_count as usize) {
            // SAFETY: parameter strings are 'static C string literals.
            let (id_empty, name_empty) = unsafe { (*p.id == 0, *p.name == 0) };
            c.require(!p.id.is_null() && !id_empty && !p.name.is_null() && !name_empty && !p.unit.is_null(), "complete metadata");
            c.require(ids.insert(p.id().to_bytes().to_vec()), "unique persistent parameter IDs");
            c.require(p.minimum <= p.default_value && p.default_value <= p.maximum, "defaults within ranges");
            if p.label_count != 0 {
                c.require(!p.labels.is_null() && p.maximum - p.minimum + 1.0 == p.label_count as f64, "enum labels match range");
            }
            if (p.flags & PARAMETER_LOG) != 0 {
                c.require(p.minimum > 0.0, "positive logarithmic minimum");
            }
        }
    }
}

fn check_common_contracts(c: &mut Checks) {
    for id in IDS {
        let mut silence = effect(id);
        let mut zero = Audio::new(2048);
        render(&mut silence, &mut zero, 0);
        c.require(energy(&zero.l, 0, 0) == 0.0 && energy(&zero.r, 0, 0) == 0.0, &format!("{id} fresh silence"));

        let mut dry = effect(id);
        dry.set("mix", 0.0);
        let input = varied(4097);
        let mut output = input.clone();
        render(&mut dry, &mut output, 73);
        c.require(input.l == output.l && input.r == output.r, &format!("{id} exact zero-mix dry path"));

        let (mut one_block, mut partitioned) = (effect(id), effect(id));
        let (mut full, mut chunks) = (input.clone(), input.clone());
        render(&mut one_block, &mut full, 0);
        render(&mut partitioned, &mut chunks, 113);
        c.require(full.l == chunks.l && full.r == chunks.r, &format!("{id} partition invariant render"));
        counted(|| partitioned.dsp.reset());
        chunks = input.clone();
        render(&mut partitioned, &mut chunks, 31);
        c.require(full.l == chunks.l && full.r == chunks.r, &format!("{id} deterministic reset"));

        let mut stereo = effect(id);
        let mut mono = tone(4096);
        render(&mut stereo, &mut mono, 79);
        c.require(mono.l == mono.r, &format!("{id} identical stereo channels remain identical"));
        stereo.dsp.reset();
        let mut left_only = tone(4096);
        left_only.r.fill(0.0);
        render(&mut stereo, &mut left_only, 101);
        c.require(energy(&left_only.r, 0, 0) == 0.0, &format!("{id} no unintended channel leakage"));
        c.require(energy(&left_only.l, 0, 0) > 0.01, &format!("{id} active channel produces sound"));
        c.require(
            stereo.dsp.latency() == 0 && stereo.dsp.tail().is_finite() && stereo.dsp.tail() > 0.0,
            &format!("{id} latency/tail contract"),
        );
    }
}

fn check_extreme_stability(c: &mut Checks) {
    for sr in [8000.0f64, 44100.0, 192000.0, 384000.0] {
        for id in IDS {
            let mut effect = effect_at(id, sr);
            let mut audio = Audio::new(257);
            for block in 0..48usize {
                for i in 0..effect.info.info.parameter_count as usize {
                    effect.dsp.core_mut().p[i] = if block % 3 == 0 { effect.info.params[i].minimum } else { effect.info.params[i].maximum };
                }
                if block % 11 == 0 {
                    effect.dsp.core_mut().p[0] = f64::NAN;
                }
                for i in 0..audio.len() {
                    audio.l[i] = (8.0 * ((block * audio.len() + i) as f64 * 0.13).sin()) as f32;
                    audio.r[i] = -audio.l[i];
                }
                audio.l[5] = f32::INFINITY;
                audio.r[6] = f32::NAN;
                render(&mut effect, &mut audio, 0);
                for i in 0..audio.len() {
                    c.require(
                        audio.l[i].is_finite() && audio.r[i].is_finite() && audio.l[i].abs() < 32.0 && audio.r[i].abs() < 32.0,
                        "bounded extreme render",
                    );
                }
            }
            // Every state must remain usable after malformed input and extreme automation.
            effect.dsp.reset();
            let mut zero = Audio::new(2048);
            render(&mut effect, &mut zero, 0);
            c.require(energy(&zero.l, 0, 0) == 0.0 && energy(&zero.r, 0, 0) == 0.0, &format!("{id} reset removes all state"));
        }
    }
}

fn check_fuzz_and_analog(c: &mut Checks) {
    let (mut no_feedback, mut feedback) = (effect("distortion"), effect("distortion"));
    for effect in [&mut no_feedback, &mut feedback] {
        effect.set("gate", 0.0);
        effect.set("drive", 0.0);
        effect.set("output", 0.0);
        effect.set("color", 100.0);
        effect.set("tone", 16000.0);
    }
    no_feedback.set("feedback", 0.0);
    feedback.set("feedback", 95.0);
    let (mut plain, mut echoed) = (Audio::new(144000), Audio::new(144000));
    plain.l[0] = 0.5;
    plain.r[0] = 0.5;
    echoed.l[0] = 0.5;
    echoed.r[0] = 0.5;
    render(&mut no_feedback, &mut plain, 137);
    render(&mut feedback, &mut echoed, 137);
    c.require(energy(&echoed.l, 500, 0) > energy(&plain.l, 500, 0) + 1e-8, "fuzz feedback creates an audible tail");
    c.require(energy(&echoed.l, 96000, 0) < 1e-12, "maximum feedback decays within reported tail");

    let (mut open_gate, mut gated) = (effect("distortion"), effect("distortion"));
    for effect in [&mut open_gate, &mut gated] {
        effect.set("drive", 0.0);
        effect.set("feedback", 95.0);
        effect.set("output", 0.0);
        effect.set("gate_threshold", -18.0);
        effect.set("gate_release", 5.0);
    }
    open_gate.set("gate", 0.0);
    gated.set("gate", 1.0);
    let mut ungated = tone(14400);
    ungated.l[4800..].fill(0.0);
    ungated.r = ungated.l.clone();
    let mut closed = ungated.clone();
    render(&mut open_gate, &mut ungated, 0);
    render(&mut gated, &mut closed, 0);
    c.require(energy(&closed.l, 5600, 0) < energy(&ungated.l, 5600, 0) * 0.05, "gate suppresses fuzz feedback after input stops");

    let (mut bass, mut cut_bass) = (effect("distortion"), effect("distortion"));
    for effect in [&mut bass, &mut cut_bass] {
        effect.set("mode", 1.0);
        effect.set("drive", 0.0);
        effect.set("saturation", 0.0);
        effect.set("output", 0.0);
    }
    cut_bass.set("bass", 0.0);
    let mut full = tone_with(24000, 50.0, 0.03);
    let mut cut = full.clone();
    render(&mut bass, &mut full, 0);
    render(&mut cut_bass, &mut cut, 0);
    c.require(energy(&cut.l, 12000, 0) < energy(&full.l, 12000, 0) * 0.15, "analog bass attenuates low frequencies");

    let (mut dark, mut bright) = (effect("distortion"), effect("distortion"));
    dark.set("mode", 1.0);
    bright.set("mode", 1.0);
    dark.set("tone", 150.0);
    bright.set("tone", 16000.0);
    let mut dull = tone_with(16000, 6000.0, 0.2);
    let mut clear = dull.clone();
    render(&mut dark, &mut dull, 0);
    render(&mut bright, &mut clear, 0);
    c.require(energy(&clear.l, 8000, 0) > energy(&dull.l, 8000, 0) * 50.0, "analog tone controls the high cut");
}

fn check_lofi(c: &mut Checks) {
    let mut hold = effect("lofi");
    hold.set("sample_rate", 12000.0);
    hold.set("bits", 24.0);
    let mut ramp = Audio::new(64);
    for i in 0..ramp.len() {
        let v = (i as f64 / 100.0) as f32;
        ramp.l[i] = v;
        ramp.r[i] = v;
    }
    render(&mut hold, &mut ramp, 7);
    for i in 0..ramp.len() {
        c.require((ramp.l[i] as f64 - (i / 4 * 4) as f64 / 100.0).abs() < 1e-6, "12 kHz hold interval is four frames at 48 kHz");
    }

    let mut crush = effect("lofi");
    crush.set("sample_rate", 48000.0);
    crush.set("bits", 4.0);
    let mut quantized = Audio::new(151);
    for i in 0..quantized.len() {
        let v = ((i as f64 - 75.0) / 100.0) as f32;
        quantized.l[i] = v;
        quantized.r[i] = v;
    }
    let original = quantized.clone();
    render(&mut crush, &mut quantized, 17);
    for i in 0..quantized.len() {
        // The C++ evaluates these in float (`quantized.left[i] * 8`) and `std::round(float)`.
        let scaled = quantized.l[i] * 8.0f32;
        c.require(((scaled - scaled.round()) as f64).abs() < 1e-7, "four-bit quantization grid");
        c.require(((quantized.l[i] - original.l[i]).abs() as f64) <= 1.0 / 16.0 + 1e-7, "four-bit quantization error bound");
        if i != 0 {
            c.require(quantized.l[i] >= quantized.l[i - 1], "monotonic quantizer");
        }
    }

    let (mut raw, mut smooth) = (effect("lofi"), effect("lofi"));
    raw.set("sample_rate", 3000.0);
    smooth.set("sample_rate", 3000.0);
    smooth.set("smooth", 100.0);
    let mut aliased = tone_with(24000, 7250.0, 0.5);
    let mut filtered = aliased.clone();
    render(&mut raw, &mut aliased, 0);
    render(&mut smooth, &mut filtered, 0);
    c.require(energy(&filtered.l, 12000, 0) < energy(&aliased.l, 12000, 0) * 0.02, "smooth attenuates out-of-band aliasing");

    let mut mono = effect("lofi");
    mono.set("stereo", 0.0);
    let mut stereo = varied(1024);
    render(&mut mono, &mut stereo, 0);
    c.require(stereo.l == stereo.r, "zero stereo width folds wet signal to mono");

    let mut jitter = effect("lofi");
    jitter.set("jitter", 100.0);
    let mut first = varied(4096);
    let mut second = first.clone();
    render(&mut jitter, &mut first, 51);
    jitter.dsp.reset();
    render(&mut jitter, &mut second, 103);
    c.require(first.l == second.l && first.r == second.r, "clock jitter is deterministic across reset and block size");
    let mut regular_clock = effect("lofi");
    let mut regular = varied(4096);
    render(&mut regular_clock, &mut regular, 0);
    c.require(difference(&first.l, &regular.l, 0) > 0.1, "clock jitter changes the sample clock");
}

fn check_saturation(c: &mut Checks) {
    let input = tone_with(24000, 400.0, 0.25);
    let mut modes = [input.clone(), input.clone(), input.clone()];
    for i in 0..3 {
        let mut mode = effect("saturator");
        mode.set("mode", i as f64);
        mode.set("drive", 65.0);
        mode.set("input", 3.0);
        render(&mut mode, &mut modes[i], 89);
    }
    for a in 0..3 {
        for b in a + 1..3 {
            c.require(difference(&modes[a].l, &modes[b].l, 12000) > 1.0, "saturation modes have distinct responses");
        }
    }

    let (mut subtle, mut driven) = (effect("saturator"), effect("saturator"));
    subtle.set("drive", 0.0);
    driven.set("drive", 100.0);
    let (mut low, mut high) = (input.clone(), input.clone());
    render(&mut subtle, &mut low, 0);
    render(&mut driven, &mut high, 0);
    let low_ratio = bin_amplitude(&low.l, 12000, 1200.0) / bin_amplitude(&low.l, 12000, 400.0);
    let high_ratio = bin_amplitude(&high.l, 12000, 1200.0) / bin_amplitude(&high.l, 12000, 400.0);
    c.require(high_ratio > low_ratio * 10.0, "classic drive increases harmonic content");

    c.require(bin_amplitude(&modes[2].l, 12000, 800.0) > 0.002, "tube mode produces asymmetric even harmonics");
    let mut mean = 0.0f64;
    for i in 12000..modes[2].len() {
        mean += modes[2].l[i] as f64;
    }
    c.require((mean / 12000.0).abs() < 0.003, "tube DC blocker removes steady bias");

    let (mut neutral, mut eq_bypassed) = (effect("saturator"), effect("saturator"));
    neutral.set("mode", 2.0);
    eq_bypassed.set("mode", 2.0);
    eq_bypassed.set("bass", 18.0);
    eq_bypassed.set("treble", -18.0);
    eq_bypassed.set("eq_bypass", 1.0);
    let (mut normal, mut bypassed) = (input.clone(), input.clone());
    render(&mut neutral, &mut normal, 0);
    render(&mut eq_bypassed, &mut bypassed, 0);
    c.require(normal.l == bypassed.l, "tube EQ bypass removes bass and treble coloration");

    let (mut mode_change, mut unchanged) = (effect("saturator"), effect("saturator"));
    let mut warm = tone(8192);
    let mut warm_copy = warm.clone();
    render(&mut mode_change, &mut warm, 0);
    render(&mut unchanged, &mut warm_copy, 0);
    mode_change.set("mode", 2.0);
    let mut next = tone(2048);
    let mut reference = next.clone();
    render(&mut mode_change, &mut next, 0);
    render(&mut unchanged, &mut reference, 0);
    c.require(((next.l[0] - reference.l[0]).abs() as f64) < 0.005, "mode changes crossfade instead of jumping");
    c.require(difference(&next.l, &reference.l, 1024) > 0.001, "mode transition reaches the new response");
}

fn check_conservative_tails(c: &mut Checks) {
    for sr in [8000.0f64, 48000.0, 192000.0] {
        for id in IDS {
            let modes = match id {
                "saturator" => 3,
                "distortion" => 2,
                _ => 1,
            };
            for mode in 0..modes {
                let mut effect = effect_at(id, sr);
                if id == "distortion" {
                    effect.set("mode", mode as f64);
                    effect.set("gate", 0.0);
                    effect.set("feedback", 95.0);
                    effect.set("tone_mod", 100.0);
                    effect.set("tone", 150.0);
                    effect.set("color", 0.0);
                    effect.set("drive", 42.0);
                    effect.set("saturation", 100.0);
                    effect.set("output", 12.0);
                    effect.set("mod_rate", 0.05);
                } else if id == "lofi" {
                    effect.set("sample_rate", 99.5);
                    effect.set("smooth", 100.0);
                    effect.set("jitter", 100.0);
                    effect.set("bits", 24.0);
                    effect.set("input", 24.0);
                    effect.set("output", 12.0);
                } else {
                    effect.set("mode", mode as f64);
                    effect.set("input", 24.0);
                    effect.set("output", 12.0);
                    effect.set("drive", 100.0);
                    effect.set("charge", 100.0);
                    effect.set("overload", 18.0);
                    effect.set("bass", 18.0);
                    effect.set("treble", 18.0);
                    effect.set("contour", 0.0);
                }
                // Sustained full-scale DC excites slow filter and charge states,
                // rather than relying on an impulse that may miss the sample clock.
                let mut warm = Audio::new((sr * 2.0) as usize);
                warm.l.fill(1.0);
                warm.r.fill(-1.0);
                render(&mut effect, &mut warm, 127);
                let reported = effect.tail_now(120.0);
                let mut tail = Audio::new(((reported + 0.1) * sr).ceil() as usize);
                render(&mut effect, &mut tail, 127);
                let mut peak = 0.0f64;
                for i in (reported * sr).ceil() as usize..tail.len() {
                    peak = peak.max((tail.l[i] as f64).abs().max((tail.r[i] as f64).abs()));
                }
                c.require(peak < 1e-6, &format!("{id} worst-case state decays before reported tail at {sr}"));
            }
        }
    }
}

#[test]
fn distortion_family() {
    let mut c = Checks::default();
    check_catalogue(&mut c);
    check_common_contracts(&mut c);
    check_extreme_stability(&mut c);
    check_fuzz_and_analog(&mut c);
    check_lofi(&mut c);
    check_saturation(&mut c);
    check_conservative_tails(&mut c);
    c.require(allocations() == 0, "no allocation in process/reset");
    println!("PASS: distortion family metadata, signal behavior, reset/partition invariance, transitions, stability, and render allocation checks ({} checks)", c.count);
    assert_eq!(c.count, 148742, "same number of checks as tests/effects_distortion_tests.cpp");
}

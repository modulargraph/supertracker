//! Port of the former C++ test program `effects_perform_tests.cpp`: the Perform processors exercised directly
//! through `effects::perform` (no C API). Same checks, tolerances and check count (197764)
//! as the C++ program.
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod effects_support;

use effects_support::{
    descriptor, energy, squared_difference as difference, Audio, Checks, Effect,
};
use std::collections::BTreeSet;
use tracker_engine::effects::common::{wrap, PARAMETER_LOG, PI};
use tracker_engine::effects::perform;

fn effect_at(id: &str, rate: f64) -> Effect {
    let mut effect = Effect::with_defaults(descriptor(perform::descriptors(), id), rate);
    effect.dsp.reset();
    effect
}

fn effect(id: &str) -> Effect {
    effect_at(id, 8000.0)
}

fn tone(n: usize, hz: f64, sr: f64, amplitude: f64) -> Audio {
    let mut a = Audio::new(n);
    for i in 0..n {
        let v = (amplitude * (2.0 * PI * hz * i as f64 / sr).sin()) as f32;
        a.l[i] = v;
        a.r[i] = v;
    }
    a
}

fn tone8k(n: usize, hz: f64) -> Audio {
    tone(n, hz, 8000.0, 0.2)
}

fn render_block(effect: &mut Effect, a: &mut Audio, block: usize) {
    for (l, r) in a.l.chunks_mut(block).zip(a.r.chunks_mut(block)) {
        effect.dsp.process(l, r, None, None);
    }
}

fn render(effect: &mut Effect, a: &mut Audio) {
    render_block(effect, a, 97);
}

fn bin(a: &[f32], hz: f64, sr: f64, begin: usize) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for i in begin..a.len() {
        let phase = 2.0 * PI * hz * i as f64 / sr;
        re += a[i] as f64 * phase.cos();
        im += a[i] as f64 * phase.sin();
    }
    2.0 * re.hypot(im) / (a.len() - begin) as f64
}

fn variation(a: &[f32], begin: usize, end: usize) -> f64 {
    let mut sum = 0.0f64;
    for i in begin + 1..end {
        let d = (a[i] - a[i - 1]) as f64;
        sum += d * d;
    }
    sum
}

const IDS: [&str; 8] = [
    "performfilter",
    "performflanger",
    "burstecho",
    "resoecho",
    "ring",
    "stutter",
    "tremolo",
    "scratcher",
];

fn catalogue_and_dry(c: &mut Checks) {
    let all = perform::descriptors();
    c.require(all.len() == 8, "eight Perform effects");
    for (i, d) in all.iter().enumerate() {
        c.require(
            d.info.id().to_bytes() == IDS[i].as_bytes(),
            "stable effect ID order",
        );
        c.require(d.info.parameter_count <= 16, "at most two encoder pages");
        let mut names = BTreeSet::new();
        for v in d.params.iter().take(d.info.parameter_count as usize) {
            c.require(
                names.insert(v.id().to_bytes().to_vec()),
                "unique parameter IDs",
            );
            c.require(
                v.default_value >= v.minimum && v.default_value <= v.maximum,
                "valid default",
            );
            c.require(
                (v.flags & PARAMETER_LOG) == 0 || v.minimum > 0.0,
                "positive log range",
            );
            if v.label_count() != 0 {
                c.require(
                    v.label(0).is_some() && v.maximum - v.minimum + 1.0 == v.label_count() as f64,
                    "complete enum labels",
                );
            }
        }
        let mut e = effect(IDS[i]);
        let input = tone8k(4097, 247.0);
        let mut output = input.clone();
        render(&mut e, &mut output);
        c.require(
            input.l == output.l && input.r == output.r,
            &format!("{} disengaged dry", IDS[i]),
        );
    }
}

fn filter_and_flanger(c: &mut Checks) {
    let filtered = |hz: f64, mode: i32| {
        let mut e = effect_at("performfilter", 48000.0);
        e.set("engage", 1.0);
        e.set("min_frequency", 500.0);
        e.set("max_frequency", 500.0);
        e.set("resonance", 0.0);
        e.set("type", mode as f64);
        let mut a = tone(24000, hz, 48000.0, 0.03);
        render(&mut e, &mut a);
        energy(&a.l, 12000, 0)
    };
    c.require(
        filtered(100.0, 0) > filtered(6000.0, 0) * 100.0,
        "low-pass rejects high frequencies",
    );
    c.require(
        filtered(6000.0, 2) > filtered(100.0, 2) * 100.0,
        "high-pass rejects low frequencies",
    );
    let flange = |polarity: i32, stereo: f64| {
        let mut e = effect("performflanger");
        e.set("engage", 1.0);
        e.set("frequency", 100.0);
        e.set("rate", 0.0);
        e.set("depth", 0.0);
        e.set("stereo", stereo);
        e.set("saturation", 0.0);
        e.set("polarity", polarity as f64);
        e.set("mix", 1.0);
        let mut a = Audio::new(2000);
        a.l[0] = 0.25;
        render(&mut e, &mut a);
        a
    };
    let (positive, negative, stereo) = (flange(0, 0.0), flange(1, 0.0), flange(0, 1.0));
    c.require(
        positive.l[80] > 0.17 && energy(&positive.l, 1, 80) == 0.0,
        "comb echo at reciprocal frequency",
    );
    c.require(
        positive.l[160] > 0.0 && negative.l[160] < 0.0,
        "comb feedback polarity changes second repeat",
    );
    c.require(
        energy(&positive.r, 0, 0) == 0.0 && energy(&stereo.r, 0, 0) > 1e-6,
        "stereo setting crossfeeds feedback",
    );
}

fn burst(tempo: f64, mode: f64, feed: f64, impulse: usize, release: bool) -> Audio {
    let mut e = effect("burstecho");
    e.set_tempo(tempo);
    e.set("engage", 1.0);
    e.set("position", 0.0);
    e.set("time", 1.0);
    e.set("time_mode", mode);
    e.set("feed", feed);
    e.set("wet", 1.0);
    e.set("mix_mode", 2.0);
    e.set("free_time", 100.0);
    let mut a = Audio::new(18000);
    a.l[impulse] = 0.5;
    a.r[impulse] = 0.5;
    if !release {
        render(&mut e, &mut a);
    } else {
        let (head_l, tail_l) = a.l.split_at_mut(1000);
        let (head_r, tail_r) = a.r.split_at_mut(1000);
        e.dsp.process(head_l, head_r, None, None);
        e.set("engage", 0.0);
        e.dsp.process(tail_l, tail_r, None, None);
    }
    a
}

fn burst_timing_and_trigger(c: &mut Checks) {
    let straight = burst(120.0, 0.0, 1.0, 0, false);
    let slow = burst(60.0, 0.0, 1.0, 0, false);
    let dotted = burst(120.0, 1.0, 1.0, 0, false);
    let triplet = burst(120.0, 2.0, 1.0, 0, false);
    let free = burst(120.0, 3.0, 1.0, 0, false);
    c.require(
        straight.l[4000] > 0.49 && energy(&straight.l, 0, 4000) == 0.0,
        "quarter-note echo at 120 BPM",
    );
    c.require(
        slow.l[8000] > 0.49 && energy(&slow.l, 0, 8000) == 0.0,
        "echo follows tempo",
    );
    let fast = burst(999.0, 0.0, 1.0, 0, false);
    c.require(
        fast.l[480] + fast.l[481] > 0.49,
        "echo follows the full 999 BPM tracker range",
    );
    c.require(dotted.l[6000] > 0.49, "dotted echo time");
    c.require(
        triplet.l[2666] + triplet.l[2667] > 0.49,
        "triplet fractional echo time",
    );
    c.require(free.l[800] > 0.49, "free millisecond echo time");
    let gated = burst(120.0, 0.0, 0.05, 1600, false);
    let continuous = burst(120.0, 0.0, 1.0, 1600, false);
    let released = burst(120.0, 0.0, 1.0, 0, true);
    c.require(
        energy(&gated.l, 0, 0) < 1e-30 && continuous.l[5600] > 0.49,
        "burst stops accepting late input; continuous feed does not",
    );
    c.require(
        released.l[4000] > 0.49,
        "release retains already captured echo",
    );
    let mut e = effect("burstecho");
    e.set("engage", 1.0);
    e.set("time_mode", 3.0);
    e.set("free_time", 50.0);
    e.set("feed", 0.02);
    e.set("position", 0.0);
    e.set("mix_mode", 2.0);
    e.set("wet", 1.0);
    let mut first = Audio::new(1000);
    first.l[0] = 0.5;
    first.r[0] = 0.5;
    render(&mut e, &mut first);
    e.set("engage", 0.0);
    let mut gap = Audio::new(1000);
    render(&mut e, &mut gap);
    e.set("engage", 1.0);
    let mut second = Audio::new(1000);
    second.l[80] = 0.5;
    second.r[80] = 0.5;
    render(&mut e, &mut second);
    c.require(second.l[480] > 0.49, "re-engage starts a new burst window");
}

fn resonance_and_ring(c: &mut Checks) {
    let resonated = |frequency: f64, pump: f64| {
        let mut e = effect("resoecho");
        e.set("engage", 1.0);
        e.set("sync", 0.0);
        e.set("free_time", 20.0);
        e.set("feedback", 0.8);
        e.set("mix_mode", 2.0);
        e.set("wet", 1.0);
        e.set("frequency", frequency);
        e.set("pump", pump);
        e.set("saturation", 0.0);
        let mut a = tone(16000, 440.0, 8000.0, 0.7);
        render(&mut e, &mut a);
        a
    };
    let (plain, pumped, detuned) = (
        resonated(440.0, 0.0),
        resonated(440.0, 1.0),
        resonated(1600.0, 0.0),
    );
    c.require(
        energy(&plain.l, 8000, 0) > energy(&pumped.l, 8000, 0) * 2.0,
        "pump controls resonant feedback energy",
    );
    c.require(
        energy(&plain.l, 8000, 0) > energy(&detuned.l, 8000, 0) * 2.0,
        "resonator frequency selects spectral energy",
    );
    let mut e = effect_at("ring", 48000.0);
    e.set("engage", 1.0);
    e.set("frequency", 200.0);
    e.set("spread", 0.0);
    e.set("reverb_mix", 0.0);
    e.set("mix", 1.0);
    let mut a = tone(48000, 1000.0, 48000.0, 0.4);
    render(&mut e, &mut a);
    c.require(
        bin(&a.l, 800.0, 48000.0, 0) > 0.19 && bin(&a.l, 1200.0, 48000.0, 0) > 0.19,
        "ring creates sum and difference sidebands",
    );
    c.require(
        bin(&a.l, 1000.0, 48000.0, 0) < 1e-5,
        "fully wet ring suppresses original carrier",
    );
    let mut plate = effect("ring");
    plate.set("engage", 1.0);
    plate.set("mix", 1.0);
    plate.set("reverb_mix", 1.0);
    plate.set("reverb_time", 3.0);
    let mut excite = tone8k(2000, 997.0);
    render(&mut plate, &mut excite);
    plate.set("engage", 0.0);
    let mut tail = Audio::new(12000);
    render(&mut plate, &mut tail);
    c.require(
        energy(&tail.l, 2000, 0) > 1e-6 && energy(&tail.r, 2000, 0) > 1e-6,
        "ring plate tail survives release",
    );
}

fn stutter(pitch: f64, gate: f64, direction: i32, tempo: f64, quantize: bool) -> Audio {
    let mut e = effect("stutter");
    e.set("quantize", if quantize { 1.0 } else { 0.0 });
    e.set("free_length", 125.0);
    e.set("length", 0.25);
    e.set("pitch", pitch);
    e.set("gate", gate);
    e.set("direction", direction as f64);
    e.set("fade", 0.1);
    e.set_tempo(tempo);
    let mut history = tone8k(16000, 224.0);
    render(&mut e, &mut history);
    e.set("engage", 1.0);
    let mut a = Audio::new(16000);
    render(&mut e, &mut a);
    a
}

fn capture_and_pitch(c: &mut Checks) {
    let normal = stutter(0.0, 1.0, 0, 120.0, false);
    let pitched = stutter(12.0, 1.0, 0, 120.0, false);
    let gated = stutter(0.0, 0.15, 0, 120.0, false);
    let reverse = stutter(0.0, 1.0, 1, 120.0, false);
    c.require(
        energy(&normal.l, 4000, 0) > 10.0,
        "stutter repeats capture with silent live input",
    );
    let mut repeat_error = 0.0f64;
    for i in 6000..14000 {
        let d = (normal.l[i] - normal.l[i + 1000]) as f64;
        repeat_error += d * d;
    }
    c.require(
        repeat_error < 1e-9,
        "free stutter repeats requested 125 ms loop",
    );
    c.require(
        bin(&pitched.l, 448.0, 8000.0, 8000) > 0.15 && bin(&pitched.l, 224.0, 8000.0, 8000) < 0.01,
        "stutter octave pitch doubles playback rate",
    );
    c.require(
        energy(&gated.l, 4000, 0) < energy(&normal.l, 4000, 0) * 0.3,
        "stutter gate shortens audible part of loop",
    );
    c.require(
        difference(&normal.l, &reverse.l, 4000) > 20.0,
        "reverse changes captured playback direction",
    );
    let quantized = stutter(0.0, 1.0, 0, 60.0, true);
    let mut slow_error = 0.0f64;
    for i in 6000..13000 {
        let d = (quantized.l[i] - quantized.l[i + 2000]) as f64;
        slow_error += d * d;
    }
    c.require(slow_error < 1e-9, "quantized stutter length follows tempo");
    let mut near_start = effect("stutter");
    let mut short_history = tone8k(70, 224.0);
    render(&mut near_start, &mut short_history);
    near_start.set("engage", 1.0);
    let mut early = Audio::new(2000);
    render(&mut near_start, &mut early);
    c.require(
        energy(&early.l, 0, 0) == 0.0,
        "quantized early capture does not expose unquantized history",
    );
}

fn tremolo_and_shift(c: &mut Checks) {
    let mut e = effect("tremolo");
    e.set("engage", 1.0);
    e.set("depth_min", 1.0);
    e.set("depth_max", 1.0);
    e.set("rate_min", 4.0);
    e.set("rate_max", 4.0);
    e.set("stereo", 1.0);
    let mut a = Audio::new(8000);
    a.l.fill(1.0);
    a.r = a.l.clone();
    render(&mut e, &mut a);
    let (mut minimum, mut maximum, mut max_sum_error) = (1.0f64, 0.0f64, 0.0f64);
    for i in 0..a.len() {
        minimum = minimum.min(a.l[i] as f64);
        maximum = maximum.max(a.l[i] as f64);
        max_sum_error = max_sum_error.max((a.l[i] as f64 + a.r[i] as f64 - 1.0).abs());
    }
    c.require(
        minimum < 1e-6 && maximum > 1.0 - 1e-6,
        "full-depth tremolo reaches silence and unity",
    );
    c.require(
        max_sum_error < 1e-6,
        "stereo tremolo gives complementary amplitude cycles",
    );
    let (mut up, mut down) = (
        perform::FrequencyShifter::new(),
        perform::FrequencyShifter::new(),
    );
    let mut shifted = Audio::new(8000);
    let mut phase = 0.0f64;
    for i in 0..shifted.len() {
        phase = wrap(phase + 500.0 / 8000.0);
        let input = 0.5 * (2.0 * PI * 1000.0 * i as f64 / 8000.0).sin();
        shifted.l[i] = up.tick(input, phase) as f32;
        shifted.r[i] = down.tick(input, -phase) as f32;
    }
    c.require(
        bin(&shifted.l, 1500.0, 8000.0, 4000) > 0.49
            && bin(&shifted.l, 500.0, 8000.0, 4000) < 0.002,
        "positive frequency shift selects upper sideband",
    );
    c.require(
        bin(&shifted.r, 500.0, 8000.0, 4000) > 0.49
            && bin(&shifted.r, 1500.0, 8000.0, 4000) < 0.002,
        "negative frequency shift selects lower sideband",
    );
    let mut vibrato = effect("tremolo");
    vibrato.set("engage", 1.0);
    vibrato.set("blend", 1.0);
    let input = tone8k(16000, 1000.0);
    let mut varispeed = input.clone();
    render(&mut vibrato, &mut varispeed);
    c.require(
        difference(&input.l, &varispeed.l, 4000) > 10.0,
        "varispeed vibrato changes pitch over time",
    );
}

fn scratch_brake(c: &mut Checks) {
    let mut e = effect("scratcher");
    e.set("wet", 0.0);
    e.set("brake", 0.02);
    let mut history = tone8k(16000, 224.0);
    render(&mut e, &mut history);
    e.set("engage", 1.0);
    let mut a = Audio::new(8000);
    render(&mut e, &mut a);
    c.require(
        variation(&a.l, 0, 1000) > 1e-4,
        "scratch capture initially moves",
    );
    c.require(
        variation(&a.l, 6000, 8000) < 1e-12,
        "brake brings captured playback to rest",
    );
    e.set("position", 0.8);
    let mut moved = Audio::new(2000);
    render(&mut e, &mut moved);
    c.require(
        variation(&moved.l, 0, 1500) > 1e-3,
        "moving position scratches the braked buffer",
    );
    e.set("engage", 0.0);
    let live = tone8k(4000, 300.0);
    let mut released = live.clone();
    render(&mut e, &mut released);
    c.require(
        difference(&live.l, &released.l, 3000) < 1e-12,
        "releasing scratch returns live input",
    );
}

fn tails(c: &mut Checks) {
    let mut burst_effect = effect("burstecho");
    burst_effect.set("engage", 1.0);
    burst_effect.set("position", 1.0);
    burst_effect.set("time", 4.0);
    burst_effect.set("time_mode", 1.0);
    burst_effect.set("saturation", 0.0);
    let mut targets = burst_effect.dsp.core().p;
    let long_tail = burst_effect.dsp.tail_for_parameters(&targets, 20.0);
    c.require(
        long_tail > 12000.0 && long_tail < 13000.0,
        "long finite burst feedback is not truncated to a two-minute tail",
    );
    c.require(
        burst_effect.dsp.tail_for_parameters(&targets, 120.0) < long_tail * 0.18,
        "finite tail follows current tempo without rendering",
    );
    burst_effect.dsp.core_mut().p.fill(0.0);
    burst_effect.set_tempo(999.0);
    c.require(
        burst_effect.dsp.tail_for_parameters(&targets, 20.0) == long_tail,
        "tail reads supplied targets rather than mutable DSP parameters",
    );
    targets[1] = 0.0;
    c.require(
        (burst_effect.dsp.tail_for_parameters(&targets, 20.0) - 18.25).abs() < 1e-9,
        "feedback-free long burst includes its first repeat",
    );
    targets[1] = 1.0;
    targets[11] = 18.0;
    c.require(
        burst_effect.dsp.tail_for_parameters(&targets, 20.0) == 120.0,
        "driven sustaining burst uses explicit export cap",
    );
    let mut filter = effect("performfilter");
    filter.set("engage", 1.0);
    filter.set("resonance", 1.05);
    c.require(
        filter.tail_now(120.0) == 120.0,
        "negative damping filter declares sustaining export cap",
    );
    filter.set("resonance", 0.0);
    filter.set("min_frequency", 20.0);
    filter.set("max_frequency", 20.0);
    c.require(
        filter.tail_now(120.0) > 0.1,
        "stable filter reports ringing and control release",
    );
    filter.set("resonance", (2.0 - 1e-6) / 2.08);
    c.require(
        filter.tail_now(120.0) > 200000.0,
        "near-critical positive damping keeps its finite decay estimate",
    );
    let mut flange = effect("performflanger");
    flange.set("engage", 1.0);
    flange.set("frequency", 100.0);
    flange.set("decay", 12.0);
    flange.set("saturation", 0.0);
    let clean_tail = flange.tail_now(120.0);
    c.require(
        clean_tail > 20.0 && clean_tail < 40.0,
        "contractive flanger tail reflects feedback decay",
    );
    flange.set("saturation", 24.0);
    c.require(
        flange.tail_now(120.0) == 120.0,
        "driven flanger declares possible sustain",
    );
    let mut reso = effect("resoecho");
    reso.set("feedback", 0.98);
    reso.set("resonance", 1.0);
    reso.set("saturation", 24.0);
    c.require(
        reso.tail_now(120.0) == 120.0,
        "resonator feedback declares possible sustain",
    );
    for id in ["stutter", "scratcher"] {
        let mut e = effect(id);
        e.set("engage", 1.0);
        c.require(
            e.tail_now(120.0) == 120.0,
            &format!("{id} capture sustains after live input ends"),
        );
        e.set("engage", 0.0);
        c.require(
            e.tail_now(120.0) > 0.1 && e.tail_now(120.0) < 1.0,
            &format!("{id} release includes smoothing without excessive tail"),
        );
    }
    let tremolo = effect("tremolo");
    c.require(
        tremolo.tail_now(120.0) >= 0.02,
        "vibrato delay and FIR history are retained",
    );
    let mut ring = effect("ring");
    ring.set("reverb_time", 15.0);
    c.require(
        ring.tail_now(120.0) >= 25.0,
        "ring plate reports beyond T100 at longest decay",
    );
    // Render a finite long-feedback case far past the reported bound, verifying
    // that it represents decay rather than just a monotonic metadata formula.
    let mut finite = effect("burstecho");
    finite.set("engage", 1.0);
    finite.set("position", 1.0);
    finite.set("time_mode", 3.0);
    finite.set("free_time", 20.0);
    finite.set("mix_mode", 2.0);
    finite.set("wet", 1.0);
    let end = finite.tail_now(120.0);
    let mut impulse = Audio::new(((end + 0.2) * finite.sr) as usize);
    impulse.l[0] = 0.5;
    impulse.r[0] = 0.5;
    render(&mut finite, &mut impulse);
    c.require(
        energy(&impulse.l, (end * finite.sr) as usize, 0) < 1e-10,
        "finite burst has decayed by reported tail",
    );
}

fn extremes(c: &mut Checks) {
    for sr in [8000.0f64, 44100.0, 192000.0, 384000.0] {
        for id in IDS {
            let mut e = effect_at(id, sr);
            let mut a = Audio::new(257);
            for block in 0..24usize {
                e.set_tempo(if block % 2 != 0 { 20.0 } else { 999.0 });
                for p in 0..e.info.info.parameter_count as usize {
                    e.dsp.core_mut().p[p] = if block % 3 != 0 {
                        e.info.params[p].maximum
                    } else {
                        e.info.params[p].minimum
                    };
                }
                for i in 0..a.len() {
                    a.l[i] = (4.0 * ((i + block * a.len()) as f64 * 0.13).sin()) as f32;
                    a.r[i] = -a.l[i];
                }
                render(&mut e, &mut a);
                for i in 0..a.len() {
                    c.require(
                        a.l[i].is_finite()
                            && a.r[i].is_finite()
                            && a.l[i].abs() <= 16.0
                            && a.r[i].abs() <= 16.0,
                        "bounded at extreme sample rate and automation",
                    );
                }
            }
            e.dsp.reset();
            e.set("engage", 0.0);
            let mut silent = Audio::new(257);
            render(&mut e, &mut silent);
            c.require(
                energy(&silent.l, 0, 0) == 0.0 && energy(&silent.r, 0, 0) == 0.0,
                &format!("{id} reset clears all captured/tail state"),
            );
        }
    }
}

#[test]
fn perform_dsp() {
    let mut c = Checks::default();
    catalogue_and_dry(&mut c);
    filter_and_flanger(&mut c);
    burst_timing_and_trigger(&mut c);
    resonance_and_ring(&mut c);
    capture_and_pitch(&mut c);
    tremolo_and_shift(&mut c);
    scratch_brake(&mut c);
    tails(&mut c);
    extremes(&mut c);
    println!("Perform DSP tests passed ({} checks)", c.count);
    assert_eq!(
        c.count, 197764,
        "same number of checks as the C++ program effects_perform_tests.cpp"
    );
}

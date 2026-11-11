//! Distortion processors: Distortion (feedback fuzz / analog), Lofi and Saturator.
//!
//! Independently written algorithms. The parameter vocabulary follows familiar
//! studio effects; these processors do not model or reproduce proprietary DSP.

use super::common::{
    clamp, clean, core_accessors, db_gain, fast_sin_2pi, fast_tanh, fast_tanh_f32, frame_buffers,
    max, min, mix, pole, pow2, select, wrap, Biquad, BiquadType, Core, DelayLine, Descriptor,
    Label, ParameterCache, ParameterInfo as P, Processor, Smoother, PARAMETER_INTEGER as INTEGER,
    PARAMETER_LOG as LOG, PARAMETER_TOGGLE as TOGGLE, PI,
};

const DISTORTION_MODES: &[Label] = &[Label::new(c"Feedback Fuzz"), Label::new(c"Analog")];
const SATURATION_MODES: &[Label] = &[
    Label::new(c"Classic"),
    Label::new(c"Tape"),
    Label::new(c"Tube"),
];
const SWITCHES: &[Label] = &[Label::new(c"Off"), Label::new(c"On")];

static DISTORTION_PARAMETERS: [P; 16] = [
    P::new(c"drive", c"Drive", c"dB", 0.0, 42.0, 12.0, 0),
    P::new(c"color", c"Fuzz Color", c"%", 0.0, 100.0, 65.0, 0),
    P::new(c"feedback", c"Fuzz Feedback", c"%", 0.0, 95.0, 15.0, 0),
    P::new(c"tone", c"Tone", c"Hz", 150.0, 16000.0, 5000.0, LOG),
    P::new(c"tone_mod", c"Fuzz Tone Mod", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
    P::new(c"output", c"Output", c"dB", -24.0, 12.0, -6.0, 0),
    P::choice(
        c"mode",
        c"Mode",
        c"",
        0.0,
        1.0,
        0.0,
        INTEGER,
        DISTORTION_MODES,
    ),
    P::new(c"bass", c"Analog Bass", c"%", 0.0, 100.0, 100.0, 0),
    P::choice(
        c"focus",
        c"Analog Focus",
        c"",
        0.0,
        1.0,
        0.0,
        TOGGLE,
        SWITCHES,
    ),
    P::new(
        c"definition",
        c"Analog Definition",
        c"%",
        0.0,
        100.0,
        50.0,
        0,
    ),
    P::choice(c"gate", c"Fuzz Gate", c"", 0.0, 1.0, 1.0, TOGGLE, SWITCHES),
    P::new(
        c"gate_release",
        c"Gate Release",
        c"ms",
        5.0,
        2000.0,
        100.0,
        LOG,
    ),
    P::new(
        c"gate_threshold",
        c"Gate Threshold",
        c"dB",
        -96.0,
        -18.0,
        -60.0,
        0,
    ),
    P::new(c"mod_rate", c"Fuzz Mod Rate", c"Hz", 0.05, 10.0, 0.4, LOG),
    P::new(
        c"saturation",
        c"Analog Saturation",
        c"%",
        0.0,
        100.0,
        25.0,
        0,
    ),
];

static LOFI_PARAMETERS: [P; 8] = [
    P::new(
        c"sample_rate",
        c"Sample Rate",
        c"Hz",
        99.5,
        48000.0,
        12000.0,
        LOG,
    ),
    P::new(c"bits", c"Bits", c"bits", 1.0, 24.0, 12.0, 0),
    P::new(c"smooth", c"Smooth", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"stereo", c"Stereo Width", c"%", 0.0, 200.0, 100.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
    P::new(c"input", c"Input", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(c"output", c"Output", c"dB", -24.0, 12.0, 0.0, 0),
    P::new(c"jitter", c"Clock Jitter", c"%", 0.0, 100.0, 0.0, 0),
];

static SATURATION_PARAMETERS: [P; 11] = [
    P::choice(
        c"mode",
        c"Mode",
        c"",
        0.0,
        2.0,
        0.0,
        INTEGER,
        SATURATION_MODES,
    ),
    P::new(c"input", c"Input", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(c"contour", c"Contour", c"%", 0.0, 100.0, 50.0, 0),
    P::new(c"drive", c"Drive", c"%", 0.0, 100.0, 25.0, 0),
    P::new(c"charge", c"Tube Charge", c"%", 0.0, 100.0, 25.0, 0),
    P::new(c"overload", c"Tube Overload", c"dB", 0.0, 18.0, 0.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
    P::new(c"output", c"Output", c"dB", -24.0, 12.0, 0.0, 0),
    P::new(c"bass", c"Tube Bass", c"dB", -18.0, 18.0, 0.0, 0),
    P::new(c"treble", c"Tube Treble", c"dB", -18.0, 18.0, 0.0, 0),
    P::choice(
        c"eq_bypass",
        c"Tube EQ Bypass",
        c"",
        0.0,
        1.0,
        0.0,
        TOGGLE,
        SWITCHES,
    ),
];

/// A raw parameter value clamped to its range; nonfinite values use the default.
#[inline]
fn parameter(core: &Core, info: &[P], i: usize) -> f64 {
    let value = core.p[i];
    if value.is_finite() {
        clamp(value, info[i].minimum, info[i].maximum)
    } else {
        info[i].default_value
    }
}

/// Smoothed controls. Once every value has reached (snapped to) its target,
/// ticking is skipped until a target changes: identical values, no per-sample work.
#[derive(Clone)]
struct Controls<const N: usize> {
    smooth: [Smoother; N],
    target: [f64; N],
    value: [f64; N],
    settled: bool,
}

impl<const N: usize> Controls<N> {
    fn new() -> Self {
        Controls {
            smooth: [Smoother::default(); N],
            target: [0.0; N],
            value: [0.0; N],
            settled: false,
        }
    }

    fn set_targets(&mut self, t: [f64; N]) {
        if t != self.target {
            self.target = t;
            self.settled = false;
        }
    }

    #[inline(always)]
    fn tick(&mut self, coefficient: f64) {
        if self.settled {
            return;
        }
        let mut all = true;
        for i in 0..N {
            self.value[i] = self.smooth[i].tick(self.target[i], coefficient);
            all = all && self.value[i] == self.target[i];
        }
        self.settled = all;
    }

    fn reset(&mut self) {
        for s in self.smooth.iter_mut() {
            s.reset();
        }
        self.settled = false;
    }
}

#[inline]
fn filter_pole(hz: f64, sample_rate: f64) -> f64 {
    (-2.0 * PI * clamp(hz, 5.0, sample_rate * 0.45) / sample_rate).exp()
}

// One-pole low-pass and DC blocker are contractive (coefficient < 1), so
// finite input keeps them finite and bounded; render runs with denormals
// flushed, so no per-sample clean-up is needed.
#[inline(always)]
fn lowpass(input: f64, state: &mut f64, coefficient: f64) -> f64 {
    *state = input + coefficient * (*state - input);
    *state
}

#[derive(Clone, Copy, Default)]
struct DcBlock {
    previous: f64,
    state: f64,
}

impl DcBlock {
    #[inline(always)]
    fn tick(&mut self, x: f64, coefficient: f64) -> f64 {
        let y = x - self.previous + coefficient * self.state;
        self.previous = x;
        self.state = y;
        y
    }

    fn reset(&mut self) {
        self.previous = 0.0;
        self.state = 0.0;
    }
}

/// Fixed 2x interpolation plus a four-pole decimation filter reduces fold-back
/// from the nonlinear stages. No explicit buffered latency is introduced.
/// The decimation filter is a fixed, stable low-pass: its state needs no
/// per-sample clean-up (render runs with denormals flushed).
#[derive(Clone, Copy)]
pub struct OversampledShape {
    first: Biquad,
    second: Biquad,
    previous: f64,
}

impl OversampledShape {
    pub fn new(sr: f64) -> Self {
        let mut first = Biquad::default();
        let mut second = Biquad::default();
        first.set(BiquadType::LowPass, sr * 0.45, 0.5411961001, 0.0, sr * 2.0);
        second.set(BiquadType::LowPass, sr * 0.45, 1.306562965, 0.0, sr * 2.0);
        OversampledShape {
            first,
            second,
            previous: 0.0,
        }
    }

    #[inline(always)]
    fn step(f: &mut Biquad, x: f64) -> f64 {
        let y = f.b0 * x + f.z1;
        f.z1 = f.b1 * x - f.a1 * y + f.z2;
        f.z2 = f.b2 * x - f.a2 * y;
        y
    }

    #[inline(always)]
    pub fn tick(&mut self, input: f64, shape: impl Fn(f64) -> f64) -> f64 {
        let mid = Self::step(&mut self.first, shape((self.previous + input) * 0.5));
        Self::step(&mut self.second, mid);
        let cur = Self::step(&mut self.first, shape(input));
        let result = clean(Self::step(&mut self.second, cur));
        self.previous = input;
        result
    }

    pub fn reset(&mut self) {
        self.previous = 0.0;
        self.first.reset();
        self.second.reset();
    }
}

/// `OversampledShape` for both channels at once: identical arithmetic, laid out
/// as two lanes so the shaper and decimation filters vectorize.
#[derive(Clone, Copy)]
struct StereoShape {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    c0: f64,
    c1: f64,
    c2: f64,
    e1: f64,
    e2: f64,
    previous: [f64; 2],
    z1a: [f64; 2],
    z2a: [f64; 2],
    z1b: [f64; 2],
    z2b: [f64; 2],
}

impl StereoShape {
    fn new(sr: f64) -> Self {
        let mono = OversampledShape::new(sr);
        StereoShape {
            b0: mono.first.b0,
            b1: mono.first.b1,
            b2: mono.first.b2,
            a1: mono.first.a1,
            a2: mono.first.a2,
            c0: mono.second.b0,
            c1: mono.second.b1,
            c2: mono.second.b2,
            e1: mono.second.a1,
            e2: mono.second.a2,
            previous: [0.0; 2],
            z1a: [0.0; 2],
            z2a: [0.0; 2],
            z1b: [0.0; 2],
            z2b: [0.0; 2],
        }
    }

    #[inline(always)]
    fn tick(&mut self, input: &[f64; 2], shape: impl Fn(f64) -> f64) -> [f64; 2] {
        let mut mid = [0.0f64; 2];
        let mut cur = [0.0f64; 2];
        for c in 0..2 {
            mid[c] = shape((self.previous[c] + input[c]) * 0.5);
            cur[c] = shape(input[c]);
            self.previous[c] = input[c];
        }
        let mut out = [0.0f64; 2];
        for c in 0..2 {
            let mut y = self.b0 * mid[c] + self.z1a[c];
            self.z1a[c] = self.b1 * mid[c] - self.a1 * y + self.z2a[c];
            self.z2a[c] = self.b2 * mid[c] - self.a2 * y;
            let mut w = self.c0 * y + self.z1b[c];
            self.z1b[c] = self.c1 * y - self.e1 * w + self.z2b[c];
            self.z2b[c] = self.c2 * y - self.e2 * w;
            y = self.b0 * cur[c] + self.z1a[c];
            self.z1a[c] = self.b1 * cur[c] - self.a1 * y + self.z2a[c];
            self.z2a[c] = self.b2 * cur[c] - self.a2 * y;
            w = self.c0 * y + self.z1b[c];
            self.z1b[c] = self.c1 * y - self.e1 * w + self.z2b[c];
            self.z2b[c] = self.c2 * y - self.e2 * w;
            out[c] = w;
        }
        [clean(out[0]), clean(out[1])]
    }

    fn reset(&mut self) {
        self.previous = [0.0; 2];
        self.z1a = [0.0; 2];
        self.z2a = [0.0; 2];
        self.z1b = [0.0; 2];
        self.z2b = [0.0; 2];
    }
}

struct DistortionChannel {
    feedback: DelayLine,
    fuzz_dc: DcBlock,
    analog_dc: DcBlock,
    tone: f64,
    color: f64,
    bass: f64,
    focus_low: f64,
    focus_high: f64,
    analog_tone: f64,
}

impl DistortionChannel {
    fn new(sr: f64) -> Self {
        DistortionChannel {
            feedback: DelayLine::new((sr * 0.012).ceil() as usize + 4),
            fuzz_dc: DcBlock::default(),
            analog_dc: DcBlock::default(),
            tone: 0.0,
            color: 0.0,
            bass: 0.0,
            focus_low: 0.0,
            focus_high: 0.0,
            analog_tone: 0.0,
        }
    }

    fn reset(&mut self) {
        self.feedback.reset();
        self.fuzz_dc.reset();
        self.analog_dc.reset();
        self.tone = 0.0;
        self.color = 0.0;
        self.bass = 0.0;
        self.focus_low = 0.0;
        self.focus_high = 0.0;
        self.analog_tone = 0.0;
    }
}

pub struct Distortion {
    core: Core,
    channels: [DistortionChannel; 2],
    fuzz_shape: StereoShape,
    analog_shape: StereoShape,
    c: Controls<16>,
    smoothing: f64,
    gate_attack: f64,
    envelope_release: f64,
    bass_pole: f64,
    focus_low_pole: f64,
    focus_high_pole: f64,
    dc_pole: f64,
    phase: f64,
    envelope: f64,
    gate_gain: f64,
    // Paths whose output weight is exactly zero (Mode crossfade settled, or
    // Mix at 0) are not computed; their state restarts when they return.
    fuzz_live: bool,
    analog_live: bool,
    cache: ParameterCache<16>,
}

impl Distortion {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sample_rate;
        let mut core = Core::new(sr);
        for (slot, info) in core.p.iter_mut().zip(DISTORTION_PARAMETERS.iter()) {
            *slot = info.default_value;
        }
        Distortion {
            core,
            channels: [DistortionChannel::new(sr), DistortionChannel::new(sr)],
            fuzz_shape: StereoShape::new(sr),
            analog_shape: StereoShape::new(sr),
            c: Controls::new(),
            smoothing: pole(8.0, sr),
            gate_attack: pole(0.5, sr),
            envelope_release: pole(10.0, sr),
            bass_pole: filter_pole(180.0, sr),
            focus_low_pole: filter_pole(500.0, sr),
            focus_high_pole: filter_pole(2200.0, sr),
            dc_pole: filter_pole(12.0, sr),
            phase: 0.0,
            envelope: 0.0,
            gate_gain: 0.0,
            fuzz_live: false,
            analog_live: false,
            cache: ParameterCache::default(),
        }
    }

    fn reset_fuzz(&mut self) {
        self.fuzz_shape.reset();
        for ch in self.channels.iter_mut() {
            ch.feedback.reset();
            ch.fuzz_dc.reset();
            ch.tone = 0.0;
            ch.color = 0.0;
        }
    }

    fn reset_analog(&mut self) {
        self.analog_shape.reset();
        for ch in self.channels.iter_mut() {
            ch.analog_dc.reset();
            ch.bass = 0.0;
            ch.focus_low = 0.0;
            ch.focus_high = 0.0;
            ch.analog_tone = 0.0;
        }
    }
}

impl Processor for Distortion {
    core_accessors!(core);

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.core.sr;
        if self.cache.changed(&self.core) {
            let core = &self.core;
            let value = |i: usize| parameter(core, &DISTORTION_PARAMETERS, i);
            self.c.set_targets([
                db_gain(value(0)),
                filter_pole(300.0 * 60.0f64.powf(value(1) * 0.01), sr),
                value(2) * 0.0099,
                filter_pole(value(3), sr),
                value(4) * 0.01,
                value(5) * 0.01,
                db_gain(value(6)),
                value(7).round(),
                value(8) * 0.01,
                value(9).round(),
                value(10) * 0.01,
                value(11).round(),
                pole(value(12), sr),
                db_gain(value(13)),
                value(14) / sr,
                1.0 + value(15) * 0.15,
            ]);
        }
        let (bass_pole, focus_low_pole, focus_high_pole, dc_pole) = (
            self.bass_pole,
            self.focus_low_pole,
            self.focus_high_pole,
            self.dc_pole,
        );
        for i in 0..l.len() {
            self.c.tick(self.smoothing);
            let v = self.c.value;
            let a = clean(l[i] as f64);
            let b = clean(r[i] as f64);
            let level = max(a.abs(), b.abs());
            self.envelope = max(level, self.envelope * self.envelope_release);
            let target = if self.envelope >= v[13] { 1.0 } else { 0.0 };
            self.gate_gain = target
                + (self.gate_gain - target)
                    * select(target > self.gate_gain, self.gate_attack, v[12]);
            let gate = mix(1.0, self.gate_gain, v[11]);
            let wet = v[5] != 0.0;
            let fuzz_on = wet && v[7] != 1.0;
            let analog_on = wet && v[7] != 0.0;
            if fuzz_on && !self.fuzz_live {
                self.reset_fuzz();
            }
            if analog_on && !self.analog_live {
                self.reset_analog();
            }
            self.fuzz_live = fuzz_on;
            self.analog_live = analog_on;
            let input = [a, b];
            let mut colored = [0.0f64; 2];
            let mut rounded = [0.0f64; 2];
            if fuzz_on {
                // Zero Tone Mod: the delay is exactly constant, skip the LFO.
                let delay = if v[4] == 0.0 {
                    sr * 0.0007
                } else {
                    sr * (0.0007 + 0.0045 * v[4] * (0.5 + 0.5 * fast_sin_2pi(self.phase)))
                };
                let mut x = [0.0f64; 2];
                for k in 0..2 {
                    let ch = &mut self.channels[k];
                    let feedback = lowpass(ch.feedback.read(delay), &mut ch.tone, v[3]);
                    x[k] = input[k] * v[0] + feedback * v[2];
                }
                let fuzz = self.fuzz_shape.tick(&x, fast_tanh);
                for k in 0..2 {
                    let ch = &mut self.channels[k];
                    // Bound filter overshoot before storing feedback. Its loop gain stays
                    // below one, including at the maximum Feedback setting.
                    ch.feedback.push(clamp(fuzz[k], -1.0, 1.0) * gate);
                    colored[k] =
                        lowpass(ch.fuzz_dc.tick(fuzz[k], dc_pole), &mut ch.color, v[1]) * gate;
                }
            }
            if analog_on {
                let mut x = [0.0f64; 2];
                for k in 0..2 {
                    let ch = &mut self.channels[k];
                    let low = lowpass(input[k], &mut ch.bass, bass_pole);
                    let mid = lowpass(input[k], &mut ch.focus_high, focus_high_pole)
                        - lowpass(input[k], &mut ch.focus_low, focus_low_pole);
                    x[k] = (input[k] - low * (1.0 - v[8]) + mid * v[9] * (1.0 - v[10]) * 1.5)
                        * v[0]
                        * v[15];
                }
                let drive = v[10];
                let shaped = self
                    .analog_shape
                    .tick(&x, |y| mix(fast_tanh(y), clamp(y, -1.0, 1.0), drive));
                for k in 0..2 {
                    let ch = &mut self.channels[k];
                    rounded[k] = lowpass(
                        ch.analog_dc.tick(shaped[k], dc_pole),
                        &mut ch.analog_tone,
                        v[3],
                    );
                }
            }
            l[i] = clean(mix(a, mix(colored[0], rounded[0], v[7]) * v[6], v[5])) as f32;
            r[i] = clean(mix(b, mix(colored[1], rounded[1], v[7]) * v[6], v[5])) as f32;
            self.phase = wrap(self.phase + v[14]);
        }
    }

    fn reset(&mut self) {
        for ch in self.channels.iter_mut() {
            ch.reset();
        }
        self.fuzz_shape.reset();
        self.analog_shape.reset();
        self.c.reset();
        self.cache.reset();
        self.phase = 0.0;
        self.envelope = 0.0;
        self.gate_gain = 0.0;
        self.fuzz_live = false;
        self.analog_live = false;
    }

    fn tail(&self) -> f64 {
        3.0
    }
}

const LOFI_SEED: u32 = 0x5a17b3c9;

pub struct Lofi {
    core: Core,
    c: Controls<9>,
    cache: ParameterCache<8>,
    phase: f64,
    held: [f64; 2],
    pre: [[f64; 2]; 2],
    post: [[f64; 2]; 2],
    clock_scale: f64,
    random: u32,
    smoothing: f64,
}

impl Lofi {
    pub fn new(sample_rate: f64) -> Self {
        let mut core = Core::new(sample_rate);
        for (slot, info) in core.p.iter_mut().zip(LOFI_PARAMETERS.iter()) {
            *slot = info.default_value;
        }
        Lofi {
            core,
            c: Controls::new(),
            cache: ParameterCache::default(),
            phase: 1.0,
            held: [0.0; 2],
            pre: [[0.0; 2]; 2],
            post: [[0.0; 2]; 2],
            clock_scale: 1.0,
            random: LOFI_SEED,
            smoothing: pole(8.0, sample_rate),
        }
    }

    #[inline(always)]
    fn random_signed(&mut self) -> f64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        self.random as f64 / 2147483648.0 - 1.0
    }
}

impl Processor for Lofi {
    core_accessors!(core);

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.core.sr;
        let rate = min(parameter(&self.core, &LOFI_PARAMETERS, 0), sr);
        if self.cache.changed(&self.core) {
            let core = &self.core;
            let value = |i: usize| parameter(core, &LOFI_PARAMETERS, i);
            self.c.set_targets([
                rate / sr,
                pow2(value(1) - 1.0),
                value(2) * 0.01,
                value(3) * 0.01,
                value(4) * 0.01,
                db_gain(value(5)),
                db_gain(value(6)),
                value(7) * 0.01,
                filter_pole(rate * 0.4, sr),
            ]);
        }
        for i in 0..l.len() {
            self.c.tick(self.smoothing);
            let v = self.c.value;
            let dry = [clean(l[i] as f64), clean(r[i] as f64)];
            let mut input = [0.0f64; 2];
            for channel in 0..2 {
                let gained = dry[channel] * v[5];
                let first = lowpass(gained, &mut self.pre[channel][0], v[8]);
                let filtered = lowpass(first, &mut self.pre[channel][1], v[8]);
                input[channel] = mix(gained, filtered, v[2]);
            }
            if self.phase >= 1.0 {
                self.phase -= self.phase.floor();
                // A symmetric, zero-centered quantizer preserves digital
                // silence and both full-scale endpoints. Bits controls its
                // step resolution, not a signed PCM container/code count.
                for channel in 0..2 {
                    self.held[channel] = (clamp(input[channel], -1.0, 1.0) * v[1]).round() / v[1];
                }
                self.clock_scale = 1.0 + self.random_signed() * v[7] * 0.45;
            }
            self.phase += min(1.0, v[0] * self.clock_scale);
            let mut wet = [0.0f64; 2];
            for channel in 0..2 {
                let first = lowpass(self.held[channel], &mut self.post[channel][0], v[8]);
                let filtered = lowpass(first, &mut self.post[channel][1], v[8]);
                wet[channel] = mix(self.held[channel], filtered, v[2]);
            }
            let mid = (wet[0] + wet[1]) * 0.5;
            let side = (wet[0] - wet[1]) * 0.5 * v[3];
            l[i] = clean(mix(dry[0], (mid + side) * v[6], v[4])) as f32;
            r[i] = clean(mix(dry[1], (mid - side) * v[6], v[4])) as f32;
        }
    }

    fn reset(&mut self) {
        self.c.reset();
        self.cache.reset();
        self.phase = 1.0;
        self.clock_scale = 1.0;
        self.random = LOFI_SEED;
        self.held = [0.0; 2];
        self.pre = [[0.0; 2]; 2];
        self.post = [[0.0; 2]; 2];
    }

    fn tail(&self) -> f64 {
        0.5
    }
}

const BLOCK: usize = 32;

#[derive(Clone, Copy, Default)]
struct SaturatorChannel {
    tape_dc: DcBlock,
    tube_dc: DcBlock,
    envelope: f64,
    bass: f64,
    treble: f64,
    tape_bass: f64,
    tape_tone: f64,
    tape_memory: f64,
    tube_bass: f64,
    charge: f64,
}

/// Per-mode stereo decimator state.
#[derive(Clone, Copy, Default)]
struct DecimatorState {
    previous: [f32; 2],
    z1a: [f32; 2],
    z2a: [f32; 2],
    z1b: [f32; 2],
    z2b: [f32; 2],
}

/// The 2x decimation filter of `OversampledShape` in single precision.
#[derive(Clone, Copy)]
struct Decimator {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    c0: f32,
    c1: f32,
    c2: f32,
    e1: f32,
    e2: f32,
}

impl Decimator {
    // Two cascaded biquads over the interleaved 2x stream (mid, cur) of one
    // sample, both channels; returns the decimated (cur) output.
    #[inline(always)]
    fn decimate(&self, st: &mut DecimatorState, mid: &[f32; 2], cur: &[f32; 2]) -> [f32; 2] {
        let mut out = [0.0f32; 2];
        for k in 0..2 {
            let mut y = self.b0 * mid[k] + st.z1a[k];
            st.z1a[k] = self.b1 * mid[k] - self.a1 * y + st.z2a[k];
            st.z2a[k] = self.b2 * mid[k] - self.a2 * y;
            let mut w = self.c0 * y + st.z1b[k];
            st.z1b[k] = self.c1 * y - self.e1 * w + st.z2b[k];
            st.z2b[k] = self.c2 * y - self.e2 * w;
            y = self.b0 * cur[k] + st.z1a[k];
            st.z1a[k] = self.b1 * cur[k] - self.a1 * y + st.z2a[k];
            st.z2a[k] = self.b2 * cur[k] - self.a2 * y;
            w = self.c0 * y + st.z1b[k];
            st.z1b[k] = self.c1 * y - self.e1 * w + st.z2b[k];
            st.z2b[k] = self.c2 * y - self.e2 * w;
            out[k] = w;
        }
        out
    }
}

/// Saturator, block-processed. The shapers and their 2x decimation filters
/// run in single precision (same filter design as OversampledShape); the
/// low-frequency one-poles and DC blockers stay double. Classic mode has no
/// feedback through the shaper, so a whole sub-block is shaped at once
/// (vectorized tanh) and then decimated; Tape and Tube feed their shaper
/// output back (memory/charge), so they step sample by sample.
pub struct Saturator {
    core: Core,
    channels: [SaturatorChannel; 2],
    shapes: [DecimatorState; 3],
    decimator: Decimator,
    c: Controls<12>,
    cache: ParameterCache<11>,
    weights: [Smoother; 3],
    smoothing: f64,
    mode_smoothing: f64,
    attack: f64,
    release: f64,
    bass_pole: f64,
    treble_pole: f64,
    memory_pole: f64,
    charge_pole: f64,
    dc_pole: f64,
    /// `fast_tanh_f32(0.18)`: the tube shaper's bias point.
    tube_bias: f32,
    mode_weights: [f64; 3],
    weight_mode: i32,
    weights_settled: bool,
    // Only modes with a nonzero crossfade weight (and a nonzero Mix) run;
    // a mode's state restarts when it fades back in.
    live: [bool; 3],
    // Per-sample controls of the current sub-block (constant while settled).
    ctl: [[f64; BLOCK]; 15],
    ctl_constant: bool,
}

impl Saturator {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sample_rate;
        let mut core = Core::new(sr);
        for (slot, info) in core.p.iter_mut().zip(SATURATION_PARAMETERS.iter()) {
            *slot = info.default_value;
        }
        let mono = OversampledShape::new(sr);
        Saturator {
            core,
            channels: [SaturatorChannel::default(); 2],
            shapes: [DecimatorState::default(); 3],
            decimator: Decimator {
                b0: mono.first.b0 as f32,
                b1: mono.first.b1 as f32,
                b2: mono.first.b2 as f32,
                a1: mono.first.a1 as f32,
                a2: mono.first.a2 as f32,
                c0: mono.second.b0 as f32,
                c1: mono.second.b1 as f32,
                c2: mono.second.b2 as f32,
                e1: mono.second.a1 as f32,
                e2: mono.second.a2 as f32,
            },
            c: Controls::new(),
            cache: ParameterCache::default(),
            weights: [Smoother::default(); 3],
            smoothing: pole(8.0, sr),
            mode_smoothing: pole(5.0, sr),
            attack: pole(2.0, sr),
            release: pole(100.0, sr),
            bass_pole: filter_pole(180.0, sr),
            treble_pole: filter_pole(3000.0, sr),
            memory_pole: pole(0.6, sr),
            charge_pole: pole(60.0, sr),
            dc_pole: filter_pole(12.0, sr),
            tube_bias: fast_tanh_f32(0.18),
            mode_weights: [0.0; 3],
            weight_mode: -1,
            weights_settled: false,
            live: [false; 3],
            ctl: [[0.0; BLOCK]; 15],
            ctl_constant: false,
        }
    }

    /// Restarts the state of crossfade mode `m` (0 Classic, 1 Tape, 2 Tube).
    fn reset_mode(
        channels: &mut [SaturatorChannel; 2],
        shapes: &mut [DecimatorState; 3],
        m: usize,
    ) {
        for ch in channels.iter_mut() {
            if m == 1 {
                ch.tape_dc.reset();
                ch.tape_bass = 0.0;
                ch.tape_tone = 0.0;
                ch.tape_memory = 0.0;
            } else if m == 2 {
                ch.tube_dc.reset();
                ch.bass = 0.0;
                ch.treble = 0.0;
                ch.tube_bass = 0.0;
                ch.charge = 0.0;
            }
        }
        shapes[m] = DecimatorState::default();
    }
}

impl Processor for Saturator {
    core_accessors!(core);

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.core.sr;
        let mode = parameter(&self.core, &SATURATION_PARAMETERS, 0).round() as i32;
        if self.cache.changed(&self.core) {
            let core = &self.core;
            let value = |i: usize| parameter(core, &SATURATION_PARAMETERS, i);
            self.c.set_targets([
                filter_pole(1000.0 * 20.0f64.powf(value(2) * 0.01), sr),
                db_gain(value(1)),
                value(2) * 0.01,
                value(3) * 0.01,
                value(4) * 0.01,
                db_gain(value(5)),
                value(6) * 0.01,
                db_gain(value(7)),
                db_gain(value(8)),
                db_gain(value(9)),
                value(10).round(),
                db_gain((value(3) * 0.01 - 0.5) * 12.0),
            ]);
        }
        if mode != self.weight_mode {
            self.weight_mode = mode;
            self.weights_settled = false;
        }
        let (attack, release) = (self.attack, self.release);
        let (bass_pole, treble_pole, memory_pole, charge_pole, dc_pole) = (
            self.bass_pole,
            self.treble_pole,
            self.memory_pole,
            self.charge_pole,
            self.dc_pole,
        );
        let decimator = self.decimator;
        let tube_bias = self.tube_bias;
        for (bl, br) in l.chunks_mut(BLOCK).zip(r.chunks_mut(BLOCK)) {
            let n = bl.len();
            // Controls: per-sample while smoothing, otherwise constant.
            if !self.c.settled || !self.weights_settled {
                self.ctl_constant = false;
                for i in 0..n {
                    self.c.tick(self.smoothing);
                    let mut all = true;
                    for m in 0..3 {
                        let goal = if m as i32 == mode { 1.0 } else { 0.0 };
                        self.mode_weights[m] = self.weights[m].tick(goal, self.mode_smoothing);
                        all = all && self.mode_weights[m] == goal;
                    }
                    self.weights_settled = all;
                    for k in 0..12 {
                        self.ctl[k][i] = self.c.value[k];
                    }
                    for m in 0..3 {
                        self.ctl[12 + m][i] = self.mode_weights[m];
                    }
                }
            } else if !self.ctl_constant {
                for k in 0..12 {
                    self.ctl[k] = [self.c.value[k]; BLOCK];
                }
                for m in 0..3 {
                    self.ctl[12 + m] = [self.mode_weights[m]; BLOCK];
                }
                self.ctl_constant = true;
            }
            let ctl = &self.ctl;
            let (v0, v1, v2, v3, v4, v5) = (
                &ctl[0][..n],
                &ctl[1][..n],
                &ctl[2][..n],
                &ctl[3][..n],
                &ctl[4][..n],
                &ctl[5][..n],
            );
            let (v6, v7, v8, v9, v10, v11) = (
                &ctl[6][..n],
                &ctl[7][..n],
                &ctl[8][..n],
                &ctl[9][..n],
                &ctl[10][..n],
                &ctl[11][..n],
            );
            let (w0, w1, w2) = (&ctl[12][..n], &ctl[13][..n], &ctl[14][..n]);
            let mut any_mix = false;
            for i in 0..n {
                any_mix = any_mix || v6[i] != 0.0;
            }
            let mut on = [false; 3];
            for m in 0..3 {
                let mut any = false;
                for i in 0..n {
                    any = any || ctl[12 + m][i] != 0.0;
                }
                on[m] = any && any_mix;
                if on[m] && !self.live[m] {
                    Self::reset_mode(&mut self.channels, &mut self.shapes, m);
                }
                self.live[m] = on[m];
            }
            let mut dry = [[0.0f64; BLOCK]; 2];
            let mut input = [[0.0f64; BLOCK]; 2];
            let mut wet = [[0.0f64; BLOCK]; 2];
            for i in 0..n {
                dry[0][i] = clean(bl[i] as f64);
                dry[1][i] = clean(br[i] as f64);
                for k in 0..2 {
                    input[k][i] = dry[k][i] * v1[i];
                    wet[k][i] = 0.0;
                }
            }
            // Compression envelope (always tracked, like the per-sample form).
            let mut compression = [[0.0f64; BLOCK]; 2];
            for k in 0..2 {
                let mut env = self.channels[k].envelope;
                for i in 0..n {
                    let level = input[k][i].abs();
                    env = level + (env - level) * select(level > env, attack, release);
                    compression[k][i] = 1.0 / (1.0 + v2[i] * max(0.0, env - 0.25) * 2.0);
                }
                self.channels[k].envelope = env;
            }
            if on[0] {
                // Shape the whole sub-block (2x: midpoint + sample), then decimate.
                let mut shaped = [[0.0f32; 2 * BLOCK]; 2];
                let st = &mut self.shapes[0];
                for k in 0..2 {
                    let mut previous = st.previous[k];
                    for i in 0..n {
                        let x =
                            (input[k][i] * (1.0 + 24.0 * v3[i] * v3[i]) * compression[k][i]) as f32;
                        shaped[k][2 * i] = (previous + x) * 0.5f32;
                        shaped[k][2 * i + 1] = x;
                        previous = x;
                    }
                    st.previous[k] = previous;
                    for value in shaped[k][..2 * n].iter_mut() {
                        *value = fast_tanh_f32(*value);
                    }
                }
                for i in 0..n {
                    let mid = [shaped[0][2 * i], shaped[1][2 * i]];
                    let cur = [shaped[0][2 * i + 1], shaped[1][2 * i + 1]];
                    let y = decimator.decimate(st, &mid, &cur);
                    for k in 0..2 {
                        wet[k][i] += clean(y[k] as f64) * w0[i];
                    }
                }
            }
            if on[1] {
                let st = &mut self.shapes[1];
                for i in 0..n {
                    let mut mid = [0.0f32; 2];
                    let mut cur = [0.0f32; 2];
                    for k in 0..2 {
                        let ch = &mut self.channels[k];
                        let tape_low = lowpass(input[k][i], &mut ch.tape_bass, bass_pole);
                        // Tape Drive tilts the low-frequency response; Input drives compression.
                        let x = (input[k][i] + tape_low * (v11[i] - 1.0) + ch.tape_memory * 0.12)
                            as f32;
                        mid[k] = fast_tanh_f32(1.4f32 * ((st.previous[k] + x) * 0.5f32));
                        cur[k] = fast_tanh_f32(1.4f32 * x);
                        st.previous[k] = x;
                    }
                    let y = decimator.decimate(st, &mid, &cur);
                    for k in 0..2 {
                        let ch = &mut self.channels[k];
                        let shaped = clean(y[k] as f64);
                        lowpass(clamp(shaped, -1.0, 1.0), &mut ch.tape_memory, memory_pole);
                        wet[k][i] +=
                            lowpass(ch.tape_dc.tick(shaped, dc_pole), &mut ch.tape_tone, v0[i])
                                * w1[i];
                    }
                }
            }
            if on[2] {
                let st = &mut self.shapes[2];
                for i in 0..n {
                    let mut mid = [0.0f32; 2];
                    let mut cur = [0.0f32; 2];
                    for k in 0..2 {
                        let ch = &mut self.channels[k];
                        let x_in = input[k][i];
                        let low = lowpass(x_in, &mut ch.bass, bass_pole);
                        let high = x_in - lowpass(x_in, &mut ch.treble, treble_pole);
                        let eq =
                            x_in + (low * (v8[i] - 1.0) + high * (v9[i] - 1.0)) * (1.0 - v10[i]);
                        let x = ((eq + low * (v5[i] - 1.0)) * (1.0 + 32.0 * v3[i] * v3[i])
                            / (1.0 + 4.0 * v4[i] * ch.charge))
                            as f32;
                        const BIAS: f32 = 0.18;
                        mid[k] = fast_tanh_f32((st.previous[k] + x) * 0.5f32 + BIAS) - tube_bias;
                        cur[k] = fast_tanh_f32(x + BIAS) - tube_bias; // exactly 0 at x = 0
                        st.previous[k] = x;
                    }
                    let y = decimator.decimate(st, &mid, &cur);
                    for k in 0..2 {
                        let ch = &mut self.channels[k];
                        let tube = ch.tube_dc.tick(clean(y[k] as f64), dc_pole);
                        let charge_level = lowpass(tube, &mut ch.tube_bass, bass_pole).abs();
                        let charge_pole_now = select(charge_level > ch.charge, attack, charge_pole);
                        lowpass(charge_level, &mut ch.charge, charge_pole_now);
                        wet[k][i] += tube * w2[i];
                    }
                }
            }
            for i in 0..n {
                bl[i] = clean(mix(dry[0][i], wet[0][i] * v7[i], v6[i])) as f32;
                br[i] = clean(mix(dry[1][i], wet[1][i] * v7[i], v6[i])) as f32;
            }
        }
    }

    fn reset(&mut self) {
        self.channels = [SaturatorChannel::default(); 2];
        self.shapes = [DecimatorState::default(); 3];
        self.c.reset();
        self.cache.reset();
        self.mode_weights = [0.0; 3];
        self.live = [false; 3];
        self.weight_mode = -1;
        self.weights_settled = false;
        self.ctl_constant = false;
        for w in self.weights.iter_mut() {
            w.reset();
        }
    }

    fn tail(&self) -> f64 {
        0.5
    }
}

static DESCRIPTORS: [Descriptor; 3] = [
    Descriptor::new(
        c"distortion",
        c"Distortion",
        c"Distortion",
        0,
        &DISTORTION_PARAMETERS,
        |sr, _| Box::new(Distortion::new(sr)),
        |_, _, _| 3.0,
    ),
    Descriptor::new(
        c"lofi",
        c"Lofi",
        c"Distortion",
        0,
        &LOFI_PARAMETERS,
        |sr, _| Box::new(Lofi::new(sr)),
        |_, _, _| 0.5,
    ),
    Descriptor::new(
        c"saturator",
        c"Saturator",
        c"Distortion",
        0,
        &SATURATION_PARAMETERS,
        |sr, _| Box::new(Saturator::new(sr)),
        |_, _, _| 0.5,
    ),
];

/// The catalogue entries of the distortion effects.
pub fn descriptors() -> &'static [Descriptor] {
    &DESCRIPTORS
}

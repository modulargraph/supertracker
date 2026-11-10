//! Modulation processors: chorus, flanger, audio-rate FM, frequency shifter and phaser.
//!
//! Replaces `Sources/TrackerEngine/EffectsModulation.hpp` (namespace `stfx::modulation`).

use super::common::{
    clamp, clean, core_accessors, fast_cos_2pi, fast_exp, fast_sin_2pi, fast_tanh, frame_buffers, max, min, mix, pole, wrap, Core,
    DelayLine, Descriptor, Label, ParameterInfo as P, Processor, Smoother, PARAMETER_INTEGER as INTEGER, PARAMETER_LOG as LOG,
    PARAMETER_TOGGLE as TOGGLE, PI,
};

static CHORUS_PARAMS: [P; 4] = [
    P::new(c"rate", c"Rate", c"Hz", 0.03, 8.0, 0.8, LOG),
    P::new(c"amount", c"Amount", c"%", 0.0, 100.0, 50.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 35.0, 0),
    P::new(c"stereo", c"Stereo", c"deg", 0.0, 180.0, 90.0, 0),
];

pub struct Chorus {
    core: Core,
    d: [DelayLine; 2],
    phase: f64,
    rate: Smoother,
    depth: Smoother,
    wet: Smoother,
    stereo: Smoother,
}

impl Chorus {
    pub fn new(sr: f64) -> Self {
        Chorus {
            core: Core::new(sr),
            d: [DelayLine::new((sr * 0.05) as usize + 4), DelayLine::new((sr * 0.05) as usize + 4)],
            phase: 0.0,
            rate: Smoother::default(),
            depth: Smoother::default(),
            wet: Smoother::default(),
            stereo: Smoother::default(),
        }
    }
}

impl Processor for Chorus {
    core_accessors!(core);

    fn reset(&mut self) {
        for v in self.d.iter_mut() {
            v.reset();
        }
        self.phase = 0.0;
        self.rate.reset();
        self.depth.reset();
        self.wet.reset();
        self.stereo.reset();
    }

    fn tail(&self) -> f64 {
        0.04
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let smooth = pole(10.0, sr);
        for i in 0..l.len() {
            let speed = self.rate.tick(p[0], smooth);
            let amount = self.depth.tick(p[1] / 100.0, smooth);
            let w = self.wet.tick(p[2] / 100.0, smooth);
            let s = self.stereo.tick(p[3] / 360.0, smooth);
            let dl = l[i] as f64;
            let dr = r[i] as f64;
            let yl = self.d[0].read(sr * (0.015 + 0.006 * amount * fast_sin_2pi(self.phase)));
            let yr = self.d[1].read(sr * (0.015 + 0.006 * amount * fast_sin_2pi(self.phase + s)));
            self.d[0].push(dl);
            self.d[1].push(dr);
            self.phase = wrap(self.phase + speed / sr);
            l[i] = mix(dl, yl, w) as f32;
            r[i] = mix(dr, yr, w) as f32;
        }
    }
}

const SOURCES: &[Label] = &[Label::new(c"LFO"), Label::new(c"LFO Sync"), Label::new(c"Envelope")];
static FLANGER_PARAMS: [P; 10] = [
    P::new(c"frequency", c"Frequency", c"Hz", 20.0, 5000.0, 500.0, LOG),
    P::new(c"feedback", c"Feedback", c"%", -95.0, 95.0, 35.0, 0),
    P::new(c"invert", c"Invert", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"amount", c"Amount", c"%", -100.0, 100.0, 50.0, 0),
    P::choice(c"source", c"Modulation source", c"", 0.0, 2.0, 0.0, INTEGER, SOURCES),
    P::new(c"speed", c"LFO speed", c"Hz", 0.03, 8.0, 0.3, LOG),
    P::new(c"beats", c"LFO period", c"beats", 0.25, 64.0, 4.0, LOG),
    P::new(c"envshape", c"Envelope shape", c"", 0.25, 4.0, 1.0, LOG),
    P::new(c"stereo", c"Stereo", c"deg", 0.0, 180.0, 90.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 50.0, 0),
];

pub struct Flanger {
    core: Core,
    d: [DelayLine; 2],
    phase: f64,
    envelope: f64,
    delay: [Smoother; 2],
    wet: Smoother,
    feedback: Smoother,
}

impl Flanger {
    pub fn new(sr: f64) -> Self {
        Flanger {
            core: Core::new(sr),
            d: [DelayLine::new((sr * 0.12) as usize + 4), DelayLine::new((sr * 0.12) as usize + 4)],
            phase: 0.0,
            envelope: 0.0,
            delay: [Smoother::default(); 2],
            wet: Smoother::default(),
            feedback: Smoother::default(),
        }
    }
}

impl Processor for Flanger {
    core_accessors!(core);

    fn reset(&mut self) {
        for v in self.d.iter_mut() {
            v.reset();
        }
        self.phase = 0.0;
        self.envelope = 0.0;
        for v in self.delay.iter_mut() {
            v.reset();
        }
        self.wet.reset();
        self.feedback.reset();
    }

    fn tail(&self) -> f64 {
        15.0
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let rate = if p[4] == 1.0 { self.core.tempo / (60.0 * p[6]) } else { p[5] };
        let attack = pole(2.0, sr);
        let release = pole(100.0, sr);
        let smooth = pole(10.0, sr);
        for i in 0..l.len() {
            let x = [l[i] as f64, r[i] as f64];
            let peak = max(x[0].abs(), x[1].abs());
            self.envelope = clean(peak + (if peak > self.envelope { attack } else { release }) * (self.envelope - peak));
            let w = self.wet.tick(p[9] / 100.0, smooth);
            let fb = self.feedback.tick(p[1] / 100.0, smooth);
            let inv = if p[2] >= 0.5 { -1.0 } else { 1.0 };
            let mut out = [0.0f32; 2];
            for c in 0..2 {
                let modulation = if p[4] == 2.0 {
                    clamp(self.envelope, 0.0, 1.0).powf(p[7])
                } else {
                    fast_sin_2pi(self.phase + c as f64 * p[8] / 360.0)
                };
                let dt = self.delay[c].tick(
                    clamp(sr / (2.0 * p[0]) * fast_exp(0.6931471805599453 * (2.0 * p[3] / 100.0 * modulation)), 1.0, sr * 0.115),
                    smooth,
                );
                let y = self.d[c].read(dt);
                self.d[c].push(x[c] + fast_tanh(y) * fb);
                out[c] = mix(x[c], y * inv, w) as f32;
            }
            l[i] = out[0];
            r[i] = out[1];
            self.phase = wrap(self.phase + rate / sr);
        }
    }
}

static FM_PARAMS: [P; 5] = [
    P::new(c"rate", c"Rate", c"Hz", 0.1, 10000.0, 100.0, LOG),
    P::new(c"split", c"Split", c"Hz", 20.0, 16000.0, 500.0, LOG),
    P::new(c"contour", c"Contour", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"amount", c"Amount", c"%", 0.0, 100.0, 30.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
];

pub struct Fm {
    core: Core,
    low: [DelayLine; 2],
    high: [DelayLine; 2],
    dry: [DelayLine; 2],
    state: [f64; 2],
    phase: f64,
    envelope: f64,
    amount: Smoother,
    wet: Smoother,
    split: Smoother,
}

impl Fm {
    pub fn new(sr: f64) -> Self {
        Fm {
            core: Core::new(sr),
            low: [DelayLine::new(132), DelayLine::new(132)],
            high: [DelayLine::new(132), DelayLine::new(132)],
            dry: [DelayLine::new(132), DelayLine::new(132)],
            state: [0.0; 2],
            phase: 0.0,
            envelope: 0.0,
            amount: Smoother::default(),
            wet: Smoother::default(),
            split: Smoother::default(),
        }
    }
}

impl Processor for Fm {
    core_accessors!(core);

    fn latency(&self) -> u32 {
        64
    }

    fn tail(&self) -> f64 {
        0.15
    }

    fn reset(&mut self) {
        for v in self.low.iter_mut().chain(self.high.iter_mut()).chain(self.dry.iter_mut()) {
            v.reset();
        }
        self.state = [0.0; 2];
        self.phase = 0.0;
        self.envelope = 0.0;
        self.amount.reset();
        self.wet.reset();
        self.split.reset();
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let release = pole(80.0, sr);
        let smooth = pole(5.0, sr);
        let rate = min(p[0], sr * 0.4);
        let split_pole = (-2.0 * PI * min(p[1], sr * 0.45) / sr).exp();
        for i in 0..l.len() {
            let x = [l[i] as f64, r[i] as f64];
            let pk = max(x[0].abs(), x[1].abs());
            self.envelope = clean(max(pk, self.envelope * release));
            let depth = self.amount.tick(p[3] / 100.0, smooth) * mix(1.0, clamp(self.envelope, 0.0, 1.0), p[2] / 100.0);
            let w = self.wet.tick(p[4] / 100.0, smooth);
            let a = self.split.tick(split_pole, smooth);
            let offset = 63.0 * depth * fast_sin_2pi(self.phase);
            self.phase = wrap(self.phase + rate / sr);
            let mut out = [0.0f32; 2];
            for c in 0..2 {
                self.state[c] = clean(x[c] + a * (self.state[c] - x[c]));
                let yd = self.dry[c].read(64.0);
                let yl = self.low[c].read(64.0);
                let yh = self.high[c].read(64.0 + offset);
                self.low[c].push(self.state[c]);
                self.high[c].push(x[c] - self.state[c]);
                self.dry[c].push(x[c]);
                out[c] = mix(yd, yl + yh, w) as f32;
            }
            l[i] = out[0];
            r[i] = out[1];
        }
    }
}

static SHIFT_PARAMS: [P; 6] = [
    P::new(c"coarse", c"Coarse", c"Hz", -5000.0, 5000.0, 0.0, 0),
    P::new(c"fine", c"Fine", c"Hz", -10.0, 10.0, 0.0, 0),
    P::new(c"feedback", c"Feedback", c"%", -95.0, 95.0, 0.0, 0),
    P::new(c"stereo", c"Stereo", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"invert", c"Invert", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
];

const TAPS: usize = 63;
const GROUP_DELAY: usize = 31;

pub struct FrequencyShifter {
    core: Core,
    // History is mirrored (each sample stored at i and i+taps) so the FIR
    // window is contiguous; only the 32 nonzero (even-index) taps are used.
    coefficients: [f64; TAPS],
    dry: [[f64; TAPS]; 2],
    history: [[f64; 2 * TAPS]; 2],
    cursor: usize,
    phase: [f64; 2],
    last: [f64; 2],
    shift: Smoother,
    feedback: Smoother,
    wet: Smoother,
}

impl FrequencyShifter {
    pub fn new(sr: f64) -> Self {
        let mut coefficients = [0.0f64; TAPS];
        // Windowed analytic-signal FIR, derived directly from the ideal
        // Hilbert transform kernel. Odd tap offsets alone are nonzero.
        for (i, coefficient) in coefficients.iter_mut().enumerate() {
            let k = i as i32 - GROUP_DELAY as i32;
            if k != 0 && (k % 2) != 0 {
                *coefficient = 2.0 / (PI * k as f64) * (0.54 - 0.46 * (2.0 * PI * i as f64 / (TAPS - 1) as f64).cos());
            }
        }
        FrequencyShifter {
            core: Core::new(sr),
            coefficients,
            dry: [[0.0; TAPS]; 2],
            history: [[0.0; 2 * TAPS]; 2],
            cursor: 0,
            phase: [0.0; 2],
            last: [0.0; 2],
            shift: Smoother::default(),
            feedback: Smoother::default(),
            wet: Smoother::default(),
        }
    }
}

impl Processor for FrequencyShifter {
    core_accessors!(core);

    fn latency(&self) -> u32 {
        GROUP_DELAY as u32
    }

    fn tail(&self) -> f64 {
        1.0
    }

    fn reset(&mut self) {
        self.history = [[0.0; 2 * TAPS]; 2];
        self.dry = [[0.0; TAPS]; 2];
        self.cursor = 0;
        self.phase = [0.0; 2];
        self.last = [0.0; 2];
        self.shift.reset();
        self.feedback.reset();
        self.wet.reset();
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let smooth = pole(5.0, sr);
        for i in 0..l.len() {
            let freq = self.shift.tick((p[0] + p[1]) * (if p[4] >= 0.5 { -1.0 } else { 1.0 }), smooth);
            let fb = self.feedback.tick(p[2] / 100.0, smooth);
            let w = self.wet.tick(p[5] / 100.0, smooth);
            // The cursor only ever takes values 0..TAPS-1 (see the wrap below).
            let cursor = self.cursor % TAPS;
            let x_in = [l[i] as f64, r[i] as f64];
            let mut out = [0.0f32; 2];
            for c in 0..2 {
                let x = x_in[c];
                let h = clamp(x + self.last[c] * fb, -16.0, 16.0);
                self.history[c][cursor] = h;
                self.history[c][cursor + TAPS] = h;
                self.dry[c][cursor] = x;
                let delayed = (cursor + TAPS - GROUP_DELAY) % TAPS;
                // Newest sample at cursor+taps; tap t reads t samples back.
                // Four partial sums: independent chains instead of one serial chain.
                // window[j] is history[cursor + 1 + j], so "t samples back" is window[TAPS - 1 - t].
                let window: &[f64; TAPS] = self.history[c][cursor + 1..cursor + 1 + TAPS].try_into().unwrap_or(&[0.0; TAPS]);
                let (mut q0, mut q1, mut q2, mut q3) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
                let mut t = 0;
                while t < 64 {
                    q0 += self.coefficients[t] * window[TAPS - 1 - t];
                    q1 += self.coefficients[t + 2] * window[TAPS - 1 - (t + 2)];
                    q2 += self.coefficients[t + 4] * window[TAPS - 1 - (t + 4)];
                    if t + 6 < TAPS {
                        q3 += self.coefficients[t + 6] * window[TAPS - 1 - (t + 6)];
                    }
                    t += 8;
                }
                let q = (q0 + q1) + (q2 + q3);
                let real = self.history[c][delayed];
                self.last[c] = clean(real * fast_cos_2pi(self.phase[c]) - q * fast_sin_2pi(self.phase[c]));
                out[c] = mix(self.dry[c][delayed], self.last[c], w) as f32;
                self.phase[c] = wrap(self.phase[c] + freq * (if c != 0 { 1.0 - 2.0 * p[3] / 100.0 } else { 1.0 }) / sr);
            }
            l[i] = out[0];
            r[i] = out[1];
            self.cursor = cursor + 1;
            if self.cursor == TAPS {
                self.cursor = 0;
            }
        }
    }
}

static PHASER_PARAMS: [P; 10] = [
    P::new(c"frequency", c"Frequency", c"Hz", 20.0, 10000.0, 600.0, LOG),
    P::new(c"feedback", c"Feedback", c"%", -95.0, 95.0, 30.0, 0),
    P::new(c"eightpole", c"Eight poles", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"amount", c"Amount", c"%", -100.0, 100.0, 60.0, 0),
    P::choice(c"source", c"Modulation source", c"", 0.0, 2.0, 0.0, INTEGER, SOURCES),
    P::new(c"speed", c"LFO speed", c"Hz", 0.03, 8.0, 0.4, LOG),
    P::new(c"beats", c"LFO period", c"beats", 0.25, 64.0, 4.0, LOG),
    P::new(c"envshape", c"Envelope shape", c"", 0.25, 4.0, 1.0, LOG),
    P::new(c"stereo", c"Stereo", c"deg", 0.0, 180.0, 90.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 50.0, 0),
];

pub struct Phaser {
    core: Core,
    state: [[f64; 8]; 2],
    last: [f64; 2],
    coefficient: [f64; 2],
    phase: f64,
    envelope: f64,
    control: u32,
    frequency: [Smoother; 2],
    wet: Smoother,
    feedback: Smoother,
    // Only the selected number of all-pass stages runs; stages 5-8 restart
    // from rest when Eight poles is switched on.
    active_stages: usize,
}

impl Phaser {
    pub fn new(sr: f64) -> Self {
        Phaser {
            core: Core::new(sr),
            state: [[0.0; 8]; 2],
            last: [0.0; 2],
            coefficient: [0.0; 2],
            phase: 0.0,
            envelope: 0.0,
            control: 0,
            frequency: [Smoother::default(); 2],
            wet: Smoother::default(),
            feedback: Smoother::default(),
            active_stages: 8,
        }
    }
}

impl Processor for Phaser {
    core_accessors!(core);

    fn reset(&mut self) {
        self.state = [[0.0; 8]; 2];
        self.last = [0.0; 2];
        self.phase = 0.0;
        self.envelope = 0.0;
        self.control = 0;
        for v in self.frequency.iter_mut() {
            v.reset();
        }
        self.wet.reset();
        self.feedback.reset();
        self.active_stages = 8;
    }

    fn tail(&self) -> f64 {
        45.0
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let rate = if p[4] == 1.0 { self.core.tempo / (60.0 * p[6]) } else { p[5] };
        let attack = pole(2.0, sr);
        let release = pole(100.0, sr);
        let smooth = pole(5.0, sr);
        let stages: usize = if p[2] >= 0.5 { 8 } else { 4 };
        if stages > self.active_stages {
            for c in self.state.iter_mut() {
                for v in c[self.active_stages..stages].iter_mut() {
                    *v = 0.0;
                }
            }
        }
        self.active_stages = stages;
        for i in 0..l.len() {
            let x = [l[i] as f64, r[i] as f64];
            let pk = max(x[0].abs(), x[1].abs());
            self.envelope = clean(pk + (if pk > self.envelope { attack } else { release }) * (self.envelope - pk));
            let w = self.wet.tick(p[9] / 100.0, smooth);
            let fb = self.feedback.tick(p[1] / 100.0, smooth);
            let mut out = [0.0f32; 2];
            for c in 0..2 {
                let modulation = if p[4] == 2.0 {
                    clamp(self.envelope, 0.0, 1.0).powf(p[7])
                } else {
                    fast_sin_2pi(self.phase + c as f64 * p[8] / 360.0)
                };
                let hz = self.frequency[c]
                    .tick(clamp(p[0] * fast_exp(0.6931471805599453 * (3.0 * p[3] / 100.0 * modulation)), 10.0, sr * 0.45), smooth);
                if self.control == 0 {
                    let g = (PI * hz / sr).tan();
                    self.coefficient[c] = (1.0 - g) / (1.0 + g);
                }
                let mut y = x[c] + fast_tanh(self.last[c]) * fb;
                let a = self.coefficient[c];
                for state in self.state[c][..stages].iter_mut() {
                    let stage_out = -a * y + *state;
                    *state = clean(y + a * stage_out);
                    y = clean(stage_out);
                }
                self.last[c] = y;
                out[c] = mix(x[c], self.last[c], w) as f32;
            }
            l[i] = out[0];
            r[i] = out[1];
            self.control = (self.control + 1) % 8;
            self.phase = wrap(self.phase + rate / sr);
        }
    }
}

static DESCRIPTORS: [Descriptor; 5] = [
    Descriptor::new(c"chorus", c"Chorus", c"Modulation", 0, &CHORUS_PARAMS, |sr, _| Box::new(Chorus::new(sr)), |_, _, _| 0.04),
    Descriptor::new(c"flanger", c"Flanger", c"Modulation", 0, &FLANGER_PARAMS, |sr, _| Box::new(Flanger::new(sr)), |_, _, _| 15.0),
    Descriptor::new(c"fm", c"Audio FM", c"Modulation", 0, &FM_PARAMS, |sr, _| Box::new(Fm::new(sr)), |_, _, _| 0.15),
    Descriptor::new(
        c"freqshifter",
        c"Frequency Shifter",
        c"Modulation",
        0,
        &SHIFT_PARAMS,
        |sr, _| Box::new(FrequencyShifter::new(sr)),
        |_, _, _| 1.0,
    ),
    Descriptor::new(c"phaser", c"Phaser", c"Modulation", 0, &PHASER_PARAMS, |sr, _| Box::new(Phaser::new(sr)), |_, _, _| 45.0),
];

/// `stfx::modulationDescriptors`.
pub fn descriptors() -> &'static [Descriptor] {
    &DESCRIPTORS
}

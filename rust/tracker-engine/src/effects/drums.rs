//! Drum synthesis voices: Kick, Snare, Hihat, Tom, Perc and Cymbal.
//!
//! Replaces `Sources/TrackerEngine/MaschineDrums.hpp` (namespace `stdrums`).
//!
//! Original drum voices inspired by common analog and modal synthesis methods.
//! These six families are not NI's individual Drum Synth engines or a decoder
//! for their parameter arrays. Callers must map imported controls explicitly.

use super::common::{clamp, clean, db_gain, max, min, wrap, ParameterInfo as P, PARAMETER_LOG as LOG, PI};
use std::ffi::CStr;

/// No drum model.
pub const NONE: i32 = 0;
pub const KICK: i32 = 1;
pub const SNARE: i32 = 2;
pub const HIHAT: i32 = 3;
pub const TOM: i32 = 4;
pub const PERC: i32 = 5;
pub const CYMBAL: i32 = 6;

/// Every model has eight normalized controls.
pub const PARAMETER_COUNT: u32 = 8;

pub fn valid_model(model: i32) -> bool {
    (KICK..=CYMBAL).contains(&model)
}

/// The persistent id of a model ("none" for anything invalid).
pub fn model_id(model: i32) -> &'static CStr {
    const NAMES: [&CStr; 7] = [c"none", c"kick", c"snare", c"hihat", c"tom", c"perc", c"cymbal"];
    NAMES[if valid_model(model) { model as usize } else { 0 }]
}

static COMMON: [P; 8] = [
    P::new(c"tune", c"Tune", c"st", -24.0, 24.0, 0.0, 0),
    P::new(c"decay", c"Decay", c"ms", 20.0, 3000.0, 350.0, LOG),
    P::new(c"tone", c"Tone", c"%", 0.0, 100.0, 50.0, 0),
    P::new(c"impact", c"Impact", c"%", 0.0, 100.0, 50.0, 0),
    P::new(c"noise", c"Noise", c"%", 0.0, 100.0, 25.0, 0),
    P::new(c"attack", c"Attack", c"ms", 0.05, 50.0, 0.2, LOG),
    P::new(c"drive", c"Drive", c"dB", 0.0, 24.0, 0.0, 0),
    P::new(c"character", c"Character", c"%", 0.0, 100.0, 30.0, 0),
];
static DECAYS: [P; 6] = [
    P::new(c"decay", c"Decay", c"ms", 20.0, 3000.0, 350.0, LOG),
    P::new(c"decay", c"Decay", c"ms", 20.0, 2000.0, 220.0, LOG),
    P::new(c"decay", c"Decay", c"ms", 10.0, 3000.0, 120.0, LOG),
    P::new(c"decay", c"Decay", c"ms", 30.0, 4000.0, 450.0, LOG),
    P::new(c"decay", c"Decay", c"ms", 10.0, 4000.0, 300.0, LOG),
    P::new(c"decay", c"Decay", c"ms", 100.0, 12000.0, 2500.0, LOG),
];

/// Metadata of control `index` of `model`; the decay range differs per model.
pub fn parameter_info(model: i32, index: u32) -> Option<&'static P> {
    if !valid_model(model) || index >= PARAMETER_COUNT {
        return None;
    }
    if index == 1 {
        DECAYS.get((model - 1) as usize)
    } else {
        COMMON.get(index as usize)
    }
}

/// Normalized control -> physical value; nonfinite controls use the default.
pub fn physical_parameter(model: i32, index: u32, n: f64) -> f64 {
    let Some(p) = parameter_info(model, index) else {
        return 0.0;
    };
    if !n.is_finite() {
        return p.default_value;
    }
    let n = clamp(n, 0.0, 1.0);
    if (p.flags & LOG) != 0 {
        p.minimum * (p.maximum / p.minimum).powf(n)
    } else {
        p.minimum + (p.maximum - p.minimum) * n
    }
}

/// The normalized default of control `index` of `model`.
pub fn default_parameter(model: i32, index: u32) -> f32 {
    let Some(p) = parameter_info(model, index) else {
        return 0.0;
    };
    (if (p.flags & LOG) != 0 {
        (p.default_value / p.minimum).ln() / (p.maximum / p.minimum).ln()
    } else {
        (p.default_value - p.minimum) / (p.maximum - p.minimum)
    }) as f32
}

/// One drum hit. `start` prepares every coefficient; `tick` renders a sample without
/// allocating.
#[derive(Clone)]
pub struct Voice {
    model: i32,
    sr: f64,
    base_hz: f64,
    decay: f64,
    body_decay: f64,
    noise_decay: f64,
    impact_decay: f64,
    attack_decay: f64,
    tone: f64,
    impact: f64,
    noise_mix: f64,
    drive: f64,
    character: f64,
    envelope: f64,
    body_envelope: f64,
    noise_envelope: f64,
    pitch_envelope: f64,
    attack_envelope: f64,
    noise_lp: f64,
    noise_hp: f64,
    body_lp: f64,
    metal_hp: f64,
    dc_input: f64,
    dc_state: f64,
    low_pole: f64,
    high_pole: f64,
    body_pole: f64,
    dc_pole: f64,
    phase: [f64; 6],
    modal_phase: [f64; 8],
    modal_envelope: [f64; 8],
    noise_state: u32,
    age: u64,
    max_age: u64,
    running: bool,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            model: NONE,
            sr: 48000.0,
            base_hz: 55.0,
            decay: 0.999,
            body_decay: 0.998,
            noise_decay: 0.997,
            impact_decay: 0.99,
            attack_decay: 0.9,
            tone: 0.5,
            impact: 0.5,
            noise_mix: 0.25,
            drive: 1.0,
            character: 0.3,
            envelope: 0.0,
            body_envelope: 0.0,
            noise_envelope: 0.0,
            pitch_envelope: 0.0,
            attack_envelope: 1.0,
            noise_lp: 0.0,
            noise_hp: 0.0,
            body_lp: 0.0,
            metal_hp: 0.0,
            dc_input: 0.0,
            dc_state: 0.0,
            low_pole: 0.5,
            high_pole: 0.9,
            body_pole: 0.5,
            dc_pole: 0.999,
            phase: [0.0; 6],
            modal_phase: [0.0; 8],
            modal_envelope: [0.0; 8],
            noise_state: 1,
            age: 0,
            max_age: 0,
            running: false,
        }
    }
}

/// xorshift32 step mapped to [-1, 1).
#[inline(always)]
fn noise(state: &mut u32) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state as f64 / 2147483648.0 - 1.0
}

/// Polynomial band-limited step residual around a phase discontinuity.
#[inline(always)]
fn blep(phase: f64, dt: f64) -> f64 {
    if phase < dt {
        let x = phase / dt;
        return 2.0 * x - x * x - 1.0;
    }
    if phase > 1.0 - dt {
        let x = (phase - 1.0) / dt;
        return x * x + 2.0 * x + 1.0;
    }
    0.0
}

impl Voice {
    /// Triggers the voice. `normalized` holds up to eight normalized controls; missing or
    /// nonfinite ones use the model defaults. A zero `seed` is replaced by 1 (the C++ default
    /// seed is also 1).
    pub fn start(&mut self, kind: i32, sample_rate: f64, normalized: &[f32], seed: u32) {
        *self = Voice::default();
        self.model = if valid_model(kind) { kind } else { NONE };
        if self.model == NONE {
            return;
        }
        let model = self.model;
        self.sr = if sample_rate.is_finite() { clamp(sample_rate, 8000.0, 384000.0) } else { 48000.0 };
        let sr = self.sr;
        let mut p = [0.0f64; PARAMETER_COUNT as usize];
        for (j, slot) in p.iter_mut().enumerate() {
            let control = normalized.get(j).copied().unwrap_or_else(|| default_parameter(model, j as u32));
            *slot = physical_parameter(model, j as u32, control as f64);
        }
        const BASE: [f64; 7] = [0.0, 55.0, 185.0, 420.0, 125.0, 330.0, 370.0];
        self.base_hz = BASE[model as usize] * (p[0] / 12.0).exp2();
        let seconds = p[1] * 0.001;
        self.decay = (0.001f64.ln() / (seconds * sr)).exp();
        self.body_decay = (0.001f64.ln() / (seconds * sr * (if model == SNARE { 0.55 } else { 0.85 }))).exp();
        self.noise_decay = (0.001f64.ln()
            / (seconds
                * sr
                * (if model == KICK {
                    0.12
                } else if model == TOM {
                    0.2
                } else {
                    1.0
                })))
        .exp();
        self.impact_decay = (-1.0 / (sr * (if model == KICK { 0.025 } else { 0.012 }))).exp();
        self.attack_decay = (-1.0 / (sr * p[5] * 0.001)).exp();
        self.tone = p[2] / 100.0;
        self.impact = p[3] / 100.0;
        self.noise_mix = p[4] / 100.0;
        self.drive = db_gain(p[6]);
        self.character = p[7] / 100.0;
        let tone = self.tone;
        let upper = min(sr * 0.44, 1800.0 + tone * tone * 16000.0);
        let lower = if model == HIHAT {
            2500.0 + tone * 3000.0
        } else if model == CYMBAL {
            700.0 + tone * 2000.0
        } else if model == SNARE {
            600.0 + tone * 1300.0
        } else {
            100.0
        };
        self.low_pole = (-2.0 * PI * upper / sr).exp();
        self.high_pole = (-2.0 * PI * min(sr * 0.35, lower) / sr).exp();
        self.body_pole = (-2.0 * PI * min(sr * 0.4, 600.0 + tone * 6000.0) / sr).exp();
        self.dc_pole = (-2.0 * PI * 12.0 / sr).exp();
        self.envelope = 1.0;
        self.body_envelope = 1.0;
        self.noise_envelope = 1.0;
        self.pitch_envelope = 1.0;
        self.attack_envelope = 1.0;
        self.modal_envelope = [1.0; 8];
        self.noise_state = if seed != 0 { seed } else { 1 };
        self.running = true;
        self.max_age = ((seconds * 2.7 + 0.1) * sr).ceil() as u64;
    }

    pub fn active(&self) -> bool {
        self.running
    }

    pub fn stop(&mut self) {
        self.running = false;
        self.envelope = 0.0;
        self.body_envelope = 0.0;
        self.noise_envelope = 0.0;
    }

    /// Renders one sample at `pitch_ratio` times the model's root pitch (1 = root).
    pub fn tick(&mut self, pitch_ratio: f64) -> f32 {
        if !self.running {
            return 0.0;
        }
        let pitch_ratio = if pitch_ratio.is_finite() { clamp(pitch_ratio, 1e-9, 1e9) } else { 1.0 };
        let sr = self.sr;
        let model = self.model;
        let (impact, character, noise_mix) = (self.impact, self.character, self.noise_mix);
        let white = noise(&mut self.noise_state);
        self.noise_lp = clean(white + self.low_pole * (self.noise_lp - white));
        self.noise_hp = clean(self.noise_lp + self.high_pole * (self.noise_hp - self.noise_lp));
        let n = self.noise_lp - self.noise_hp;
        let pitch = self.base_hz * pitch_ratio;
        let mut out;
        let sweep = (impact * self.pitch_envelope * (if model == KICK { 2.8 } else { 0.8 })).exp2();
        if model == KICK {
            self.phase[0] = wrap(self.phase[0] + min(sr * 0.4, pitch * sweep) / sr);
            let fundamental = (2.0 * PI * self.phase[0]).sin();
            let harmonic = (4.0 * PI * self.phase[0]).sin();
            out = (fundamental * (1.0 - character * 0.35) + harmonic * character * 0.35) * self.envelope
                + n * noise_mix * self.noise_envelope * 0.4;
        } else if model == SNARE {
            self.phase[0] = wrap(self.phase[0] + min(sr * 0.4, pitch * sweep) / sr);
            self.phase[1] = wrap(self.phase[1] + min(sr * 0.4, pitch * 1.618) / sr);
            let body = 0.7 * (2.0 * PI * self.phase[0]).sin() + 0.3 * (2.0 * PI * self.phase[1]).sin();
            let snap = n * (0.25 + noise_mix * 1.25) * (1.0 + impact * self.pitch_envelope * 2.0);
            out = body * self.body_envelope * (0.8 - noise_mix * 0.55) + snap * self.noise_envelope;
            out += character * 0.2 * (2.0 * PI * (self.phase[0] + 1.7 * (2.0 * PI * self.phase[1]).sin())).sin() * self.body_envelope;
        } else if model == HIHAT {
            const RATIOS: [f64; 6] = [1.0, 1.342, 1.789, 2.321, 2.891, 3.671];
            let mut metal = 0.0f64;
            for j in 0..6 {
                let dt = min(0.42, pitch * RATIOS[j] / sr);
                self.phase[j] = wrap(self.phase[j] + dt);
                let square = (if self.phase[j] < 0.5 { 1.0 } else { -1.0 }) + blep(self.phase[j], dt) - blep(wrap(self.phase[j] + 0.5), dt);
                metal += square * (if j % 2 != 0 { 0.17 } else { 0.16 });
            }
            self.metal_hp = clean(metal + self.high_pole * (self.metal_hp - metal));
            metal -= self.metal_hp;
            out = (metal * (0.7 + character * 0.3) + n * (0.2 + noise_mix)) * self.envelope * (1.0 + impact * self.pitch_envelope);
        } else if model == TOM {
            const RATIOS: [f64; 4] = [1.0, 1.593, 2.135, 2.917];
            let mut body = 0.0f64;
            for j in 0..4 {
                self.phase[j] = wrap(self.phase[j] + min(sr * 0.4, pitch * RATIOS[j] * (1.0 + impact * self.pitch_envelope * 0.3)) / sr);
                let harmonic = if j != 0 { character / (j + 1) as f64 } else { 1.0 };
                body += (2.0 * PI * self.phase[j]).sin() * harmonic;
            }
            out = body * self.envelope * 0.8 + n * noise_mix * self.noise_envelope * 0.5;
        } else if model == PERC {
            self.phase[0] = wrap(self.phase[0] + min(sr * 0.4, pitch * sweep) / sr);
            self.phase[1] = wrap(self.phase[1] + min(sr * 0.4, pitch * (1.41 + character * 4.1)) / sr);
            let modulation = (2.0 * PI * self.phase[1]).sin() * (0.2 + character * 5.0) * self.body_envelope;
            out = (2.0 * PI * self.phase[0] + modulation).sin() * self.envelope * (1.0 - noise_mix * 0.65) + n * noise_mix * self.noise_envelope;
        } else {
            const RATIOS: [f64; 8] = [1.0, 1.483, 2.187, 3.271, 4.563, 6.119, 8.217, 11.713];
            let mut metal = 0.0f64;
            let decay = self.decay;
            for j in 0..8 {
                let jf = j as f64;
                let hz = min(sr * 0.44, pitch * RATIOS[j] * (1.0 + character * 0.07 * jf));
                self.modal_phase[j] = wrap(self.modal_phase[j] + hz / sr);
                metal += (2.0 * PI * self.modal_phase[j]).sin() * self.modal_envelope[j] * (1.0 / (2.0 + jf * 0.6));
                // Higher modes damp more quickly; coefficient depends only on the
                // prepared decay and partial index, avoiding allocations/state IO.
                self.modal_envelope[j] *= decay;
                self.modal_envelope[j] *= 1.0 - (1.0 - decay) * jf * 0.18;
            }
            self.metal_hp = clean(metal + self.high_pole * (self.metal_hp - metal));
            metal -= self.metal_hp;
            out = metal * (0.5 + character * 0.3) + n * (0.2 + noise_mix) * self.noise_envelope * (1.0 + impact * self.pitch_envelope);
        }
        self.body_lp = clean(out + self.body_pole * (self.body_lp - out));
        if model == KICK || model == TOM || model == PERC {
            out = self.body_lp;
        }
        out *= 1.0 - self.attack_envelope;
        out = (out * self.drive).tanh() / max(1.0, self.drive.sqrt());
        let dc = out - self.dc_input + self.dc_pole * self.dc_state;
        self.dc_input = out;
        self.dc_state = clean(dc);
        self.envelope *= self.decay;
        self.body_envelope *= self.body_decay;
        self.noise_envelope *= self.noise_decay;
        self.pitch_envelope *= self.impact_decay;
        self.attack_envelope *= self.attack_decay;
        self.age = self.age.wrapping_add(1);
        if self.age >= self.max_age || (self.envelope < 1e-8 && self.noise_envelope < 1e-8) {
            self.running = false;
            return 0.0;
        }
        clamp(self.dc_state, -1.0, 1.0) as f32
    }
}

//! Delay processors: Beat Delay, Grain Delay, Grain Stretch and Resochord.
//!
//! Ported from the C++ `EffectsDelay.hpp` (namespace `stfx::delay`).

use super::common::{
    core_accessors, frame_buffers, max, min, pow2, Core, Descriptor, Label, ParameterInfo as P,
    Processor, MAX_PARAMETERS, PARAMETER_INTEGER as INTEGER, PARAMETER_LOG as LOG,
    PARAMETER_TOGGLE as TOGGLE,
};
use super::spatial::{self, bounded, clean, soft, Line, ZeroMix};

/// Playback rate for a pitch offset in semitones (clamped to +-36).
#[inline]
pub fn rate(semitones: f64) -> f64 {
    pow2(bounded(semitones, -36.0, 36.0) / 12.0)
}

/// Tempo clamped to the tracker range.
#[inline]
pub fn bpm(value: f64) -> f64 {
    bounded(value, 20.0, 999.0)
}

/// Wraps a read position into [0, length).
#[inline(always)]
pub fn wrap_position(p: f64, length: usize) -> f64 {
    let len = length.max(1) as f64;
    p - (p / len).floor() * len
}

/// Linear-interpolated read of the first `length` samples of `line` as a loop.
#[inline(always)]
pub fn at(line: &Line, position: f64, length: usize) -> f32 {
    let length = length.min(line.data.len()).max(1);
    let position = wrap_position(position, length);
    let a = position as usize;
    let b = if a + 1 == length { 0 } else { a + 1 };
    let f = (position - a as f64) as f32;
    // Rounding in wrap_position can return exactly `length`; where that is the end of the
    // buffer (an out-of-bounds read in the C++), the missing samples read as silence.
    let xa = line.data.get(a).copied().unwrap_or(0.0);
    let xb = line.data.get(b).copied().unwrap_or(0.0);
    xa + (xb - xa) * f
}

/// Delay time units in beats: 1/64 .. 1/4, then the 1/16, 1/8 and 1/4 triplets.
const UNITS: [f64; 8] = [
    0.0625,
    0.125,
    0.25,
    0.5,
    1.0,
    1.0 / 6.0,
    1.0 / 3.0,
    2.0 / 3.0,
];

#[inline(always)]
fn unit(value: f64) -> f64 {
    UNITS[(bounded(value, 0.0, 7.0) as usize).min(7)]
}

pub struct BeatDelay {
    core: Core,
    memory: [Line; 2],
    low: [f64; 2],
    del: f64,
    target: f64,
    smoothing: f64,
    first: bool,
}

impl BeatDelay {
    pub fn new(sr: f64) -> Self {
        BeatDelay {
            core: Core::new(sr),
            memory: std::array::from_fn(|_| Line::with_capacity((sr * 51.0) as usize + 8)),
            low: [0.0; 2],
            del: 1.0,
            target: 1.0,
            smoothing: 1.0 - (-1.0 / (0.02 * sr)).exp(),
            first: true,
        }
    }

    /// `BeatDelay::tailForParameters`.
    pub fn tail_estimate(sr: f64, values: &[f64; MAX_PARAMETERS], tempo_bpm: f64) -> f64 {
        let duration = max(
            1.0 / sr,
            60.0 / bpm(tempo_bpm) * unit(values[8]) * ((values[0] + 1.0) * 0.5 + values[1] * 0.01),
        );
        let feedback = bounded(values[2] * 0.01, 0.0, 0.97);
        // Estimated time to -100 dB including the first echo; pure target-data
        // calculation is safe before first render and during concurrent edits.
        duration
            * (if feedback > 0.0 {
                1.0 + (1.0e-5f64.ln() / feedback.ln()).ceil()
            } else {
                1.0
            })
    }
}

impl Processor for BeatDelay {
    core_accessors!(core);

    fn reset(&mut self) {
        for d in self.memory.iter_mut() {
            d.reset();
        }
        self.low = [0.0; 2];
        self.first = true;
    }

    fn tail(&self) -> f64 {
        20000.0
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        Self::tail_estimate(self.core.sr, values, tempo)
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        self.target = max(
            1.0,
            sr * 60.0 / bpm(self.core.tempo) * unit(p[8]) * ((p[0] + 1.0) * 0.5 + p[1] * 0.01),
        );
        if self.first {
            self.del = self.target;
            self.first = false;
        }
        let feedback = bounded(p[2] * 0.01, 0.0, 0.97);
        let cross = bounded(p[3] * 0.01, 0.0, 1.0);
        let split = p[5] * 0.01;
        let mix = bounded(p[7] * 0.01, 0.0, 1.0);
        let width = p[6] * 0.01;
        // std::pow(x, 2.) is compiled as x*x.
        let color = p[4] * 0.01;
        let cut = 250.0 + (color * color) * 18000.0;
        let a = [
            spatial::pole(cut * pow2(-split), sr),
            spatial::pole(cut * pow2(split), sr),
        ];
        let [memory_l, memory_r] = &mut self.memory;
        for i in 0..l.len() {
            let x = l[i];
            let y = r[i];
            self.del += self.smoothing * (self.target - self.del);
            let wet = [memory_l.read(self.del), memory_r.read(self.del)];
            for ch in 0..2 {
                self.low[ch] += a[ch] * (wet[ch] as f64 - self.low[ch]);
            }
            memory_l.write(soft(
                (x as f64 + feedback * ((1.0 - cross) * self.low[0] + cross * self.low[1])) as f32,
            ));
            memory_r.write(soft(
                (y as f64 + feedback * ((1.0 - cross) * self.low[1] + cross * self.low[0])) as f32,
            ));
            let mid = (wet[0] + wet[1]) * 0.5f32;
            let side = ((wet[0] - wet[1]) as f64 * 0.5 * width) as f32;
            l[i] = clean((x as f64 * (1.0 - mix) + (mid + side) as f64 * mix) as f32);
            r[i] = clean((y as f64 * (1.0 - mix) + (mid - side) as f64 * mix) as f32);
        }
    }
}

/// One playing grain of the granular processors.
#[derive(Clone, Copy)]
struct Grain {
    position: f64,
    step: f64,
    age: f64,
    length: f64,
    pan: f64,
    active: bool,
}

impl Default for Grain {
    fn default() -> Self {
        Grain {
            position: 0.0,
            step: 1.0,
            age: 0.0,
            length: 1.0,
            pan: 0.0,
            active: false,
        }
    }
}

const GRAIN_SEED: u32 = 0x91e10da5;

pub struct GrainDelay {
    core: Core,
    memory: [Line; 2],
    grains: [Grain; 32],
    random: u32,
    clock: f64,
    feedback: [f32; 2],
}

/// xorshift32 step mapped to [-1, 1).
#[inline(always)]
fn noise(random: &mut u32) -> f64 {
    *random ^= *random << 13;
    *random ^= *random >> 17;
    *random ^= *random << 5;
    *random as f64 / 2147483648.0 - 1.0
}

impl GrainDelay {
    pub fn new(sr: f64) -> Self {
        GrainDelay {
            core: Core::new(sr),
            memory: std::array::from_fn(|_| Line::with_capacity((sr * 6.0) as usize + 8)),
            grains: [Grain::default(); 32],
            random: GRAIN_SEED,
            clock: 0.0,
            feedback: [0.0; 2],
        }
    }

    /// `GrainDelay::tailForParameters`.
    pub fn tail_estimate(values: &[f64; MAX_PARAMETERS]) -> f64 {
        let size = values[1] * 0.001 * (1.0 + values[2] * 0.005);
        let space = values[4] * 0.001;
        let pitch = rate(values[0]) * pow2(values[6] * 0.0025);
        let history = min(
            6.0,
            space + size * max(1.0, pitch) + min(space * 0.4, 0.1) * values[2] * 0.01,
        );
        let duration = history + size;
        let feedback = bounded(values[5] * 0.0085, 0.0, 0.85);
        duration
            * (if feedback > 0.0 {
                1.0 + (1.0e-5f64.ln() / feedback.ln()).ceil()
            } else {
                1.0
            })
    }
}

impl Processor for GrainDelay {
    core_accessors!(core);

    fn reset(&mut self) {
        for d in self.memory.iter_mut() {
            d.reset();
        }
        for g in self.grains.iter_mut() {
            g.active = false;
        }
        self.clock = 0.0;
        self.feedback = [0.0; 2];
        self.random = GRAIN_SEED;
    }

    fn tail(&self) -> f64 {
        500.0
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], _tempo: f64) -> f64 {
        Self::tail_estimate(values)
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let pitch = rate(p[0]);
        let size = max(16.0, p[1] * 0.001 * sr);
        let jitter = p[2] * 0.01;
        let space = p[4] * 0.001 * sr;
        let density = p[5] * 0.01;
        let modulation = p[6] * 0.01;
        let mix = bounded(p[7] * 0.01, 0.0, 1.0);
        let stereo = p[8] * 0.01;
        let reverse = p[3] >= 0.5;
        let interval = max(16.0, size / (2.0 + density * 10.0) + space * 0.12);
        let [memory_l, memory_r] = &mut self.memory;
        for i in 0..l.len() {
            let x = l[i];
            let y = r[i];
            memory_l.write(soft(x + self.feedback[0] * (density * 0.85) as f32));
            memory_r.write(soft(y + self.feedback[1] * (density * 0.85) as f32));
            if self.clock <= 0.0 {
                self.clock += interval * (1.0 + noise(&mut self.random) * jitter * 0.6);
                for g in self.grains.iter_mut() {
                    if !g.active {
                        g.active = true;
                        g.age = 0.0;
                        g.length = max(16.0, size * (1.0 + noise(&mut self.random) * jitter * 0.5));
                        g.step = pitch
                            * (if reverse { -1.0 } else { 1.0 })
                            * pow2(noise(&mut self.random) * modulation * 0.25);
                        // Forward grains have enough history to finish without crossing the write head.
                        let history = max(1.0, space)
                            + (if !reverse {
                                g.length * max(1.0, g.step)
                            } else {
                                0.0
                            })
                            + noise(&mut self.random) * jitter * min(space * 0.4, sr * 0.1);
                        g.position =
                            wrap_position(memory_l.pos as f64 - history, memory_l.data.len());
                        g.pan = noise(&mut self.random) * stereo;
                        break;
                    }
                }
            }
            self.clock -= 1.0;
            let mut sum = [0.0f64; 2];
            let mut weights = 0.0f64;
            for g in self.grains.iter_mut() {
                if g.active {
                    let phase = g.age / g.length;
                    let weight = 4.0 * phase * (1.0 - phase);
                    let a = at(memory_l, g.position, memory_l.data.len());
                    let b = at(memory_r, g.position, memory_r.data.len());
                    let mid = (a + b) as f64 * 0.5;
                    let side = (a - b) as f64 * 0.5 * stereo;
                    sum[0] += (mid + side) * weight * (1.0 - max(0.0, g.pan));
                    sum[1] += (mid - side) * weight * (1.0 + min(0.0, g.pan));
                    weights += weight;
                    g.position = wrap_position(g.position + g.step, memory_l.data.len());
                    g.age += 1.0;
                    if g.age >= g.length {
                        g.active = false;
                    }
                }
            }
            let norm = 1.0 / max(1.0, weights);
            self.feedback[0] = clean((sum[0] * norm) as f32);
            self.feedback[1] = clean((sum[1] * norm) as f32);
            l[i] = clean((x as f64 * (1.0 - mix) + self.feedback[0] as f64 * mix) as f32);
            r[i] = clean((y as f64 * (1.0 - mix) + self.feedback[1] as f64 * mix) as f32);
        }
    }
}

pub struct GrainStretch {
    core: Core,
    capture: [Line; 2],
    grains: [Grain; 8],
    captured: usize,
    capture_length: usize,
    cursor: f64,
    clock: f64,
    was_on: bool,
}

impl GrainStretch {
    pub fn new(sr: f64) -> Self {
        GrainStretch {
            core: Core::new(sr),
            capture: std::array::from_fn(|_| Line::with_capacity((sr * 24.0) as usize + 8)),
            grains: [Grain::default(); 8],
            captured: 0,
            capture_length: 0,
            cursor: 0.0,
            clock: 0.0,
            was_on: false,
        }
    }
}

impl Processor for GrainStretch {
    core_accessors!(core);

    fn reset(&mut self) {
        for d in self.capture.iter_mut() {
            d.reset();
        }
        for g in self.grains.iter_mut() {
            g.active = false;
        }
        self.captured = 0;
        self.capture_length = 0;
        self.cursor = 0.0;
        self.clock = 0.0;
        self.was_on = false;
    }

    fn tail(&self) -> f64 {
        120.0
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let tempo = self.core.tempo;
        let on = p[0] >= 0.5;
        if !on {
            self.was_on = false;
            return;
        }
        if !self.was_on {
            self.captured = 0;
            self.capture_length = self.capture[0]
                .data
                .len()
                .min((sr * 60.0 / bpm(tempo) * 8.0).ceil() as usize);
            self.cursor = 0.0;
            self.clock = 0.0;
            for g in self.grains.iter_mut() {
                g.active = false;
            }
            self.was_on = true;
        }
        let pitch = rate(p[3]);
        let speed = p[1] * 0.01;
        let mix = bounded(p[6] * 0.01, 0.0, 1.0);
        let mut size = max(
            16.0,
            p[5] * 0.001 * sr / (if p[4] >= 0.5 { pitch } else { 1.0 }),
        );
        size = min(size, sr * 0.8);
        let wanted_loop = ((sr * 60.0 / bpm(tempo) * 0.25 * p[2]) as usize).max(1);
        let [capture_l, capture_r] = &mut self.capture;
        for i in 0..l.len() {
            let x = l[i];
            let y = r[i];
            if self.captured < self.capture_length {
                // capture_length never exceeds either buffer (both have the same size).
                if let (Some(slot_l), Some(slot_r)) = (
                    capture_l.data.get_mut(self.captured),
                    capture_r.data.get_mut(self.captured),
                ) {
                    *slot_l = x;
                    *slot_r = y;
                }
                self.captured += 1;
            }
            let loop_length = self.captured.min(wanted_loop).max(1);
            // During the first grain there is no complete source yet: monitor the input.
            if (self.captured as f64) < size {
                continue;
            }
            if self.clock <= 0.0 {
                self.clock += size * 0.25;
                for g in self.grains.iter_mut() {
                    if !g.active {
                        g.active = true;
                        g.age = 0.0;
                        g.length = size;
                        g.step = pitch;
                        g.position = wrap_position(self.cursor, loop_length);
                        break;
                    }
                }
            }
            self.clock -= 1.0;
            let mut sum = [0.0f64; 2];
            let mut weights = 0.0f64;
            for g in self.grains.iter_mut() {
                if g.active {
                    let phase = g.age / g.length;
                    let weight = 4.0 * phase * (1.0 - phase);
                    sum[0] += at(capture_l, g.position, loop_length) as f64 * weight;
                    sum[1] += at(capture_r, g.position, loop_length) as f64 * weight;
                    weights += weight;
                    g.position = wrap_position(g.position + g.step, loop_length);
                    g.age += 1.0;
                    if g.age >= g.length {
                        g.active = false;
                    }
                }
            }
            self.cursor = wrap_position(self.cursor + speed, loop_length);
            let norm = 1.0 / max(1.0, weights);
            l[i] = clean((x as f64 * (1.0 - mix) + sum[0] * norm * mix) as f32);
            r[i] = clean((y as f64 * (1.0 - mix) + sum[1] * norm * mix) as f32);
        }
    }
}

pub struct Resochord {
    core: Core,
    comb: [Line; 6],
    lengths: [f64; 6],
    filtered: [f64; 6],
    gain: [f64; 6],
    smooth: f64,
    initialized: bool,
    zero: ZeroMix,
}

impl Resochord {
    pub fn new(sr: f64) -> Self {
        Resochord {
            core: Core::new(sr),
            comb: std::array::from_fn(|_| Line::with_capacity((sr * 0.15) as usize + 8)),
            lengths: [0.0; 6],
            filtered: [0.0; 6],
            gain: [0.0; 6],
            smooth: 1.0 - (-1.0 / (0.01 * sr)).exp(),
            initialized: false,
            zero: ZeroMix::default(),
        }
    }

    fn reset_wet(&mut self) {
        for d in self.comb.iter_mut() {
            d.reset();
        }
        self.filtered = [0.0; 6];
        self.initialized = false;
    }
}

impl Processor for Resochord {
    core_accessors!(core);

    fn reset(&mut self) {
        self.reset_wet();
        self.zero.reset();
    }

    fn tail(&self) -> f64 {
        40.0
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        const CHORDS: [[i32; 6]; 8] = [
            [0, 4, 7, 12, 16, 19],
            [0, 3, 7, 12, 15, 19],
            [0, 4, 7, 11, 14, 19],
            [0, 3, 7, 10, 14, 19],
            [0, 4, 7, 10, 14, 19],
            [0, 5, 7, 12, 17, 19],
            [0, 3, 6, 9, 12, 15],
            [0, 4, 8, 12, 16, 20],
        ];
        let string = p[0] >= 0.5;
        let style = (bounded(p[1], 0.0, 7.0) as usize).min(7);
        let root = bounded(p[2], 0.0, 11.0) as i32;
        let tune = p[3];
        let bright = bounded(p[4] * 0.01, 0.0, 1.0);
        let feedback = bounded(p[5] * 0.01, 0.0, 0.99);
        let decay = bounded(p[6], 0.03, 20.0);
        let mix = bounded(p[7] * 0.01, 0.0, 1.0);
        let spread = p[8] * 0.01;
        let tail = self.tail();
        let (skip, restart) = self.zero.skip(mix == 0.0, || tail, sr, l.len());
        if skip {
            for i in 0..l.len() {
                l[i] = clean((l[i] as f64 * (1.0 - mix)) as f32);
                r[i] = clean((r[i] as f64 * (1.0 - mix)) as f32);
            }
            return;
        }
        if restart {
            self.reset_wet();
        }
        let mut target = [0.0f64; 6];
        let lowpass = spatial::pole(1200.0 + bright * bright * 18500.0, sr);
        for k in 0..6 {
            let semitones = root as f64
                + tune
                + (if string {
                    (k as f64 - 2.5) * spread * 0.6
                } else {
                    CHORDS[style][k] as f64
                });
            let hz = 130.81278265 * pow2(semitones / 12.0);
            target[k] = sr / bounded(hz, 12.0, sr * 0.4);
            self.gain[k] = min(
                0.9998,
                0.001f64.powf(target[k] / (sr * decay)) * (0.65 + feedback * 0.35),
            );
        }
        if !self.initialized {
            self.lengths = target;
            self.initialized = true;
        }
        for i in 0..l.len() {
            let x = l[i];
            let y = r[i];
            let mut wet = [0.0f64; 2];
            for k in 0..6 {
                self.lengths[k] += self.smooth * (target[k] - self.lengths[k]);
                let v = self.comb[k].read(self.lengths[k]);
                self.filtered[k] += lowpass * (v as f64 - self.filtered[k]);
                self.comb[k].write(soft(
                    ((x + y) as f64 * 0.15 + self.filtered[k] * self.gain[k]) as f32,
                ));
                wet[k & 1] += v as f64 * 0.28;
            }
            l[i] = clean((x as f64 * (1.0 - mix) + wet[0] * mix) as f32);
            r[i] = clean((y as f64 * (1.0 - mix) + wet[1] * mix) as f32);
        }
    }
}

const TIMES: &[Label] = &[
    Label::new(c"0.5"),
    Label::new(c"1"),
    Label::new(c"1.5"),
    Label::new(c"2"),
    Label::new(c"2.5"),
    Label::new(c"3"),
    Label::new(c"3.5"),
    Label::new(c"4"),
    Label::new(c"4.5"),
    Label::new(c"5"),
    Label::new(c"5.5"),
    Label::new(c"6"),
    Label::new(c"6.5"),
    Label::new(c"7"),
    Label::new(c"7.5"),
    Label::new(c"8"),
    Label::new(c"8.5"),
    Label::new(c"9"),
    Label::new(c"9.5"),
    Label::new(c"10"),
    Label::new(c"10.5"),
    Label::new(c"11"),
    Label::new(c"11.5"),
    Label::new(c"12"),
    Label::new(c"12.5"),
    Label::new(c"13"),
    Label::new(c"13.5"),
    Label::new(c"14"),
    Label::new(c"14.5"),
    Label::new(c"15"),
    Label::new(c"15.5"),
    Label::new(c"16"),
];
const UNIT_LABELS: &[Label] = &[
    Label::new(c"1/64"),
    Label::new(c"1/32"),
    Label::new(c"1/16"),
    Label::new(c"1/8"),
    Label::new(c"1/4"),
    Label::new(c"1/16T"),
    Label::new(c"1/8T"),
    Label::new(c"1/4T"),
];
const RESO_MODES: &[Label] = &[Label::new(c"Chord"), Label::new(c"String")];
const STYLES: &[Label] = &[
    Label::new(c"Major"),
    Label::new(c"Minor"),
    Label::new(c"Major 7"),
    Label::new(c"Minor 7"),
    Label::new(c"Dominant 7"),
    Label::new(c"Suspended"),
    Label::new(c"Diminished"),
    Label::new(c"Augmented"),
];
const ROOTS: &[Label] = &[
    Label::new(c"C"),
    Label::new(c"C#"),
    Label::new(c"D"),
    Label::new(c"D#"),
    Label::new(c"E"),
    Label::new(c"F"),
    Label::new(c"F#"),
    Label::new(c"G"),
    Label::new(c"G#"),
    Label::new(c"A"),
    Label::new(c"A#"),
    Label::new(c"B"),
];
static BEAT_PARAMS: [P; 9] = [
    P::choice(c"time", c"Time", c"units", 0.0, 31.0, 7.0, INTEGER, TIMES),
    P::new(c"offset", c"Offset", c"% unit", -49.0, 100.0, 0.0, 0),
    P::new(c"feedback", c"Feedback", c"%", 0.0, 97.0, 35.0, 0),
    P::new(c"crossover", c"Crossover", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"color", c"Color", c"%", 0.0, 100.0, 75.0, 0),
    P::new(c"split", c"Split", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"stereo", c"Stereo", c"%", -100.0, 100.0, 100.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 30.0, 0),
    P::choice(c"unit", c"Unit", c"", 0.0, 7.0, 2.0, INTEGER, UNIT_LABELS),
];
static GRAIN_PARAMS: [P; 9] = [
    P::new(c"pitch", c"Pitch", c"st", -24.0, 24.0, 0.0, 0),
    P::new(c"size", c"Size", c"ms", 5.0, 500.0, 80.0, LOG),
    P::new(c"jitter", c"Jitter", c"%", 0.0, 100.0, 10.0, 0),
    P::new(c"reverse", c"Reverse", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"space", c"Space", c"ms", 0.0, 2000.0, 180.0, 0),
    P::new(c"density", c"Density", c"%", 0.0, 100.0, 45.0, 0),
    P::new(c"mod", c"Mod", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 35.0, 0),
    P::new(c"stereo", c"Stereo", c"%", 0.0, 100.0, 80.0, 0),
];
static STRETCH_PARAMS: [P; 7] = [
    P::new(c"on", c"On / Capture", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"stretch", c"Stretch", c"%", 0.0, 200.0, 50.0, 0),
    P::new(c"loop", c"Loop", c"1/16 steps", 1.0, 32.0, 16.0, INTEGER),
    P::new(c"pitch", c"Pitch", c"st", -24.0, 24.0, 0.0, 0),
    P::new(c"link", c"Link", c"", 0.0, 1.0, 1.0, TOGGLE),
    P::new(c"size", c"Size", c"ms", 5.0, 300.0, 80.0, LOG),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
];
static RESO_PARAMS: [P; 9] = [
    P::choice(c"mode", c"Mode", c"", 0.0, 1.0, 0.0, INTEGER, RESO_MODES),
    P::choice(c"style", c"Style", c"", 0.0, 7.0, 0.0, INTEGER, STYLES),
    P::choice(c"chord", c"Chord Root", c"", 0.0, 11.0, 0.0, INTEGER, ROOTS),
    P::new(c"tune", c"Tune", c"st", -24.0, 24.0, 0.0, 0),
    P::new(c"brightness", c"Brightness", c"%", 0.0, 100.0, 70.0, 0),
    P::new(c"feedback", c"Feedback", c"%", 0.0, 99.0, 90.0, 0),
    P::new(c"decay", c"Decay", c"s", 0.03, 20.0, 2.0, LOG),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 35.0, 0),
    P::new(c"spread", c"Spread", c"%", 0.0, 100.0, 25.0, 0),
];

static DESCRIPTORS: [Descriptor; 4] = [
    Descriptor::new(
        c"beatdelay",
        c"Beat Delay",
        c"Delay",
        0,
        &BEAT_PARAMS,
        |sr, _| Box::new(BeatDelay::new(sr)),
        BeatDelay::tail_estimate,
    ),
    Descriptor::new(
        c"graindelay",
        c"Grain Delay",
        c"Delay",
        0,
        &GRAIN_PARAMS,
        |sr, _| Box::new(GrainDelay::new(sr)),
        |_, v, _| GrainDelay::tail_estimate(v),
    ),
    Descriptor::new(
        c"grainstretch",
        c"Grain Stretch",
        c"Delay",
        0,
        &STRETCH_PARAMS,
        |sr, _| Box::new(GrainStretch::new(sr)),
        |_, _, _| 120.0,
    ),
    Descriptor::new(
        c"resochord",
        c"Resochord",
        c"Delay",
        0,
        &RESO_PARAMS,
        |sr, _| Box::new(Resochord::new(sr)),
        |_, _, _| 40.0,
    ),
];

/// `stfx::delayDescriptors`.
pub fn descriptors() -> &'static [Descriptor] {
    &DESCRIPTORS
}

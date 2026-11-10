//! Performance effects: Perform Filter, Perform Flanger, Burst Echo, Reso Echo, Ring, Stutter,
//! Tremolo and Scratcher.
//!
//! Replaces `Sources/TrackerEngine/EffectsPerform.hpp` (namespace `stfx::perform_detail` and
//! the `Perform*` classes).
//!
//! Original performance DSP, informed by the public NI effect-reference feature
//! descriptions, not by NI code or presets. Parameter order is persistent and
//! grouped into successive banks of eight controls for keyboard/hardware pages.

use super::common::{
    clamp, clean, core_accessors, db_gain, fast_cos_2pi, fast_sin_2pi, fast_tanh, frame_buffers, max, min, mix, pole, pow2, wrap, Core,
    DelayLine, Descriptor, Label, Memo, ParameterInfo as P, Processor, Smoother, MAX_PARAMETERS, PARAMETER_INTEGER as INTEGER,
    PARAMETER_LOG as LOG, PARAMETER_TOGGLE as TOGGLE, PI,
};

/// Shared state of the Perform processors (`perform_detail::Base`).
///
/// Per-sample control derivations (dB gains, poles, pow/tan of smoothed
/// values) are memoized: once smoothing has settled they cost a comparison.
pub struct Base {
    pub core: Core,
    smooth: [Smoother; 24],
    pole_memo: [Memo; 24],
    smoothing: f64,
}

impl Base {
    pub fn new(rate: f64) -> Self {
        Base { core: Core::new(rate), smooth: [Smoother::default(); 24], pole_memo: [Memo::default(); 24], smoothing: pole(12.0, rate) }
    }

    /// Parameter `index` smoothed with the default 12 ms time constant.
    #[inline(always)]
    fn value(&mut self, index: usize) -> f64 {
        let index = index % 24;
        self.smooth[index].tick(self.core.p[index], self.smoothing)
    }

    /// Parameter `index` smoothed with a time constant of `ms` milliseconds.
    #[inline(always)]
    fn value_ms(&mut self, index: usize, ms: f64) -> f64 {
        let index = index % 24;
        let sr = self.core.sr;
        let coefficient = self.pole_memo[index].get([ms], || pole(ms, sr));
        self.smooth[index].tick(self.core.p[index], coefficient)
    }

    fn reset_parameters(&mut self) {
        for v in self.smooth.iter_mut() {
            v.reset();
        }
    }

    fn beat_seconds(&self, beats: f64) -> f64 {
        beats * 60.0 / clamp(self.core.tempo, 20.0, 999.0)
    }
}

#[derive(Clone, Copy, Default)]
struct OnePole {
    z: f64,
}

impl OnePole {
    #[inline(always)]
    fn tick(&mut self, x: f64, coefficient: f64) -> f64 {
        self.z = clean(x + coefficient * (self.z - x));
        self.z
    }

    fn reset(&mut self) {
        self.z = 0.0;
    }
}

#[inline]
fn low_pole(frequency: f64, rate: f64) -> f64 {
    (-2.0 * PI * clamp(frequency, 5.0, rate * 0.45) / rate).exp()
}

/// `perform_detail::bounded`: clamp to +-16; nonfinite values become 0.
#[inline(always)]
fn bounded(x: f64) -> f64 {
    if x.is_finite() {
        clamp(x, -16.0, 16.0)
    } else {
        0.0
    }
}

#[inline(always)]
fn soft(x: f64) -> f64 {
    fast_tanh(clamp(x, -40.0, 40.0))
}

// T120 bound for a contractive feedback path; noncontractive
// nonlinear loops instead use the explicit host export cap. These helpers read
// only immutable rate and the host's supplied physical parameter snapshot.
fn feedback_tail(seconds: f64, gain: f64) -> f64 {
    if gain >= 1.0 {
        return 120.0;
    }
    let repeats = if gain > 1e-12 { (1e-6f64.ln() / gain.ln()).ceil() } else { 0.0 };
    seconds * (1.0 + max(0.0, repeats)) + 0.25
}

/// The musical note division (in beats) nearest to `beats` on a log scale.
pub fn division(beats: f64) -> f64 {
    const VALUES: [f64; 15] =
        [1.0 / 32.0, 1.0 / 24.0, 1.0 / 16.0, 1.0 / 12.0, 1.0 / 8.0, 1.0 / 6.0, 1.0 / 4.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0, 1.5, 2.0, 3.0, 4.0];
    let mut best = VALUES[0];
    let mut distance = 1e9;
    for v in VALUES {
        let d = (max(beats, 1e-9) / v).log2().abs();
        if d < distance {
            distance = d;
            best = v;
        }
    }
    best
}

/// Low, band and high outputs of one `Svf` step.
#[derive(Clone, Copy)]
struct SvfOut {
    low: f64,
    band: f64,
    high: f64,
}

/// Stable topology-preserving state-variable filter. Softly bounded integrators
/// allow negative damping for controlled self oscillation, rather than a runaway.
#[derive(Clone, Copy, Default)]
struct Svf {
    s1: f64,
    s2: f64,
    coefficients: Memo,
    g: f64,
    a: f64,
}

impl Svf {
    #[inline(always)]
    fn tick(&mut self, input: f64, frequency: f64, damping: f64, rate: f64) -> SvfOut {
        let (g, a) = (&mut self.g, &mut self.a);
        self.coefficients.get([frequency, damping, rate], || {
            *g = (PI * clamp(frequency, 10.0, rate * 0.42) / rate).tan();
            *a = 1.0 / (1.0 + *g * (*g + damping));
            0.0
        });
        let band = self.a * (self.s1 + self.g * (input - self.s2));
        let low = self.s2 + self.g * band;
        self.s1 = clean(4.0 * soft((2.0 * band - self.s1) * 0.25));
        self.s2 = clean(4.0 * soft((2.0 * low - self.s2) * 0.25));
        SvfOut { low: bounded(low), band: bounded(band), high: bounded(input - damping * band - low) }
    }

    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// N independent SVF lanes stepped together (structure of arrays), so the
/// per-sample work vectorizes. Same arithmetic as `Svf`, lane by lane.
#[derive(Clone, Copy)]
struct SvfBank<const N: usize> {
    s1: [f64; N],
    s2: [f64; N],
    g: [f64; N],
    a: [f64; N],
}

impl<const N: usize> SvfBank<N> {
    fn new() -> Self {
        SvfBank { s1: [0.0; N], s2: [0.0; N], g: [0.0; N], a: [0.0; N] }
    }

    #[inline(always)]
    fn clean_lane(x: f64) -> f64 {
        let m = x.abs();
        if m > 1e-30 && m <= f64::MAX {
            1e6f64.min((-1e6f64).max(x))
        } else {
            0.0
        }
    }

    #[inline(always)]
    fn bound_lane(x: f64) -> f64 {
        if x.abs() <= f64::MAX {
            16.0f64.min((-16.0f64).max(x))
        } else {
            0.0
        }
    }

    fn set(&mut self, k: usize, frequency: f64, damping: f64, rate: f64) {
        let k = k % N;
        self.g[k] = (PI * clamp(frequency, 10.0, rate * 0.42) / rate).tan();
        self.a[k] = 1.0 / (1.0 + self.g[k] * (self.g[k] + damping));
    }

    #[inline(always)]
    fn band(&mut self, input: &[f64; N]) -> [f64; N] {
        let mut out = [0.0f64; N];
        for k in 0..N {
            let b = self.a[k] * (self.s1[k] + self.g[k] * (input[k] - self.s2[k]));
            let low = self.s2[k] + self.g[k] * b;
            self.s1[k] = Self::clean_lane(4.0 * fast_tanh((2.0 * b - self.s1[k]) * 0.25));
            self.s2[k] = Self::clean_lane(4.0 * fast_tanh((2.0 * low - self.s2[k]) * 0.25));
            out[k] = Self::bound_lane(b);
        }
        out
    }

    fn reset(&mut self) {
        self.s1 = [0.0; N];
        self.s2 = [0.0; N];
    }
}

/// An actual single-sideband shifter: windowed Hilbert FIR plus a matched-delay
/// real path. Fixed arrays, 31-sample internal group delay, no heap in processing.
/// History is mirrored (each sample stored twice) so the FIR reads one
/// contiguous window; only the 32 nonzero (even-index) taps are evaluated.
#[derive(Clone)]
pub struct FrequencyShifter {
    history: [f64; 126],
    taps: [f64; 32],
    write: usize,
}

impl Default for FrequencyShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl FrequencyShifter {
    pub fn new() -> Self {
        let mut taps = [0.0f64; 32];
        let mut i = 0;
        while i < 63 {
            let n = i as i32 - 31;
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / 62.0).cos() + 0.08 * (4.0 * PI * i as f64 / 62.0).cos();
            taps[i / 2] = 2.0 * w / (PI * n as f64);
            i += 2;
        }
        FrequencyShifter { history: [0.0; 126], taps, write: 0 }
    }

    /// Store one input sample; the shifted output can then be read with `output`.
    #[inline(always)]
    pub fn push(&mut self, input: f64) {
        // `write` only ever takes values 0..=62.
        let write = self.write % 63;
        self.history[write] = input;
        self.history[write + 63] = input;
        self.write = if write == 62 { 0 } else { write + 1 };
    }

    /// Output for the most recently pushed sample (write has advanced past it).
    #[inline(always)]
    pub fn output(&self, phase: f64) -> f64 {
        // newest sample at index write+62 (mirrored); tap i multiplies the sample i back.
        // window[j] is history[write + j], so "i samples back" is window[62 - i].
        let write = self.write % 63;
        let window: &[f64; 63] = self.history[write..write + 63].try_into().unwrap_or(&[0.0; 63]);
        // Four partial sums: independent chains instead of one serial chain.
        let (mut i0, mut i1, mut i2, mut i3) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut k = 0;
        while k < 32 {
            i0 += self.taps[k] * window[62 - 2 * k];
            i1 += self.taps[k + 1] * window[62 - (2 * k + 2)];
            i2 += self.taps[k + 2] * window[62 - (2 * k + 4)];
            i3 += self.taps[k + 3] * window[62 - (2 * k + 6)];
            k += 4;
        }
        let imaginary = (i0 + i1) + (i2 + i3);
        let real = window[62 - 31];
        real * fast_cos_2pi(phase) - imaginary * fast_sin_2pi(phase)
    }

    pub fn tick(&mut self, input: f64, phase: f64) -> f64 {
        self.push(input);
        self.output(phase)
    }

    pub fn reset(&mut self) {
        self.history = [0.0; 126];
        self.write = 0;
    }
}

/// Dense short diffusion + a four-line orthogonal feedback network. Fixed
/// incommensurate lengths avoid a single obvious pitched echo in Ring's tail.
struct Plate {
    lines: [DelayLine; 4],
    lengths: [f64; 4],
    damp: [OnePole; 4],
    ap_l: f64,
    ap_r: f64,
    damp_pole: Memo,
    gain_memo: Memo,
    gains: [f64; 4],
}

impl Plate {
    fn new(sr: f64) -> Self {
        Plate {
            lines: [
                DelayLine::new((sr * 0.0311) as usize + 4),
                DelayLine::new((sr * 0.0379) as usize + 4),
                DelayLine::new((sr * 0.0437) as usize + 4),
                DelayLine::new((sr * 0.0521) as usize + 4),
            ],
            lengths: [sr * 0.0311, sr * 0.0379, sr * 0.0437, sr * 0.0521],
            damp: [OnePole::default(); 4],
            ap_l: 0.0,
            ap_r: 0.0,
            damp_pole: Memo::default(),
            gain_memo: Memo::default(),
            gains: [0.0; 4],
        }
    }

    #[inline(always)]
    fn tick(&mut self, left: f64, right: f64, decay: f64, rate: f64) -> [f64; 2] {
        let lp = self.damp_pole.get([rate], || low_pole(7500.0, rate));
        let (gains, lengths) = (&mut self.gains, &self.lengths);
        self.gain_memo.get([decay, rate], || {
            for j in 0..4 {
                gains[j] = 0.001f64.powf(lengths[j] / rate / max(0.08, decay));
            }
            0.0
        });
        let mut v = [0.0f64; 4];
        for j in 0..4 {
            v[j] = self.damp[j].tick(self.lines[j].read(self.lengths[j]), lp);
        }
        let h = [
            (v[0] + v[1] + v[2] + v[3]) * 0.5,
            (v[0] - v[1] + v[2] - v[3]) * 0.5,
            (v[0] + v[1] - v[2] - v[3]) * 0.5,
            (v[0] - v[1] - v[2] + v[3]) * 0.5,
        ];
        let a = left * 0.7 + self.ap_l * 0.3;
        let b = right * 0.7 + self.ap_r * 0.3;
        self.ap_l = left - a * 0.3;
        self.ap_r = right - b * 0.3;
        let input = [a, b, (a + b) * 0.5, (a - b) * 0.5];
        for j in 0..4 {
            self.lines[j].push(bounded(input[j] * 0.35 + h[j] * self.gains[j]));
        }
        [(v[0] + v[2] + v[3]) * 0.55, (v[1] + v[2] - v[3]) * 0.55]
    }

    fn reset(&mut self) {
        for d in self.lines.iter_mut() {
            d.reset();
        }
        for f in self.damp.iter_mut() {
            f.reset();
        }
        self.ap_l = 0.0;
        self.ap_r = 0.0;
    }
}

/// Stereo capture storage uses floats to keep long, high-rate history practical.
/// Freeze is just retaining the write index: no copying or allocation on Engage.
struct Capture {
    data: Vec<[f32; 2]>,
    write: usize,
    valid: usize,
    end: usize,
    captured_valid: usize,
    frames: u64,
}

impl Capture {
    fn new(capacity: usize) -> Self {
        Capture { data: vec![[0.0; 2]; capacity.max(8)], write: 0, valid: 0, end: 0, captured_valid: 0, frames: 0 }
    }

    #[inline(always)]
    fn push(&mut self, l: f64, r: f64) {
        let size = self.data.len();
        if let Some(slot) = self.data.get_mut(self.write) {
            *slot = [bounded(l) as f32, bounded(r) as f32];
        }
        self.write += 1;
        if self.write == size {
            self.write = 0;
        }
        self.valid = (self.valid + 1).min(size - 2);
    }

    fn capture(&mut self, back: usize) {
        let size = self.data.len();
        let back = back.min(self.valid);
        self.end = (self.write + size - back) % size;
        self.captured_valid = self.valid - back;
    }

    #[inline(always)]
    fn read(&self, length: f64, position: f64) -> [f64; 2] {
        if self.captured_valid < 2 {
            return [0.0, 0.0];
        }
        let size = self.data.len();
        let length = clamp(length, 2.0, self.captured_valid.max(2) as f64);
        let position = wrap(position / length) * length;
        let mut absolute = self.end as f64 - length + position;
        while absolute < 0.0 {
            absolute += size as f64;
        }
        let a = absolute as usize % size;
        let b = (a + 1) % size;
        let f = absolute - absolute.floor();
        let (xa, xb) = (self.data[a], self.data[b]);
        [mix(xa[0] as f64, xb[0] as f64, f), mix(xa[1] as f64, xb[1] as f64, f)]
    }

    fn reset(&mut self) {
        self.data.fill([0.0; 2]);
        self.write = 0;
        self.valid = 0;
        self.end = 0;
        self.captured_valid = 0;
        self.frames = 0;
    }
}

/// Raised-cosine fade towards both ends of a loop of `length` samples.
#[inline(always)]
fn seam(phase: f64, length: f64, fade: f64) -> f64 {
    let edge = min(phase, length - phase);
    if edge < fade {
        0.5 - 0.5 * (PI * clamp(edge / max(1.0, fade), 0.0, 1.0)).cos()
    } else {
        1.0
    }
}

pub struct PerformFilter {
    base: Base,
    filter: [Svf; 2],
    drive_gain: Memo,
    out_gain: Memo,
    center: Memo,
    // While Engage x Mix is exactly zero the output is the dry input: the
    // filter is not run, and restarts from rest when it is audible again.
    live: bool,
}

impl PerformFilter {
    pub fn new(sr: f64) -> Self {
        PerformFilter {
            base: Base::new(sr),
            filter: [Svf::default(); 2],
            drive_gain: Memo::default(),
            out_gain: Memo::default(),
            center: Memo::default(),
            live: false,
        }
    }

    /// `PerformFilter::tailForParameters`.
    pub fn tail_estimate(sr: f64, v: &[f64; MAX_PARAMETERS], _tempo: f64) -> f64 {
        let release = 0.001 * clamp(v[7], 1.0, 100.0) * 16.0 + 0.1;
        if v[0] < 0.5 {
            return release;
        }
        let damping = 2.0 - 2.08 * clamp(v[3], 0.0, 1.05);
        // Zero/negative damping is an intentional oscillator. Small positive
        // damping still gets its finite decay estimate, however long it is.
        if damping <= 0.0 {
            return 120.0;
        }
        let lo = clamp(v[8], 20.0, sr * 0.42);
        let hi = max(lo, clamp(v[9], 20.0, sr * 0.42));
        let frequency = lo * (hi / lo).powf(clamp(v[1], 0.0, 1.0));
        1e6f64.ln() / (PI * frequency * damping) + release
    }
}

impl Processor for PerformFilter {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        for f in self.filter.iter_mut() {
            f.reset();
        }
        self.live = false;
    }

    fn tail(&self) -> f64 {
        3e18
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        Self::tail_estimate(self.base.core.sr, values, tempo)
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        for i in 0..l.len() {
            let ms = self.base.value(7);
            let engage = self.base.value_ms(0, ms);
            let position = self.base.value_ms(1, ms);
            let kind = self.base.value(2);
            let res = self.base.value(3);
            let v4 = self.base.value(4);
            let v5 = self.base.value(5);
            let amount = self.base.value(6);
            let drive = self.drive_gain.get([v4], || db_gain(v4));
            let out = self.out_gain.get([v5], || db_gain(v5));
            let lo = clamp(self.base.value(8), 20.0, sr * 0.42);
            let hi = clamp(self.base.value(9), 20.0, sr * 0.42);
            let hz = self.center.get([lo, hi, position], || lo * (max(hi, lo) / lo).powf(position));
            let damping = 2.0 - 2.08 * res;
            let dry = [l[i] as f64, r[i] as f64];
            let mut wet = [0.0f64; 2];
            let amount_wet = engage * amount;
            if amount_wet != 0.0 {
                if !self.live {
                    for f in self.filter.iter_mut() {
                        f.reset();
                    }
                    self.live = true;
                }
                for c in 0..2 {
                    let v = self.filter[c].tick(soft(dry[c] * drive), hz, damping, sr);
                    wet[c] = (if kind < 1.0 { mix(v.low, v.band, kind) } else { mix(v.band, v.high, kind - 1.0) }) * out;
                }
            } else {
                self.live = false;
            }
            l[i] = bounded(mix(dry[0], wet[0], amount_wet)) as f32;
            r[i] = bounded(mix(dry[1], wet[1], amount_wet)) as f32;
        }
    }
}

pub struct PerformFlanger {
    base: Base,
    left: DelayLine,
    right: DelayLine,
    phase: f64,
    ap: [f64; 2],
    delay: Smoother,
    drive_gain: Memo,
    inv_sqrt: Memo,
    feedback_gain: Memo,
    // Disengaged (Engage settled at 0) the output is exactly dry: the comb is
    // not run, and restarts from silence when engaged again.
    live: bool,
}

impl PerformFlanger {
    pub fn new(sr: f64) -> Self {
        PerformFlanger {
            base: Base::new(sr),
            left: DelayLine::new((sr * 0.2) as usize + 8),
            right: DelayLine::new((sr * 0.2) as usize + 8),
            phase: 0.0,
            ap: [0.0; 2],
            delay: Smoother::default(),
            drive_gain: Memo::default(),
            inv_sqrt: Memo::default(),
            feedback_gain: Memo::default(),
            live: false,
        }
    }

    /// `PerformFlanger::tailForParameters`.
    pub fn tail_estimate(_sr: f64, v: &[f64; MAX_PARAMETERS], _tempo: f64) -> f64 {
        if v[0] < 0.5 {
            return 0.25;
        }
        let base = 1.0 / clamp(v[1], 10.0, 4000.0);
        let gain = min(0.995, 0.001f64.powf(base / clamp(v[2], 0.02, 12.0))) * db_gain(clamp(v[3], 0.0, 24.0) * 0.5);
        feedback_tail(base * (1.0 + clamp(v[4], 0.0, 1.0) * 0.03 + clamp(v[9], 0.0, 1.0) * 0.45), gain)
    }
}

impl Processor for PerformFlanger {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        self.left.reset();
        self.right.reset();
        self.delay.reset();
        self.phase = 0.0;
        self.ap = [0.0; 2];
        self.live = true;
    }

    // Target-free bounds include representable feedback arbitrarily below one;
    // callers should use the useful, parameter-dependent estimate below.
    fn tail(&self) -> f64 {
        3e16
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        Self::tail_estimate(self.base.core.sr, values, tempo)
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        let engaged = self.base.core.p[0];
        let smoothing = self.base.smoothing;
        for i in 0..l.len() {
            let on = self.base.value(0);
            let hz = self.base.value(1);
            let decay = self.base.value(2);
            let v3 = self.base.value(3);
            let spread = self.base.value(4);
            let polarity = 1.0 - 2.0 * self.base.value(5);
            let tone = self.base.value(6);
            let amount = self.base.value(7);
            let rate = self.base.value(8);
            let depth = self.base.value(9);
            self.phase = wrap(self.phase + rate / sr);
            let base = self.delay.tick(sr / max(10.0, hz), smoothing);
            if on == 0.0 && engaged < 0.5 {
                if self.live {
                    self.left.reset();
                    self.right.reset();
                    self.ap = [0.0; 2];
                    self.live = false;
                }
                l[i] = bounded(l[i] as f64) as f32;
                r[i] = bounded(r[i] as f64) as f32;
                continue;
            }
            self.live = true;
            let drive = self.drive_gain.get([v3], || db_gain(v3));
            let scale = self.inv_sqrt.get([drive], || 1.0 / drive.sqrt());
            let dl = clamp(base * (1.0 + depth * 0.45 * fast_sin_2pi(self.phase)), 1.0, (self.left.len() - 2) as f64);
            let dr = clamp(
                base * (1.0 + spread * 0.03 + depth * 0.45 * fast_sin_2pi(self.phase + spread * 0.25)),
                1.0,
                (self.right.len() - 2) as f64,
            );
            let mut a = self.left.read(dl);
            let mut b = self.right.read(dr);
            let fb = self.feedback_gain.get([base, decay], || min(0.995, 0.001f64.powf(base / sr / max(0.02, decay)))) * polarity;
            let old_l = self.ap[0];
            let old_r = self.ap[1];
            self.ap[0] = bounded(a + 0.55 * old_l);
            self.ap[1] = bounded(b + 0.55 * old_r);
            a = mix(a, old_l - 0.55 * self.ap[0], tone);
            b = mix(b, old_r - 0.55 * self.ap[1], tone);
            let (xl, xr) = (l[i] as f64, r[i] as f64);
            self.left.push(xl * on + soft(mix(a, b, spread * 0.45) * fb * drive) * scale);
            self.right.push(xr * on + soft(mix(b, a, spread * 0.45) * fb * drive) * scale);
            l[i] = bounded(mix(xl, (xl + a) * 0.70710678118, on * amount)) as f32;
            r[i] = bounded(mix(xr, (xr + b) * 0.70710678118, on * amount)) as f32;
        }
    }
}

/// Echo time multiplier of a Burst Echo time mode: straight, dotted or triplet.
#[inline(always)]
fn time_mode_factor(mode: f64) -> f64 {
    if mode < 0.5 {
        1.0
    } else if mode < 1.5 {
        1.5
    } else {
        2.0 / 3.0
    }
}

/// `std::pow(128., position) / 32.`: clang lowers the power of the constant 128 to `exp2(7 x)`.
#[inline(always)]
fn position_beats(position: f64) -> f64 {
    pow2(position * 7.0) / 32.0
}

pub struct BurstEcho {
    base: Base,
    left: DelayLine,
    right: DelayLine,
    low: [OnePole; 2],
    high: [OnePole; 2],
    delay: Smoother,
    dry_gain: Smoother,
    feed_gain: Smoother,
    was_on: bool,
    elapsed: u64,
    drive_gain: Memo,
    out_gain: Memo,
    inv_sqrt: Memo,
    position_beats: Memo,
    divided: Memo,
    filter_poles: Memo,
    constants: Memo,
    lp: f64,
    hp: f64,
    feed_pole: f64,
    dry_pole: f64,
}

impl BurstEcho {
    pub fn new(sr: f64) -> Self {
        BurstEcho {
            base: Base::new(sr),
            left: DelayLine::new((sr * 20.0) as usize + 8),
            right: DelayLine::new((sr * 20.0) as usize + 8),
            low: [OnePole::default(); 2],
            high: [OnePole::default(); 2],
            delay: Smoother::default(),
            dry_gain: Smoother::default(),
            feed_gain: Smoother::default(),
            was_on: false,
            elapsed: 0,
            drive_gain: Memo::default(),
            out_gain: Memo::default(),
            inv_sqrt: Memo::default(),
            position_beats: Memo::default(),
            divided: Memo::default(),
            filter_poles: Memo::default(),
            constants: Memo::default(),
            lp: 0.0,
            hp: 0.0,
            feed_pole: 0.0,
            dry_pole: 0.0,
        }
    }

    /// `BurstEcho::tailForParameters`.
    pub fn tail_estimate(_sr: f64, v: &[f64; MAX_PARAMETERS], bpm: f64) -> f64 {
        let assignment = clamp(v[2], 0.0, 1.0);
        let position = clamp(v[1], 0.0, 1.0);
        let beats = mix(clamp(v[3], 1.0 / 32.0, 4.0), position_beats(position), assignment);
        let seconds = if v[4] >= 2.5 {
            clamp(v[10], 1.0, 2000.0) * 0.001
        } else {
            division(beats) * 60.0 / clamp(bpm, 20.0, 999.0) * time_mode_factor(v[4])
        };
        let gain = mix(clamp(v[5], 0.0, 0.98), position * 0.98, 1.0 - assignment)
            * (if v[0] >= 0.5 { 1.0 } else { 0.45 })
            * db_gain(clamp(v[11], 0.0, 18.0) * 0.5);
        feedback_tail(seconds, gain)
    }
}

impl Processor for BurstEcho {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        self.left.reset();
        self.right.reset();
        for f in self.low.iter_mut().chain(self.high.iter_mut()) {
            f.reset();
        }
        self.delay.reset();
        self.dry_gain.reset();
        self.feed_gain.reset();
        self.was_on = false;
        self.elapsed = 0;
    }

    fn tail(&self) -> f64 {
        3e18
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        Self::tail_estimate(self.base.core.sr, values, tempo)
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        let p = self.base.core.p;
        let smoothing = self.base.smoothing;
        let (feed_pole, dry_pole) = (&mut self.feed_pole, &mut self.dry_pole);
        self.constants.get([sr], || {
            *feed_pole = pole(2.0, sr);
            *dry_pole = pole(4.0, sr);
            0.0
        });
        for i in 0..l.len() {
            let on = p[0] >= 0.5;
            if on && !self.was_on {
                self.elapsed = 0;
            }
            self.was_on = on;
            let engage = self.base.value(0);
            let position = self.base.value(1);
            let assignment = self.base.value(2);
            let beats = self.base.value(3);
            let mode = p[4];
            let feedback = self.base.value(5);
            let filter = self.base.value(6);
            let wet = self.base.value(7);
            let feed = self.base.value(8);
            let mix_mode = p[9];
            let free_ms = self.base.value(10);
            let v11 = self.base.value(11);
            let stereo = self.base.value(12);
            let v13 = self.base.value(13);
            let drive = self.drive_gain.get([v11], || db_gain(v11));
            let out = self.out_gain.get([v13], || db_gain(v13));
            let scale = self.inv_sqrt.get([drive], || 1.0 / drive.sqrt());
            let selected_beats = mix(beats, self.position_beats.get([position], || position_beats(position)), assignment);
            let seconds = if mode >= 2.5 {
                free_ms * 0.001
            } else {
                self.base.beat_seconds(self.divided.get([selected_beats], || division(selected_beats))) * time_mode_factor(mode)
            };
            let frames = self.delay.tick(clamp(seconds * sr, 1.0, (self.left.len() - 2) as f64), smoothing);
            let feeding = on && (feed >= 0.999 || (self.elapsed as f64) < feed * sr);
            let input_gain = self.feed_gain.tick(if feeding { 1.0 } else { 0.0 }, self.feed_pole);
            let fb = mix(feedback, position * 0.98, 1.0 - assignment) * mix(0.45, 1.0, engage);
            let a = self.left.read(frames);
            let b = self.right.read(frames * (1.0 - stereo * 0.013));
            let (lp, hp) = (&mut self.lp, &mut self.hp);
            self.filter_poles.get([filter], || {
                *lp = low_pole(18000.0 * 0.055f64.powf(filter), sr);
                *hp = low_pole(15.0 * 100.0f64.powf(filter), sr);
                0.0
            });
            let values = [a, b];
            let mut echo = [0.0f64; 2];
            for c in 0..2 {
                let x = self.low[c].tick(values[c], self.lp);
                echo[c] = x - self.high[c].tick(x, self.hp);
            }
            let (xl, xr) = (l[i] as f64, r[i] as f64);
            self.left.push(xl * input_gain + soft(mix(echo[0], echo[1], stereo) * fb * drive) * scale);
            self.right.push(xr * input_gain + soft(mix(echo[1], echo[0], stereo) * fb * drive) * scale);
            let target_dry = if mix_mode >= 1.5 {
                0.0
            } else if mix_mode >= 0.5 && on && self.elapsed as f64 >= frames {
                0.0
            } else {
                1.0
            };
            let dry = self.dry_gain.tick(target_dry, self.dry_pole);
            l[i] = bounded((xl * dry + a * wet) * out) as f32;
            r[i] = bounded((xr * dry + b * wet) * out) as f32;
            if on {
                self.elapsed = self.elapsed.wrapping_add(1);
            }
        }
    }
}

pub struct ResoEcho {
    base: Base,
    left: DelayLine,
    right: DelayLine,
    bank: SvfBank<6>,
    envelope: [OnePole; 2],
    delay: Smoother,
    dry_gain: Smoother,
    drive_gain: Memo,
    inv_sqrt: Memo,
    bank_memo: Memo,
    constants: Memo,
    envelope_pole: f64,
}

impl ResoEcho {
    pub fn new(sr: f64) -> Self {
        ResoEcho {
            base: Base::new(sr),
            left: DelayLine::new((sr * 12.0) as usize + 8),
            right: DelayLine::new((sr * 12.0) as usize + 8),
            bank: SvfBank::new(),
            envelope: [OnePole::default(); 2],
            delay: Smoother::default(),
            dry_gain: Smoother::default(),
            drive_gain: Memo::default(),
            inv_sqrt: Memo::default(),
            bank_memo: Memo::default(),
            constants: Memo::default(),
            envelope_pole: 0.0,
        }
    }

    /// `ResoEcho::tailForParameters`.
    pub fn tail_estimate(_sr: f64, v: &[f64; MAX_PARAMETERS], bpm: f64) -> f64 {
        let damping = 0.12 + 1.6 * (1.0 - clamp(v[7], 0.0, 1.0));
        let gain = clamp(v[3], 0.0, 0.98) * db_gain(clamp(v[8], 0.0, 24.0) * 0.5) / damping;
        if gain >= 1.0 {
            return 120.0;
        }
        let seconds = mix(
            clamp(v[9], 1.0, 2000.0) * 0.001,
            clamp(v[2], 1.0 / 32.0, 4.0) * 60.0 / clamp(bpm, 20.0, 999.0),
            clamp(v[10], 0.0, 1.0),
        );
        let filter_decay = 1e6f64.ln() / (PI * clamp(v[1], 20.0, 4000.0) * 0.98 * damping);
        feedback_tail(seconds + filter_decay, gain)
    }
}

impl Processor for ResoEcho {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        self.left.reset();
        self.right.reset();
        self.bank.reset();
        for f in self.envelope.iter_mut() {
            f.reset();
        }
        self.delay.reset();
        self.dry_gain.reset();
    }

    fn tail(&self) -> f64 {
        3e18
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        Self::tail_estimate(self.base.core.sr, values, tempo)
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        let smoothing = self.base.smoothing;
        let envelope_pole = &mut self.envelope_pole;
        self.constants.get([sr], || {
            *envelope_pole = pole(30.0, sr);
            0.0
        });
        let envelope_pole = self.envelope_pole;
        for i in 0..l.len() {
            let on = self.base.value(0);
            let frequency = self.base.value(1);
            let beats = self.base.value(2);
            let feedback = self.base.value(3);
            let pump = self.base.value(4);
            let wet = self.base.value(5);
            let mode = self.base.value(6);
            let res = self.base.value(7);
            let v8 = self.base.value(8);
            let free_ms = self.base.value(9);
            let sync = self.base.value(10);
            let spread = self.base.value(11);
            let drive = self.drive_gain.get([v8], || db_gain(v8));
            let scale = self.inv_sqrt.get([drive], || 1.0 / drive.sqrt());
            let frames = self.delay.tick(
                clamp(mix(free_ms * 0.001, self.base.beat_seconds(beats), sync) * sr, 1.0, (self.left.len() - 2) as f64),
                smoothing,
            );
            let echoes = [self.left.read(frames), self.right.read(frames * (1.0 - spread * 0.02))];
            let input = [l[i] as f64, r[i] as f64];
            let mut filtered = [0.0f64; 2];
            let mut feed = [0.0f64; 2];
            let damping = 0.12 + 1.6 * (1.0 - res);
            let bank = &mut self.bank;
            self.bank_memo.get([frequency, spread, damping], || {
                for c in 0..2 {
                    for j in 0..3 {
                        let side = if c != 0 { 1.0 } else { -1.0 };
                        bank.set(c * 3 + j, frequency * pow2(j as f64 * 0.5 + spread * side * 0.02), damping, sr);
                    }
                }
                0.0
            });
            let mut excite = [0.0f64; 6];
            for k in 0..6 {
                excite[k] = input[k / 3] * on + echoes[k / 3] * feedback;
            }
            let bands = self.bank.band(&excite);
            for c in 0..2 {
                let mut sum = 0.0f64;
                for j in 0..3 {
                    sum += bands[c * 3 + j];
                }
                filtered[c] = sum / 3.0;
                let env = self.envelope[c].tick(filtered[c].abs(), envelope_pole);
                feed[c] = soft(filtered[c] * drive / (1.0 + pump * env * 8.0)) * scale;
            }
            self.left.push(feed[0]);
            self.right.push(feed[1]);
            let dry = self.dry_gain.tick(1.0 - clamp(mode - 1.0, 0.0, 1.0), smoothing);
            let direct_l = mix(input[0], filtered[0] * wet, on * min(1.0, mode)) * dry;
            let direct_r = mix(input[1], filtered[1] * wet, on * min(1.0, mode)) * dry;
            l[i] = bounded(direct_l + echoes[0] * wet) as f32;
            r[i] = bounded(direct_r + echoes[1] * wet) as f32;
        }
    }
}

pub struct PerformRing {
    base: Base,
    plate: Plate,
    phases: [f64; 5],
    frequency: Smoother,
    glide_pole: Memo,
    out_gain: Memo,
    increments: Memo,
    step: [f64; 5],
}

impl PerformRing {
    pub fn new(sr: f64) -> Self {
        PerformRing {
            base: Base::new(sr),
            plate: Plate::new(sr),
            phases: [0.0; 5],
            frequency: Smoother::default(),
            glide_pole: Memo::default(),
            out_gain: Memo::default(),
            increments: Memo::default(),
            step: [0.0; 5],
        }
    }

    /// `PerformRing::tailForParameters`.
    pub fn tail_estimate(_sr: f64, v: &[f64; MAX_PARAMETERS], _tempo: f64) -> f64 {
        // Plate decay is T60; twice that covers T120 plus the longest line.
        clamp(v[4], 0.1, 15.0) * 2.0 + 0.25
    }
}

impl Processor for PerformRing {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        self.plate.reset();
        self.phases = [0.0; 5];
        self.frequency.reset();
    }

    fn tail(&self) -> f64 {
        30.25
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        Self::tail_estimate(self.base.core.sr, values, tempo)
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        const INTERVALS: [f64; 5] = [-12.0, -7.0, 0.0, 7.0, 12.0];
        for i in 0..l.len() {
            let on = self.base.value(0);
            let target = self.base.core.p[1];
            let spread = self.base.value(2);
            let glide = self.base.value(3);
            let decay = self.base.value(4);
            let reverb = self.base.value(5);
            let amount = self.base.value(6);
            let wet_only = self.base.value(7);
            let v8 = self.base.value(8);
            let out = self.out_gain.get([v8], || db_gain(v8));
            let hz = self.frequency.tick(target, self.glide_pole.get([glide], || pole(max(1.0, glide), sr)));
            let step = &mut self.step;
            self.increments.get([hz, spread], || {
                for j in 0..5 {
                    step[j] = clamp(hz * pow2(INTERVALS[j] * spread / 12.0), 1.0, sr * 0.45) / sr;
                }
                0.0
            });
            let (mut carrier_l, mut carrier_r) = (0.0f64, 0.0f64);
            for j in 0..5 {
                self.phases[j] = wrap(self.phases[j] + self.step[j]);
            }
            // Disengaged, the carriers multiply by zero: skip the oscillators.
            if on != 0.0 {
                for j in 0..5 {
                    carrier_l += fast_sin_2pi(self.phases[j]);
                    carrier_r += fast_sin_2pi(self.phases[j] + spread * (j as f64 - 2.0) * 0.04);
                }
            }
            let (xl, xr) = (l[i] as f64, r[i] as f64);
            let a = xl * carrier_l * 0.2 * on;
            let b = xr * carrier_r * 0.2 * on;
            let tail = self.plate.tick(a, b, decay, sr);
            let dry = (1.0 - wet_only) * (1.0 - amount * on);
            l[i] = bounded((xl * dry + mix(a, tail[0], reverb) * amount) * out) as f32;
            r[i] = bounded((xr * dry + mix(b, tail[1], reverb) * amount) * out) as f32;
        }
    }
}

pub struct PerformStutter {
    base: Base,
    history: Capture,
    was_on: bool,
    phase: f64,
    last_position: f64,
    direction: f64,
    loop_length: Smoother,
    speed: Smoother,
    requested_memo: Memo,
    seconds_memo: Memo,
    pitch_memo: Memo,
}

impl PerformStutter {
    pub fn new(sr: f64) -> Self {
        PerformStutter {
            base: Base::new(sr),
            history: Capture::new((sr * 12.0) as usize + 8),
            was_on: false,
            phase: 0.0,
            last_position: 0.5,
            direction: 1.0,
            loop_length: Smoother::default(),
            speed: Smoother::default(),
            requested_memo: Memo::default(),
            seconds_memo: Memo::default(),
            pitch_memo: Memo::default(),
        }
    }
}

/// Tail estimate shared by the capture effects (Stutter, Scratcher): the 120 s export cap
/// while engaged, the control release otherwise.
fn capture_tail(_sr: f64, v: &[f64; MAX_PARAMETERS], _tempo: f64) -> f64 {
    if v[0] >= 0.5 {
        120.0
    } else {
        0.25
    }
}

impl Processor for PerformStutter {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        self.history.reset();
        self.was_on = false;
        self.phase = 0.0;
        self.last_position = 0.5;
        self.direction = 1.0;
        self.loop_length.reset();
        self.speed.reset();
    }

    fn tail(&self) -> f64 {
        120.0
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        capture_tail(self.base.core.sr, values, tempo)
    }

    // The capture history must keep recording silence.
    fn gateable(&self) -> bool {
        false
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        let p = self.base.core.p;
        let tempo = self.base.core.tempo;
        let smoothing = self.base.smoothing;
        for i in 0..l.len() {
            let input_l = l[i] as f64;
            let input_r = r[i] as f64;
            let on = p[0] >= 0.5;
            let engage = self.base.value(0);
            let position = self.base.value(1);
            let assign = p[2];
            let beats = self.base.value(3);
            let pitch = self.base.value(4);
            let gate = self.base.value(5);
            let dir = p[6];
            let quantize = p[7];
            let free_ms = self.base.value(8);
            let amount = self.base.value(9);
            let fade_ms = self.base.value(10);
            let scale = self.requested_memo.get([assign, position], || pow2(if assign < 0.5 { 4.0 * (position - 0.5) } else { 0.0 }));
            let requested = clamp(beats * scale, 1.0 / 32.0, 4.0);
            let base = &self.base;
            let seconds = self.seconds_memo.get([quantize, requested, free_ms * scale, tempo], || {
                if quantize >= 0.5 {
                    base.beat_seconds(division(requested))
                } else {
                    clamp(free_ms * scale, 5.0, 2000.0) * 0.001
                }
            });
            let target_length = seconds * sr;
            if on && !self.was_on {
                let back = if quantize >= 0.5 { (self.history.frames as f64 % max(2.0, target_length)) as usize } else { 0 };
                self.history.capture(back);
                self.phase = 0.0;
                self.direction = if dir >= 0.5 && dir < 1.5 { -1.0 } else { 1.0 };
            }
            let available = if on { self.history.captured_valid } else { self.history.valid };
            let limit = available.max(2) as f64;
            let length = clamp(self.loop_length.tick(clamp(target_length, 2.0, limit), smoothing), 2.0, limit);
            if dir < 0.5 {
                self.direction = 1.0;
            } else if dir < 1.5 {
                self.direction = -1.0;
            } else if (position - self.last_position).abs() > 1e-7 {
                self.direction = if position > self.last_position { 1.0 } else { -1.0 };
            }
            let semitones = clamp(pitch + (if assign >= 0.5 { (position - 0.5) * 48.0 } else { 0.0 }), -48.0, 48.0);
            self.last_position = position;
            let rate = self.speed.tick(self.direction * self.pitch_memo.get([semitones], || pow2(semitones / 12.0)), smoothing);
            self.phase = wrap(self.phase / length) * length;
            let wet = self.history.read(length, self.phase);
            let cycle = self.phase / length;
            let duty = 0.03 + 0.97 * gate;
            let mut shape = seam(self.phase, length, min(fade_ms * 0.001 * sr, length * 0.05));
            if gate < 0.999 {
                let fade = min(0.02, duty * 0.1);
                shape *= if cycle >= duty { 0.0 } else { clamp((duty - cycle) / fade, 0.0, 1.0) };
            }
            l[i] = bounded(mix(input_l, wet[0] * shape, engage * amount)) as f32;
            r[i] = bounded(mix(input_r, wet[1] * shape, engage * amount)) as f32;
            if !on {
                self.history.push(input_l, input_r);
            }
            self.phase += rate;
            self.history.frames = self.history.frames.wrapping_add(1);
            self.was_on = on;
        }
    }
}

pub struct PerformTremolo {
    base: Base,
    left: DelayLine,
    right: DelayLine,
    shifter: [FrequencyShifter; 2],
    phase: f64,
    shift_phase: [f64; 2],
    out_gain: Memo,
    rate_memo: Memo,
}

impl PerformTremolo {
    pub fn new(sr: f64) -> Self {
        PerformTremolo {
            base: Base::new(sr),
            left: DelayLine::new((sr * 0.08) as usize + 8),
            right: DelayLine::new((sr * 0.08) as usize + 8),
            shifter: [FrequencyShifter::new(), FrequencyShifter::new()],
            phase: 0.0,
            shift_phase: [0.0; 2],
            out_gain: Memo::default(),
            rate_memo: Memo::default(),
        }
    }
}

impl Processor for PerformTremolo {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        self.left.reset();
        self.right.reset();
        for f in self.shifter.iter_mut() {
            f.reset();
        }
        self.phase = 0.0;
        self.shift_phase = [0.0; 2];
    }

    fn tail(&self) -> f64 {
        0.1
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        let tempo = self.base.core.tempo;
        for i in 0..l.len() {
            let position = self.base.value(1);
            let blend = self.base.value(2);
            let mode = self.base.value(3);
            let attack = self.base.value(4);
            let on = self.base.value_ms(0, attack);
            let stereo = self.base.value(5);
            let depth_min = self.base.value(6);
            let depth_max = self.base.value(7);
            let depth = mix(depth_min, depth_max, position);
            let min_rate = self.base.value(8);
            let max_rate = self.base.value(9);
            let sync = self.base.value(10);
            let beats = self.base.value(11);
            let v12 = self.base.value(12);
            let out = self.out_gain.get([v12], || db_gain(v12));
            let base = &self.base;
            let rate = self.rate_memo.get([min_rate, max_rate, position, sync, beats, tempo], || {
                mix(min_rate * (max(0.01, max_rate) / max(0.01, min_rate)).powf(position), 1.0 / base.beat_seconds(beats), sync)
            });
            let input = [l[i] as f64, r[i] as f64];
            let osc = [fast_sin_2pi(self.phase), fast_sin_2pi(self.phase + stereo * 0.5)];
            self.phase = wrap(self.phase + rate / sr);
            self.left.push(input[0]);
            self.right.push(input[1]);
            let mut result = [0.0f64; 2];
            // Terms with an exactly zero weight are not evaluated (the FIR
            // history is still recorded), which leaves the result unchanged.
            let vibrato = on != 0.0 && blend != 0.0;
            let shifted = vibrato && mode != 0.0;
            for c in 0..2 {
                self.shift_phase[c] = wrap(self.shift_phase[c] + osc[c] * depth * 80.0 / sr);
                self.shifter[c].push(input[c]);
                if on == 0.0 {
                    result[c] = input[c] * out;
                    continue;
                }
                let trem = input[c] * (1.0 - depth * (0.5 + 0.5 * osc[c]));
                let mut moved = 0.0;
                if vibrato {
                    let line = if c != 0 { &self.right } else { &self.left };
                    let delayed = line.read(sr * (0.012 + 0.008 * depth * osc[c]));
                    moved = if shifted { mix(delayed, self.shifter[c].output(self.shift_phase[c]), mode) } else { delayed };
                }
                result[c] = mix(input[c], mix(trem, moved, blend), on) * out;
            }
            l[i] = bounded(result[0]) as f32;
            r[i] = bounded(result[1]) as f32;
        }
    }
}

pub struct PerformScratcher {
    base: Base,
    history: Capture,
    left: DelayLine,
    right: DelayLine,
    shifter: [FrequencyShifter; 2],
    delay: Smoother,
    was_on: bool,
    phase: f64,
    last_position: f64,
    brake: f64,
    shift_phase: f64,
    out_gain: Memo,
    brake_pole: Memo,
}

impl PerformScratcher {
    pub fn new(sr: f64) -> Self {
        PerformScratcher {
            base: Base::new(sr),
            history: Capture::new((sr * 4.0) as usize + 8),
            left: DelayLine::new((sr * 1.01) as usize + 8),
            right: DelayLine::new((sr * 1.01) as usize + 8),
            shifter: [FrequencyShifter::new(), FrequencyShifter::new()],
            delay: Smoother::default(),
            was_on: false,
            phase: 0.0,
            last_position: 0.5,
            brake: 1.0,
            shift_phase: 0.0,
            out_gain: Memo::default(),
            brake_pole: Memo::default(),
        }
    }
}

impl Processor for PerformScratcher {
    core_accessors!(base.core);

    fn reset(&mut self) {
        self.base.reset_parameters();
        self.history.reset();
        self.left.reset();
        self.right.reset();
        for f in self.shifter.iter_mut() {
            f.reset();
        }
        self.delay.reset();
        self.was_on = false;
        self.phase = 0.0;
        self.last_position = 0.5;
        self.brake = 1.0;
        self.shift_phase = 0.0;
    }

    fn tail(&self) -> f64 {
        120.0
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], tempo: f64) -> f64 {
        capture_tail(self.base.core.sr, values, tempo)
    }

    // The capture history must keep recording silence.
    fn gateable(&self) -> bool {
        false
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], _sc_l: Option<&[f32]>, _sc_r: Option<&[f32]>) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let sr = self.base.core.sr;
        let engaged = self.base.core.p[0];
        let smoothing = self.base.smoothing;
        for i in 0..l.len() {
            let on = engaged >= 0.5;
            let engage = self.base.value(0);
            let position = self.base.value(1);
            let milliseconds = self.base.value(2);
            let feedback = self.base.value(3);
            let shift = self.base.value(4);
            let wet = self.base.value(5);
            let brake_time = self.base.value(6);
            let capture_time = self.base.value(7);
            let amount = self.base.value(8);
            let v9 = self.base.value(9);
            let out = self.out_gain.get([v9], || db_gain(v9));
            let input = [l[i] as f64, r[i] as f64];
            let length = clamp(capture_time * sr, 2.0, self.history.valid.max(2) as f64);
            if on && !self.was_on {
                self.history.capture(0);
                self.phase = 0.0;
                self.brake = 1.0;
                self.last_position = position;
            }
            let motion = (position - self.last_position) * length * 2.0;
            self.last_position = position;
            self.brake *= self.brake_pole.get([brake_time, position], || pole(brake_time * 1000.0 * (0.25 + position * 1.75), sr));
            let travel = clamp(self.brake + motion, -16.0, 16.0);
            self.phase = wrap(self.phase / length) * length;
            let captured = self.history.read(length, self.phase);
            let edge = seam(self.phase, length, min(sr * 0.002, length * 0.02));
            let a = captured[0] * edge;
            let b = captured[1] * edge;
            self.phase += travel;
            let frames = self.delay.tick(max(1.0, milliseconds * 0.001 * sr * (0.2 + 0.8 * position)), smoothing);
            let dl = self.left.read(frames);
            let dr = self.right.read(frames);
            self.shift_phase = wrap(self.shift_phase + shift / sr);
            // Zero shift gives the shifted path zero weight: record, skip the FIR.
            let shift_weight = min(1.0, shift.abs());
            let (mut sl, mut sr_) = (0.0f64, 0.0f64);
            self.shifter[0].push(dl);
            self.shifter[1].push(dr);
            if shift_weight != 0.0 {
                sl = self.shifter[0].output(self.shift_phase);
                sr_ = self.shifter[1].output(self.shift_phase);
            }
            self.left.push(a * engage + soft(mix(dl, sl, shift_weight) * feedback));
            self.right.push(b * engage + soft(mix(dr, sr_, shift_weight) * feedback));
            l[i] = bounded(mix(input[0], a + dl * wet, engage * amount) * out) as f32;
            r[i] = bounded(mix(input[1], b + dr * wet, engage * amount) * out) as f32;
            if !on {
                self.history.push(input[0], input[1]);
            }
            self.history.frames = self.history.frames.wrapping_add(1);
            self.was_on = on;
        }
    }
}

const FILTER_TYPES: &[Label] = &[Label::new(c"Low pass"), Label::new(c"Band pass"), Label::new(c"High pass")];
const POLARITIES: &[Label] = &[Label::new(c"Positive"), Label::new(c"Negative")];
const TONES: &[Label] = &[Label::new(c"Flanger"), Label::new(c"Phaser")];
const ASSIGNMENTS: &[Label] = &[Label::new(c"Feedback"), Label::new(c"Time")];
const TIME_MODES: &[Label] = &[Label::new(c"Straight"), Label::new(c"Dotted"), Label::new(c"Triplet"), Label::new(c"Milliseconds")];
const ECHO_MODES: &[Label] = &[Label::new(c"Mix"), Label::new(c"Replace"), Label::new(c"Wet only")];
const RESO_MODES: &[Label] = &[Label::new(c"Mix"), Label::new(c"Insert"), Label::new(c"Wet only")];
const RING_MODES: &[Label] = &[Label::new(c"Mix"), Label::new(c"Wet only")];
const STUTTER_ASSIGN: &[Label] = &[Label::new(c"Length"), Label::new(c"Pitch")];
const DIRECTIONS: &[Label] = &[Label::new(c"Forward"), Label::new(c"Reverse"), Label::new(c"Follow movement")];
const VIBRATO_MODES: &[Label] = &[Label::new(c"Varispeed"), Label::new(c"Frequency shift")];

static FILTER: [P; 10] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"position", c"Frequency position", c"%", 0.0, 1.0, 0.65, 0),
    P::choice(c"type", c"Filter type", c"", 0.0, 2.0, 0.0, INTEGER, FILTER_TYPES),
    P::new(c"resonance", c"Resonance", c"%", 0.0, 1.05, 0.2, 0),
    P::new(c"saturation", c"Saturation", c"dB", 0.0, 30.0, 0.0, 0),
    P::new(c"output", c"Output", c"dB", -36.0, 12.0, 0.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 1.0, 1.0, 0),
    P::new(c"response", c"Response", c"ms", 1.0, 100.0, 12.0, LOG),
    P::new(c"min_frequency", c"Minimum frequency", c"Hz", 20.0, 20000.0, 40.0, LOG),
    P::new(c"max_frequency", c"Maximum frequency", c"Hz", 20.0, 20000.0, 18000.0, LOG),
];
static FLANGER: [P; 10] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"frequency", c"Comb frequency", c"Hz", 10.0, 4000.0, 220.0, LOG),
    P::new(c"decay", c"Decay", c"s", 0.02, 12.0, 1.5, LOG),
    P::new(c"saturation", c"Saturation", c"dB", 0.0, 24.0, 3.0, 0),
    P::new(c"stereo", c"Stereo", c"%", 0.0, 1.0, 0.35, 0),
    P::choice(c"polarity", c"Polarity", c"", 0.0, 1.0, 0.0, INTEGER, POLARITIES),
    P::choice(c"tone", c"Tone", c"", 0.0, 1.0, 0.0, INTEGER, TONES),
    P::new(c"mix", c"Mix", c"%", 0.0, 1.0, 0.7, 0),
    P::new(c"rate", c"Motion rate", c"Hz", 0.0, 10.0, 0.25, 0),
    P::new(c"depth", c"Motion depth", c"%", 0.0, 1.0, 0.15, 0),
];
static BURST: [P; 14] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"position", c"Performance position", c"%", 0.0, 1.0, 0.6, 0),
    P::choice(c"assignment", c"Position controls", c"", 0.0, 1.0, 0.0, INTEGER, ASSIGNMENTS),
    P::new(c"time", c"Time", c"beats", 1.0 / 32.0, 4.0, 0.5, LOG),
    P::choice(c"time_mode", c"Time mode", c"", 0.0, 3.0, 0.0, INTEGER, TIME_MODES),
    P::new(c"feedback", c"Feedback", c"%", 0.0, 0.98, 0.6, 0),
    P::new(c"filter", c"Feedback filter", c"%", 0.0, 1.0, 0.2, 0),
    P::new(c"wet", c"Wet level", c"%", 0.0, 1.0, 0.5, 0),
    P::new(c"feed", c"Burst length (1 = continuous)", c"s", 0.01, 1.0, 0.15, 0),
    P::choice(c"mix_mode", c"Mix mode", c"", 0.0, 2.0, 0.0, INTEGER, ECHO_MODES),
    P::new(c"free_time", c"Free time", c"ms", 1.0, 2000.0, 250.0, LOG),
    P::new(c"saturation", c"Saturation", c"dB", 0.0, 18.0, 0.0, 0),
    P::new(c"stereo", c"Ping-pong", c"%", 0.0, 1.0, 0.0, 0),
    P::new(c"output", c"Output", c"dB", -24.0, 12.0, 0.0, 0),
];
static RESO: [P; 12] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"frequency", c"Resonator frequency", c"Hz", 20.0, 4000.0, 440.0, LOG),
    P::new(c"time", c"Time", c"beats", 1.0 / 32.0, 4.0, 0.5, LOG),
    P::new(c"feedback", c"Feedback", c"%", 0.0, 0.98, 0.65, 0),
    P::new(c"pump", c"Pump", c"%", 0.0, 1.0, 0.3, 0),
    P::new(c"wet", c"Wet level", c"%", 0.0, 1.0, 0.5, 0),
    P::choice(c"mix_mode", c"Mix mode", c"", 0.0, 2.0, 0.0, INTEGER, RESO_MODES),
    P::new(c"resonance", c"Resonance", c"%", 0.0, 1.0, 0.6, 0),
    P::new(c"saturation", c"Saturation", c"dB", 0.0, 24.0, 6.0, 0),
    P::new(c"free_time", c"Free time", c"ms", 1.0, 2000.0, 250.0, LOG),
    P::new(c"sync", c"Tempo sync", c"", 0.0, 1.0, 1.0, TOGGLE),
    P::new(c"spread", c"Stereo spread", c"%", 0.0, 1.0, 0.25, 0),
];
static RING: [P; 9] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"frequency", c"Frequency", c"Hz", 5.0, 6000.0, 220.0, LOG),
    P::new(c"spread", c"Oscillator spread", c"%", 0.0, 1.0, 0.15, 0),
    P::new(c"glide", c"Glide", c"ms", 1.0, 2000.0, 25.0, LOG),
    P::new(c"reverb_time", c"Reverb time", c"s", 0.1, 15.0, 2.0, LOG),
    P::new(c"reverb_mix", c"Reverb mix", c"%", 0.0, 1.0, 0.25, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 1.0, 0.75, 0),
    P::choice(c"mix_mode", c"Mix mode", c"", 0.0, 1.0, 0.0, INTEGER, RING_MODES),
    P::new(c"output", c"Output", c"dB", -24.0, 12.0, 0.0, 0),
];
static STUTTER: [P; 11] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"position", c"Performance position", c"%", 0.0, 1.0, 0.5, 0),
    P::choice(c"assignment", c"Position controls", c"", 0.0, 1.0, 0.0, INTEGER, STUTTER_ASSIGN),
    P::new(c"length", c"Loop length", c"beats", 1.0 / 32.0, 4.0, 0.5, LOG),
    P::new(c"pitch", c"Pitch", c"st", -24.0, 24.0, 0.0, 0),
    P::new(c"gate", c"Gate open", c"%", 0.0, 1.0, 1.0, 0),
    P::choice(c"direction", c"Direction", c"", 0.0, 2.0, 0.0, INTEGER, DIRECTIONS),
    P::new(c"quantize", c"Tempo quantize", c"", 0.0, 1.0, 1.0, TOGGLE),
    P::new(c"free_length", c"Free loop length", c"ms", 5.0, 2000.0, 125.0, LOG),
    P::new(c"mix", c"Mix", c"%", 0.0, 1.0, 1.0, 0),
    P::new(c"fade", c"Seam fade", c"ms", 0.1, 20.0, 2.0, LOG),
];
static TREMOLO: [P; 13] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"position", c"Performance position", c"%", 0.0, 1.0, 0.5, 0),
    P::new(c"blend", c"Tremolo / Vibrato", c"%", 0.0, 1.0, 0.0, 0),
    P::choice(c"vibrato_mode", c"Vibrato mode", c"", 0.0, 1.0, 0.0, INTEGER, VIBRATO_MODES),
    P::new(c"response", c"Attack / Release", c"ms", 1.0, 1000.0, 20.0, LOG),
    P::new(c"stereo", c"Stereo phase", c"%", 0.0, 1.0, 0.0, 0),
    P::new(c"depth_min", c"Depth minimum", c"%", 0.0, 1.0, 0.0, 0),
    P::new(c"depth_max", c"Depth maximum", c"%", 0.0, 1.0, 1.0, 0),
    P::new(c"rate_min", c"Rate minimum", c"Hz", 0.05, 30.0, 1.0, LOG),
    P::new(c"rate_max", c"Rate maximum", c"Hz", 0.05, 30.0, 12.0, LOG),
    P::new(c"sync", c"Tempo sync", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"division", c"Period", c"beats", 1.0 / 32.0, 4.0, 0.5, LOG),
    P::new(c"output", c"Output", c"dB", -24.0, 12.0, 0.0, 0),
];
static SCRATCH: [P; 10] = [
    P::new(c"engage", c"Engage", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"position", c"Scratch position", c"%", 0.0, 1.0, 0.5, 0),
    P::new(c"delay", c"Swirl delay", c"ms", 1.0, 1000.0, 125.0, LOG),
    P::new(c"feedback", c"Feedback", c"%", 0.0, 0.95, 0.3, 0),
    P::new(c"frequency_shift", c"Feedback shift", c"Hz", -2000.0, 2000.0, 0.0, 0),
    P::new(c"wet", c"Swirl level", c"%", 0.0, 1.0, 0.25, 0),
    P::new(c"brake", c"Brake time", c"s", 0.01, 3.0, 0.25, LOG),
    P::new(c"capture", c"Capture length", c"s", 0.05, 4.0, 1.0, LOG),
    P::new(c"mix", c"Mix", c"%", 0.0, 1.0, 1.0, 0),
    P::new(c"output", c"Output", c"dB", -24.0, 12.0, 0.0, 0),
];

static DESCRIPTORS: [Descriptor; 8] = [
    Descriptor::new(
        c"performfilter",
        c"Perform Filter",
        c"Perform",
        0,
        &FILTER,
        |sr, _| Box::new(PerformFilter::new(sr)),
        PerformFilter::tail_estimate,
    ),
    Descriptor::new(
        c"performflanger",
        c"Perform Flanger",
        c"Perform",
        0,
        &FLANGER,
        |sr, _| Box::new(PerformFlanger::new(sr)),
        PerformFlanger::tail_estimate,
    ),
    Descriptor::new(c"burstecho", c"Burst Echo", c"Perform", 0, &BURST, |sr, _| Box::new(BurstEcho::new(sr)), BurstEcho::tail_estimate),
    Descriptor::new(c"resoecho", c"Reso Echo", c"Perform", 0, &RESO, |sr, _| Box::new(ResoEcho::new(sr)), ResoEcho::tail_estimate),
    Descriptor::new(c"ring", c"Ring", c"Perform", 0, &RING, |sr, _| Box::new(PerformRing::new(sr)), PerformRing::tail_estimate),
    Descriptor::new(c"stutter", c"Stutter", c"Perform", 0, &STUTTER, |sr, _| Box::new(PerformStutter::new(sr)), capture_tail),
    Descriptor::new(c"tremolo", c"Tremolo", c"Perform", 0, &TREMOLO, |sr, _| Box::new(PerformTremolo::new(sr)), |_, _, _| 0.1),
    Descriptor::new(c"scratcher", c"Scratcher", c"Perform", 0, &SCRATCH, |sr, _| Box::new(PerformScratcher::new(sr)), capture_tail),
];

/// `stfx::performDescriptors`.
pub fn descriptors() -> &'static [Descriptor] {
    &DESCRIPTORS
}

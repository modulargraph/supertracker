//! Spatial processors: Ice, Metaverb, Reflex and the three reverbs (legacy, algorithmic, plate).
//!
//! Ported from the C++ `EffectsSpatial.hpp` (namespace `stfx::spatial`).
//!
//! Original implementations informed by NI's public effect descriptions. These
//! are deliberately not described as bit-compatible NI algorithms or presets.

use super::common::{
    core_accessors, frame_buffers, max, min, Core, Descriptor, Label, Memo, ParameterInfo as P,
    Processor, MAX_PARAMETERS, PARAMETER_INTEGER as INTEGER, PARAMETER_LOG as LOG,
    PARAMETER_TOGGLE as TOGGLE, PI,
};

/// `spatial::bounded`: clamp to [lo, hi]; nonfinite values become `lo`.
#[inline(always)]
pub fn bounded(x: f64, lo: f64, hi: f64) -> f64 {
    if x.is_finite() {
        max(lo, min(hi, x))
    } else {
        lo
    }
}

/// `spatial::clean`: nonfinite and tiny single-precision values become 0.
#[inline(always)]
pub fn clean(x: f32) -> f32 {
    if x.is_finite() && x.abs() > 1.0e-25 {
        x
    } else {
        0.0
    }
}

/// `spatial::soft`: gentle single-precision limiter for feedback paths.
#[inline(always)]
pub fn soft(x: f32) -> f32 {
    x / (1.0 + 0.05 * x.abs())
}

/// `spatial::pole`: one-pole low-pass coefficient for a cutoff in Hz.
#[inline]
pub fn pole(hz: f64, sr: f64) -> f64 {
    1.0 - (-2.0 * PI * bounded(hz, 1.0, 0.45 * sr) / sr).exp()
}

/// `spatial::Line`: single-precision delay line with linear-interpolated reads.
#[derive(Clone, Default)]
pub struct Line {
    pub(crate) data: Vec<f32>,
    pub(crate) pos: usize,
}

impl Line {
    /// Allocates `max(count, 8)` samples of silence.
    pub fn prepare(&mut self, count: usize) {
        self.data = vec![0.0; count.max(8)];
        self.pos = 0;
    }

    pub fn with_capacity(count: usize) -> Self {
        let mut line = Line::default();
        line.prepare(count);
        line
    }

    pub fn reset(&mut self) {
        self.data.fill(0.0);
        self.pos = 0;
    }

    #[inline(always)]
    pub fn read(&self, samples: f64) -> f32 {
        let samples = bounded(samples, 1.0, self.data.len().wrapping_sub(2) as f64);
        let whole = samples as usize;
        let f = (samples - whole as f64) as f32;
        self.read_at(whole, f)
    }

    // 1 <= whole <= size-2 (positions validated per block).
    #[inline(always)]
    pub(crate) fn read_at(&self, whole: usize, f: f32) -> f32 {
        let size = self.data.len();
        let a = if self.pos >= whole {
            self.pos - whole
        } else {
            (self.pos + size).wrapping_sub(whole)
        };
        let b = if a == 0 { size.wrapping_sub(1) } else { a - 1 };
        // Out-of-range slots cannot occur for validated positions; they read as silence.
        let xa = self.data.get(a).copied().unwrap_or(0.0);
        let xb = self.data.get(b).copied().unwrap_or(0.0);
        xa + f * (xb - xa)
    }

    #[inline(always)]
    pub fn write(&mut self, x: f32) {
        if let Some(slot) = self.data.get_mut(self.pos) {
            *slot = clean(x);
        }
        self.pos += 1;
        if self.pos == self.data.len() {
            self.pos = 0;
        }
    }

    // Block helpers. read_block: out[i] is what read_at(whole, f) returns just
    // before the i-th of the next out.len() writes; valid while whole >= out.len()
    // (the block never reads its own writes).
    fn read_block(&self, whole: usize, f: f32, out: &mut [f32]) {
        let size = self.data.len();
        let mut a = if self.pos >= whole {
            self.pos - whole
        } else {
            (self.pos + size).wrapping_sub(whole)
        };
        for slot in out.iter_mut() {
            let b = if a == 0 { size.wrapping_sub(1) } else { a - 1 };
            let xa = self.data.get(a).copied().unwrap_or(0.0);
            let xb = self.data.get(b).copied().unwrap_or(0.0);
            *slot = xa + f * (xb - xa);
            a += 1;
            if a == size {
                a = 0;
            }
        }
    }

    // Already-cleaned samples.
    fn write_block(&mut self, x: &[f32]) {
        let size = self.data.len();
        let mut i = 0;
        while i < x.len() {
            let run = (x.len() - i).min(size - self.pos);
            self.data[self.pos..self.pos + run].copy_from_slice(&x[i..i + run]);
            i += run;
            self.pos += run;
            if self.pos == size {
                self.pos = 0;
            }
        }
    }

    #[inline(always)]
    pub fn allpass(&mut self, x: f32, samples: f64, gain: f32) -> f32 {
        let d = self.read(samples);
        let y = d - gain * x;
        self.write(x + gain * y);
        y
    }
}

/// Branch-free `clean` (nonfinite or |x| <= 1e-25 -> 0).
#[inline(always)]
fn clean_fast(x: f32) -> f32 {
    let a = x.abs();
    if a > 1.0e-25 && a <= f32::MAX {
        x
    } else {
        0.0
    }
}

/// `Network::kChunk`: the largest sub-block the network renders at once.
pub const CHUNK: usize = 64;

/// Eight-line orthogonal feedback network: per-line T60 gains avoid a decay
/// change with sample rate. Room, Hall and Plate have different delay families,
/// diffusion and early-reflection balances; this is an algorithmic plate model.
/// Render cost: control-rate values (T60 gains, allpass/predelay positions)
/// are derived once per parameter change, smoothed delay times snap to their
/// targets when converged, and the per-sample path has no clamps.
pub struct Network {
    sr: f64,
    tank: [Line; 8],
    ap: [Line; 6],
    pre: [Line; 2],
    times: [f64; 8],
    target: [f64; 8],
    gains: [f64; 8],
    low: [f64; 8],
    phase: [f64; 8],
    increment: [f64; 8],
    feedback: [f64; 8],
    ap_whole: [usize; 6],
    ap_frac: [f32; 6],
    lp: f64,
    modulation: f64,
    diff: f64,
    predelay: f64,
    desired_pre: f64,
    smooth: f64,
    max_time: f64,
    frozen: bool,
    configured: bool,
    times_settled: bool,
    mode: i32,
    early: f64,
    last_args: [f64; 8],
    have_args: bool,
}

impl Network {
    pub fn new(rate: f64) -> Self {
        let sr = rate;
        let tank: [Line; 8] =
            std::array::from_fn(|_| Line::with_capacity((sr * 0.35) as usize + 8));
        let ap: [Line; 6] = std::array::from_fn(|_| Line::with_capacity((sr * 0.035) as usize + 8));
        let pre: [Line; 2] = std::array::from_fn(|_| Line::with_capacity((sr * 0.26) as usize + 8));
        let mut network = Network {
            sr,
            tank,
            ap,
            pre,
            times: [0.0; 8],
            target: [0.0; 8],
            gains: [0.0; 8],
            low: [0.0; 8],
            phase: [0.0; 8],
            increment: [0.0; 8],
            feedback: [0.0; 8],
            ap_whole: [0; 6],
            ap_frac: [0.0; 6],
            lp: 0.4,
            modulation: 0.0,
            diff: 0.6,
            predelay: 1.0,
            desired_pre: 1.0,
            smooth: 1.0 - (-1.0 / (0.025 * rate)).exp(),
            max_time: 1.0,
            frozen: false,
            configured: false,
            times_settled: false,
            mode: 0,
            early: 0.2,
            last_args: [0.0; 8],
            have_args: false,
        };
        for k in 0..8 {
            network.times[k] = sr * 0.025;
            network.target[k] = sr * 0.025;
            network.phase[k] = k as f64 * 0.719;
            network.increment[k] = (0.07 + 0.013 * k as f64) * 2.0 * PI / sr;
        }
        const APS: [f64; 6] = [0.0047, 0.0073, 0.0107, 0.0059, 0.0089, 0.0121];
        for j in 0..6 {
            let t = bounded(APS[j] * sr, 1.0, (network.ap[j].data.len() - 2) as f64);
            network.ap_whole[j] = t as usize;
            network.ap_frac[j] = (t - network.ap_whole[j] as f64) as f32;
        }
        network.max_time = (network.tank[0].data.len() - 2) as f64;
        network
    }

    #[allow(clippy::too_many_arguments)]
    pub fn configure(
        &mut self,
        kind: i32,
        size: f64,
        t60: f64,
        damping: f64,
        modulation: f64,
        diffusion: f64,
        pre_ms: f64,
        freeze: bool,
    ) {
        let args = [
            kind as f64,
            size,
            t60,
            damping,
            modulation,
            diffusion,
            pre_ms,
            freeze as u8 as f64,
        ];
        if self.have_args && args == self.last_args && self.configured {
            return;
        }
        self.last_args = args;
        self.have_args = true;
        self.mode = kind;
        self.frozen = freeze;
        const ROOM: [f64; 8] = [
            0.0173, 0.0239, 0.0311, 0.0377, 0.0419, 0.0473, 0.0533, 0.0599,
        ];
        const HALL: [f64; 8] = [
            0.0297, 0.0371, 0.0411, 0.0539, 0.0671, 0.0713, 0.0797, 0.0893,
        ];
        const PLATE: [f64; 8] = [
            0.0229, 0.0317, 0.0401, 0.0487, 0.0613, 0.0689, 0.0833, 0.0971,
        ];
        const META: [f64; 8] = [0.031, 0.0465, 0.062, 0.0775, 0.093, 0.1085, 0.124, 0.1395];
        let base = match kind {
            1 => &HALL,
            2 => &PLATE,
            3 => &META,
            _ => &ROOM,
        };
        let sr = self.sr;
        let scale = 0.3 + bounded(size, 0.0, 100.0) * 0.015;
        for k in 0..8 {
            self.target[k] = sr * base[k] * scale;
            self.gains[k] = if freeze {
                0.999995
            } else {
                0.001f64.powf(self.target[k] / (sr * bounded(t60, 0.1, 30.0)))
            };
            self.feedback[k] = 0.3535533905932738 * self.gains[k];
        }
        self.lp = if freeze {
            1.0
        } else {
            // std::pow(x, 2.) is compiled as x*x.
            let open = 1.0 - bounded(damping, 0.0, 100.0) * 0.01;
            pole(1200.0 + (open * open) * 17000.0, sr)
        };
        self.modulation = bounded(modulation, 0.0, 100.0) * 0.000018 * sr;
        self.diff = bounded(0.15 + diffusion * 0.0065, 0.15, 0.8);
        self.desired_pre = max(1.0, bounded(pre_ms, 0.0, 250.0) * 0.001 * sr);
        self.early = if kind == 0 {
            0.35 * (1.0 - diffusion * 0.008)
        } else if kind == 1 {
            0.12 * (1.0 - diffusion * 0.009)
        } else {
            0.04
        };
        if !self.configured {
            self.times = self.target;
            self.predelay = self.desired_pre;
            self.configured = true;
        }
        self.times_settled = false;
    }

    pub fn reset(&mut self) {
        for d in self.tank.iter_mut() {
            d.reset();
        }
        for d in self.ap.iter_mut() {
            d.reset();
        }
        for d in self.pre.iter_mut() {
            d.reset();
        }
        self.low = [0.0; 8];
        self.times = self.target;
        self.predelay = self.desired_pre;
        self.configured = false;
        self.have_args = false;
        self.times_settled = false;
        for k in 0..8 {
            self.phase[k] = k as f64 * 0.719;
        }
    }

    #[inline(always)]
    fn snap(value: f64, goal: f64) -> f64 {
        if (value - goal).abs() <= 1e-9 * goal.abs() {
            goal
        } else {
            value
        }
    }

    // Largest sub-block whose tank and allpass reads all precede its writes.
    fn chunk_limit(&self) -> usize {
        let mut shortest = 1e9;
        for k in 0..8 {
            shortest = min(shortest, min(self.times[k], self.target[k]));
        }
        shortest -= self.modulation + 2.0;
        let mut limit = CHUNK;
        for j in 0..6 {
            limit = limit.min(self.ap_whole[j]);
        }
        limit
            .min(if shortest > 1.0 { shortest as usize } else { 1 })
            .max(1)
    }

    // Renders in_l.len() samples of wet output; identical arithmetic to a per-sample
    // loop (predelay -> 3 allpasses per side -> 8 modulated lines -> damping
    // -> Walsh-Hadamard feedback), but staged block-wise so loops vectorize.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        let mut offset = 0;
        while offset < n {
            let count = (n - offset).min(self.chunk_limit());
            let end = offset + count;
            self.chunk(
                &in_l[offset..end],
                &in_r[offset..end],
                &mut out_l[offset..end],
                &mut out_r[offset..end],
            );
            offset = end;
        }
    }

    fn chunk(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l
            .len()
            .min(in_r.len())
            .min(out_l.len())
            .min(out_r.len())
            .min(CHUNK);
        let (in_l, in_r, out_l, out_r) = (&in_l[..n], &in_r[..n], &mut out_l[..n], &mut out_r[..n]);
        let mut x = [[0.0f32; CHUNK]; 2];
        let mut early_out = [[0.0f32; CHUNK]; 2];
        let mut buffer = [0.0f32; CHUNK];
        let e = self.early as f32;
        let frozen = self.frozen;
        let pre_limit = (self.pre[0].data.len() - 2) as f64;
        for i in 0..n {
            if self.predelay != self.desired_pre {
                self.predelay = Self::snap(
                    self.predelay + self.smooth * (self.desired_pre - self.predelay),
                    self.desired_pre,
                );
            }
            let pd = min(self.predelay, pre_limit);
            let pw = pd as usize;
            let pf = (pd - pw as f64) as f32;
            x[0][i] = self.pre[0].read_at(pw, pf);
            x[1][i] = self.pre[1].read_at(pw, pf);
            self.pre[0].write(if frozen { 0.0 } else { in_l[i] });
            self.pre[1].write(if frozen { 0.0 } else { in_r[i] });
        }
        for c in 0..2 {
            for i in 0..n {
                early_out[c][i] = x[c][i] * e;
            }
        }
        let g = self.diff as f32;
        for c in 0..2 {
            for j in 0..3 {
                let index = c * 3 + j;
                self.ap[index].read_block(
                    self.ap_whole[index],
                    self.ap_frac[index],
                    &mut buffer[..n],
                );
                let xc = &mut x[c][..n];
                for (b, xv) in buffer[..n].iter_mut().zip(xc.iter_mut()) {
                    let y = *b - g * *xv;
                    *b = clean_fast(*xv + g * y);
                    *xv = y;
                }
                self.ap[index].write_block(&buffer[..n]);
            }
        }
        // Per-sample modulated read positions (smoothed times, triangle LFOs).
        let mut position = [[0.0f64; CHUNK]; 8];
        for i in 0..n {
            if !self.times_settled {
                let mut all = true;
                for k in 0..8 {
                    self.times[k] = Self::snap(
                        self.times[k] + self.smooth * (self.target[k] - self.times[k]),
                        self.target[k],
                    );
                    all = all && self.times[k] == self.target[k];
                }
                self.times_settled = all;
            }
            for k in 0..8 {
                let mut ph = self.phase[k] + self.increment[k];
                ph = if ph > 2.0 * PI { ph - 2.0 * PI } else { ph };
                self.phase[k] = ph;
                // Triangle modulation keeps the inner loop free from transcendental calls.
                let triangle = 1.0 - 2.0 * (ph * (1.0 / PI) - 1.0).abs();
                position[k][i] = min(
                    self.max_time,
                    max(1.0, self.times[k] + self.modulation * triangle),
                );
            }
        }
        let mut d = [[0.0f32; CHUNK]; 8];
        for k in 0..8 {
            let t = &self.tank[k];
            let size = t.data.len();
            let data = &t.data[..];
            let pos = t.pos;
            for i in 0..n {
                let whole = position[k][i] as usize;
                let f = (position[k][i] - whole as f64) as f32;
                // Slot of "whole samples ago" relative to this sample's write position.
                let write_at = if pos + i >= size {
                    pos + i - size
                } else {
                    pos + i
                };
                let a = if write_at >= whole {
                    write_at - whole
                } else {
                    (write_at + size).wrapping_sub(whole)
                };
                let b = if a == 0 { size - 1 } else { a - 1 };
                // 1 <= whole <= size-2 and write_at < size, so both slots are in range.
                let xa = data.get(a).copied().unwrap_or(0.0);
                let xb = data.get(b).copied().unwrap_or(0.0);
                d[k][i] = xa + f * (xb - xa);
            }
        }
        let lpd = self.lp;
        // Damping one-poles, interleaved across lines so the eight recursions overlap.
        {
            let mut state = self.low;
            for i in 0..n {
                for k in 0..8 {
                    state[k] += lpd * (d[k][i] as f64 - state[k]);
                    d[k][i] = state[k] as f32;
                }
            }
            self.low = state;
        }
        for i in 0..n {
            out_l[i] = early_out[0][i] + (d[0][i] + d[2][i] - d[4][i] + d[6][i]) * 0.30f32;
            out_r[i] = early_out[1][i] + (d[1][i] + d[3][i] - d[5][i] + d[7][i]) * 0.30f32;
        }
        // Normalised Walsh-Hadamard transform has unit spectral radius.
        let mut span = 1;
        while span < 8 {
            let mut i0 = 0;
            while i0 < 8 {
                for j in 0..span {
                    let (head, tail) = d.split_at_mut(i0 + j + span);
                    let p0 = &mut head[i0 + j][..n];
                    let p1 = &mut tail[0][..n];
                    for (a, b) in p0.iter_mut().zip(p1.iter_mut()) {
                        let (va, vb) = (*a, *b);
                        *a = va + vb;
                        *b = va - vb;
                    }
                }
                i0 += span * 2;
            }
            span *= 2;
        }
        for k in 0..8 {
            let input = &x[k & 1][..n];
            let fb = self.feedback[k];
            for i in 0..n {
                buffer[i] = clean_fast(soft(
                    (if frozen { 0.0f32 } else { input[i] * 0.24f32 })
                        + (d[k][i] as f64 * fb) as f32,
                ));
            }
            self.tank[k].write_block(&buffer[..n]);
        }
    }

    /// One sample through the network.
    pub fn tick(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let (mut out_l, mut out_r) = ([0.0f32; 1], [0.0f32; 1]);
        self.process(&[in_l], &[in_r], &mut out_l, &mut out_r);
        (out_l[0], out_r[0])
    }
}

/// Zero-Mix idling. With Mix at exactly 0 the output is the exact dry path;
/// once that has lasted longer than the effect's tail, the wet DSP stops (so a
/// short automation dip through 0 is unchanged). When Mix rises again the wet
/// state restarts from silence.
#[derive(Clone, Copy, Default)]
pub struct ZeroMix {
    frames: u64,
    idle: bool,
}

impl ZeroMix {
    /// Returns `(skip, restart)`. skip: do not run the wet DSP for this block. restart: reset
    /// the wet state first. `tail` is the effect's `tail_for_parameters` for its current
    /// parameters and tempo; it is only evaluated while counting zero-mix frames.
    #[inline]
    pub fn skip(
        &mut self,
        zero: bool,
        tail: impl FnOnce() -> f64,
        sr: f64,
        n: usize,
    ) -> (bool, bool) {
        if !zero {
            let restart = self.idle;
            self.idle = false;
            self.frames = 0;
            return (false, restart);
        }
        if self.idle {
            return (true, false);
        }
        self.frames = self.frames.wrapping_add(n as u64);
        if self.frames as f64 > tail() * sr {
            self.idle = true;
        }
        (false, false)
    }

    pub fn reset(&mut self) {
        self.frames = 0;
        self.idle = false;
    }
}

/// Two-band tone control on the wet signal.
#[derive(Clone, Copy)]
pub struct WetEq {
    bass: [f64; 2],
    treble: [f64; 2],
    low_gain: f64,
    high_gain: f64,
    high_a: f64,
    bass_a: f64,
    last_args: [f64; 4],
}

impl Default for WetEq {
    fn default() -> Self {
        WetEq {
            bass: [0.0; 2],
            treble: [0.0; 2],
            low_gain: 1.0,
            high_gain: 1.0,
            high_a: 0.9,
            bass_a: 0.03,
            last_args: [-1.0, 0.0, 0.0, 0.0],
        }
    }
}

impl WetEq {
    // Called every block; the gains/poles are only re-derived when an argument changes.
    pub fn set(&mut self, sr: f64, low_db: f64, high_db: f64, cutoff: f64) {
        if sr == self.last_args[0]
            && low_db == self.last_args[1]
            && high_db == self.last_args[2]
            && cutoff == self.last_args[3]
        {
            return;
        }
        self.last_args = [sr, low_db, high_db, cutoff];
        self.low_gain = 10.0f64.powf(low_db / 20.0);
        self.high_gain = 10.0f64.powf(high_db / 20.0);
        self.high_a = pole(cutoff, sr);
        self.bass_a = pole(300.0, sr);
    }

    #[inline(always)]
    pub fn tick(&mut self, ch: usize, x: f32) -> f32 {
        let ch = ch & 1;
        self.bass[ch] += self.bass_a * (x as f64 - self.bass[ch]);
        self.treble[ch] += self.high_a * (x as f64 - self.treble[ch]);
        clean(
            (self.treble[ch] * self.high_gain + self.bass[ch] * (self.low_gain - self.high_gain))
                as f32,
        )
    }

    pub fn reset(&mut self) {
        self.bass = [0.0; 2];
        self.treble = [0.0; 2];
    }
}

/// The default cutoff of `WetEQ::set` in the C++ signature.
const WET_EQ_OPEN: f64 = 20000.0;

/// Which parameter layout a `Reverb` instance serves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReverbKind {
    /// "Reverb Legacy": room selector, pan, stereo width and freeze.
    Legacy,
    /// "Reverb": Room / Hall / Plate modes.
    Algorithmic,
    /// "Plate Reverb".
    Plate,
}

pub struct Reverb {
    core: Core,
    net: Network,
    eq: WetEq,
    kind: ReverbKind,
    zero: ZeroMix,
}

impl Reverb {
    pub fn new(rate: f64, kind: ReverbKind) -> Self {
        Reverb {
            core: Core::new(rate),
            net: Network::new(rate),
            eq: WetEq::default(),
            kind,
            zero: ZeroMix::default(),
        }
    }

    /// `Reverb::tailForParameters` for the given layout.
    pub fn tail_estimate(kind: ReverbKind, values: &[f64; MAX_PARAMETERS]) -> f64 {
        match kind {
            ReverbKind::Legacy => {
                if values[6] >= 0.5 {
                    120.0
                } else {
                    (0.4 + values[1] * 0.08) * 1.9 + 0.5
                }
            }
            ReverbKind::Plate => values[0] * 1.9 + 0.5,
            ReverbKind::Algorithmic => values[1] * 1.9 + 0.5,
        }
    }
}

impl Processor for Reverb {
    core_accessors!(core);

    fn reset(&mut self) {
        self.net.reset();
        self.eq.reset();
        self.zero.reset();
    }

    fn tail(&self) -> f64 {
        if self.kind == ReverbKind::Legacy {
            120.0
        } else {
            40.0
        }
    }

    fn tail_for_parameters(&self, values: &[f64; MAX_PARAMETERS], _tempo: f64) -> f64 {
        Self::tail_estimate(self.kind, values)
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
        let mut mix;
        let mut pan = 0.0;
        let mut width = 1.0;
        let mut freeze = false;
        match self.kind {
            ReverbKind::Legacy => {
                let kind = bounded(p[0], 0.0, 3.0) as i32;
                freeze = p[6] >= 0.5;
                mix = p[7] * 0.01;
                pan = p[4] * 0.01;
                width = p[5] * 0.01;
                self.net.configure(
                    if kind == 3 { 3 } else { 0 },
                    p[1],
                    0.4 + p[1] * 0.08,
                    if kind == 1 {
                        10.0
                    } else if kind == 2 {
                        65.0
                    } else {
                        40.0
                    },
                    if kind == 3 { 70.0 } else { 12.0 },
                    if kind == 3 { 35.0 } else { 80.0 },
                    0.0,
                    freeze,
                );
                self.eq.set(sr, p[2], p[3], WET_EQ_OPEN);
            }
            ReverbKind::Plate => {
                mix = p[5] * 0.01;
                self.net
                    .configure(2, 55.0, p[0], p[2], 25.0, 95.0, p[3], false);
                self.eq.set(sr, p[1], 0.0, p[4]);
            }
            ReverbKind::Algorithmic => {
                mix = p[7] * 0.01;
                self.net.configure(
                    bounded(p[0], 0.0, 2.0) as i32,
                    p[2],
                    p[1],
                    p[3],
                    p[4],
                    p[5],
                    p[6],
                    false,
                );
                self.eq.set(sr, p[9], 0.0, p[8]);
            }
        }
        mix = bounded(mix, 0.0, 1.0);
        let dry_l = if pan > 0.0 { 1.0 - pan } else { 1.0 };
        let dry_r = if pan < 0.0 { 1.0 + pan } else { 1.0 };
        let kind = self.kind;
        let (skip, restart) = self.zero.skip(
            mix == 0.0 && !freeze,
            || Self::tail_estimate(kind, &p),
            sr,
            l.len(),
        );
        if skip {
            for i in 0..l.len() {
                l[i] = clean((l[i] as f64 * dry_l * (1.0 - mix)) as f32);
                r[i] = clean((r[i] as f64 * dry_r * (1.0 - mix)) as f32);
            }
            return;
        }
        if restart {
            self.net.reset();
            self.eq.reset();
            return self.process(l, r, None, None);
        }
        let mut wet_l = [0.0f32; CHUNK];
        let mut wet_r = [0.0f32; CHUNK];
        for (bl, br) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            let count = bl.len();
            self.net
                .process(bl, br, &mut wet_l[..count], &mut wet_r[..count]);
            for i in 0..count {
                let a = bl[i];
                let b = br[i];
                let wl = self.eq.tick(0, wet_l[i]);
                let wr = self.eq.tick(1, wet_r[i]);
                let mid = (wl + wr) * 0.5f32;
                let side = ((wl - wr) as f64 * 0.5 * width) as f32;
                bl[i] = clean(
                    ((if freeze { 0.0 } else { a as f64 * dry_l }) * (1.0 - mix)
                        + (mid + side) as f64 * mix) as f32,
                );
                br[i] = clean(
                    ((if freeze { 0.0 } else { b as f64 * dry_r }) * (1.0 - mix)
                        + (mid - side) as f64 * mix) as f32,
                );
            }
        }
    }
}

pub struct Metaverb {
    core: Core,
    net: Network,
    eq: WetEq,
    zero: ZeroMix,
}

impl Metaverb {
    pub fn new(rate: f64) -> Self {
        Metaverb {
            core: Core::new(rate),
            net: Network::new(rate),
            eq: WetEq::default(),
            zero: ZeroMix::default(),
        }
    }

    /// `Metaverb::tailForParameters`.
    pub fn tail_estimate(values: &[f64; MAX_PARAMETERS]) -> f64 {
        (1.0 + values[0] * 0.09) * 1.9 + 0.5
    }
}

impl Processor for Metaverb {
    core_accessors!(core);

    fn reset(&mut self) {
        self.net.reset();
        self.eq.reset();
        self.zero.reset();
    }

    fn tail(&self) -> f64 {
        20.0
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
        self.net
            .configure(3, p[0], 1.0 + p[0] * 0.09, 28.0, 35.0, 48.0, 0.0, false);
        self.eq.set(sr, p[1], p[2], WET_EQ_OPEN);
        let mix = bounded(p[4] * 0.01, 0.0, 1.0);
        let pan = bounded(p[3] * 0.01, -1.0, 1.0);
        let (skip, restart) = self
            .zero
            .skip(mix == 0.0, || Self::tail_estimate(&p), sr, l.len());
        if skip {
            for i in 0..l.len() {
                l[i] = clean((l[i] as f64 * (1.0 - max(0.0, pan)) * (1.0 - mix)) as f32);
                r[i] = clean((r[i] as f64 * (1.0 + min(0.0, pan)) * (1.0 - mix)) as f32);
            }
            return;
        }
        if restart {
            self.net.reset();
            self.eq.reset();
            return self.process(l, r, None, None);
        }
        let mut wet_l = [0.0f32; CHUNK];
        let mut wet_r = [0.0f32; CHUNK];
        for (bl, br) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            let count = bl.len();
            self.net
                .process(bl, br, &mut wet_l[..count], &mut wet_r[..count]);
            for i in 0..count {
                let a = bl[i];
                let b = br[i];
                let x = self.eq.tick(0, wet_l[i]);
                let y = self.eq.tick(1, wet_r[i]);
                bl[i] =
                    clean((a as f64 * (1.0 - max(0.0, pan)) * (1.0 - mix) + x as f64 * mix) as f32);
                br[i] =
                    clean((b as f64 * (1.0 + min(0.0, pan)) * (1.0 - mix) + y as f64 * mix) as f32);
            }
        }
    }
}

pub struct Ice {
    core: Core,
    net: Network,
    re: [f64; 12],
    im: [f64; 12],
    co: [f64; 12],
    si: [f64; 12],
    color_state: [f64; 2],
    bank: Memo,
    zero: ZeroMix,
}

impl Ice {
    pub fn new(rate: f64) -> Self {
        Ice {
            core: Core::new(rate),
            net: Network::new(rate),
            re: [0.0; 12],
            im: [0.0; 12],
            co: [0.0; 12],
            si: [0.0; 12],
            color_state: [0.0; 2],
            bank: Memo::default(),
            zero: ZeroMix::default(),
        }
    }

    /// `Ice::tailForParameters`.
    pub fn tail_estimate(values: &[f64; MAX_PARAMETERS]) -> f64 {
        let ice = bounded(values[1] * 0.01, 0.0, 1.0);
        let size = bounded(values[2] * 0.01, 0.0, 1.0);
        // Finite decay measured against offline NI reference exports; no forced infinite sustain.
        let resonance = 0.12 + ice * (0.22 + size * 1.7);
        let space = 0.35 + size * 2.5 + ice * ice * (1.2 + size * 5.0);
        1.0e5f64.ln() * resonance + space * 1.9 + 0.25
    }
}

impl Processor for Ice {
    core_accessors!(core);

    fn reset(&mut self) {
        self.net.reset();
        self.re = [0.0; 12];
        self.im = [0.0; 12];
        self.color_state = [0.0; 2];
        self.bank.reset();
        self.zero.reset();
    }

    fn tail(&self) -> f64 {
        45.0
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
        let ice = bounded(p[1] * 0.01, 0.0, 1.0);
        let size = bounded(p[2] * 0.01, 0.0, 1.0);
        let mix = bounded(p[3] * 0.01, 0.0, 1.0);
        let color = bounded(p[0] * 0.01, 0.0, 1.0);
        let (skip, restart) = self
            .zero
            .skip(mix == 0.0, || Self::tail_estimate(&p), sr, l.len());
        if skip {
            for i in 0..l.len() {
                l[i] = clean((l[i] as f64 * (1.0 - mix)) as f32);
                r[i] = clean((r[i] as f64 * (1.0 - mix)) as f32);
            }
            return;
        }
        if restart {
            self.net.reset();
            self.re = [0.0; 12];
            self.im = [0.0; 12];
            self.color_state = [0.0; 2];
        }
        // Color sweeps the resonant band, rather than merely low-pass filtering a fixed bank.
        // Original resonator placement/voicing calibrated on a generated break fixture.
        let (co, si) = (&mut self.co, &mut self.si);
        self.bank.get([ice, size, color], || {
            let radius = (-1.0 / (sr * (0.12 + ice * (0.22 + size * 1.7)))).exp();
            const RATIOS: [f64; 12] = [
                -1.0, -0.83, -0.64, -0.47, -0.29, -0.13, 0.04, 0.19, 0.38, 0.59, 0.79, 1.0,
            ];
            let base = 65.0 * 130.0f64.powf(color.powf(1.1));
            let width = 0.35 - 0.23 * color;
            for k in 0..12 {
                let w = 2.0 * PI * min(sr * 0.42, base * (1.0 + width * RATIOS[k])) / sr;
                co[k] = w.cos() * radius;
                si[k] = w.sin() * radius;
            }
            0.0
        });
        let damping = pole(900.0 + color * color * 18000.0, sr);
        let bank_mix = ice.powf(4.0);
        let bank_gain = 0.12 * (0.4 + 0.6 * (color / 0.75).powf(1.6)) / (0.75 + 0.5 * size);
        let space_gain = 0.82 * (1.0 - 0.93 * ice.powf(1.3)) / (0.75 + 0.5 * size);
        self.net.configure(
            1,
            p[2],
            0.35 + size * 2.5 + ice * ice * (1.2 + size * 5.0),
            70.0 - color * 60.0,
            6.0,
            75.0,
            0.0,
            false,
        );
        const SIGN: [f64; 12] = [
            0.22, -0.22, 0.22, 0.22, -0.22, 0.22, 0.22, -0.22, 0.22, 0.22, -0.22, 0.22,
        ];
        let mut net_l = [0.0f32; CHUNK];
        let mut net_r = [0.0f32; CHUNK];
        let mut wet_l = [0.0f32; CHUNK];
        let mut wet_r = [0.0f32; CHUNK];
        let mut metallic_l = [0.0f64; CHUNK];
        let mut metallic_r = [0.0f64; CHUNK];
        let (co, si) = (self.co, self.si);
        for (bl, br) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            let count = bl.len();
            let (mut re, mut im) = (self.re, self.im);
            for i in 0..count {
                let a = bl[i] as f64;
                let b = br[i] as f64;
                // Twelve complex one-pole resonators, structure of arrays.
                for k in 0..12 {
                    let input = (if k & 1 != 0 { b } else { a }) * 0.045;
                    let nr = co[k] * re[k] - si[k] * im[k] + input;
                    let ni = si[k] * re[k] + co[k] * im[k];
                    // Amplitude-dependent damping bounds high-Q resonances under loud excitation.
                    let limit = 1.0 / (1.0 + 0.0005 * (nr * nr + ni * ni));
                    re[k] = nr * limit;
                    im[k] = ni * limit;
                }
                let (mut m0, mut m1) = (0.0f64, 0.0f64);
                let mut k = 0;
                while k < 12 {
                    m0 += re[k] * SIGN[k];
                    m1 += re[k + 1] * SIGN[k + 1];
                    k += 2;
                }
                metallic_l[i] = m0;
                metallic_r[i] = m1;
                net_l[i] = (a * 0.55 + m0 * ice * 0.13) as f32;
                net_r[i] = (b * 0.55 + m1 * ice * 0.13) as f32;
            }
            self.re = re;
            self.im = im;
            self.net.process(
                &net_l[..count],
                &net_r[..count],
                &mut wet_l[..count],
                &mut wet_r[..count],
            );
            for i in 0..count {
                let a = bl[i];
                let b = br[i];
                self.color_state[0] += damping
                    * (wet_l[i] as f64 * space_gain + metallic_l[i] * bank_mix * bank_gain
                        - self.color_state[0]);
                self.color_state[1] += damping
                    * (wet_r[i] as f64 * space_gain + metallic_r[i] * bank_mix * bank_gain
                        - self.color_state[1]);
                bl[i] = clean((a as f64 * (1.0 - mix) + self.color_state[0] * mix) as f32);
                br[i] = clean((b as f64 * (1.0 - mix) + self.color_state[1] * mix) as f32);
            }
        }
    }
}

pub struct Reflex {
    core: Core,
    comb: [Line; 6],
    ap: [Line; 4],
    low: [f64; 6],
    time: [f64; 6],
    smooth_rate: f64,
    initialized: bool,
    zero: ZeroMix,
}

impl Reflex {
    pub fn new(rate: f64) -> Self {
        Reflex {
            core: Core::new(rate),
            comb: std::array::from_fn(|_| Line::with_capacity((rate * 0.11) as usize + 8)),
            ap: std::array::from_fn(|_| Line::with_capacity((rate * 0.025) as usize + 8)),
            low: [0.0; 6],
            time: [rate * 0.01; 6],
            smooth_rate: 1.0 - (-1.0 / (0.025 * rate)).exp(),
            initialized: false,
            zero: ZeroMix::default(),
        }
    }

    /// `Reflex::tailForParameters`.
    pub fn tail_estimate(values: &[f64; MAX_PARAMETERS]) -> f64 {
        let size = bounded(values[2] * 0.01, 0.0, 1.0);
        let feedback = 0.72 + size * 0.265;
        0.0101 * (0.5 + size * 7.0) * (1.0 + (1.0e-5f64.ln() / feedback.ln()).ceil()) + 1.0
    }

    fn reset_wet(&mut self) {
        for d in self.comb.iter_mut() {
            d.reset();
        }
        for d in self.ap.iter_mut() {
            d.reset();
        }
        self.low = [0.0; 6];
        self.initialized = false;
    }
}

impl Processor for Reflex {
    core_accessors!(core);

    fn reset(&mut self) {
        self.reset_wet();
        self.zero.reset();
    }

    fn tail(&self) -> f64 {
        60.0
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
        let size = bounded(p[2] * 0.01, 0.0, 1.0);
        let smooth = bounded(p[1] * 0.01, 0.0, 1.0);
        let mix = bounded(p[3] * 0.01, 0.0, 1.0);
        let a = pole(500.0 + p[0] * p[0] * 1.8, sr);
        let fb = 0.72 + size * 0.265;
        let (skip, restart) = self
            .zero
            .skip(mix == 0.0, || Self::tail_estimate(&p), sr, l.len());
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
        const DS: [f64; 6] = [0.0023, 0.0037, 0.0051, 0.0067, 0.0083, 0.0101];
        if !self.initialized {
            for k in 0..6 {
                self.time[k] = DS[k] * (0.5 + size * 7.0) * sr;
            }
            self.initialized = true;
        }
        for i in 0..l.len() {
            let x = l[i];
            let y = r[i];
            let mut wet = [0.0f32; 2];
            for k in 0..6 {
                self.time[k] += self.smooth_rate * (DS[k] * (0.5 + size * 7.0) * sr - self.time[k]);
                let v = self.comb[k].read(self.time[k]);
                self.low[k] += a * (v as f64 - self.low[k]);
                self.comb[k].write(soft(
                    ((if k & 1 != 0 { y } else { x }) as f64 * 0.35 + self.low[k] * fb) as f32,
                ));
                wet[k & 1] += v * 0.45f32;
            }
            for ch in 0..2 {
                let mut diffuse =
                    self.ap[ch * 2].allpass(wet[ch], (0.0071 + ch as f64 * 0.0013) * sr, 0.72f32);
                diffuse = self.ap[ch * 2 + 1].allpass(
                    diffuse,
                    (0.0127 + ch as f64 * 0.0017) * sr,
                    0.62f32,
                );
                wet[ch] = (wet[ch] as f64 * (1.0 - smooth) + diffuse as f64 * smooth) as f32;
            }
            l[i] = clean((x as f64 * (1.0 - mix) + wet[0] as f64 * mix) as f32);
            r[i] = clean((y as f64 * (1.0 - mix) + wet[1] as f64 * mix) as f32);
        }
    }
}

const MODES: &[Label] = &[
    Label::new(c"Room"),
    Label::new(c"Hall"),
    Label::new(c"Plate"),
];
const ROOMS: &[Label] = &[
    Label::new(c"General"),
    Label::new(c"Bright"),
    Label::new(c"Guitar"),
    Label::new(c"Shatter"),
];
static ICE_PARAMS: [P; 4] = [
    P::new(c"color", c"Color", c"%", 0.0, 100.0, 65.0, 0),
    P::new(c"ice", c"Ice", c"%", 0.0, 100.0, 35.0, 0),
    P::new(c"size", c"Size", c"%", 0.0, 100.0, 50.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 30.0, 0),
];
static META_PARAMS: [P; 5] = [
    P::new(c"size", c"Size", c"%", 0.0, 100.0, 45.0, 0),
    P::new(c"low", c"Low", c"dB", -24.0, 12.0, 0.0, 0),
    P::new(c"high", c"High", c"dB", -24.0, 12.0, 0.0, 0),
    P::new(c"pan", c"Dry Pan", c"%", -100.0, 100.0, 0.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 30.0, 0),
];
static REFLEX_PARAMS: [P; 4] = [
    P::new(c"color", c"Color", c"%", 0.0, 100.0, 60.0, 0),
    P::new(c"smooth", c"Smooth", c"%", 0.0, 100.0, 45.0, 0),
    P::new(c"size", c"Size", c"%", 0.0, 100.0, 25.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 30.0, 0),
];
static LEGACY_PARAMS: [P; 8] = [
    P::choice(c"room", c"Room", c"", 0.0, 3.0, 0.0, INTEGER, ROOMS),
    P::new(c"size", c"Size", c"%", 0.0, 100.0, 40.0, 0),
    P::new(c"low", c"Low", c"dB", -24.0, 12.0, 0.0, 0),
    P::new(c"high", c"High", c"dB", -24.0, 12.0, 0.0, 0),
    P::new(c"pan", c"Dry Pan", c"%", -100.0, 100.0, 0.0, 0),
    P::new(c"stereo", c"Stereo", c"%", 0.0, 150.0, 100.0, 0),
    P::new(c"freeze", c"Freeze", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 30.0, 0),
];
static REVERB_PARAMS: [P; 10] = [
    P::choice(c"mode", c"Mode", c"", 0.0, 2.0, 0.0, INTEGER, MODES),
    P::new(c"time", c"Reverb Time", c"s", 0.5, 20.2, 1.0, LOG),
    P::new(c"size", c"Size", c"%", 0.0, 100.0, 20.0, 0),
    P::new(c"damping", c"Damping", c"%", 0.0, 100.0, 50.0, 0),
    P::new(c"modulation", c"Modulation", c"%", 0.0, 100.0, 50.0, 0),
    P::new(
        c"diffusion",
        c"Diffusion / Softness",
        c"%",
        0.0,
        100.0,
        80.0,
        0,
    ),
    P::new(c"predelay", c"Pre Delay", c"ms", 0.0, 250.0, 0.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 50.0, 0),
    P::new(
        c"highcut",
        c"High Cut",
        c"Hz",
        2000.0,
        20000.0,
        20000.0,
        LOG,
    ),
    P::new(c"lowshelf", c"Low Shelf", c"dB", -12.0, 0.0, 0.0, 0),
];
static PLATE_PARAMS: [P; 6] = [
    P::new(c"decay", c"Decay", c"s", 0.2, 20.2, 2.0, LOG),
    P::new(c"lowshelf", c"Low Shelf", c"dB", -24.0, 6.0, 0.0, 0),
    P::new(c"highdamp", c"High Damp", c"%", 0.0, 100.0, 45.0, 0),
    P::new(c"predelay", c"Pre Delay", c"ms", 0.0, 250.0, 0.0, 0),
    P::new(
        c"highcut",
        c"High Cut",
        c"Hz",
        2000.0,
        20000.0,
        16000.0,
        LOG,
    ),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 35.0, 0),
];

static DESCRIPTORS: [Descriptor; 6] = [
    Descriptor::new(
        c"ice",
        c"Ice",
        c"Spatial",
        0,
        &ICE_PARAMS,
        |sr, _| Box::new(Ice::new(sr)),
        |_, v, _| Ice::tail_estimate(v),
    ),
    Descriptor::new(
        c"metaverb",
        c"Metaverb",
        c"Spatial",
        0,
        &META_PARAMS,
        |sr, _| Box::new(Metaverb::new(sr)),
        |_, v, _| Metaverb::tail_estimate(v),
    ),
    Descriptor::new(
        c"reflex",
        c"Reflex",
        c"Spatial",
        0,
        &REFLEX_PARAMS,
        |sr, _| Box::new(Reflex::new(sr)),
        |_, v, _| Reflex::tail_estimate(v),
    ),
    Descriptor::new(
        c"reverblegacy",
        c"Reverb Legacy",
        c"Reverb",
        0,
        &LEGACY_PARAMS,
        |sr, _| Box::new(Reverb::new(sr, ReverbKind::Legacy)),
        |_, v, _| Reverb::tail_estimate(ReverbKind::Legacy, v),
    ),
    Descriptor::new(
        c"reverb",
        c"Reverb",
        c"Reverb",
        0,
        &REVERB_PARAMS,
        |sr, _| Box::new(Reverb::new(sr, ReverbKind::Algorithmic)),
        |_, v, _| Reverb::tail_estimate(ReverbKind::Algorithmic, v),
    ),
    Descriptor::new(
        c"plate",
        c"Plate Reverb",
        c"Reverb",
        0,
        &PLATE_PARAMS,
        |sr, _| Box::new(Reverb::new(sr, ReverbKind::Plate)),
        |_, v, _| Reverb::tail_estimate(ReverbKind::Plate, v),
    ),
];

/// `stfx::spatialDescriptors`.
pub fn descriptors() -> &'static [Descriptor] {
    &DESCRIPTORS
}

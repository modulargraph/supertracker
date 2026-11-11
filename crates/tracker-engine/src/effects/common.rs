//! Shared DSP building blocks for the built-in effects.
//!
//! Ported from the C++ `BuiltinEffectsCommon.hpp` (namespace `stfx`): math helpers,
//! the fast transcendental approximations, memoization, the processor interface, parameter
//! metadata, biquads, delay lines and parameter smoothing.
//!
//! The translation is deliberately literal. Floating-point expressions keep the C++ operand
//! order and association, `min`/`max`/`clamp` reproduce `std::min`/`std::max` exactly (including
//! which operand is returned for NaN and signed zero), and `f32`/`f64` follow the C++ `float`/
//! `double` types, so that on the same machine and libm the output is bit-identical to the C++
//! implementation.

use std::ffi::CStr;

/// `stfx::pi`.
pub const PI: f64 = 3.1415926535897932384626433832795; // the same double as std::f64::consts::PI

/// Parameter flag: the normalized value maps logarithmically onto the range.
pub const PARAMETER_LOG: u32 = 1;
/// Parameter flag: the physical value is rounded to an integer.
pub const PARAMETER_INTEGER: u32 = 2;
/// Parameter flag: an on/off switch (0 or 1).
pub const PARAMETER_TOGGLE: u32 = 4;
/// Effect flag: the effect has a detector that accepts an external key.
pub const EFFECT_SIDECHAIN: u32 = 1;

/// One entry of an enum parameter's label table.
pub struct Label(&'static CStr);

impl Label {
    pub const fn new(text: &'static CStr) -> Self {
        Label(text)
    }

    pub const fn text(&self) -> &'static CStr {
        self.0
    }
}

/// One parameter of an effect or drum model. All ranges and defaults are in displayed physical
/// units. The texts are C strings because that is how the catalogue tables are written; they
/// are plain ASCII/UTF-8 (`to_str` never fails on them).
pub struct ParameterInfo {
    id: &'static CStr,
    name: &'static CStr,
    unit: &'static CStr,
    pub minimum: f64,
    pub maximum: f64,
    pub default_value: f64,
    pub flags: u32,
    labels: &'static [Label],
}

impl ParameterInfo {
    /// A continuous, integer or toggle parameter without value labels.
    pub const fn new(
        id: &'static CStr,
        name: &'static CStr,
        unit: &'static CStr,
        minimum: f64,
        maximum: f64,
        default_value: f64,
        flags: u32,
    ) -> Self {
        ParameterInfo { id, name, unit, minimum, maximum, default_value, flags, labels: &[] }
    }

    /// A parameter whose integer values have display labels.
    #[allow(clippy::too_many_arguments)]
    pub const fn choice(
        id: &'static CStr,
        name: &'static CStr,
        unit: &'static CStr,
        minimum: f64,
        maximum: f64,
        default_value: f64,
        flags: u32,
        labels: &'static [Label],
    ) -> Self {
        ParameterInfo { id, name, unit, minimum, maximum, default_value, flags, labels }
    }

    /// The persistent parameter identifier.
    pub fn id(&self) -> &'static CStr {
        self.id
    }

    /// The display name.
    pub fn name(&self) -> &'static CStr {
        self.name
    }

    /// The display unit (may be empty).
    pub fn unit(&self) -> &'static CStr {
        self.unit
    }

    /// Number of value labels (0 for parameters without labels).
    pub fn label_count(&self) -> u32 {
        self.labels.len() as u32
    }

    /// The display label of integer value `index`, if the parameter has labels.
    pub fn label(&self, index: u32) -> Option<&'static CStr> {
        self.labels.get(index as usize).map(Label::text)
    }

    /// Normalized [0, 1] to the displayed physical value: clamped, logarithmic for
    /// [`PARAMETER_LOG`] ranges, rounded for integers and toggles. Non-finite input maps as 0.
    pub fn physical(&self, normalized: f64) -> f64 {
        let n = if normalized.is_finite() { clamp(normalized, 0.0, 1.0) } else { 0.0 };
        let mut v = if (self.flags & PARAMETER_LOG) != 0 && self.minimum > 0.0 {
            self.minimum * (self.maximum / self.minimum).powf(n)
        } else {
            self.minimum + (self.maximum - self.minimum) * n
        };
        if (self.flags & (PARAMETER_INTEGER | PARAMETER_TOGGLE)) != 0 {
            v = v.round();
        }
        clamp(v, self.minimum, self.maximum)
    }

    /// Displayed physical value to normalized [0, 1]. Non-finite values map the default.
    pub fn normalized(&self, physical: f64) -> f64 {
        let mut v = clamp(if physical.is_finite() { physical } else { self.default_value }, self.minimum, self.maximum);
        if (self.flags & (PARAMETER_INTEGER | PARAMETER_TOGGLE)) != 0 {
            v = v.round();
        }
        if self.maximum == self.minimum {
            return 0.0;
        }
        clamp(
            if (self.flags & PARAMETER_LOG) != 0 && self.minimum > 0.0 {
                (v / self.minimum).ln() / (self.maximum / self.minimum).ln()
            } else {
                (v - self.minimum) / (self.maximum - self.minimum)
            },
            0.0,
            1.0,
        )
    }

    /// The default as a normalized value.
    pub fn default_normalized(&self) -> f64 {
        self.normalized(self.default_value)
    }

    /// [`ParameterInfo::physical`] for values that may come from outside: a non-finite
    /// normalized value gives the default instead of the minimum.
    pub fn physical_or_default(&self, normalized: f64) -> f64 {
        self.physical(if normalized.is_finite() { normalized } else { self.default_normalized() })
    }
}

/// Catalogue entry of an effect or drum model.
pub struct EffectInfo {
    id: &'static CStr,
    name: &'static CStr,
    category: &'static CStr,
    pub parameter_count: u32,
    pub flags: u32,
}

impl EffectInfo {
    pub const fn new(
        id: &'static CStr,
        name: &'static CStr,
        category: &'static CStr,
        parameter_count: u32,
        flags: u32,
    ) -> Self {
        EffectInfo { id, name, category, parameter_count, flags }
    }

    /// The persistent effect identifier.
    pub fn id(&self) -> &'static CStr {
        self.id
    }

    /// The display name.
    pub fn name(&self) -> &'static CStr {
        self.name
    }

    /// The catalogue category.
    pub fn category(&self) -> &'static CStr {
        self.category
    }
}

/// Number of parameter slots every processor carries (`std::array<double,64> p`).
pub const MAX_PARAMETERS: usize = 64;

/// Pure control-side tail estimate: `(sample rate, physical parameter targets, tempo) -> seconds`.
/// This is `Processor::tailForParameters` without the processor, so the control thread can call
/// it while the render thread owns the processor.
pub type TailFn = fn(f64, &[f64; MAX_PARAMETERS], f64) -> f64;

/// `stfx::Descriptor`: catalogue entry plus the processor factory.
pub struct Descriptor {
    pub info: EffectInfo,
    pub params: &'static [ParameterInfo],
    /// Builds a processor for `(sample rate, maximum block size)`. Allocates.
    pub create: fn(f64, u32) -> Box<dyn Processor>,
    /// The same estimate as the created processor's `tail_for_parameters`.
    pub tail: TailFn,
}

impl Descriptor {
    pub const fn new(
        id: &'static CStr,
        name: &'static CStr,
        category: &'static CStr,
        flags: u32,
        params: &'static [ParameterInfo],
        create: fn(f64, u32) -> Box<dyn Processor>,
        tail: TailFn,
    ) -> Self {
        Descriptor {
            info: EffectInfo::new(id, name, category, params.len() as u32, flags),
            params,
            create,
            tail,
        }
    }
}

/// `std::max(a, b)`: returns `a` unless `a < b` (so `a` for NaN operands and equal zeros).
#[inline(always)]
pub fn max(a: f64, b: f64) -> f64 {
    if a < b {
        b
    } else {
        a
    }
}

/// `std::min(a, b)`: returns `a` unless `b < a`.
#[inline(always)]
pub fn min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// `stfx::clamp`: `std::max(lo, std::min(hi, x))`. A NaN `x` yields `hi`.
#[inline(always)]
pub fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    max(lo, min(hi, x))
}

/// `std::pow(2., x)`. clang lowers a power of the constant base two to `exp2`, whose result
/// differs from `pow(2, x)` in the last bit for some arguments, so the call is spelled out.
#[inline(always)]
pub fn pow2(x: f64) -> f64 {
    x.exp2()
}

/// `stfx::dbGain`.
#[inline]
pub fn db_gain(db: f64) -> f64 {
    10.0f64.powf(db / 20.0)
}

/// `stfx::gainDB`.
#[inline(always)]
pub fn gain_db(x: f64) -> f64 {
    20.0 * max(x, 1.0e-12).log10()
}

/// `stfx::pole`: one-pole smoothing coefficient for a time constant in milliseconds.
#[inline]
pub fn pole(milliseconds: f64, sr: f64) -> f64 {
    (-1.0 / (max(milliseconds, 0.001) * 0.001 * sr)).exp()
}

/// `stfx::clean`: nonfinite and tiny values become 0, everything else is clamped to +-1e6.
#[inline(always)]
pub fn clean(x: f64) -> f64 {
    if x.is_finite() && x.abs() > 1e-30 {
        clamp(x, -1e6, 1e6)
    } else {
        0.0
    }
}

/// `condition ? a : b` for per-sample choices that depend on the audio (an envelope follower
/// picking its attack or release coefficient). Same value as an `if`; the hint only asks for
/// a conditional select instead of a branch that mispredicts on noisy material.
#[inline(always)]
pub fn select(condition: bool, a: f64, b: f64) -> f64 {
    std::hint::select_unpredictable(condition, a, b)
}

/// `stfx::mix`: linear crossfade from `dry` to `wet`.
#[inline(always)]
pub fn mix(dry: f64, wet: f64, amount: f64) -> f64 {
    dry + (wet - dry) * amount
}

/// `stfx::wrap`: fractional part in [0, 1).
#[inline(always)]
pub fn wrap(p: f64) -> f64 {
    p - p.floor()
}

/* Branch-free, vectorizable approximations for render loops. Errors are far
below audibility (and below float output precision), so replacing libm
calls with these is sound-equivalent:
  fast_exp     relative error < 3e-13 over [-700, 700]
  fast_tanh    absolute error < 1e-12
  fast_sin_2pi sin(2*pi*phase), absolute error < 1e-11 for any finite phase */

/// `stfx::fastExp`.
#[inline(always)]
pub fn fast_exp(x: f64) -> f64 {
    let x = max(-700.0, min(700.0, x));
    let n = (x * 1.4426950408889634).round_ties_even();
    let r = (x - n * 0.693147180369123816490) - n * 1.90821492927058770002e-10;
    // Degree-11 Taylor polynomial, Estrin evaluation (short dependency chain).
    let r2 = r * r;
    let r4 = r2 * r2;
    let r8 = r4 * r4;
    let q0 = 1.0 + r;
    let q1 = 0.5 + r * (1.0 / 6.0);
    let q2 = 1.0 / 24.0 + r * (1.0 / 120.0);
    let q3 = 1.0 / 720.0 + r * (1.0 / 5040.0);
    let q4 = 1.0 / 40320.0 + r * (1.0 / 362880.0);
    let q5 = 1.0 / 3628800.0 + r * (1.0 / 39916800.0);
    let y = (q0 + q1 * r2) + (q2 + q3 * r2) * r4 + (q4 + q5 * r2) * r8;
    // n is integral and within [-1010, 1010] after the clamp above.
    let bits = ((n as i64).wrapping_add(1023)) << 52;
    y * f64::from_bits(bits as u64)
}

/// `stfx::fastTanh`.
#[inline(always)]
pub fn fast_tanh(x: f64) -> f64 {
    let e = fast_exp(-2.0 * min(x.abs(), 40.0));
    ((1.0 - e) / (1.0 + e)).copysign(x)
}

/// `stfx::fastSin2pi`: sin(2*pi*phase).
#[inline(always)]
pub fn fast_sin_2pi(phase: f64) -> f64 {
    let mut p = phase - phase.round_ties_even(); // [-0.5, 0.5]
    let h = 0.5f64.copysign(p);
    p = if p.abs() > 0.25 { h - p } else { p }; // [-0.25, 0.25]
    let x = 2.0 * PI * p;
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let a = -1.0 / 6.0 + x2 * (1.0 / 120.0);
    let b = -1.0 / 5040.0 + x2 * (1.0 / 362880.0);
    let c = -1.0 / 39916800.0 + x2 * (1.0 / 6227020800.0);
    let y = a + b * x4 + (c - x4 * (1.0 / 1307674368000.0)) * x8;
    x + x * x2 * y
}

/// `stfx::fastCos2pi`: cos(2*pi*phase).
#[inline(always)]
pub fn fast_cos_2pi(phase: f64) -> f64 {
    fast_sin_2pi(phase + 0.25)
}

/// `stfx::fastDbGain`: `db_gain` for per-sample paths, no libm call
/// (relative error < 3e-13; exactly 1 at 0 dB).
#[inline(always)]
pub fn fast_db_gain(db: f64) -> f64 {
    fast_exp(db * 0.11512925464970229)
}

/// `stfx::fastTanhf`: single-precision tanh for vectorized shapers: |error| < 2e-7 (float
/// resolution near 1), exactly 0 at 0 and odd-symmetric.
#[inline(always)]
pub fn fast_tanh_f32(x: f32) -> f32 {
    let ax = x.abs().min(9.0);
    let y = -2.0 * ax; // [-18, 0]
    let n = (y * 1.44269504f32).round_ties_even();
    let r = (y - n * 0.693359375f32) + n * 2.12194440e-4f32; // Cody-Waite ln2 split
    let r2 = r * r;
    let p = (1.0 + r)
        + r2 * ((0.5 + r * (1.0f32 / 6.0))
            + r2 * ((1.0f32 / 24.0 + r * (1.0f32 / 120.0))
                + r2 * (1.0f32 / 720.0 + r * (1.0f32 / 5040.0))));
    // n is integral and within [-26, 0].
    let bits = ((n as i32).wrapping_add(127)) << 23;
    let e = p * f32::from_bits(bits as u32);
    ((1.0 - e) / (1.0 + e)).copysign(x)
}

/// `stfx::Memo`: exact memoization of a pure per-sample derivation (dB->gain, pole, tan...):
/// while its inputs are unchanged (settled smoothing), reuse the result. Results are
/// bit-identical. Unused key slots are 0, as with the C++ default arguments.
#[derive(Clone, Copy, Default)]
pub struct Memo {
    key: [f64; 6],
    value: f64,
    valid: bool,
}

impl Memo {
    /// Returns the cached value, recomputing it with `f` when any key differs (a NaN key
    /// never matches, so it recomputes every time).
    #[inline(always)]
    pub fn get<const N: usize>(&mut self, keys: [f64; N], f: impl FnOnce() -> f64) -> f64 {
        let mut key = [0.0; 6];
        let mut i = 0;
        while i < N && i < 6 {
            key[i] = keys[i];
            i += 1;
        }
        if !self.valid
            || !(key[0] == self.key[0])
            || !(key[1] == self.key[1])
            || !(key[2] == self.key[2])
            || !(key[3] == self.key[3])
            || !(key[4] == self.key[4])
            || !(key[5] == self.key[5])
        {
            self.key = key;
            self.value = f();
            self.valid = true;
        }
        self.value
    }

    pub fn reset(&mut self) {
        self.valid = false;
    }
}

/// The data members of the C++ `stfx::Processor` base class.
#[derive(Clone)]
pub struct Core {
    /// Sample rate in Hz; immutable after construction.
    pub sr: f64,
    /// Project tempo in BPM.
    pub tempo: f64,
    /// Physical parameter values, indexed like the effect's parameter table.
    pub p: [f64; MAX_PARAMETERS],
}

impl Core {
    pub fn new(sample_rate: f64) -> Self {
        Core {
            sr: sample_rate,
            tempo: 120.0,
            p: [0.0; MAX_PARAMETERS],
        }
    }
}

/// `stfx::Processor`: one effect instance. `process` and `reset` belong to the render owner
/// and must not allocate, lock or block.
pub trait Processor: Send {
    fn core(&self) -> &Core;
    fn core_mut(&mut self) -> &mut Core;
    /// In-place stereo render of `min(left.len(), right.len())` frames. A sidechain slice
    /// shorter than that is ignored. With only one sidechain channel it is used for both.
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        sidechain_left: Option<&[f32]>,
        sidechain_right: Option<&[f32]>,
    );
    fn reset(&mut self);
    fn latency(&self) -> u32 {
        0
    }
    /// Immutable conservative maximum in seconds, independent of p and tempo.
    /// Sustaining/freeze processors use an explicit finite export cap (120 s).
    fn tail(&self) -> f64 {
        0.0
    }
    /// Pure control-side estimate from explicit physical targets. Implementations
    /// must not read p or any mutable render state; sr is immutable. This lets
    /// long feedback delays report their actual target decay before rendering.
    fn tail_for_parameters(&self, _values: &[f64; MAX_PARAMETERS], _tempo: f64) -> f64 {
        self.tail()
    }
    /// Whether the host may stop calling process() once the input has been
    /// silent for longer than the tail and the output has decayed (silence
    /// gating). Processors whose state must keep advancing on silence (capture
    /// history) return false; sustaining modes are excluded by their 120 s cap.
    fn gateable(&self) -> bool {
        true
    }
}

/// Implements the `core`/`core_mut` accessors for a processor whose `Core` lives at `self.$field`.
macro_rules! core_accessors {
    ($($field:tt).+) => {
        fn core(&self) -> &$crate::effects::common::Core {
            &self.$($field).+
        }
        fn core_mut(&mut self) -> &mut $crate::effects::common::Core {
            &mut self.$($field).+
        }
    };
}
pub(crate) use core_accessors;

/// The render buffers of one `process` call, all with the same frame count.
pub type FrameBuffers<'a, 'b> = (
    &'a mut [f32],
    &'a mut [f32],
    Option<&'b [f32]>,
    Option<&'b [f32]>,
);

/// Narrows the render buffers to a common frame count: `left`/`right` are cut to the shorter
/// of the two, and a sidechain channel is kept only if it covers that many frames.
#[inline(always)]
pub fn frame_buffers<'a, 'b>(
    left: &'a mut [f32],
    right: &'a mut [f32],
    sidechain_left: Option<&'b [f32]>,
    sidechain_right: Option<&'b [f32]>,
) -> FrameBuffers<'a, 'b> {
    let n = left.len().min(right.len());
    (
        &mut left[..n],
        &mut right[..n],
        sidechain_left.and_then(|s| s.get(..n)),
        sidechain_right.and_then(|s| s.get(..n)),
    )
}

/// `stfx::ParameterCache`: recomputes derived control targets only when the raw parameters
/// change (keeps pow/exp10 out of the steady-state render path).
#[derive(Clone)]
pub struct ParameterCache<const N: usize> {
    last: [f64; N],
    valid: bool,
}

impl<const N: usize> Default for ParameterCache<N> {
    fn default() -> Self {
        ParameterCache {
            last: [0.0; N],
            valid: false,
        }
    }
}

impl<const N: usize> ParameterCache<N> {
    pub fn changed(&mut self, core: &Core) -> bool {
        let mut diff = !self.valid;
        for (last, value) in self.last.iter_mut().zip(core.p.iter()) {
            if !(*value == *last) {
                *last = *value;
                diff = true;
            }
        }
        self.valid = true;
        diff
    }

    pub fn reset(&mut self) {
        self.valid = false;
    }
}

/// `Biquad::Type`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BiquadType {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peak,
    LowShelf,
    HighShelf,
}

/// `stfx::Biquad`: DF2T biquad, double state; normalized RBJ coefficients. Coefficients may be
/// set per block; zero-gain shelves/peaks use exact identity coefficients.
#[derive(Clone, Copy)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    pub z1: f64,
    pub z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biquad {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline(always)]
    pub fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = clean(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = clean(self.b2 * x - self.a2 * y);
        clean(y)
    }

    pub fn set(&mut self, kind: BiquadType, hz: f64, q: f64, gain: f64, sr: f64) {
        use BiquadType::*;
        if matches!(kind, Peak | LowShelf | HighShelf) && gain == 0.0 {
            self.b0 = 1.0;
            self.b1 = 0.0;
            self.b2 = 0.0;
            self.a1 = 0.0;
            self.a2 = 0.0;
            return;
        }
        let w = 2.0 * PI * clamp(hz, 5.0, sr * 0.45) / sr;
        let c = w.cos();
        let s = w.sin();
        let alpha = s / (2.0 * clamp(q, 0.1, 30.0));
        let a = 10.0f64.powf(gain / 40.0);
        let mut aa0 = 1.0 + alpha;
        let mut aa1 = -2.0 * c;
        let mut aa2 = 1.0 - alpha;
        let (bb0, bb1, bb2);
        match kind {
            LowPass => {
                bb0 = (1.0 - c) / 2.0;
                bb1 = 1.0 - c;
                bb2 = bb0;
            }
            HighPass => {
                bb0 = (1.0 + c) / 2.0;
                bb1 = -(1.0 + c);
                bb2 = bb0;
            }
            BandPass => {
                bb0 = alpha;
                bb1 = 0.0;
                bb2 = -alpha;
            }
            Notch => {
                bb0 = 1.0;
                bb1 = -2.0 * c;
                bb2 = 1.0;
            }
            Peak => {
                bb0 = 1.0 + alpha * a;
                bb1 = -2.0 * c;
                bb2 = 1.0 - alpha * a;
                aa0 = 1.0 + alpha / a;
                aa2 = 1.0 - alpha / a;
            }
            LowShelf => {
                let t = 2.0 * a.sqrt() * alpha;
                bb0 = a * ((a + 1.0) - (a - 1.0) * c + t);
                bb1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * c);
                bb2 = a * ((a + 1.0) - (a - 1.0) * c - t);
                aa0 = (a + 1.0) + (a - 1.0) * c + t;
                aa1 = -2.0 * ((a - 1.0) + (a + 1.0) * c);
                aa2 = (a + 1.0) + (a - 1.0) * c - t;
            }
            HighShelf => {
                let t = 2.0 * a.sqrt() * alpha;
                bb0 = a * ((a + 1.0) + (a - 1.0) * c + t);
                bb1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * c);
                bb2 = a * ((a + 1.0) + (a - 1.0) * c - t);
                aa0 = (a + 1.0) - (a - 1.0) * c + t;
                aa1 = 2.0 * ((a - 1.0) - (a + 1.0) * c);
                aa2 = (a + 1.0) - (a - 1.0) * c - t;
            }
        }
        self.b0 = bb0 / aa0;
        self.b1 = bb1 / aa0;
        self.b2 = bb2 / aa0;
        self.a1 = aa1 / aa0;
        self.a2 = aa2 / aa0;
    }
}

/// `stfx::DelayLine`: circular buffer of doubles with linear-interpolated reads.
#[derive(Clone)]
pub struct DelayLine {
    data: Vec<f64>,
    write: usize,
}

impl DelayLine {
    /// Allocates `max(capacity, 4)` samples.
    pub fn new(capacity: usize) -> Self {
        DelayLine {
            data: vec![0.0; capacity.max(4)],
            write: 0,
        }
    }

    /// Number of samples in the buffer (`data.size()`); never less than 4.
    #[inline(always)]
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn reset(&mut self) {
        self.data.fill(0.0);
        self.write = 0;
    }

    #[inline(always)]
    pub fn push(&mut self, x: f64) {
        // `write` is always a valid index: it starts at 0 and wraps at len (>= 4).
        if let Some(slot) = self.data.get_mut(self.write) {
            *slot = clean(x);
        }
        self.write += 1;
        if self.write == self.data.len() {
            self.write = 0;
        }
    }

    /// The sample `samples` frames ago (clamped to [1, len-2]), linearly interpolated.
    #[inline(always)]
    pub fn read(&self, samples: f64) -> f64 {
        let size = self.data.len();
        let delay = clamp(samples, 1.0, (size - 2) as f64);
        let whole = delay as usize;
        let fraction = delay - whole as f64;
        // write+size-whole lies in [write+2, write+size-1] < 2*size: one conditional subtract replaces the modulo.
        let mut a = self.write + size - whole;
        if a >= size {
            a -= size;
        }
        let b = if a != 0 { a - 1 } else { size - 1 };
        let (xa, xb) = (self.data[a], self.data[b]);
        xa + (xb - xa) * fraction
    }
}

/// A feedback comb filter with a one-pole low-pass (the damping) in its feedback path. With
/// [`Allpass`], what the mixer crate builds the reverb of its send return from; the line is
/// allocated by whoever makes the filter, and `process` never allocates.
#[derive(Default)]
pub struct Comb {
    pub data: Vec<f32>,
    pub position: usize,
    pub filter: f32,
}

impl Comb {
    #[inline(always)]
    pub fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let Some(slot) = self.data.get_mut(self.position) else { return 0.0 };
        let y = *slot;
        self.filter = y * (1.0 - damp) + self.filter * damp;
        *slot = x + self.filter * feedback;
        self.position += 1;
        if self.position == self.data.len() {
            self.position = 0;
        }
        y
    }
    pub fn clear(&mut self) {
        self.data.fill(0.0);
        self.filter = 0.0;
        self.position = 0;
    }
}

/// A Schroeder all-pass filter with a gain of one half (see [`Comb`]).
#[derive(Default)]
pub struct Allpass {
    pub data: Vec<f32>,
    pub position: usize,
}

impl Allpass {
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let Some(slot) = self.data.get_mut(self.position) else { return 0.0 };
        let b = *slot;
        let y = b - x;
        *slot = x + b * 0.5;
        self.position += 1;
        if self.position == self.data.len() {
            self.position = 0;
        }
        y
    }
    pub fn clear(&mut self) {
        self.data.fill(0.0);
        self.position = 0;
    }
}

/// `stfx::Smoother`: one-pole parameter smoothing.
#[derive(Clone, Copy, Default)]
pub struct Smoother {
    pub value: f64,
    pub initialized: bool,
}

impl Smoother {
    pub fn reset(&mut self) {
        self.value = 0.0;
        self.initialized = false;
    }

    // Converged values snap to the target (within 1e-9 relative), so settled
    // controls are exactly constant and per-sample derivations can be memoized.
    #[inline(always)]
    pub fn tick(&mut self, target: f64, coefficient: f64) -> f64 {
        if !self.initialized {
            self.value = target;
            self.initialized = true;
        } else if self.value != target {
            self.value = target + coefficient * (self.value - target);
            if (self.value - target).abs() <= 1e-12 + 1e-9 * target.abs() {
                self.value = target;
            }
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static MODES: [Label; 3] = [Label::new(c"Off"), Label::new(c"Soft"), Label::new(c"Hard")];
    static MODE: ParameterInfo = ParameterInfo::choice(c"mode", c"Mode", c"", 0.0, 2.0, 1.0, PARAMETER_INTEGER, &MODES);
    static CUTOFF: ParameterInfo = ParameterInfo::new(c"cutoff", c"Cutoff", c"Hz", 20.0, 20000.0, 1000.0, PARAMETER_LOG);

    #[test]
    fn parameter_metadata_and_mapping() {
        assert_eq!((MODE.id(), MODE.name(), MODE.unit()), (c"mode", c"Mode", c""));
        assert_eq!((MODE.label_count(), MODE.label(2), MODE.label(3)), (3, Some(c"Hard"), None));
        assert_eq!((CUTOFF.label_count(), CUTOFF.label(0)), (0, None));
        assert_eq!((MODE.physical(0.4), MODE.physical(0.8), MODE.physical(7.0)), (1.0, 2.0, 2.0));
        assert_eq!((MODE.normalized(1.4), MODE.default_normalized()), (0.5, 0.5));
        // Logarithmic: the geometric middle of the range is the normalized middle.
        assert!((CUTOFF.physical(0.5) - (20.0f64 * 20000.0).sqrt()).abs() < 1e-9);
        assert!((CUTOFF.normalized(CUTOFF.physical(0.3)) - 0.3).abs() < 1e-12);
        // Values from outside: non-finite normalized values are the minimum for `physical` and
        // the default for `physical_or_default`; a non-finite physical value is the default.
        assert_eq!(CUTOFF.physical(f64::NAN), 20.0);
        assert!((CUTOFF.physical_or_default(f64::NAN) - 1000.0).abs() < 1e-9);
        assert_eq!(CUTOFF.normalized(f64::INFINITY), CUTOFF.default_normalized());
        let info = EffectInfo::new(c"eq", c"EQ", c"Filter", 2, EFFECT_SIDECHAIN);
        assert_eq!((info.id(), info.name(), info.category(), info.parameter_count), (c"eq", c"EQ", c"Filter", 2));
    }

    /// The comb and all-pass filters the mixer crate builds its reverb return from, sample for sample.
    #[test]
    fn comb_and_allpass_filters() {
        let impulse = |n: usize| if n == 0 { 1.0f32 } else { 0.0 };
        // A comb of four samples: the input comes back four samples later. What it feeds back is its
        // output through a one-pole low-pass (the damping), times the feedback: the low-pass keeps
        // decaying between the echoes, so each echo is followed by its smear.
        let mut comb = Comb { data: vec![0.0; 4], position: 0, filter: 0.0 };
        let echoes: Vec<f32> = (0..13).map(|n| comb.process(impulse(n), 0.5, 0.25)).collect();
        assert_eq!(echoes, [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.375, 0.09375, 0.0234375, 0.005859375, 0.14208984375]);
        comb.clear();
        assert!((0..8).all(|_| comb.process(0.0, 0.5, 0.25) == 0.0), "a cleared comb is silent");
        // An all-pass of two samples: the inverted input at once, then the stored input, halving.
        let mut allpass = Allpass { data: vec![0.0; 2], position: 0 };
        let ringing: Vec<f32> = (0..8).map(|n| allpass.process(impulse(n))).collect();
        assert_eq!(ringing, [-1.0, 0.0, 1.0, 0.0, 0.5, 0.0, 0.25, 0.0]);
        allpass.clear();
        assert!((0..4).all(|_| allpass.process(0.0) == 0.0), "a cleared all-pass is silent");
        // Without a line there is nothing to process.
        assert_eq!((Comb::default().process(1.0, 0.5, 0.25), Allpass::default().process(1.0)), (0.0, 0.0));
    }
}

//! A built-in effect as a host uses it: [`Effect`], the instance one owner renders, and
//! [`EffectControls`], its parameters for every other thread.
//!
//! An effect is created by its persistent id ([`Effect::create`]). Parameters are set as
//! normalized values (0..=1) into lock-free mailboxes, from the owner or, through a cloned
//! [`EffectControls`], from any thread while the owner renders; the next [`Effect::process`]
//! call maps what changed onto the processor. The handle adds what every insert needs around
//! the raw [`Processor`]: denormal flushing, clean-up of non-finite input and output, bypass,
//! tempo, a control-side tail estimate, and silence gating (after silent input has lasted
//! longer than the tail, the DSP is skipped and the output is exact silence).
//!
//! Threading: `create` and dropping an effect allocate and belong on a non-render thread.
//! `process` and `reset` need `&mut Effect`, so one owner renders at a time; everything on
//! `EffectControls` is atomic and may overlap `process`.

use super::common::{clamp, Descriptor, Processor, MAX_PARAMETERS};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Flush-to-zero / denormals-are-zero for the duration of one render call:
/// recursive filters and decaying tails then never hit slow subnormal
/// arithmetic. The caller's floating-point environment is restored on drop.
struct FlushDenormals {
    #[cfg(target_arch = "aarch64")]
    saved: u64,
    #[cfg(any(
        target_arch = "x86_64",
        all(target_arch = "x86", target_feature = "sse")
    ))]
    saved: u32,
}

impl FlushDenormals {
    #[cfg(target_arch = "aarch64")]
    #[inline(always)]
    fn new() -> Self {
        let saved: u64;
        // SAFETY: reads and writes FPCR only. Setting FZ (bit 24) changes how subnormal results
        // are produced, which is the documented intent; rounding mode and traps are untouched.
        unsafe {
            core::arch::asm!("mrs {}, fpcr", out(reg) saved, options(nomem, nostack, preserves_flags));
            let flush = saved | (1u64 << 24);
            if flush != saved {
                core::arch::asm!("msr fpcr, {}", in(reg) flush, options(nomem, nostack, preserves_flags));
            }
        }
        FlushDenormals { saved }
    }

    #[cfg(any(
        target_arch = "x86_64",
        all(target_arch = "x86", target_feature = "sse")
    ))]
    #[inline(always)]
    fn new() -> Self {
        let mut saved: u32 = 0;
        // SAFETY: stores MXCSR to a local and loads it back with FTZ (bit 15) and DAZ (bit 6)
        // set. Only subnormal handling changes; rounding mode and exception masks are kept.
        unsafe {
            core::arch::asm!("stmxcsr [{}]", in(reg) &mut saved, options(nostack, preserves_flags));
            let flush: u32 = saved | 0x8040;
            core::arch::asm!("ldmxcsr [{}]", in(reg) &flush, options(nostack, readonly, preserves_flags));
        }
        FlushDenormals { saved }
    }

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "x86", target_feature = "sse")
    )))]
    #[inline(always)]
    fn new() -> Self {
        FlushDenormals {}
    }
}

impl Drop for FlushDenormals {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(target_arch = "aarch64")]
        // SAFETY: restores the FPCR value saved in `new`.
        unsafe {
            let now: u64;
            core::arch::asm!("mrs {}, fpcr", out(reg) now, options(nomem, nostack, preserves_flags));
            if now != self.saved {
                core::arch::asm!("msr fpcr, {}", in(reg) self.saved, options(nomem, nostack, preserves_flags));
            }
        }
        #[cfg(any(
            target_arch = "x86_64",
            all(target_arch = "x86", target_feature = "sse")
        ))]
        // SAFETY: restores the MXCSR value saved in `new`.
        unsafe {
            core::arch::asm!("ldmxcsr [{}]", in(reg) &self.saved, options(nostack, readonly, preserves_flags));
        }
    }
}

/// Live input/output clean-up: nonfinite -> 0, |x| <= 1e-30 -> 0, clamp to
/// +-1e6 (`common::clean` semantics), without a libm call per sample.
#[inline(always)]
fn clean_sample(x: f32) -> f32 {
    let a = x.abs();
    // Branch-free so the four-lane loops below compile to packed compares, min/max and a mask.
    let keep = (a > 1e-30) & (a <= f32::MAX);
    // For the kept (finite, nonzero) samples this is fmin(1e6, fmax(-1e6, x)).
    let low = if x < -1e6 { -1e6 } else { x };
    let clamped = if low > 1e6 { 1e6 } else { low };
    f32::from_bits(clamped.to_bits() & (keep as u32).wrapping_neg())
}

/// `clean_sample` on four lanes; `peak` accumulates the largest |result| per lane. The lanes
/// are independent, so the compiler turns this into packed compares, blends and a packed max.
#[inline(always)]
fn clean_lanes(x: [f32; 4], peak: &mut [f32; 4]) -> [f32; 4] {
    let mut y = [0.0f32; 4];
    for j in 0..4 {
        y[j] = clean_sample(x[j]);
        let a = y[j].abs();
        peak[j] = if a > peak[j] { a } else { peak[j] };
    }
    y
}

/// The largest of the four lane peaks (all are non-negative and never NaN).
#[inline(always)]
fn lane_peak(peak: [f32; 4]) -> f32 {
    peak[0].max(peak[1]).max(peak[2].max(peak[3]))
}

/// `clean_sample` over a block in place, four lanes at a time (the same per-sample
/// operations, so the results are identical to a scalar loop); returns the largest |sample|
/// after clean-up.
#[inline(always)]
fn clean_block(block: &mut [f32]) -> f32 {
    let mut peak = [0.0f32; 4];
    let mut lanes = block.chunks_exact_mut(4);
    for chunk in &mut lanes {
        let y = clean_lanes([chunk[0], chunk[1], chunk[2], chunk[3]], &mut peak);
        chunk.copy_from_slice(&y);
    }
    let mut result = lane_peak(peak);
    for sample in lanes.into_remainder() {
        *sample = clean_sample(*sample);
        result = result.max(sample.abs());
    }
    result
}

/// `dst[i] = clean_sample(src[i])` for the frames both hold; returns the largest |dst[i]|.
#[inline(always)]
fn clean_block_from(src: &[f32], dst: &mut [f32]) -> f32 {
    let mut peak = [0.0f32; 4];
    let mut inputs = src.chunks_exact(4);
    let mut lanes = dst.chunks_exact_mut(4);
    for (chunk, input) in (&mut lanes).zip(&mut inputs) {
        chunk.copy_from_slice(&clean_lanes([input[0], input[1], input[2], input[3]], &mut peak));
    }
    let mut result = lane_peak(peak);
    for (out, input) in lanes.into_remainder().iter_mut().zip(inputs.remainder()) {
        *out = clean_sample(*input);
        result = result.max(out.abs());
    }
    result
}

/// Silence gating threshold: -120 dBFS.
const SILENCE: f32 = 1e-6;

/// The key input of one [`Effect::process`] call. Effects without a detector ignore it.
#[derive(Clone, Copy, Debug)]
pub enum Sidechain<'a> {
    /// No external key: the effect listens to its own input.
    None,
    /// One key channel, used for both sides.
    Mono(&'a [f32]),
    /// A stereo key.
    Stereo(&'a [f32], &'a [f32]),
    /// The main input itself, as it is before the effect processes it: what a host passes
    /// when the key is routed from the same signal.
    MainInput,
}

/// What every thread shares of one effect: the parameter mailboxes and switches.
struct Shared {
    descriptor: &'static Descriptor,
    sample_rate: f64,
    /// Normalized parameter mailboxes (f64 bit patterns), written by any thread.
    parameters: [AtomicU64; MAX_PARAMETERS],
    tempo: AtomicU64,
    bypass: AtomicU32,
    // Bumped (release) after every parameter/tempo store; render re-reads the
    // parameter mailboxes only when it moved (acquire), not on every call.
    version: AtomicU32,
    max_frames: u32,
    latency: u32,
}

/// The controls of an [`Effect`] that any thread may use while its owner renders.
#[derive(Clone)]
pub struct EffectControls {
    shared: Arc<Shared>,
}

/// One instance of a built-in effect. See the module documentation.
pub struct Effect {
    controls: EffectControls,
    processor: Box<dyn Processor>,
    seen_version: u32,
    side_l: Vec<f32>,
    side_r: Vec<f32>,
    /// Last normalized value mapped to `processor.p`.
    mapped: [f64; MAX_PARAMETERS],
    // Silence gating: consecutive frames of silent input (main and sidechain),
    // and whether the last processed block's output had decayed below
    // SILENCE. Once silent input has lasted longer than the tail (+ margin),
    // process() is skipped and the output is exact silence.
    silent_frames: u64,
    output_decayed: bool,
    gate_tail: f64,
    gate_frames: u64,
}

impl EffectControls {
    /// The catalogue entry of the effect.
    pub fn descriptor(&self) -> &'static Descriptor {
        self.shared.descriptor
    }

    pub fn sample_rate(&self) -> f64 {
        self.shared.sample_rate
    }

    pub fn parameter_count(&self) -> usize {
        self.shared.descriptor.params.len().min(MAX_PARAMETERS)
    }

    /// Sets parameter `parameter` (catalogue order) to a normalized value, which is clamped to
    /// 0..=1 and snapped to what the parameter can hold (integers, toggles). False for an
    /// unknown parameter or a non-finite value. The render owner applies it at its next block.
    pub fn set_parameter(&self, parameter: u32, normalized: f64) -> bool {
        if parameter as usize >= self.parameter_count() || !normalized.is_finite() {
            return false;
        }
        let info = &self.shared.descriptor.params[parameter as usize];
        self.shared.parameters[parameter as usize].store(info.normalized(info.physical(normalized)).to_bits(), Ordering::Relaxed);
        self.shared.version.fetch_add(1, Ordering::Release);
        true
    }

    /// The normalized value last set (the default after creation); 0 for an unknown parameter.
    pub fn parameter(&self, parameter: u32) -> f64 {
        if (parameter as usize) < self.parameter_count() {
            f64::from_bits(self.shared.parameters[parameter as usize].load(Ordering::Relaxed))
        } else {
            0.0
        }
    }

    /// While bypassed, `process` leaves its buffers untouched.
    pub fn set_bypass(&self, bypass: bool) {
        self.shared.bypass.store(bypass as u32, Ordering::Relaxed);
    }

    pub fn bypassed(&self) -> bool {
        self.shared.bypass.load(Ordering::Relaxed) != 0
    }

    /// The song tempo for tempo-synced effects. Finite tempos are clamped to the tracker tempo
    /// range, 20..999 BPM; anything else is ignored.
    pub fn set_tempo(&self, bpm: f64) {
        if bpm.is_finite() {
            self.shared.tempo.store(clamp(bpm, 20.0, 999.0).to_bits(), Ordering::Relaxed);
            self.shared.version.fetch_add(1, Ordering::Release);
        }
    }

    pub fn tempo(&self) -> f64 {
        f64::from_bits(self.shared.tempo.load(Ordering::Relaxed))
    }

    /// The delay of the effect in frames, fixed at creation.
    pub fn latency(&self) -> u32 {
        self.shared.latency
    }

    /// How long the effect rings after its input ends, in seconds: a conservative estimate
    /// from the parameter targets (or a fixed family maximum), available immediately after
    /// creation or a parameter change without requiring a render.
    pub fn tail_seconds(&self) -> f64 {
        let mut values = [0.0f64; MAX_PARAMETERS];
        for (i, info) in self.shared.descriptor.params.iter().enumerate().take(MAX_PARAMETERS) {
            values[i] = info.physical(f64::from_bits(self.shared.parameters[i].load(Ordering::Relaxed)));
        }
        // The descriptor's estimate is the processor's tail_for_parameters as a pure function, so
        // this never touches the processor the render thread may be running.
        (self.shared.descriptor.tail)(self.shared.sample_rate, &values, self.tempo())
    }
}

impl Effect {
    /// Creates the effect with persistent id `id` for `sample_rate` (8 kHz ... 384 kHz) and
    /// blocks of at most `max_frames` frames (1..=1048576; longer `process` calls are split).
    /// Every parameter starts at its default. `None` for an unknown id or arguments out of range.
    pub fn create(id: &str, sample_rate: f64, max_frames: u32) -> Option<Effect> {
        Effect::from_descriptor(super::find_str(id)?, sample_rate, max_frames)
    }

    /// As [`Effect::create`], for a catalogue entry already at hand.
    pub fn from_descriptor(descriptor: &'static Descriptor, sample_rate: f64, max_frames: u32) -> Option<Effect> {
        if descriptor.params.len() > MAX_PARAMETERS
            || !sample_rate.is_finite()
            || !(8000.0..=384000.0).contains(&sample_rate)
            || !(1..=1048576).contains(&max_frames)
        {
            return None;
        }
        let mut processor = (descriptor.create)(sample_rate, max_frames);
        let latency = processor.latency();
        let parameters: [AtomicU64; MAX_PARAMETERS] = std::array::from_fn(|i| {
            let value = descriptor.params.get(i).map_or(0.0, |p| p.default_normalized());
            AtomicU64::new(value.to_bits())
        });
        for (slot, p) in processor.core_mut().p.iter_mut().zip(descriptor.params.iter()) {
            *slot = p.default_value;
        }
        processor.reset();
        Some(Effect {
            controls: EffectControls {
                shared: Arc::new(Shared {
                    descriptor,
                    sample_rate,
                    parameters,
                    tempo: AtomicU64::new(120.0f64.to_bits()),
                    bypass: AtomicU32::new(0),
                    version: AtomicU32::new(1),
                    max_frames,
                    latency,
                }),
            },
            processor,
            seen_version: 0,
            side_l: vec![0.0; max_frames as usize],
            side_r: vec![0.0; max_frames as usize],
            mapped: [f64::NAN; MAX_PARAMETERS],
            silent_frames: 0,
            output_decayed: false,
            gate_tail: -1.0,
            gate_frames: 0,
        })
    }

    /// The persistent id of the effect.
    pub fn id(&self) -> &'static CStr {
        self.controls.shared.descriptor.info.id()
    }

    /// The controls any thread may use, also while this effect renders on another one.
    pub fn controls(&self) -> EffectControls {
        self.controls.clone()
    }

    /// See [`EffectControls::descriptor`].
    pub fn descriptor(&self) -> &'static Descriptor {
        self.controls.descriptor()
    }

    /// See [`EffectControls::set_parameter`].
    pub fn set_parameter(&self, parameter: u32, normalized: f64) -> bool {
        self.controls.set_parameter(parameter, normalized)
    }

    /// See [`EffectControls::parameter`].
    pub fn parameter(&self, parameter: u32) -> f64 {
        self.controls.parameter(parameter)
    }

    /// See [`EffectControls::set_bypass`].
    pub fn set_bypass(&self, bypass: bool) {
        self.controls.set_bypass(bypass);
    }

    /// See [`EffectControls::set_tempo`].
    pub fn set_tempo(&self, bpm: f64) {
        self.controls.set_tempo(bpm);
    }

    /// See [`EffectControls::latency`].
    pub fn latency(&self) -> u32 {
        self.controls.latency()
    }

    /// See [`EffectControls::tail_seconds`].
    pub fn tail_seconds(&self) -> f64 {
        self.controls.tail_seconds()
    }

    /// Maps changed mailboxes onto the processor. Returns true when a parameter or the tempo
    /// changed since the last call.
    fn refresh(&mut self) -> bool {
        let shared = &*self.controls.shared;
        let mut changed = false;
        let core = self.processor.core_mut();
        for (i, info) in shared.descriptor.params.iter().enumerate().take(MAX_PARAMETERS) {
            let n = f64::from_bits(shared.parameters[i].load(Ordering::Relaxed));
            if n != self.mapped[i] {
                self.mapped[i] = n;
                core.p[i] = info.physical(n);
                changed = true;
            }
        }
        let t = f64::from_bits(shared.tempo.load(Ordering::Relaxed));
        if t != core.tempo {
            core.tempo = t;
            changed = true;
        }
        changed
    }

    /// Frames of silent input after which the effect output is exactly silent.
    fn gate_threshold(&mut self, changed: bool) -> u64 {
        if changed || self.gate_tail < 0.0 {
            let core = self.processor.core();
            let tail = self.processor.tail_for_parameters(&core.p, core.tempo);
            self.gate_tail = tail;
            // Sustaining/freeze modes report the 120 s export cap: never gate.
            self.gate_frames = if !self.processor.gateable() || !tail.is_finite() || tail >= 120.0 {
                u64::MAX
            } else {
                (((tail + 0.1) * core.sr) as u64)
                    .wrapping_add(self.controls.shared.latency as u64)
                    .wrapping_add(self.controls.shared.max_frames as u64)
            };
        }
        self.gate_frames
    }

    fn reset_gate(&mut self) {
        self.silent_frames = 0;
        self.output_decayed = false;
    }

    /// Clears the effect's memory (delay lines, envelopes, filters) after applying pending
    /// parameter changes.
    pub fn reset(&mut self) {
        self.refresh();
        self.processor.reset();
        self.reset_gate();
    }

    /// Processes `left` and `right` in place. Returns false, leaving the buffers alone, when
    /// they differ in length or a sidechain buffer is shorter than they are; true otherwise
    /// (also for no frames and while bypassed).
    ///
    /// Input is cleaned first (non-finite samples and denormal-range values become 0, the
    /// rest is clamped to +-1e6), so is the output. Never allocates, locks or blocks.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], sidechain: Sidechain<'_>) -> bool {
        let frames = left.len();
        let sidechain_fits = match sidechain {
            Sidechain::None | Sidechain::MainInput => true,
            Sidechain::Mono(key) => key.len() >= frames,
            Sidechain::Stereo(key_left, key_right) => key_left.len() >= frames && key_right.len() >= frames,
        };
        if right.len() != frames || !sidechain_fits {
            return false;
        }
        if frames == 0 || self.controls.bypassed() {
            return true;
        }
        let _flush = FlushDenormals::new();
        let version = self.controls.shared.version.load(Ordering::Acquire);
        let mut changed = false;
        if version != self.seen_version {
            self.seen_version = version;
            changed = self.refresh();
        }
        let gate_frames = self.gate_threshold(changed);
        let max_frames = self.controls.shared.max_frames as usize;
        let keyed = !matches!(sidechain, Sidechain::None);
        let mut offset = 0;
        while offset < frames {
            let count = max_frames.min(frames - offset);
            let (l, r) = (&mut left[offset..offset + count], &mut right[offset..offset + count]);
            let mut peak = 0.0f32;
            if keyed {
                // The scratch buffers hold max_frames >= count samples. The key is copied
                // before the main input is cleaned or overwritten.
                let (side_l, side_r) = (&mut self.side_l[..count], &mut self.side_r[..count]);
                let (key_l, key_r): (&[f32], &[f32]) = match sidechain {
                    Sidechain::Mono(key) => (&key[offset..offset + count], &key[offset..offset + count]),
                    Sidechain::Stereo(key_left, key_right) => (&key_left[offset..offset + count], &key_right[offset..offset + count]),
                    Sidechain::MainInput | Sidechain::None => (&*l, &*r),
                };
                peak = clean_block_from(key_l, side_l).max(clean_block_from(key_r, side_r));
            }
            peak = peak.max(clean_block(l).max(clean_block(r)));
            let silent_input = peak < SILENCE;
            if silent_input && self.output_decayed && self.silent_frames >= gate_frames {
                // Gated: the tail has run out; skip the DSP, emit exact silence.
                l.fill(0.0);
                r.fill(0.0);
                offset += count;
                continue;
            }
            let (side_l, side_r) = if keyed { (Some(&self.side_l[..count]), Some(&self.side_r[..count])) } else { (None, None) };
            self.processor.process(l, r, side_l, side_r);
            let out = clean_block(l).max(clean_block(r));
            if silent_input {
                self.silent_frames = self.silent_frames.wrapping_add(count as u64);
                self.output_decayed = out < SILENCE;
            } else {
                self.reset_gate();
            }
            offset += count;
        }
        true
    }
}

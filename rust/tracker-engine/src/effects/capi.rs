//! The C API of the built-in effects and drum models.
//!
//! Replaces `Sources/TrackerEngine/BuiltinEffects.cpp`: the catalogue, the normalized <->
//! physical parameter mapping, the effect handle with its lock-free parameter mailboxes,
//! denormal flushing, input clean-up, silence gating, and the `st_drum_*` functions. Every
//! function declared in `Sources/TrackerEngine/include/STBuiltinEffects.h` is exported here.
//!
//! Threading contract (from the header): create/destroy allocate and belong on a non-render
//! thread. A handle has one serialized render owner; reset, destroy and process never overlap.
//! Atomic set/get/bypass/tempo/tail may overlap process.

use super::common::{
    clamp, clean, Descriptor, EffectInfo, ParameterInfo, Processor, MAX_PARAMETERS,
    PARAMETER_INTEGER, PARAMETER_LOG, PARAMETER_TOGGLE,
};
use super::{delay, distortion, drums, dynamics, filtering, modulation, perform, spatial};
use std::cell::UnsafeCell;
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// `STBuiltinParameterInfo` as declared in the C header.
pub type STBuiltinParameterInfo = ParameterInfo;
/// `STBuiltinEffectInfo` as declared in the C header.
pub type STBuiltinEffectInfo = EffectInfo;

/// Capacity of the C++ catalogue array; further descriptors would be ignored.
const CATALOGUE_CAPACITY: usize = 64;

/// The effect families in catalogue order. Catalogue indices are not project identities;
/// effect and parameter ids are.
fn families() -> [&'static [Descriptor]; 7] {
    [
        dynamics::descriptors(),
        filtering::descriptors(),
        modulation::descriptors(),
        spatial::descriptors(),
        delay::descriptors(),
        distortion::descriptors(),
        perform::descriptors(),
    ]
}

/// Every catalogue descriptor, in catalogue order.
pub fn catalogue() -> impl Iterator<Item = &'static Descriptor> {
    families()
        .into_iter()
        .flat_map(|family| family.iter())
        .take(CATALOGUE_CAPACITY)
}

/// Looks an effect up by its persistent id.
pub fn find(id: &CStr) -> Option<&'static Descriptor> {
    catalogue().find(|descriptor| descriptor.info.id() == id)
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
unsafe fn find_raw(id: *const c_char) -> Option<&'static Descriptor> {
    if id.is_null() {
        return None;
    }
    // SAFETY: non-null, and the caller guarantees a valid C string.
    find(unsafe { CStr::from_ptr(id) })
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
unsafe fn parameter_raw(id: *const c_char, parameter: u32) -> Option<&'static ParameterInfo> {
    // SAFETY: forwarded contract.
    unsafe { find_raw(id) }.and_then(|descriptor| descriptor.params.get(parameter as usize))
}

/// Normalized [0, 1] -> displayed physical value.
pub fn physical(p: &ParameterInfo, n: f64) -> f64 {
    let n = if n.is_finite() {
        clamp(n, 0.0, 1.0)
    } else {
        0.0
    };
    let mut v = if (p.flags & PARAMETER_LOG) != 0 && p.minimum > 0.0 {
        p.minimum * (p.maximum / p.minimum).powf(n)
    } else {
        p.minimum + (p.maximum - p.minimum) * n
    };
    if (p.flags & (PARAMETER_INTEGER | PARAMETER_TOGGLE)) != 0 {
        v = v.round();
    }
    clamp(v, p.minimum, p.maximum)
}

/// Displayed physical value -> normalized [0, 1]. Nonfinite values map the default.
pub fn normalized(p: &ParameterInfo, v: f64) -> f64 {
    let mut v = clamp(
        if v.is_finite() { v } else { p.default_value },
        p.minimum,
        p.maximum,
    );
    if (p.flags & (PARAMETER_INTEGER | PARAMETER_TOGGLE)) != 0 {
        v = v.round();
    }
    if p.maximum == p.minimum {
        return 0.0;
    }
    clamp(
        if (p.flags & PARAMETER_LOG) != 0 && p.minimum > 0.0 {
            (v / p.minimum).ln() / (p.maximum / p.minimum).ln()
        } else {
            (v - p.minimum) / (p.maximum - p.minimum)
        },
        0.0,
        1.0,
    )
}

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

/// `dst[i] = clean_sample(src[i])`; returns the largest |dst[i]|.
///
/// # Safety
/// `src` is valid for reading `dst.len()` floats. It may alias other live buffers (the
/// sidechain may be the main input), which is why it is read through a raw pointer; it must
/// not overlap `dst`.
#[inline(always)]
unsafe fn clean_block_from(src: *const f32, dst: &mut [f32]) -> f32 {
    let mut peak = [0.0f32; 4];
    let mut offset = 0usize;
    let mut lanes = dst.chunks_exact_mut(4);
    for chunk in &mut lanes {
        // SAFETY: offset + 4 <= dst.len() and the caller guarantees src covers that many
        // floats; an unaligned read of four f32 has no further requirements.
        let x = unsafe { src.add(offset).cast::<[f32; 4]>().read_unaligned() };
        chunk.copy_from_slice(&clean_lanes(x, &mut peak));
        offset += 4;
    }
    let mut result = lane_peak(peak);
    for out in lanes.into_remainder() {
        // SAFETY: offset < dst.len() and the caller guarantees src covers that many floats.
        *out = clean_sample(unsafe { src.add(offset).read() });
        result = result.max(out.abs());
        offset += 1;
    }
    result
}

/// Silence gating threshold: -120 dBFS.
const SILENCE: f32 = 1e-6;

/// Render-owner state of an effect handle.
struct Render {
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

/// `STBuiltinEffect`: one effect instance behind the C API.
pub struct STBuiltinEffect {
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
    /// Touched only by the render owner (process/reset) and by create/destroy.
    render: UnsafeCell<Render>,
}

impl STBuiltinEffect {
    fn new(descriptor: &'static Descriptor, sample_rate: f64, max_frames: u32) -> Box<Self> {
        let mut processor = (descriptor.create)(sample_rate, max_frames);
        let latency = processor.latency();
        let parameters: [AtomicU64; MAX_PARAMETERS] = std::array::from_fn(|i| {
            let value = descriptor
                .params
                .get(i)
                .map_or(0.0, |p| normalized(p, p.default_value));
            AtomicU64::new(value.to_bits())
        });
        for (slot, p) in processor
            .core_mut()
            .p
            .iter_mut()
            .zip(descriptor.params.iter())
        {
            *slot = p.default_value;
        }
        processor.reset();
        Box::new(STBuiltinEffect {
            descriptor,
            sample_rate,
            parameters,
            tempo: AtomicU64::new(120.0f64.to_bits()),
            bypass: AtomicU32::new(0),
            version: AtomicU32::new(1),
            max_frames,
            latency,
            render: UnsafeCell::new(Render {
                processor,
                seen_version: 0,
                side_l: vec![0.0; max_frames as usize],
                side_r: vec![0.0; max_frames as usize],
                mapped: [f64::NAN; MAX_PARAMETERS],
                silent_frames: 0,
                output_decayed: false,
                gate_tail: -1.0,
                gate_frames: 0,
            }),
        })
    }

    fn parameter_count(&self) -> usize {
        self.descriptor.params.len().min(MAX_PARAMETERS)
    }

    /// Maps changed mailboxes onto the processor. Returns true when a parameter or the tempo
    /// changed since the last call.
    fn refresh(&self, render: &mut Render) -> bool {
        let mut changed = false;
        let core = render.processor.core_mut();
        for (i, info) in self
            .descriptor
            .params
            .iter()
            .enumerate()
            .take(MAX_PARAMETERS)
        {
            let n = f64::from_bits(self.parameters[i].load(Ordering::Relaxed));
            if n != render.mapped[i] {
                render.mapped[i] = n;
                core.p[i] = physical(info, n);
                changed = true;
            }
        }
        let t = f64::from_bits(self.tempo.load(Ordering::Relaxed));
        if t != core.tempo {
            core.tempo = t;
            changed = true;
        }
        changed
    }

    /// Frames of silent input after which the effect output is exactly silent.
    fn gate_threshold(&self, render: &mut Render, changed: bool) -> u64 {
        if changed || render.gate_tail < 0.0 {
            let core = render.processor.core();
            let tail = render.processor.tail_for_parameters(&core.p, core.tempo);
            render.gate_tail = tail;
            // Sustaining/freeze modes report the 120 s export cap: never gate.
            render.gate_frames =
                if !render.processor.gateable() || !tail.is_finite() || tail >= 120.0 {
                    u64::MAX
                } else {
                    (((tail + 0.1) * core.sr) as u64)
                        .wrapping_add(self.latency as u64)
                        .wrapping_add(self.max_frames as u64)
                };
        }
        render.gate_frames
    }
}

impl Render {
    fn reset_gate(&mut self) {
        self.silent_frames = 0;
        self.output_decayed = false;
    }
}

/// # Safety
/// `effect` is null or a live handle from `st_builtin_effect_create`.
unsafe fn handle<'a>(effect: *const STBuiltinEffect) -> Option<&'a STBuiltinEffect> {
    // SAFETY: forwarded contract; only shared access is created here.
    unsafe { effect.as_ref() }
}

#[no_mangle]
pub extern "C" fn st_builtin_effect_count() -> u32 {
    catalogue().count() as u32
}

#[no_mangle]
pub extern "C" fn st_builtin_effect_info(index: u32) -> *const STBuiltinEffectInfo {
    catalogue()
        .nth(index as usize)
        .map_or(std::ptr::null(), |descriptor| &descriptor.info)
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_find(id: *const c_char) -> *const STBuiltinEffectInfo {
    // SAFETY: forwarded contract.
    unsafe { find_raw(id) }.map_or(std::ptr::null(), |descriptor| &descriptor.info)
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_parameter_info(
    id: *const c_char,
    parameter: u32,
) -> *const STBuiltinParameterInfo {
    // SAFETY: forwarded contract.
    unsafe { parameter_raw(id, parameter) }.map_or(std::ptr::null(), |p| p as *const _)
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_parameter_label(
    id: *const c_char,
    parameter: u32,
    label: u32,
) -> *const c_char {
    // SAFETY: forwarded contract.
    unsafe { parameter_raw(id, parameter) }
        .and_then(|p| p.label(label))
        .map_or(std::ptr::null(), CStr::as_ptr)
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_to_normalized(
    id: *const c_char,
    parameter: u32,
    physical_value: f64,
) -> f64 {
    // SAFETY: forwarded contract.
    unsafe { parameter_raw(id, parameter) }.map_or(0.0, |p| normalized(p, physical_value))
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_from_normalized(
    id: *const c_char,
    parameter: u32,
    normalized_value: f64,
) -> f64 {
    // SAFETY: forwarded contract.
    unsafe { parameter_raw(id, parameter) }.map_or(0.0, |p| {
        physical(
            p,
            if normalized_value.is_finite() {
                normalized_value
            } else {
                normalized(p, p.default_value)
            },
        )
    })
}

/// Create/destroy allocate and belong on a non-render thread. Supported rate 8k..384k,
/// maxFrames 1..1048576. Unknown IDs/invalid arguments return NULL.
///
/// # Safety
/// `id` is null or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_create(
    id: *const c_char,
    sample_rate: f64,
    max_frames: u32,
) -> *mut STBuiltinEffect {
    // SAFETY: forwarded contract.
    let Some(descriptor) = (unsafe { find_raw(id) }) else {
        return std::ptr::null_mut();
    };
    if descriptor.params.len() > MAX_PARAMETERS
        || !sample_rate.is_finite()
        || sample_rate < 8000.0
        || sample_rate > 384000.0
        || max_frames < 1
        || max_frames > 1048576
    {
        return std::ptr::null_mut();
    }
    Box::into_raw(STBuiltinEffect::new(descriptor, sample_rate, max_frames))
}

/// # Safety
/// `effect` is null or a handle from `st_builtin_effect_create` that is not in use and is
/// not used again.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_destroy(effect: *mut STBuiltinEffect) {
    if !effect.is_null() {
        // SAFETY: the handle came from Box::into_raw in create and ownership returns here.
        drop(unsafe { Box::from_raw(effect) });
    }
}

/// # Safety
/// `effect` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_set_parameter(
    effect: *mut STBuiltinEffect,
    parameter: u32,
    normalized_value: f64,
) -> i32 {
    // SAFETY: forwarded contract.
    let Some(effect) = (unsafe { handle(effect) }) else {
        return 0;
    };
    if parameter as usize >= effect.parameter_count() || !normalized_value.is_finite() {
        return 0;
    }
    let p = &effect.descriptor.params[parameter as usize];
    effect.parameters[parameter as usize].store(
        normalized(p, physical(p, normalized_value)).to_bits(),
        Ordering::Relaxed,
    );
    effect.version.fetch_add(1, Ordering::Release);
    1
}

/// # Safety
/// `effect` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_get_parameter(
    effect: *const STBuiltinEffect,
    parameter: u32,
) -> f64 {
    // SAFETY: forwarded contract.
    match unsafe { handle(effect) } {
        Some(effect) if (parameter as usize) < effect.parameter_count() => {
            f64::from_bits(effect.parameters[parameter as usize].load(Ordering::Relaxed))
        }
        _ => 0.0,
    }
}

/// # Safety
/// `effect` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_set_bypass(effect: *mut STBuiltinEffect, bypass: i32) {
    // SAFETY: forwarded contract.
    if let Some(effect) = unsafe { handle(effect) } {
        effect.bypass.store((bypass != 0) as u32, Ordering::Relaxed);
    }
}

/// Finite tempos are clamped to the tracker tempo range, 20..999 BPM.
///
/// # Safety
/// `effect` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_set_tempo(effect: *mut STBuiltinEffect, bpm: f64) {
    // SAFETY: forwarded contract.
    if let Some(effect) = unsafe { handle(effect) } {
        if bpm.is_finite() {
            effect
                .tempo
                .store(clamp(bpm, 20.0, 999.0).to_bits(), Ordering::Relaxed);
            effect.version.fetch_add(1, Ordering::Release);
        }
    }
}

/// # Safety
/// `effect` is null or a live handle, and the caller is its render owner (no concurrent
/// process, reset or destroy).
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_reset(effect: *mut STBuiltinEffect) {
    // SAFETY: forwarded contract.
    if let Some(effect) = unsafe { handle(effect) } {
        // SAFETY: the caller is the single render owner, so no other reference to the render
        // state exists.
        let render = unsafe { &mut *effect.render.get() };
        effect.refresh(render);
        render.processor.reset();
        render.reset_gate();
    }
}

/// # Safety
/// `effect` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_latency(effect: *const STBuiltinEffect) -> u32 {
    // SAFETY: forwarded contract.
    unsafe { handle(effect) }.map_or(0, |effect| effect.latency)
}

/// Conservative estimate from atomic parameter targets or fixed family maximum,
/// available immediately after create/restore without requiring a render.
///
/// # Safety
/// `effect` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_tail_seconds(effect: *const STBuiltinEffect) -> f64 {
    // SAFETY: forwarded contract.
    let Some(effect) = (unsafe { handle(effect) }) else {
        return 0.0;
    };
    let mut values = [0.0f64; MAX_PARAMETERS];
    for (i, info) in effect
        .descriptor
        .params
        .iter()
        .enumerate()
        .take(MAX_PARAMETERS)
    {
        values[i] = physical(
            info,
            f64::from_bits(effect.parameters[i].load(Ordering::Relaxed)),
        );
    }
    // The descriptor's estimate is the processor's tail_for_parameters as a pure function, so
    // this never touches the processor the render thread may be running.
    (effect.descriptor.tail)(
        effect.sample_rate,
        &values,
        f64::from_bits(effect.tempo.load(Ordering::Relaxed)),
    )
}

/// In-place stereo render; see the header for the full contract. Returns 0 only for an
/// invalid handle or buffers.
///
/// # Safety
/// `effect` is null or a live handle whose render owner is the caller. `left` and `right`
/// are null or distinct, non-overlapping arrays of at least `frames` floats. Each sidechain
/// pointer is null or valid for `frames` floats; the sidechain may alias the main inputs.
#[no_mangle]
pub unsafe extern "C" fn st_builtin_effect_process(
    effect: *mut STBuiltinEffect,
    left: *mut f32,
    right: *mut f32,
    sidechain_left: *const f32,
    sidechain_right: *const f32,
    frames: u32,
) -> i32 {
    // SAFETY: forwarded contract.
    let Some(effect) = (unsafe { handle(effect) }) else {
        return 0;
    };
    if frames == 0 {
        return 1;
    }
    if left.is_null() || right.is_null() || left == right {
        return 0;
    }
    if effect.bypass.load(Ordering::Relaxed) != 0 {
        return 1;
    }
    let _flush = FlushDenormals::new();
    // SAFETY: the caller is the single render owner, so no other reference to the render state exists.
    let render = unsafe { &mut *effect.render.get() };
    let version = effect.version.load(Ordering::Acquire);
    let mut changed = false;
    if version != render.seen_version {
        render.seen_version = version;
        changed = effect.refresh(render);
    }
    let gate_frames = effect.gate_threshold(render, changed);
    let sidechain = !sidechain_left.is_null() || !sidechain_right.is_null();
    let mut offset: u32 = 0;
    while offset < frames {
        let count = effect.max_frames.min(frames - offset) as usize;
        let mut peak = 0.0f32;
        if sidechain {
            let a = if !sidechain_left.is_null() {
                sidechain_left
            } else {
                sidechain_right
            };
            let b = if !sidechain_right.is_null() {
                sidechain_right
            } else {
                sidechain_left
            };
            // The scratch buffers hold max_frames >= count samples.
            let (Some(side_l), Some(side_r)) = (
                render.side_l.get_mut(..count),
                render.side_r.get_mut(..count),
            ) else {
                return 0;
            };
            // SAFETY: both sidechain pointers cover `frames` floats and offset + count <= frames.
            // They are read before the mutable main slices below are created, so aliasing the
            // main inputs is fine; the scratch buffers are private to this handle.
            peak = unsafe {
                clean_block_from(a.add(offset as usize), side_l)
                    .max(clean_block_from(b.add(offset as usize), side_r))
            };
        }
        // SAFETY: left and right are distinct arrays of at least `frames` floats and
        // offset + count <= frames; nothing else references them during this call.
        let (l, r) = unsafe {
            (
                std::slice::from_raw_parts_mut(left.add(offset as usize), count),
                std::slice::from_raw_parts_mut(right.add(offset as usize), count),
            )
        };
        peak = peak.max(clean_block(l).max(clean_block(r)));
        let silent_input = peak < SILENCE;
        if silent_input && render.output_decayed && render.silent_frames >= gate_frames {
            // Gated: the tail has run out; skip the DSP, emit exact silence.
            l.fill(0.0);
            r.fill(0.0);
            offset += count as u32;
            continue;
        }
        let (side_l, side_r) = if sidechain {
            (render.side_l.get(..count), render.side_r.get(..count))
        } else {
            (None, None)
        };
        render.processor.process(l, r, side_l, side_r);
        let out = clean_block(l).max(clean_block(r));
        if silent_input {
            render.silent_frames = render.silent_frames.wrapping_add(count as u64);
            render.output_decayed = out < SILENCE;
        } else {
            render.reset_gate();
        }
        offset += count as u32;
    }
    1
}

static DRUM_MODELS: [EffectInfo; 6] = [
    EffectInfo::new(c"kick", c"Kick", c"Drum Synth", 8, 0),
    EffectInfo::new(c"snare", c"Snare", c"Drum Synth", 8, 0),
    EffectInfo::new(c"hihat", c"Hi-hat", c"Drum Synth", 8, 0),
    EffectInfo::new(c"tom", c"Tom", c"Drum Synth", 8, 0),
    EffectInfo::new(c"perc", c"Percussion", c"Drum Synth", 8, 0),
    EffectInfo::new(c"cymbal", c"Cymbal", c"Drum Synth", 8, 0),
];

#[no_mangle]
pub extern "C" fn st_drum_model_count() -> u32 {
    6
}

#[no_mangle]
pub extern "C" fn st_drum_model_info(index: u32) -> *const STBuiltinEffectInfo {
    DRUM_MODELS
        .get(index as usize)
        .map_or(std::ptr::null(), |info| info as *const _)
}

/// # Safety
/// `id` is null or a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn st_drum_model_for_id(id: *const c_char) -> i32 {
    if id.is_null() {
        return 0;
    }
    // SAFETY: non-null, and the caller guarantees a valid C string.
    let id = unsafe { CStr::from_ptr(id) };
    (1..=6)
        .find(|&model| drums::model_id(model) == id)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn st_drum_parameter_info(
    model: i32,
    parameter: u32,
) -> *const STBuiltinParameterInfo {
    drums::parameter_info(model, parameter).map_or(std::ptr::null(), |p| p as *const _)
}

#[no_mangle]
pub extern "C" fn st_drum_parameter_default(model: i32, parameter: u32) -> f64 {
    drums::parameter_info(model, parameter).map_or(0.0, |p| normalized(p, p.default_value))
}

#[no_mangle]
pub extern "C" fn st_drum_parameter_to_normalized(
    model: i32,
    parameter: u32,
    physical_value: f64,
) -> f64 {
    drums::parameter_info(model, parameter).map_or(0.0, |p| normalized(p, physical_value))
}

#[no_mangle]
pub extern "C" fn st_drum_parameter_from_normalized(
    model: i32,
    parameter: u32,
    normalized_value: f64,
) -> f64 {
    drums::parameter_info(model, parameter).map_or(0.0, |p| {
        physical(
            p,
            if normalized_value.is_finite() {
                normalized_value
            } else {
                normalized(p, p.default_value)
            },
        )
    })
}

/// Offline render of one drum hit at its root pitch into `out` (mono), until the
/// voice ends or `capacity` frames. Nonfinite controls use the model defaults.
/// Returns the number of frames written; 0 for an invalid model or buffer.
///
/// # Safety
/// `normalized` is null or valid for `count` floats; `out` is null or valid for `capacity` floats.
#[no_mangle]
pub unsafe extern "C" fn st_drum_render(
    model: i32,
    normalized: *const f32,
    count: u32,
    sample_rate: f64,
    out: *mut f32,
    capacity: u32,
) -> u32 {
    if !drums::valid_model(model)
        || out.is_null()
        || capacity == 0
        || !sample_rate.is_finite()
        || sample_rate < 8000.0
    {
        return 0;
    }
    // Only the first eight controls are ever read.
    let controls: &[f32] = if normalized.is_null() {
        &[]
    } else {
        // SAFETY: non-null and valid for `count` floats per the caller's contract.
        unsafe {
            std::slice::from_raw_parts(
                normalized,
                (count as usize).min(drums::PARAMETER_COUNT as usize),
            )
        }
    };
    // SAFETY: non-null and valid for `capacity` floats per the caller's contract.
    let out = unsafe { std::slice::from_raw_parts_mut(out, capacity as usize) };
    let mut voice = drums::Voice::default();
    voice.start(model, sample_rate, controls, 1);
    let mut frames = 0usize;
    for slot in out.iter_mut() {
        if !voice.active() {
            break;
        }
        *slot = clean(voice.tick(1.0) as f64) as f32;
        frames += 1;
    }
    // Trim the inaudible end of the decay.
    while frames > 1 && out[frames - 1].abs() < 1e-5 {
        frames -= 1;
    }
    frames as u32
}

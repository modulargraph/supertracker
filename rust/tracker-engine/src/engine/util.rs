//! Small building blocks of `TrackerEngine.cpp`'s anonymous namespace: the C++ `clamp`/`std::min`/
//! `std::max` semantics, the lock-free single-producer/single-consumer `Ring`, the calloc-backed
//! `FloatBlock`, and the xorshift generator behind every random decision.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

/// The C++ engine's `pi` constant (3.1415926535897932384626433832795 rounds to this double).
pub const PI: f64 = std::f64::consts::PI;

/// `std::min(a, b)`: returns `a` unless `b < a` (so a NaN `b` yields `a`, a NaN `a` yields `a`).
#[inline(always)]
pub fn cmin<T: PartialOrd>(a: T, b: T) -> T {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max(a, b)`: returns `a` unless `a < b`.
#[inline(always)]
pub fn cmax<T: PartialOrd>(a: T, b: T) -> T {
    if a < b {
        b
    } else {
        a
    }
}

/// The engine's `clamp(x, a, b) = std::max(a, std::min(b, x))`. A NaN `x` yields `b`; when
/// `a > b` the result is `a`. Rust's `f32::clamp` differs on both counts, so it is not used.
#[inline(always)]
pub fn clamp<T: PartialOrd>(x: T, a: T, b: T) -> T {
    cmax(a, cmin(b, x))
}

/// `finite(float x, float fallback)`
#[inline(always)]
pub fn finite(x: f32, fallback: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        fallback
    }
}

/// `finite(double x, double fallback)`
#[inline(always)]
pub fn finite64(x: f64, fallback: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        fallback
    }
}

/// `midiFrequency`
#[inline]
pub fn midi_frequency(note: f64) -> f32 {
    (440.0 * ((note - 69.0) / 12.0).exp2()) as f32
}

/// `randomBipolar`: xorshift32 step, mapped to [-1, 1).
#[inline(always)]
pub fn random_bipolar(state: &mut u32) -> f32 {
    let mut s = *state;
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    *state = s;
    (s as f64 / 2147483648.0 - 1.0) as f32
}

/// `std::nextafter(x, -infinity)` for a finite `x`.
pub fn next_down(x: f64) -> f64 {
    if x.is_nan() || x == f64::NEG_INFINITY {
        return x;
    }
    if x == 0.0 {
        return -f64::from_bits(1);
    }
    let bits = x.to_bits();
    f64::from_bits(if x > 0.0 { bits - 1 } else { bits + 1 })
}

/// Index of the lowest set bit (`__builtin_ctzll`); callers pass a nonzero word.
#[inline(always)]
pub fn lowest_bit(word: u64) -> usize {
    word.trailing_zeros() as usize
}

/// Bounded single-producer/single-consumer queue (C++ `Ring<T,N>`): one slot stays empty, the
/// producer owns `write`, the consumer owns `read`. Neither side blocks or allocates.
pub struct Ring<T> {
    data: Box<[UnsafeCell<T>]>,
    read: AtomicU32,
    write: AtomicU32,
}

// SAFETY: the ring is shared between exactly one producer thread and one consumer thread (the
// engine's documented threading contract). A slot is written only by the producer before it
// publishes `write` (release) and read only by the consumer after it observed that `write`
// (acquire), so no slot is accessed from both sides at once.
unsafe impl<T: Send> Sync for Ring<T> {}
unsafe impl<T: Send> Send for Ring<T> {}

impl<T: Copy + Default> Ring<T> {
    pub fn new(capacity: usize) -> Self {
        let data: Vec<UnsafeCell<T>> = (0..capacity).map(|_| UnsafeCell::new(T::default())).collect();
        Ring { data: data.into_boxed_slice(), read: AtomicU32::new(0), write: AtomicU32::new(0) }
    }

    /// Free slots as seen by the producer.
    #[inline]
    pub fn available(&self) -> u32 {
        let n = self.data.len();
        let r = self.read.load(Ordering::Acquire) as usize;
        let w = self.write.load(Ordering::Relaxed) as usize;
        ((r + n - w - 1) % n) as u32
    }

    /// Producer side. Returns false when the ring is full.
    #[inline]
    pub fn push(&self, value: &T) -> bool {
        let n = self.data.len();
        let w = self.write.load(Ordering::Relaxed) as usize;
        let next = (w + 1) % n;
        if next == self.read.load(Ordering::Acquire) as usize {
            return false;
        }
        let Some(slot) = self.data.get(w) else { return false };
        // SAFETY: slot `w` is outside the consumer's readable range [read, write) until the
        // store below publishes it, and this thread is the only producer.
        unsafe { *slot.get() = *value };
        self.write.store(next as u32, Ordering::Release);
        true
    }

    /// Consumer side.
    #[inline]
    pub fn pop(&self) -> Option<T> {
        let n = self.data.len();
        let r = self.read.load(Ordering::Relaxed) as usize;
        if r == self.write.load(Ordering::Acquire) as usize {
            return None;
        }
        let slot = self.data.get(r)?;
        // SAFETY: slot `r` was published by the producer (acquire load above saw a `write`
        // beyond it) and the producer does not touch it again until `read` moves past it; this
        // thread is the only consumer.
        let value = unsafe { *slot.get() };
        self.read.store(((r + 1) % n) as u32, Ordering::Release);
        Some(value)
    }
}

extern "C" {
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn free(pointer: *mut c_void);
}

/// calloc-backed float storage (C++ `FloatBlock`): large blocks come from fresh zero pages, so
/// buses and cache history that are never written never become resident memory. Allocated and
/// freed on the control thread only.
pub struct FloatBlock {
    data: *mut f32,
    count: usize,
}

// SAFETY: a FloatBlock is plain owned memory; it is moved between threads only as part of the
// structures that own it, which carry their own access rules.
unsafe impl Send for FloatBlock {}

impl FloatBlock {
    /// Returns `None` when the allocation fails (C++ `std::bad_alloc`).
    pub fn new(count: usize) -> Option<FloatBlock> {
        if count == 0 {
            return Some(FloatBlock { data: std::ptr::null_mut(), count: 0 });
        }
        // SAFETY: plain C allocation; the result is checked for null before use.
        let data = unsafe { calloc(count, std::mem::size_of::<f32>()) } as *mut f32;
        if data.is_null() {
            None
        } else {
            Some(FloatBlock { data, count })
        }
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[f32] {
        if self.count == 0 {
            return &[];
        }
        // SAFETY: `data` points to `count` zero-initialised floats owned by this block.
        unsafe { std::slice::from_raw_parts(self.data, self.count) }
    }

    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        if self.count == 0 {
            return &mut [];
        }
        // SAFETY: as in `as_slice`, and `&mut self` guarantees exclusive access.
        unsafe { std::slice::from_raw_parts_mut(self.data, self.count) }
    }
}

impl Drop for FloatBlock {
    fn drop(&mut self) {
        // SAFETY: `data` came from calloc (or is null, which free accepts) and is freed once.
        unsafe { free(self.data as *mut c_void) }
    }
}

/// A heap array of `n` default elements without building it on the stack first.
pub fn boxed_slice<T: Clone + Default>(n: usize) -> Box<[T]> {
    vec![T::default(); n].into_boxed_slice()
}

/// `Box::new(make())` whose own allocation may fail: `None` when memory is exhausted (where the
/// C++ engine caught `std::bad_alloc`). The storage is reserved before `make` runs.
pub fn try_box_with<T>(make: impl FnOnce() -> T) -> Option<Box<T>> {
    let mut slot: Vec<T> = Vec::new();
    slot.try_reserve_exact(1).ok()?;
    slot.push(make());
    // Length equals capacity, so this does not reallocate.
    let boxed: Box<[T]> = slot.into_boxed_slice();
    // SAFETY: a boxed slice of exactly one `T` is one initialised `T` allocated with
    // `Layout::array::<T>(1)`, which is `Layout::new::<T>()`, the layout `Box<T>` frees with.
    Some(unsafe { Box::from_raw(Box::into_raw(boxed) as *mut T) })
}


//! Shared helpers for the integration tests ported from the C++ programs in `tests/` (the engine
//! feature tests, `clock_tests.cpp`, `unit_range_tests.cpp`, `builtin_effects*_tests.cpp` and the
//! two engine benchmarks). The other engine ports (`engine_core_*`, `engine_timing_*`) have their
//! own support module; nothing here is shared with them.
//!
//! * [`alloc`]: the counting / failing `#[global_allocator]` that stands in for the C++ tests'
//!   replaced `operator new` ("zero render-thread allocations", injected allocation failure).
//! * [`engine`]: an owned engine with one method per `st_engine_*` function, each forwarding to
//!   the safe methods of the engine's `Control` / `Render` / `Shared` halves.
//! * [`tone`]: this batch's copy of `tests/engine_test_tone.hpp`.
//! * [`fx`], [`unit_range`]: wrappers for the two parts of the crate that have no safe Rust API
//!   yet (the built-in effect handle and the unit range cache); every C-API call is marked
//!   `TODO(api)`.
#![allow(dead_code)]

pub mod alloc;
#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "effects")]
pub mod fx;
#[cfg(feature = "engine")]
pub mod tone;
#[cfg(feature = "unit-range")]
pub mod unit_range;

/// FNV-1a offset basis used by the C++ benchmarks' PCM hashes.
pub const FNV_OFFSET: u64 = 1469598103934665603;
/// FNV-1a prime.
pub const FNV_PRIME: u64 = 1099511628211;

/// `hash=(hash^bits)*prime` over one 32-bit word.
#[inline]
pub fn hash_word(hash: u64, word: u32) -> u64 {
    (hash ^ word as u64).wrapping_mul(FNV_PRIME)
}

/// The benchmarks' hash of one float: its bit pattern as a word.
#[inline]
pub fn hash_f32(hash: u64, value: f32) -> u64 {
    hash_word(hash, value.to_bits())
}

/// `hashDouble`: low word, then high word.
#[inline]
pub fn hash_f64(hash: u64, value: f64) -> u64 {
    let bits = value.to_bits();
    hash_word(hash_word(hash, bits as u32), (bits >> 32) as u32)
}

#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

extern "C" {
    fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
}

#[cfg(target_os = "linux")]
const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
#[cfg(target_os = "linux")]
const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
#[cfg(target_os = "macos")]
const CLOCK_PROCESS_CPUTIME_ID: i32 = 12;
#[cfg(target_os = "macos")]
const CLOCK_THREAD_CPUTIME_ID: i32 = 16;

#[cfg(all(target_pointer_width = "64", any(target_os = "linux", target_os = "macos")))]
fn cpu_clock(clock: i32) -> f64 {
    let mut time = Timespec { seconds: 0, nanoseconds: 0 };
    // SAFETY: plain libc call writing one `timespec` (two 64-bit fields on the 64-bit targets
    // this is compiled for) into storage we own.
    let status = unsafe { clock_gettime(clock, &mut time) };
    assert_eq!(status, 0, "clock_gettime");
    time.seconds as f64 + time.nanoseconds as f64 * 1e-9
}

/// CPU seconds used by the calling thread (C++ `clock_gettime(CLOCK_THREAD_CPUTIME_ID)`): the
/// budget tests time the render thread's own work, not a loaded machine's scheduling.
#[cfg(all(target_pointer_width = "64", any(target_os = "linux", target_os = "macos")))]
pub fn thread_cpu_seconds() -> f64 {
    cpu_clock(CLOCK_THREAD_CPUTIME_ID)
}

/// CPU seconds used by the whole process (C++ `std::clock()`).
#[cfg(all(target_pointer_width = "64", any(target_os = "linux", target_os = "macos")))]
pub fn process_cpu_seconds() -> f64 {
    cpu_clock(CLOCK_PROCESS_CPUTIME_ID)
}

#[cfg(not(all(target_pointer_width = "64", any(target_os = "linux", target_os = "macos"))))]
pub fn thread_cpu_seconds() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

#[cfg(not(all(target_pointer_width = "64", any(target_os = "linux", target_os = "macos"))))]
pub fn process_cpu_seconds() -> f64 {
    thread_cpu_seconds()
}

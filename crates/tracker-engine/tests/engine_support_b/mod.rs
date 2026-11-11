//! Shared helpers for the engine feature tests, the two `effects_builtin*` tests and the engine
//! benchmarks. The `engine_core_*` tests have their own support module; nothing here is shared
//! with them.
//!
//! * [`alloc`]: the counting / failing `#[global_allocator]` ("zero render-thread allocations",
//!   injected allocation failure).
//! * [`engine`]: an owned engine with one method per operation, each forwarding to the engine's
//!   handles.
//! * [`tone`]: test tones.
//! * [`fx`]: the built-in effect handle with the id and catalogue entry the tests print next to
//!   it.
#![allow(dead_code)]

pub mod alloc;
#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "effects")]
pub mod fx;
#[cfg(feature = "engine")]
pub mod tone;

/// FNV-1a offset basis of the PCM hashes.
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

/// The hash of one double: low word, then high word.
#[inline]
pub fn hash_f64(hash: u64, value: f64) -> u64 {
    let bits = value.to_bits();
    hash_word(hash_word(hash, bits as u32), (bits >> 32) as u32)
}

/// Reads a small procfs file into `buffer` without allocating and returns what was read.
#[cfg(target_os = "linux")]
fn read_small<'a>(path: &str, buffer: &'a mut [u8; 512]) -> Option<&'a str> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let count = file.read(buffer).ok()?;
    std::str::from_utf8(&buffer[..count]).ok()
}

/// Wall-clock seconds since the first call: the stand-in where no CPU clock can be read.
fn wall_seconds() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// CPU seconds used by the calling thread: the budget tests time the render thread's own work,
/// not a loaded machine's scheduling. On Linux this is the scheduler's run time of the thread
/// (`/proc/thread-self/schedstat`, nanoseconds, advanced at scheduler ticks), read without
/// allocating so it may be called inside a render section; elsewhere, and where the kernel does
/// not provide it, wall-clock time.
pub fn thread_cpu_seconds() -> f64 {
    #[cfg(target_os = "linux")]
    {
        let mut buffer = [0u8; 512];
        let run_time = read_small("/proc/thread-self/schedstat", &mut buffer)
            .and_then(|text| text.split_ascii_whitespace().next())
            .and_then(|nanoseconds| nanoseconds.parse::<u64>().ok());
        if let Some(nanoseconds) = run_time {
            return nanoseconds as f64 * 1e-9;
        }
    }
    wall_seconds()
}

/// CPU seconds used by the whole process (user and system time of every thread). On Linux from
/// `/proc/self/stat`, whose clock ticks are 1/100 s; elsewhere wall-clock time.
pub fn process_cpu_seconds() -> f64 {
    #[cfg(target_os = "linux")]
    {
        let mut buffer = [0u8; 512];
        // The fields after the command name (which may itself contain spaces and brackets):
        // state is the first, utime and stime the 12th and 13th.
        let ticks = read_small("/proc/self/stat", &mut buffer).and_then(|text| {
            let mut fields = text.get(text.rfind(')')? + 1..)?.split_ascii_whitespace().skip(11);
            let user = fields.next()?.parse::<u64>().ok()?;
            let system = fields.next()?.parse::<u64>().ok()?;
            Some(user + system)
        });
        if let Some(ticks) = ticks {
            return ticks as f64 / 100.0;
        }
    }
    wall_seconds()
}

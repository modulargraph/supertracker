//! Transport shared by the host-side client (`stpc-client`) and the container runtime
//! (`stpc-runtime`).
//!
//! * [`region`]: the per-instance shared-memory region (audio buffers, per-block event lists,
//!   transport info, the plug-in -> host notification queue).
//! * [`wake`]: the cross-process wake-up primitive used for the per-block handshake.
//! * [`channel`]: the framed control channel (Unix domain socket) for non-real-time requests.
//! * [`messages`] / [`wire`]: the control protocol and its encoding.
//! * [`shm`]: anonymous shared memory.
//!
//! Everything here is `unix` only today. The Windows port needs `channel` over a named pipe
//! and `shm` over a named file mapping; the rest of the code is platform independent.

pub mod channel;
pub mod messages;
pub mod region;
pub mod shm;
pub mod wake;
pub mod wire;

/// Version of the control protocol *and* of the shared-memory layout. Both sides must agree
/// exactly: the host rejects a container whose `Hello` carries another version, and a
/// container refuses a region whose header carries another version.
pub const PROTOCOL_VERSION: u32 = 1;

/// First field of the handshake message ("STPC").
pub const HELLO_MAGIC: u32 = 0x5354_5043;

/// Maximum number of buses per direction.
pub const MAX_BUSES: usize = 8;
/// Maximum number of channels per direction, summed over all buses.
pub const MAX_CHANNELS: usize = 64;
/// Largest block size a region can be created for.
pub const MAX_FRAMES_LIMIT: u32 = 1 << 16;
/// Largest control frame accepted (a plug-in state can be large, but not unbounded).
pub const MAX_FRAME_BYTES: usize = 1 << 30;

/// Command-line option the supervisor passes to the container: the control endpoint.
pub const ARG_ENDPOINT: &str = "--stpc-endpoint";
/// Command-line option the supervisor passes to the container: the connection token.
pub const ARG_TOKEN: &str = "--stpc-token";

/// Monotonic clock in nanoseconds (never goes backwards, unaffected by wall-clock changes).
#[inline]
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid timespec; CLOCK_MONOTONIC exists on Linux and macOS.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

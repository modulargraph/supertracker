//! Cross-process wake-up for the per-block handshake.
//!
//! The handshake state is always a sequence word (`AtomicU32`) inside the shared region; this
//! module only provides "sleep until that word may have changed" and "wake the sleeper".
//!
//! Two interchangeable mechanisms:
//!
//! * [`WakeMode::Address`]: wait on the address of the word itself. Linux: `futex(2)`
//!   (`FUTEX_WAIT`/`FUTEX_WAKE`, the shared, non-private variant). macOS: the futex-like
//!   `os_sync_wait_on_address` family (public API since macOS 14.4) with
//!   `OS_SYNC_WAIT_ON_ADDRESS_SHARED`, falling back to `__ulock_wait2`/`__ulock_wake`
//!   (`UL_COMPARE_AND_WAIT_SHARED`, the same kernel facility under its older private name,
//!   macOS 11+). Both are resolved with `dlsym` at run time. **The macOS implementation is
//!   type-checked only** (`cargo check --target aarch64-apple-darwin`); it has never run.
//! * [`WakeMode::Pipe`]: a pipe per direction; the waker writes one byte, the sleeper
//!   `poll`s. Plain POSIX, identical code on Linux and macOS, verified by running on Linux.
//!   It is the fallback when address waits are unavailable, and can be forced
//!   (`WakePreference::Pipe` in the client).
//!
//! In both modes a wait returns early when the word no longer holds the expected value, so
//! a wake-up that arrives before the sleeper sleeps is never lost.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum WakeMode {
    Address = 1,
    Pipe = 2,
}

impl WakeMode {
    pub fn from_u32(value: u32) -> Option<WakeMode> {
        match value {
            1 => Some(WakeMode::Address),
            2 => Some(WakeMode::Pipe),
            _ => None,
        }
    }
    /// Bit in the `wake_support` mask of the handshake.
    pub fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// Mask of the wake modes this process can use.
pub fn supported_mask() -> u32 {
    let mut mask = WakeMode::Pipe.bit();
    if address::supported() {
        mask |= WakeMode::Address.bit();
    }
    mask
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    /// Woken, interrupted, or the word already differed: re-check the word.
    Woken,
    TimedOut,
    /// Pipe mode only: the other end is closed (the peer process is gone).
    PeerGone,
}

/// Address waits (futex and its macOS equivalents).
pub mod address {
    use super::*;

    pub fn supported() -> bool {
        imp::supported()
    }

    /// Sleeps while `*word == expected`, at most `timeout_ns` (None: no limit).
    pub fn wait(word: &AtomicU32, expected: u32, timeout_ns: Option<u64>) -> WaitOutcome {
        imp::wait(word, expected, timeout_ns)
    }

    /// Wakes the thread(s) sleeping on `word`, in any process that maps it.
    pub fn wake(word: &AtomicU32) {
        imp::wake(word)
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod imp {
        use super::*;

        pub fn supported() -> bool {
            true
        }

        pub fn wait(word: &AtomicU32, expected: u32, timeout_ns: Option<u64>) -> WaitOutcome {
            let ts;
            let ts_ptr = match timeout_ns {
                Some(ns) => {
                    ts = libc::timespec {
                        tv_sec: (ns / 1_000_000_000) as libc::time_t,
                        tv_nsec: (ns % 1_000_000_000) as _,
                    };
                    &ts as *const libc::timespec
                }
                None => std::ptr::null(),
            };
            // SAFETY: `word` is a valid, live u32; FUTEX_WAIT (not _PRIVATE: the word is
            // shared between processes) with a relative timeout.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_futex,
                    word.as_ptr(),
                    libc::FUTEX_WAIT,
                    expected,
                    ts_ptr,
                    0usize,
                    0u32,
                )
            };
            if rc == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ETIMEDOUT) {
                WaitOutcome::TimedOut
            } else {
                WaitOutcome::Woken // woken, EAGAIN (value differed) or EINTR
            }
        }

        pub fn wake(word: &AtomicU32) {
            // SAFETY: as above. One waiter at most by protocol; waking "all" is harmless.
            unsafe {
                libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAKE, i32::MAX, 0usize, 0usize, 0u32);
            }
        }
    }

    // ---- macOS: TYPE-CHECKED ONLY (never run; see the module documentation) ----
    #[cfg(target_os = "macos")]
    mod imp {
        use super::*;
        use std::ffi::{c_int, c_void};
        use std::sync::OnceLock;

        type OsSyncWaitTimeout = unsafe extern "C" fn(*mut c_void, u64, usize, u32, u32, u64) -> c_int;
        type OsSyncWait = unsafe extern "C" fn(*mut c_void, u64, usize, u32) -> c_int;
        type OsSyncWake = unsafe extern "C" fn(*mut c_void, usize, u32) -> c_int;
        type UlockWait2 = unsafe extern "C" fn(u32, *mut c_void, u64, u64, u64) -> c_int;
        type UlockWake = unsafe extern "C" fn(u32, *mut c_void, u64) -> c_int;

        const UL_COMPARE_AND_WAIT_SHARED: u32 = 3;
        const ULF_WAKE_ALL: u32 = 0x0000_0100;
        const ULF_NO_ERRNO: u32 = 0x0100_0000;

        #[derive(Clone, Copy)]
        enum Api {
            OsSync { wait: OsSyncWait, wait_timeout: OsSyncWaitTimeout, wake_all: OsSyncWake },
            Ulock { wait2: UlockWait2, wake: UlockWake },
            Unavailable,
        }

        fn lookup(name: &[u8]) -> *mut c_void {
            debug_assert!(name.ends_with(b"\0"));
            // SAFETY: NUL-terminated name; RTLD_DEFAULT searches the already loaded images.
            unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr().cast()) }
        }

        fn api() -> Api {
            static API: OnceLock<Api> = OnceLock::new();
            *API.get_or_init(|| {
                let wait = lookup(b"os_sync_wait_on_address\0");
                let wait_timeout = lookup(b"os_sync_wait_on_address_with_timeout\0");
                let wake_all = lookup(b"os_sync_wake_by_address_all\0");
                if !wait.is_null() && !wait_timeout.is_null() && !wake_all.is_null() {
                    // SAFETY: signatures as declared in <os/os_sync_wait_on_address.h>.
                    return unsafe {
                        Api::OsSync {
                            wait: std::mem::transmute::<*mut c_void, OsSyncWait>(wait),
                            wait_timeout: std::mem::transmute::<*mut c_void, OsSyncWaitTimeout>(wait_timeout),
                            wake_all: std::mem::transmute::<*mut c_void, OsSyncWake>(wake_all),
                        }
                    };
                }
                let wait2 = lookup(b"__ulock_wait2\0");
                let wake = lookup(b"__ulock_wake\0");
                if !wait2.is_null() && !wake.is_null() {
                    // SAFETY: signatures as in xnu's bsd/sys/ulock.h.
                    return unsafe {
                        Api::Ulock {
                            wait2: std::mem::transmute::<*mut c_void, UlockWait2>(wait2),
                            wake: std::mem::transmute::<*mut c_void, UlockWake>(wake),
                        }
                    };
                }
                Api::Unavailable
            })
        }

        pub fn supported() -> bool {
            !matches!(api(), Api::Unavailable)
        }

        pub fn wait(word: &AtomicU32, expected: u32, timeout_ns: Option<u64>) -> WaitOutcome {
            let addr = word.as_ptr().cast::<c_void>();
            match api() {
                Api::OsSync { wait, wait_timeout, .. } => {
                    // SAFETY: valid 4-byte word in shared memory; SHARED keys the wait by the
                    // underlying memory object, so another process mapping it can wake us.
                    let rc = unsafe {
                        match timeout_ns {
                            // A zero timeout is rejected with EINVAL: wait at least 1 ns.
                            Some(ns) => wait_timeout(
                                addr,
                                expected as u64,
                                4,
                                libc::OS_SYNC_WAIT_ON_ADDRESS_SHARED,
                                libc::OS_CLOCK_MACH_ABSOLUTE_TIME,
                                ns.max(1),
                            ),
                            None => wait(addr, expected as u64, 4, libc::OS_SYNC_WAIT_ON_ADDRESS_SHARED),
                        }
                    };
                    if rc < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ETIMEDOUT) {
                        WaitOutcome::TimedOut
                    } else {
                        WaitOutcome::Woken
                    }
                }
                Api::Ulock { wait2, .. } => {
                    // SAFETY: as above. Timeout 0 means "forever" for __ulock_wait2.
                    let rc = unsafe {
                        wait2(
                            UL_COMPARE_AND_WAIT_SHARED | ULF_NO_ERRNO,
                            addr,
                            expected as u64,
                            timeout_ns.map_or(0, |ns| ns.max(1)),
                            0,
                        )
                    };
                    if rc == -libc::ETIMEDOUT {
                        WaitOutcome::TimedOut
                    } else {
                        WaitOutcome::Woken
                    }
                }
                Api::Unavailable => WaitOutcome::Woken,
            }
        }

        pub fn wake(word: &AtomicU32) {
            let addr = word.as_ptr().cast::<c_void>();
            match api() {
                // SAFETY: valid word; errors (ENOENT: nobody waits) are expected and ignored.
                Api::OsSync { wake_all, .. } => unsafe {
                    wake_all(addr, 4, libc::OS_SYNC_WAKE_BY_ADDRESS_SHARED);
                },
                Api::Ulock { wake, .. } => unsafe {
                    wake(UL_COMPARE_AND_WAIT_SHARED | ULF_WAKE_ALL | ULF_NO_ERRNO, addr, 0);
                },
                Api::Unavailable => {}
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    mod imp {
        use super::*;
        pub fn supported() -> bool {
            false
        }
        pub fn wait(_: &AtomicU32, _: u32, _: Option<u64>) -> WaitOutcome {
            WaitOutcome::Woken
        }
        pub fn wake(_: &AtomicU32) {}
    }
}

/// Creates a pipe with both ends non-blocking and close-on-exec: `(read end, write end)`.
pub fn pipe_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: `fds` has room for two descriptors. (No pipe2: macOS does not have it.)
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned by nobody else.
    let pair = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [pair.0.as_raw_fd(), pair.1.as_raw_fd()] {
        set_nonblocking_cloexec(fd)?;
    }
    Ok(pair)
}

pub fn set_nonblocking_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: plain fcntl calls on a descriptor the caller owns.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd_flags = libc::fcntl(fd, libc::F_GETFD);
        if fd_flags < 0 || libc::fcntl(fd, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The waking side of one direction of the handshake. `notify` is real-time safe: one
/// system call, no allocation, no lock, never blocks.
pub struct Notifier {
    pipe: Option<OwnedFd>,
}

impl Notifier {
    pub fn address() -> Notifier {
        Notifier { pipe: None }
    }
    /// `write_end` must be non-blocking.
    pub fn pipe(write_end: OwnedFd) -> Notifier {
        Notifier { pipe: Some(write_end) }
    }
    pub fn mode(&self) -> WakeMode {
        if self.pipe.is_some() { WakeMode::Pipe } else { WakeMode::Address }
    }
    /// Call after storing the new value into `word`.
    #[inline]
    pub fn notify(&self, word: &AtomicU32) {
        match &self.pipe {
            None => address::wake(word),
            Some(fd) => {
                let byte = 1u8;
                // SAFETY: valid buffer of one byte. EAGAIN (pipe full) means wake-ups are
                // already pending; EPIPE means the peer is gone. Neither needs handling here.
                unsafe { libc::write(fd.as_raw_fd(), (&byte as *const u8).cast(), 1) };
            }
        }
    }
}

/// The sleeping side of one direction of the handshake. `wait` is real-time safe apart from
/// the sleep itself: no allocation, no lock.
pub struct Waiter {
    pipe: Option<OwnedFd>,
}

impl Waiter {
    pub fn address() -> Waiter {
        Waiter { pipe: None }
    }
    /// `read_end` must be non-blocking.
    pub fn pipe(read_end: OwnedFd) -> Waiter {
        Waiter { pipe: Some(read_end) }
    }
    pub fn mode(&self) -> WakeMode {
        if self.pipe.is_some() { WakeMode::Pipe } else { WakeMode::Address }
    }

    /// Sleeps while `*word == expected`, at most `timeout_ns` (None: until woken).
    /// May return [`WaitOutcome::Woken`] spuriously: callers loop on the word.
    pub fn wait(&self, word: &AtomicU32, expected: u32, timeout_ns: Option<u64>) -> WaitOutcome {
        let fd = match &self.pipe {
            None => return address::wait(word, expected, timeout_ns),
            Some(fd) => fd.as_raw_fd(),
        };
        if word.load(Ordering::SeqCst) != expected {
            return WaitOutcome::Woken;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let rc = poll_one(&mut pfd, timeout_ns);
        if rc == 0 {
            return WaitOutcome::TimedOut;
        }
        if rc < 0 {
            return WaitOutcome::Woken; // EINTR
        }
        if pfd.revents & libc::POLLIN != 0 {
            let mut sink = [0u8; 64];
            // SAFETY: valid buffer; the descriptor is non-blocking, so this cannot block.
            let n = unsafe { libc::read(fd, sink.as_mut_ptr().cast(), sink.len()) };
            if n == 0 {
                return WaitOutcome::PeerGone; // EOF: every write end is closed
            }
            return WaitOutcome::Woken;
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return WaitOutcome::PeerGone;
        }
        WaitOutcome::Woken
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn poll_one(pfd: &mut libc::pollfd, timeout_ns: Option<u64>) -> libc::c_int {
    let ts;
    let ts_ptr = match timeout_ns {
        Some(ns) => {
            ts = libc::timespec {
                tv_sec: (ns / 1_000_000_000) as libc::time_t,
                tv_nsec: (ns % 1_000_000_000) as _,
            };
            &ts as *const libc::timespec
        }
        None => std::ptr::null(),
    };
    // SAFETY: one valid pollfd; nanosecond timeout; no signal mask change.
    unsafe { libc::ppoll(pfd, 1, ts_ptr, std::ptr::null()) }
}

/// Portable variant (macOS has no `ppoll`): millisecond resolution, rounded up, so a wait
/// can overshoot its deadline by up to 1 ms. TYPE-CHECKED ONLY on macOS.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn poll_one(pfd: &mut libc::pollfd, timeout_ns: Option<u64>) -> libc::c_int {
    let ms = match timeout_ns {
        Some(ns) => ns.div_ceil(1_000_000).min(i32::MAX as u64) as libc::c_int,
        None => -1,
    };
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(pfd, 1, ms) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn address_wait_times_out_and_wakes() {
        if !address::supported() {
            return;
        }
        let word = Arc::new(AtomicU32::new(7));
        let start = Instant::now();
        assert_eq!(address::wait(&word, 7, Some(20_000_000)), WaitOutcome::TimedOut);
        assert!(start.elapsed() >= Duration::from_millis(19));
        // A differing value returns at once.
        assert_eq!(address::wait(&word, 8, Some(5_000_000_000)), WaitOutcome::Woken);
        let other = word.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            other.store(9, Ordering::SeqCst);
            address::wake(&other);
        });
        let start = Instant::now();
        while word.load(Ordering::SeqCst) == 7 {
            address::wait(&word, 7, Some(5_000_000_000));
        }
        assert!(start.elapsed() < Duration::from_secs(2));
        handle.join().unwrap();
    }

    #[test]
    fn pipe_wait_times_out_wakes_and_reports_closed_peer() {
        let (read_end, write_end) = pipe_pair().unwrap();
        let waiter = Waiter::pipe(read_end);
        let notifier = Notifier::pipe(write_end);
        let word = AtomicU32::new(1);
        assert_eq!(waiter.wait(&word, 1, Some(10_000_000)), WaitOutcome::TimedOut);
        word.store(2, Ordering::SeqCst);
        notifier.notify(&word);
        assert_eq!(waiter.wait(&word, 1, None), WaitOutcome::Woken); // value differs
        assert_eq!(waiter.wait(&word, 2, Some(1_000_000_000)), WaitOutcome::Woken); // byte pending
        assert_eq!(waiter.wait(&word, 2, Some(5_000_000)), WaitOutcome::TimedOut); // drained
        drop(notifier);
        assert_eq!(waiter.wait(&word, 2, Some(1_000_000_000)), WaitOutcome::PeerGone);
    }
}

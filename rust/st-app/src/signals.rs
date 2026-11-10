//! Termination signals (Unix): SIGINT, SIGTERM and SIGHUP end the host in an orderly way.
//!
//! No signal handler runs application code. The three signals are blocked in the main thread
//! before any other thread exists ([`block`]), so every thread inherits the block, and one
//! thread of ours waits for them with `sigwait` ([`watch`]) and does the work as ordinary code.
//! Child processes are not affected: `std::process::Command` starts them with an empty signal
//! mask.

use std::sync::atomic::{AtomicBool, Ordering};

const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

fn set() -> libc::sigset_t {
    // SAFETY: `sigemptyset` initializes the set it is given; `sigaddset` is called with valid
    // signal numbers on that initialized set.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in SIGNALS {
            libc::sigaddset(&mut set, signal);
        }
        set
    }
}

/// Blocks the termination signals in the calling thread. Call first thing in `main`.
pub fn block() {
    let set = set();
    // SAFETY: `set` is an initialized signal set; a null old-set pointer is allowed.
    unsafe {
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// Starts the thread that waits for a termination signal. On the first one it runs `shutdown`
/// (on a thread of its own) and exits the process with 128 + the signal's number when that has
/// returned; a second signal meanwhile exits at once. Call after [`block`].
pub fn watch(shutdown: impl FnOnce(i32) + Send + 'static) {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("st-app signals".to_string()).spawn(move || {
        let set = set();
        let wait = || -> libc::c_int {
            let mut signal: libc::c_int = 0;
            loop {
                // SAFETY: `set` is an initialized signal set and `signal` a valid place for the result.
                if unsafe { libc::sigwait(&set, &mut signal) } == 0 {
                    return signal;
                }
            }
        };
        let first = wait();
        let _ = std::thread::Builder::new().name("st-app shutdown".to_string()).spawn(move || {
            shutdown(first);
            std::process::exit(128 + first);
        });
        // Impatience: the second signal does not wait for the shutdown.
        let second = wait();
        std::process::exit(128 + second);
    });
    if spawned.is_err() {
        // Without the thread nobody would ever see the signals: let them end the process as usual.
        let set = set();
        // SAFETY: as in `block`.
        unsafe {
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        }
    }
}

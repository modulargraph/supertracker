//! Small Objective-C helpers shared by the Audio Unit backend (macOS only).

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_core_foundation::{kCFRunLoopDefaultMode, CFRunLoop};
use objc2_foundation::{NSError, NSString};
use stpc_runtime::PluginError;

/// Runs `f`, turning an Objective-C exception into an error. Audio Units and the bridges
/// around them raise for many things a host cannot rule out beforehand (a malformed state
/// dictionary, an index out of range in a third-party bus array, key-value coding on a
/// missing key). Without this an exception would unwind through the runtime.
pub(super) fn guard<R>(what: &str, f: impl FnOnce() -> R) -> Result<R, PluginError> {
    objc2::exception::catch(AssertUnwindSafe(f)).map_err(|exception| {
        let reason = match exception {
            Some(exception) => format!("{exception:?}"),
            None => "nil exception".to_string(),
        };
        PluginError::failed(format!("The Audio Unit raised an exception while {what}: {reason}"))
    })
}

/// Like [`guard`] for calls whose failure only deserves a log line.
pub(super) fn guard_quiet<R>(what: &str, f: impl FnOnce() -> R) -> Option<R> {
    match guard(what, f) {
        Ok(value) => Some(value),
        Err(error) => {
            eprintln!("[au] {error}");
            None
        }
    }
}

pub(super) fn ns(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

/// "description (domain code)".
pub(super) fn describe_error(error: &NSError) -> String {
    format!("{} ({} {})", error.localizedDescription(), error.domain(), error.code())
}

/// Runs the main run loop (default mode) until `done` is set or `timeout` has passed;
/// returns whether it was set. For Audio Unit calls that answer through a completion
/// handler (instantiation, the view controller request) from calls that must return a
/// result: the main thread keeps serving its run loop meanwhile, which is also what those
/// handlers may need to be delivered. The container's own request source is not
/// re-entered (see `macos::app`).
pub(super) fn spin_until(done: &AtomicBool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    // SAFETY: reading an immutable system constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    while !done.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            return false;
        }
        CFRunLoop::run_in_mode(mode, 0.02, true);
    }
    true
}

/// Wakes the main run loop from any thread (after setting a flag [`spin_until`] waits for).
pub(super) fn wake_main_run_loop() {
    if let Some(run_loop) = CFRunLoop::main() {
        run_loop.wake_up();
    }
}

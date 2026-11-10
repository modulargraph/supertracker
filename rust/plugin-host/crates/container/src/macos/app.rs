//! The container's main thread on macOS: an `NSApplication` event loop that drives the
//! runtime.
//!
//! AppKit and Audio Units want the process's main thread: windows, view controllers and
//! most `AUAudioUnit` calls belong there. So the main thread runs `[NSApp run]`, and the
//! runtime's work is injected into that loop:
//!
//! * [`Runtime::set_main_waker`] gets a closure that signals a run-loop source (and wakes
//!   the loop). The source is scheduled in the *common* modes, so requests are also served
//!   while AppKit runs the loop in a tracking or modal mode (a slider being dragged in a
//!   plug-in editor, an open menu, a plug-in's alert). Its callback calls
//!   [`Runtime::run_pending`], which executes every queued request, i.e. every `Plugin`
//!   method, on this thread.
//! * A repeating timer calls `run_pending` as well: it drives the runtime's periodic
//!   `Plugin::idle` (see [`IDLE_INTERVAL`]) and is the safety net should a wake-up ever
//!   be lost.
//! * `run_pending` is never entered recursively. A plug-in may spin the run loop from
//!   inside one of its calls (some do, to wait for something; so does this crate while it
//!   waits for an asynchronous Audio Unit call to complete): requests that arrive
//!   meanwhile are served when the outer call has returned.
//!
//! The application is an *accessory* (no Dock icon, no menu bar of its own, but its
//! windows can become key). It still installs a main menu, which is never shown: AppKit
//! routes the standard key equivalents (copy, paste, undo, close window, ...) through the
//! main menu, and without one a text field in a plug-in editor cannot paste. There is
//! deliberately no Quit item: Command-Q in a plug-in window must not take the plug-ins
//! down. The process ends when the host says so (or disappears).

use std::cell::Cell;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::Sel;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem,
};
use objc2_core_foundation::{
    kCFRunLoopCommonModes, CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext,
    CFRunLoopTimer, CFRunLoopTimerContext,
};
use objc2_foundation::{NSPoint, NSString};
use stpc_runtime::{LoopState, Runtime};

/// The idle interval the container configures the runtime with on macOS
/// (`RuntimeConfig::idle_interval`): how often `Plugin::idle` runs without being asked.
/// The Audio Unit backend uses it to notice latency, tail and parameter-tree changes that
/// a unit does not announce.
pub const IDLE_INTERVAL: Duration = Duration::from_millis(200);

/// How often the loop looks at the runtime without having been woken.
const TICK_SECONDS: f64 = 0.05;

struct Loop {
    runtime: *mut Runtime,
    app: Retained<NSApplication>,
    source: CFRetained<CFRunLoopSource>,
    /// Inside `Runtime::run_pending`.
    busy: Cell<bool>,
    /// A wake-up arrived while busy.
    again: Cell<bool>,
    exiting: Cell<bool>,
}

/// What the waker needs; these Core Foundation calls are thread-safe.
struct Waker {
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
}
// SAFETY: CFRunLoopSourceSignal and CFRunLoopWakeUp may be called from any thread, and
// retaining / releasing Core Foundation objects is thread-safe.
unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

impl Waker {
    fn wake(&self) {
        self.source.signal();
        self.run_loop.wake_up();
    }
}

impl Loop {
    /// Runs the runtime's pending work, unless that is already happening further up the
    /// stack.
    fn pump(&self) {
        if self.exiting.get() {
            self.leave();
            return;
        }
        if self.busy.replace(true) {
            self.again.set(true);
            return;
        }
        let state = autoreleasepool(|_| {
            // An Objective-C exception from a plug-in (or from AppKit on its behalf) must
            // not unwind through the run loop: report it and keep serving.
            objc2::exception::catch(AssertUnwindSafe(|| {
                // SAFETY: `runtime` outlives the loop (see `run`), this is the main thread,
                // and `busy` guarantees there is no second reference.
                unsafe { (*self.runtime).run_pending() }
            }))
        });
        self.busy.set(false);
        match state {
            Ok(LoopState::Exit) => {
                self.exiting.set(true);
                self.leave();
            }
            Ok(LoopState::Running) => {}
            Err(exception) => {
                let what = exception.map(|e| format!("{e:?}")).unwrap_or_else(|| "nil".into());
                eprintln!("supertracker-plugin-container: uncaught Objective-C exception in a request: {what}");
            }
        }
        if self.again.replace(false) {
            // Serve what arrived meanwhile in a fresh pass of the loop.
            self.source.signal();
        }
    }

    /// Makes `[NSApp run]` return. `stop:` only takes effect once the loop has handled an
    /// event, so one is posted; if a nested (modal) loop swallows the request, the timer
    /// asks again.
    fn leave(&self) {
        self.app.stop(None);
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        );
        if let Some(event) = event {
            self.app.postEvent_atStart(&event, true);
        }
    }
}

unsafe extern "C-unwind" fn source_fired(info: *mut c_void) {
    // SAFETY: `info` is the `Loop` that `run` keeps alive while the loop runs.
    unsafe { (*(info as *const Loop)).pump() }
}

unsafe extern "C-unwind" fn timer_fired(_timer: *mut CFRunLoopTimer, info: *mut c_void) {
    // SAFETY: as above.
    unsafe { (*(info as *const Loop)).pump() }
}

/// Runs the container: makes this process an accessory application, installs the hidden
/// main menu, and runs the AppKit event loop, serving `runtime` from it, until the host
/// asks the container to shut down. Call it on the main thread after `Runtime::connect`.
pub fn run(runtime: &mut Runtime) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("supertracker-plugin-container: the event loop must run on the main thread");
        runtime.run();
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    install_main_menu(&app, mtm);

    let (Some(run_loop), Some(common_modes)) = (CFRunLoop::main(), (unsafe { kCFRunLoopCommonModes })) else {
        runtime.run();
        return;
    };
    // From here on the runtime is only reached through this pointer (by `Loop::pump`).
    let runtime: *mut Runtime = runtime;
    // SAFETY (for the fallbacks below): nothing else uses the runtime yet.
    let fallback = |runtime: *mut Runtime| unsafe { (*runtime).run() };
    // The source's context points at `state`, which is filled in below; the allocation is
    // intentionally leaked: sources and timers may fire for as long as the process lives.
    let state: *mut Loop = Box::into_raw(Box::new(std::mem::MaybeUninit::<Loop>::uninit())).cast();
    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: state.cast(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(source_fired),
    };
    // SAFETY: the context is valid for the call (it is copied); `state` is initialised
    // before the source can fire (it fires only from the run loop, which runs below).
    let Some(source) = (unsafe { CFRunLoopSource::new(None, 0, &mut context) }) else {
        fallback(runtime);
        return;
    };
    // SAFETY: `state` points to uninitialised memory of the right type.
    unsafe {
        state.write(Loop {
            runtime,
            app: app.clone(),
            source: source.clone(),
            busy: Cell::new(false),
            again: Cell::new(false),
            exiting: Cell::new(false),
        });
    }
    run_loop.add_source(Some(&source), Some(common_modes));

    let mut timer_context =
        CFRunLoopTimerContext { version: 0, info: state.cast(), retain: None, release: None, copyDescription: None };
    // SAFETY: the context is copied; the callback gets the leaked `state`.
    let timer = unsafe {
        CFRunLoopTimer::new(
            None,
            CFAbsoluteTimeGetCurrent() + TICK_SECONDS,
            TICK_SECONDS,
            0,
            0,
            Some(timer_fired),
            &mut timer_context,
        )
    };
    if let Some(timer) = &timer {
        run_loop.add_timer(Some(timer), Some(common_modes));
    }

    let waker = Arc::new(Waker { source: source.clone(), run_loop: run_loop.clone() });
    let for_runtime = waker.clone();
    // SAFETY: the loop is not running yet, so `pump` is not using the runtime.
    unsafe { (*runtime).set_main_waker(Arc::new(move || for_runtime.wake())) };
    // Requests may have been queued before the waker existed.
    waker.wake();

    app.run();

    // The host asked for shutdown and every instance is destroyed. Nothing may call into
    // the runtime any more.
    // SAFETY: `state` is still allocated (it is never freed).
    unsafe { (*state).exiting.set(true) };
    if let Some(timer) = &timer {
        timer.invalidate();
    }
    source.invalidate();
}

fn menu_item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str) -> Retained<NSMenuItem> {
    // SAFETY: the action selectors are standard responder-chain actions (nil target).
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

/// The hidden main menu that makes the standard key equivalents work in plug-in windows.
fn install_main_menu(app: &NSApplication, mtm: MainThreadMarker) {
    let main = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Main"));
    let add_menu = |title: &str, items: &[(&str, Sel, &str, bool)]| -> Retained<NSMenu> {
        let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
        for &(item_title, action, key, shift) in items {
            let item = menu_item(mtm, item_title, Some(action), key);
            if shift {
                item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Shift);
            }
            menu.addItem(&item);
        }
        let holder = menu_item(mtm, title, None, "");
        holder.setSubmenu(Some(&menu));
        main.addItem(&holder);
        menu
    };
    // The first menu is the application menu; it stays empty (no Quit, no Hide).
    add_menu("Plug-in", &[]);
    add_menu(
        "Edit",
        &[
            ("Undo", sel!(undo:), "z", false),
            ("Redo", sel!(redo:), "z", true),
            ("Cut", sel!(cut:), "x", false),
            ("Copy", sel!(copy:), "c", false),
            ("Paste", sel!(paste:), "v", false),
            ("Select All", sel!(selectAll:), "a", false),
        ],
    );
    let window = add_menu(
        "Window",
        &[("Close", sel!(performClose:), "w", false), ("Minimize", sel!(performMiniaturize:), "m", false)],
    );
    app.setMainMenu(Some(&main));
    app.setWindowsMenu(Some(&window));
}

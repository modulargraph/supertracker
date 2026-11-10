//! The editor of an Audio Unit instance (macOS only; main thread): the unit's own view
//! controller, or the system's generic parameter editor when it has none, hosted in an
//! [`EditorWindow`].

use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView, NSViewController, NSViewFrameDidChangeNotification};
use objc2_audio_toolbox::AUAudioUnit;
use objc2_core_audio_kit::{AUAudioUnit_ViewController, AUGenericViewController};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize};
use stpc_runtime::{EditorInfo, HostHandle, Notification, PluginError};

use super::objc::{guard, guard_quiet, spin_until, wake_main_run_loop};
use super::observe::Observer;
use crate::macos::EditorWindow;

/// How long a unit may take to produce its view controller before the generic editor is
/// shown instead.
const VIEW_TIMEOUT: Duration = Duration::from_secs(15);
/// Window content size when a view reports none.
const FALLBACK_SIZE: (f64, f64) = (520.0, 400.0);

struct Core {
    // Field order is drop order: the window closes before the view controller goes.
    window: EditorWindow,
    view: Retained<NSView>,
    controller: Retained<NSViewController>,
    resizable: bool,
    /// Inside `fit_window`: frame changes are this code's own.
    adjusting: Cell<bool>,
    host: HostHandle,
}

impl Core {
    /// Makes the window's content area `width` x `height` with the view filling it.
    fn fit_window(&self, width: f64, height: f64) {
        if self.adjusting.replace(true) {
            return;
        }
        // An autoresizing view would grow by the window's change on top of its own.
        let mask = self.view.autoresizingMask();
        self.view.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
        self.window.set_content_size(width, height);
        let (width, height) = self.window.content_size();
        self.view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)));
        self.view.setAutoresizingMask(mask);
        self.adjusting.set(false);
        self.host.notify(Notification::EditorResized { width: width.round() as u32, height: height.round() as u32 });
    }

    /// The view changed its own frame (an editor with a size menu, say): follow it.
    fn follow_view(&self) {
        if self.adjusting.get() || !self.window.is_open() {
            return;
        }
        let size = self.view.frame().size;
        let (width, height) = self.window.content_size();
        if size.width < 1.0 || size.height < 1.0 {
            return;
        }
        if (size.width - width).abs() >= 0.5 || (size.height - height).abs() >= 0.5 {
            self.fit_window(size.width, size.height);
        }
    }

    /// The view controller changed its `preferredContentSize`: follow it.
    fn follow_preferred_size(&self) {
        if !self.window.is_open() {
            return;
        }
        let preferred = self.controller.preferredContentSize();
        let (width, height) = self.window.content_size();
        if preferred.width >= 1.0
            && preferred.height >= 1.0
            && ((preferred.width - width).abs() >= 0.5 || (preferred.height - height).abs() >= 0.5)
        {
            self.fit_window(preferred.width, preferred.height);
        }
    }
}

pub(super) struct Editor {
    core: Rc<Core>,
    observer: Retained<Observer>,
    frame_token: Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    user_closed: Rc<Cell<bool>>,
}

enum Pending {
    Waiting,
    Abandoned,
    /// A retained `NSViewController` pointer, or 0 for "the unit has no view".
    Done(usize),
}

/// Asks the unit for its view controller and waits for the answer (the main run loop is
/// served meanwhile). `None`: the unit has no custom view, or did not answer in time.
fn request_view_controller(unit: &AUAudioUnit) -> Option<Retained<NSViewController>> {
    let done = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(Pending::Waiting));
    let (for_done, for_state) = (done.clone(), state.clone());
    let handler = RcBlock::new(move |controller: *mut NSViewController| {
        // SAFETY: the pointer is valid during the handler; a retained pointer is handed to
        // the main thread.
        let pointer = unsafe { Retained::retain(controller) }.map_or(0, |c| Retained::into_raw(c) as usize);
        let mut state = for_state.lock().unwrap();
        if matches!(*state, Pending::Waiting) {
            *state = Pending::Done(pointer);
        }
        // Abandoned: a late controller stays retained (releasing it off the main thread
        // is not allowed, and nobody will show it).
        drop(state);
        for_done.store(true, Ordering::Release);
        wake_main_run_loop();
    });
    // SAFETY: main thread; the handler is a heap block.
    guard_quiet("creating its editor", || unsafe { unit.requestViewControllerWithCompletionHandler(&handler) })?;
    spin_until(&done, VIEW_TIMEOUT);
    let outcome = std::mem::replace(&mut *state.lock().unwrap(), Pending::Abandoned);
    match outcome {
        // SAFETY: the handler stored a retained pointer; ownership moves here.
        Pending::Done(pointer) if pointer != 0 => unsafe { Retained::from_raw(pointer as *mut NSViewController) },
        _ => None,
    }
}

impl Editor {
    pub fn open(
        unit: &AUAudioUnit,
        title: &str,
        host: &HostHandle,
        observer: &Retained<Observer>,
    ) -> Result<Editor, PluginError> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| PluginError::failed("An Audio Unit editor can only be opened on the main thread."))?;
        let own = request_view_controller(unit);
        let custom = own.is_some();
        let controller = match own {
            Some(controller) => controller,
            None => guard("creating the generic editor", || {
                // SAFETY: main thread. The generic view controller shows the parameter
                // tree of the unit it is given.
                unsafe {
                    let generic = AUGenericViewController::new(mtm);
                    generic.setAuAudioUnit(Some(unit));
                    Retained::into_super(generic)
                }
            })?,
        };
        // Loading the view runs the plug-in's view code.
        let view = guard("loading its editor view", || controller.view())?;
        let preferred = controller.preferredContentSize();
        let frame = view.frame().size;
        let pick = |preferred: f64, frame: f64, fallback: f64| {
            if preferred >= 1.0 {
                preferred
            } else if frame >= 1.0 {
                frame
            } else {
                fallback
            }
        };
        let width = pick(preferred.width, frame.width, FALLBACK_SIZE.0);
        let height = pick(preferred.height, frame.height, FALLBACK_SIZE.1);
        // A view that resizes with its superview in both directions is a resizable
        // editor (the convention Audio Unit hosts follow); the generic editor always is.
        let flexible = NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
        let resizable = !custom || view.autoresizingMask().contains(flexible);

        let user_closed = Rc::new(Cell::new(false));
        let (closed_flag, closed_host) = (user_closed.clone(), host.clone());
        let window = EditorWindow::open(
            title,
            Box::new(move || {
                // Only record it: the editor is dismantled from `Plugin::idle`.
                closed_flag.set(true);
                closed_host.notify(Notification::EditorClosed);
                closed_host.request_main_callback();
            }),
        )
        .map_err(PluginError::failed)?;
        window.set_content_size(width, height);
        let (width, height) = window.content_size();
        // SAFETY: `content_view` is the window's content NSView, alive as long as `window`.
        let content: &NSView = unsafe { &*(window.content_view() as *const NSView) };
        guard("showing its editor view", || {
            view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)));
            if resizable {
                view.setAutoresizingMask(flexible);
            }
            content.addSubview(&view);
            view.setPostsFrameChangedNotifications(true);
        })?;

        let core =
            Rc::new(Core { window, view, controller, resizable, adjusting: Cell::new(false), host: host.clone() });
        let resize_host = host.clone();
        core.window.set_resizable(
            resizable,
            Some(Box::new(move |width, height| {
                resize_host
                    .notify(Notification::EditorResized { width: width.round() as u32, height: height.round() as u32 });
            })),
        );

        let weak = Rc::downgrade(&core);
        let frame_block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            if let Some(core) = weak.upgrade() {
                core.follow_view();
            }
        });
        let observed: &AnyObject = &core.view;
        // SAFETY: a nil queue delivers synchronously on the posting thread, which for a
        // view's frame change is the main thread.
        let frame_token = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSViewFrameDidChangeNotification),
                Some(observed),
                None,
                &frame_block,
            )
        };
        observer.watch_editor(&core.controller);
        Ok(Editor { core, observer: observer.clone(), frame_token: Some(frame_token), user_closed })
    }

    pub fn info(&self) -> EditorInfo {
        let (width, height) = self.core.window.content_size();
        EditorInfo {
            width: width.round() as u32,
            height: height.round() as u32,
            resizable: self.core.resizable,
            window_id: self.core.window.window_number(),
        }
    }

    /// The user closed the window.
    pub fn user_closed(&self) -> bool {
        self.user_closed.get()
    }

    pub fn follow_preferred_size(&self) {
        self.core.follow_preferred_size();
    }

    pub fn focus(&self) {
        self.core.window.focus();
    }

    pub fn set_title(&self, title: &str) {
        self.core.window.set_title(title);
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        if let Some(token) = self.frame_token.take() {
            let token: &AnyObject = AsRef::<AnyObject>::as_ref(&*token);
            // SAFETY: removes the block observer registered in `open`.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(token) };
        }
        self.observer.unwatch_editor(&self.core.controller);
        guard_quiet("closing its editor", || self.core.view.removeFromSuperview());
        // `core` goes next: the window closes, then the view and its controller are
        // released (running the plug-in's view teardown), all on the main thread.
    }
}

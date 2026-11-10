//! The window a plug-in editor is shown in (Audio Unit view controllers, VST3 views).
//! Main thread only.
//!
//! # How editor windows behave next to the host application
//!
//! The windows belong to the container process, an accessory application, not to the
//! host. Left alone they would behave like the windows of an unrelated application: a
//! click into the host's main window would bury them. Instead (mode `floating`, the
//! default):
//!
//! * An editor window is a titled, closable, miniaturisable window at the *floating*
//!   level, so it stays above the host's windows while the user works in the host, as a
//!   plug-in window does in a host that owns it.
//! * Floating windows of a background application would also cover every other
//!   application. So the container watches which application is frontmost: while it is
//!   the host (this process's parent), this container, or another container of the same
//!   host, the windows are shown; when any other application comes to the front they are
//!   ordered out, and they come back when the host or a container is frontmost again.
//!   That is what "hides on deactivate" does for the floating panels of a single
//!   application, extended over the host and its containers.
//! * Opening a window does not take the keyboard away from the host: the window is
//!   ordered front without activating this application. A click into it activates the
//!   container (the window becomes key, its text fields work), and [`EditorWindow::focus`]
//!   does the same on the host's request (`Plugin::focus_editor`).
//! * The windows may join a full-screen space (the host in full screen) and move to the
//!   active space when shown. They are excluded from window tabbing.
//!
//! `STPC_EDITOR_WINDOWS=normal` in the container's environment selects plain windows at
//! the normal level without the frontmost-application tracking (they then order like any
//! other application's windows; the host raises one with `focus_editor`).
//!
//! None of this has run on a Mac yet; the mode switch exists so that the behaviour can be
//! compared there. Known open points: the host yielding activation on macOS 14 and later
//! (`focus`), Stage Manager, and several displays in full screen.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSFloatingWindowLevel, NSRunningApplication, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask, NSWindowTabbingMode, NSWorkspace,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::{NSNotification, NSPoint, NSRect, NSSize, NSString};

/// Content size of a new window until the owner sets one.
const INITIAL_SIZE: (f64, f64) = (480.0, 320.0);

struct Shared {
    window: RefCell<Option<Retained<NSWindow>>>,
    content: RefCell<Option<Retained<NSView>>>,
    on_close: RefCell<Option<Box<dyn FnMut()>>>,
    on_resize: RefCell<Option<Box<dyn FnMut(f64, f64)>>>,
    /// Bumped whenever `on_resize` is replaced, so a running callback is not put back.
    resize_generation: Cell<u64>,
    /// Closed by the user or by the owner: no callback fires any more.
    closed: Cell<bool>,
    /// The owner itself is resizing the window: do not report it back.
    quiet: Cell<bool>,
    /// Ordered out because an unrelated application is frontmost.
    hidden_by_policy: Cell<bool>,
    last_size: Cell<(f64, f64)>,
}

impl Shared {
    fn user_closed(self: &Rc<Self>) {
        if self.closed.replace(true) {
            return;
        }
        unregister(self);
        // Taken out first: the callback may drop the `EditorWindow`.
        let callback = self.on_close.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback();
        }
    }

    fn resized(&self) {
        if self.closed.get() || self.quiet.get() {
            return;
        }
        let Some(size) = self.content.borrow().as_ref().map(|content| content.frame().size) else { return };
        let size = (size.width, size.height);
        if self.last_size.replace(size) == size {
            return;
        }
        // The callback may call `set_resizable` (replacing itself): take it out for the
        // call and put it back only if nothing replaced it.
        let generation = self.resize_generation.get();
        let callback = self.on_resize.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback(size.0, size.1);
            if self.resize_generation.get() == generation {
                *self.on_resize.borrow_mut() = Some(callback);
            }
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "STPCEditorWindowDelegate"]
    #[ivars = Rc<Shared>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            // The callback may drop the `EditorWindow` and with it the last reference to
            // this delegate: keep both alive until the call is over.
            let _keep = self.retain();
            let shared = self.ivars().clone();
            shared.user_closed();
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            let _keep = self.retain();
            let shared = self.ivars().clone();
            shared.resized();
        }
    }
);

impl Delegate {
    fn new(shared: Rc<Shared>, mtm: MainThreadMarker) -> Retained<Delegate> {
        let this = Delegate::alloc(mtm).set_ivars(shared);
        // SAFETY: plain NSObject initialisation.
        unsafe { msg_send![super(this), init] }
    }
}

// ---- frontmost-application policy ---------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Floating,
    Normal,
}

thread_local! {
    static MODE: Mode = match std::env::var("STPC_EDITOR_WINDOWS").as_deref() {
        Ok("normal") => Mode::Normal,
        _ => Mode::Floating,
    };
    /// Open editor windows of this process.
    static WINDOWS: RefCell<Vec<Rc<Shared>>> = const { RefCell::new(Vec::new()) };
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// Where the next window goes (each one is offset from the previous).
    static CASCADE: Cell<Option<NSPoint>> = const { Cell::new(None) };
}

fn unregister(shared: &Rc<Shared>) {
    WINDOWS.with(|windows| windows.borrow_mut().retain(|entry| !Rc::ptr_eq(entry, shared)));
}

/// Whether the frontmost application is the host, this container or a sibling container.
fn frontmost_is_ours() -> bool {
    let Some(front) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
        return true;
    };
    let pid = front.processIdentifier();
    // SAFETY: getppid has no preconditions.
    let host = unsafe { libc::getppid() };
    if pid == std::process::id() as libc::pid_t || pid == host || host <= 1 {
        return true;
    }
    // Another container of the same host runs the same executable.
    let path = |app: &NSRunningApplication| app.executableURL().and_then(|url| url.path()).map(|p| p.to_string());
    match (path(&front), path(&NSRunningApplication::currentApplication())) {
        (Some(theirs), Some(ours)) => theirs == ours,
        _ => false,
    }
}

fn apply_policy() {
    let ours = frontmost_is_ours();
    let windows: Vec<Rc<Shared>> = WINDOWS.with(|windows| windows.borrow().clone());
    for shared in windows {
        if shared.closed.get() {
            continue;
        }
        let Some(window) = shared.window.borrow().clone() else { continue };
        if ours {
            if shared.hidden_by_policy.replace(false) {
                window.orderFrontRegardless();
            }
        } else if window.isVisible() && !window.isMiniaturized() {
            shared.hidden_by_policy.set(true);
            window.orderOut(None);
        }
    }
}

/// Starts watching which application is frontmost (once per process).
fn start_tracking() {
    if TRACKING.replace(true) {
        return;
    }
    let block = RcBlock::new(|_notification: NonNull<NSNotification>| apply_policy());
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: a nil queue delivers on the posting thread, which for workspace
    // notifications is the main thread; the block only touches main-thread state.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidActivateApplicationNotification),
            None,
            None,
            &block,
        )
    };
    // Observed for the life of the process.
    std::mem::forget(token);
}

// ---- the window ---------------------------------------------------------------------------

/// A window for one plug-in editor. Created, used and dropped on the main thread.
///
/// The owner embeds its view into [`EditorWindow::content_view`] (a plain `NSView` that
/// always fills the window's content area) and removes it again before closing.
/// Dropping the value closes the window.
pub struct EditorWindow {
    window: Retained<NSWindow>,
    content: Retained<NSView>,
    _delegate: Retained<Delegate>,
    shared: Rc<Shared>,
}

impl EditorWindow {
    /// Creates and shows a window titled `title`. `on_close` is called once if the *user*
    /// closes the window (not by [`EditorWindow::close`] or a drop); it runs on the main
    /// thread from inside the window's close handling, so it should only record the fact
    /// (set a flag, notify the host) and leave tearing the editor down to a later call.
    pub fn open(title: &str, on_close: Box<dyn FnMut()>) -> Result<EditorWindow, String> {
        let mtm = MainThreadMarker::new().ok_or("editor windows can only be opened on the main thread")?;
        let mode = MODE.with(|mode| *mode);
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(INITIAL_SIZE.0, INITIAL_SIZE.1));
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
        // SAFETY: a plain window; `releasedWhenClosed` is switched off right below, so the
        // window lives exactly as long as it is retained here.
        let window = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            window
        };
        window.setTitle(&NSString::from_str(title));
        window.setTabbingMode(NSWindowTabbingMode::Disallowed);
        window.setRestorable(false);
        let content = NSView::initWithFrame(mtm.alloc(), rect);
        window.setContentView(Some(&content));

        let shared = Rc::new(Shared {
            window: RefCell::new(Some(window.clone())),
            content: RefCell::new(Some(content.clone())),
            on_close: RefCell::new(Some(on_close)),
            on_resize: RefCell::new(None),
            resize_generation: Cell::new(0),
            closed: Cell::new(false),
            quiet: Cell::new(false),
            hidden_by_policy: Cell::new(false),
            last_size: Cell::new(INITIAL_SIZE),
        });
        let delegate = Delegate::new(shared.clone(), mtm);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        window.center();
        let top_left = CASCADE.with(|cascade| {
            let next = window.cascadeTopLeftFromPoint(cascade.get().unwrap_or(NSPoint::new(0.0, 0.0)));
            cascade.set(Some(next));
            next
        });
        let _ = top_left;

        WINDOWS.with(|windows| windows.borrow_mut().push(shared.clone()));
        let mut behavior = NSWindowCollectionBehavior::FullScreenAuxiliary;
        let mut show = true;
        if mode == Mode::Floating {
            window.setLevel(NSFloatingWindowLevel);
            window.setHidesOnDeactivate(false);
            behavior |= NSWindowCollectionBehavior::MoveToActiveSpace;
            start_tracking();
            show = frontmost_is_ours();
        }
        window.setCollectionBehavior(behavior);
        if show {
            // Visible without activating this application: the host keeps the keyboard.
            window.orderFrontRegardless();
        } else {
            shared.hidden_by_policy.set(true);
        }
        Ok(EditorWindow { window, content, _delegate: delegate, shared })
    }

    /// The `NSView*` to embed the editor's view into. It fills the window's content area
    /// and is resized with the window. Valid until the `EditorWindow` is dropped.
    pub fn content_view(&self) -> *mut c_void {
        Retained::as_ptr(&self.content) as *mut c_void
    }

    /// Resizes the window so that its content area is `width` x `height` points; the top
    /// left corner stays where it is. Does not call the resize callback.
    pub fn set_content_size(&self, width: f64, height: f64) {
        if self.shared.closed.get() || !width.is_finite() || !height.is_finite() {
            return;
        }
        let (width, height) = (width.clamp(1.0, 16384.0).round(), height.clamp(1.0, 16384.0).round());
        let frame = self.window.frame();
        let wanted = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
        let mut next = self.window.frameRectForContentRect(wanted);
        next.origin.x = frame.origin.x;
        next.origin.y = frame.origin.y + frame.size.height - next.size.height;
        let was_quiet = self.shared.quiet.replace(true);
        self.window.setFrame_display(next, true);
        self.shared.quiet.set(was_quiet);
        self.shared.last_size.set((width, height));
    }

    /// The current size of the content area in points.
    pub fn content_size(&self) -> (f64, f64) {
        let size = self.content.frame().size;
        (size.width, size.height)
    }

    /// Lets the user resize the window, or not. `on_resize` replaces the previous
    /// callback; it is called with the new content size when the *user* resizes the
    /// window (also repeatedly during a live resize).
    pub fn set_resizable(&self, resizable: bool, on_resize: Option<Box<dyn FnMut(f64, f64)>>) {
        self.shared.resize_generation.set(self.shared.resize_generation.get() + 1);
        *self.shared.on_resize.borrow_mut() = on_resize;
        if self.shared.closed.get() {
            return;
        }
        let mut style = self.window.styleMask();
        style.set(NSWindowStyleMask::Resizable, resizable);
        self.window.setStyleMask(style);
    }

    pub fn set_title(&self, title: &str) {
        self.window.setTitle(&NSString::from_str(title));
    }

    /// Brings the window to the front and gives it the keyboard: activates this
    /// (accessory) application and makes the window key. On macOS 14 and later the system
    /// honours the activation fully only if the frontmost application yields to this
    /// process; the host should do that before it asks for focus.
    pub fn focus(&self) {
        if self.shared.closed.get() {
            return;
        }
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
        if self.window.isMiniaturized() {
            self.window.deminiaturize(None);
        }
        self.shared.hidden_by_policy.set(false);
        self.window.makeKeyAndOrderFront(None);
    }

    /// The system's window number (`EditorInfo::window_id`).
    pub fn window_number(&self) -> u64 {
        self.window.windowNumber().max(0) as u64
    }

    /// False once the user has closed the window.
    pub fn is_open(&self) -> bool {
        !self.shared.closed.get()
    }

    /// Closes the window without calling `on_close`.
    pub fn close(self) {
        drop(self);
    }

    fn shut(&self) {
        let was_open = !self.shared.closed.replace(true);
        unregister(&self.shared);
        self.window.setDelegate(None);
        if was_open {
            self.window.orderOut(None);
            self.window.close();
        }
        // Break the reference cycle through the shared state and release the callbacks.
        self.shared.window.borrow_mut().take();
        self.shared.content.borrow_mut().take();
        self.shared.on_close.borrow_mut().take();
        self.shared.resize_generation.set(self.shared.resize_generation.get() + 1);
        self.shared.on_resize.borrow_mut().take();
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        self.shut();
    }
}

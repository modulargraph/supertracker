//! The plug-in's editor: an `IPlugView` created on the main thread and attached to a
//! native parent the container provides.
//!
//! * **macOS**: the parent is the content view of a
//!   [`crate::macos::editor_window::EditorWindow`] (`kPlatformTypeNSView`). The host
//!   interface `IPlugFrame` lets the plug-in resize the window; a view that answers
//!   `canResize` gets a resizable window whose size changes go through
//!   `checkSizeConstraint` and `onSize`. **Type-checked only.**
//! * **Elsewhere**: no editor yet. On Linux an editor needs an X11 parent window
//!   (`kPlatformTypeX11EmbedWindowID`), XEmbed, and the `IRunLoop` host interface driven
//!   by an event loop the container does not have; [`Editor::open`] reports
//!   "unsupported" and instances do not advertise an editor.

use std::sync::Arc;

use stpc_runtime::{EditorInfo, PluginError};
use vst3::Steinberg::Vst::IEditController;
use vst3::ComPtr;

use super::process::Shared;

#[cfg(target_os = "macos")]
pub use mac::Editor;

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use stpc_runtime::{HostHandle, Notification};
    use vst3::Steinberg::Vst::{IEditControllerTrait, ViewType};
    use vst3::Steinberg::{
        kInvalidArgument, kPlatformTypeNSView, kResultOk, kResultTrue, tresult, IPlugFrame, IPlugFrameTrait, IPlugView,
        IPlugViewTrait, ViewRect,
    };
    use vst3::{Class, ComWrapper};

    use super::*;
    use crate::macos::editor_window::EditorWindow;

    /// What the frame object, the window callbacks and the editor share.
    struct FrameState {
        window: RefCell<Option<EditorWindow>>,
        /// The attached view (not add-ref'ed: the editor owns it and outlives its use here).
        view: Cell<*mut IPlugView>,
        handle: HostHandle,
        /// The user closed the window; the editor is dismantled from `Plugin::idle`.
        close_requested: Cell<bool>,
    }

    /// `IPlugFrame`: the view asks the host to resize its parent.
    struct PlugFrame {
        state: Rc<FrameState>,
    }

    impl Class for PlugFrame {
        type Interfaces = (IPlugFrame,);
    }

    impl IPlugFrameTrait for PlugFrame {
        unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
            if view.is_null() || new_size.is_null() || view != self.state.view.get() {
                return kInvalidArgument;
            }
            let size = *new_size;
            let (width, height) = (size.right - size.left, size.bottom - size.top);
            if width <= 0 || height <= 0 {
                return kInvalidArgument;
            }
            if let Some(window) = self.state.window.borrow().as_ref() {
                // Programmatic: the window does not report this back through its resize callback.
                window.set_content_size(width as f64, height as f64);
            }
            self.state.handle.notify(Notification::EditorResized { width: width as u32, height: height as u32 });
            match vst3::ComRef::from_raw(view) {
                Some(view) => view.onSize(new_size),
                None => kInvalidArgument,
            }
        }
    }

    pub struct Editor {
        view: ComPtr<IPlugView>,
        /// Kept alive as long as the view may call it.
        _frame: ComWrapper<PlugFrame>,
        state: Rc<FrameState>,
    }

    unsafe fn view_size(view: &ComPtr<IPlugView>) -> Option<(i32, i32)> {
        let mut rect = ViewRect { left: 0, top: 0, right: 0, bottom: 0 };
        let known = view.getSize(&mut rect) == kResultOk && rect.right > rect.left && rect.bottom > rect.top;
        known.then(|| (rect.right - rect.left, rect.bottom - rect.top))
    }

    impl Editor {
        /// Does the plug-in have an editor this host can show? Creates a view to ask and
        /// releases it again.
        ///
        /// # Safety
        /// Main thread.
        pub unsafe fn probe(controller: &ComPtr<IEditController>) -> bool {
            match ComPtr::from_raw(controller.createView(ViewType::kEditor)) {
                Some(view) => view.isPlatformTypeSupported(kPlatformTypeNSView) == kResultTrue,
                None => false,
            }
        }

        /// # Safety
        /// Main thread.
        pub unsafe fn open(
            controller: &ComPtr<IEditController>,
            shared: &Arc<Shared>,
            title: &str,
        ) -> Result<(Editor, EditorInfo), PluginError> {
            let Some(view) = ComPtr::from_raw(controller.createView(ViewType::kEditor)) else {
                return Err(PluginError::Unsupported);
            };
            if view.isPlatformTypeSupported(kPlatformTypeNSView) != kResultTrue {
                return Err(PluginError::Unsupported);
            }
            let state = Rc::new(FrameState {
                window: RefCell::new(None),
                view: Cell::new(view.as_ptr()),
                handle: shared.handle.clone(),
                close_requested: Cell::new(false),
            });
            let on_close = {
                let state = Rc::downgrade(&state);
                Box::new(move || {
                    if let Some(state) = state.upgrade() {
                        state.close_requested.set(true);
                        state.handle.notify(Notification::EditorClosed);
                        state.handle.request_main_callback();
                    }
                })
            };
            let window = EditorWindow::open(title, on_close).map_err(PluginError::Failed)?;
            let frame = ComWrapper::new(PlugFrame { state: state.clone() });
            view.setFrame(frame.as_com_ref::<IPlugFrame>().unwrap().as_ptr());
            if let Some((width, height)) = view_size(&view) {
                window.set_content_size(width as f64, height as f64);
            }
            let parent = window.content_view();
            let window_id = window.window_number();
            *state.window.borrow_mut() = Some(window);
            if view.attached(parent, kPlatformTypeNSView) != kResultOk {
                view.setFrame(std::ptr::null_mut());
                if let Some(window) = state.window.borrow_mut().take() {
                    window.close();
                }
                return Err(PluginError::failed("the VST3 plug-in failed to attach its editor"));
            }
            // Many views know their size only once attached.
            let (width, height) = view_size(&view).unwrap_or((0, 0));
            let resizable = view.canResize() == kResultTrue;
            if let Some(window) = state.window.borrow().as_ref() {
                if width > 0 && height > 0 {
                    window.set_content_size(width as f64, height as f64);
                }
                let on_resize: Option<Box<dyn FnMut(f64, f64)>> = if resizable {
                    let state = Rc::downgrade(&state);
                    Some(Box::new(move |width: f64, height: f64| {
                        let Some(state) = state.upgrade() else { return };
                        let Some(view) = vst3::ComRef::from_raw(state.view.get()) else { return };
                        // The user dragged the window: let the view pick the nearest size
                        // it supports, then tell it.
                        let mut rect = ViewRect { left: 0, top: 0, right: width.round() as i32, bottom: height.round() as i32 };
                        view.checkSizeConstraint(&mut rect);
                        view.onSize(&mut rect);
                        let (width, height) = ((rect.right - rect.left).max(0), (rect.bottom - rect.top).max(0));
                        state.handle.notify(Notification::EditorResized { width: width as u32, height: height as u32 });
                    }))
                } else {
                    None
                };
                window.set_resizable(resizable, on_resize);
            }
            let info = EditorInfo { width: width.max(0) as u32, height: height.max(0) as u32, resizable, window_id };
            Ok((Editor { view, _frame: frame, state }, info))
        }

        pub fn close(self) {
            // The window's callbacks go first, then the view leaves its parent, forgets
            // the frame and is released; the window closes last.
            let window = self.state.window.borrow_mut().take();
            if let Some(window) = &window {
                window.set_resizable(false, None);
            }
            self.state.view.set(std::ptr::null_mut());
            // SAFETY: main thread; the view is attached (or its window was closed by the user).
            unsafe {
                self.view.removed();
                self.view.setFrame(std::ptr::null_mut());
            }
            drop(self.view);
            if let Some(window) = window {
                window.close();
            }
        }

        pub fn focus(&mut self) {
            if let Some(window) = self.state.window.borrow().as_ref() {
                window.focus();
            }
        }

        pub fn set_title(&mut self, title: &str) {
            if let Some(window) = self.state.window.borrow().as_ref() {
                window.set_title(title);
            }
        }

        pub fn close_requested(&self) -> bool {
            self.state.close_requested.get()
        }

        pub fn idle(&mut self) {}
    }
}

#[cfg(not(target_os = "macos"))]
pub struct Editor {
    never: std::convert::Infallible,
}

#[cfg(not(target_os = "macos"))]
impl Editor {
    /// No editor support on this platform yet.
    ///
    /// # Safety
    /// None needed; the signature matches the macOS implementation.
    pub unsafe fn probe(_controller: &ComPtr<IEditController>) -> bool {
        false
    }

    /// # Safety
    /// As [`Editor::probe`].
    pub unsafe fn open(
        _controller: &ComPtr<IEditController>,
        _shared: &Arc<Shared>,
        _title: &str,
    ) -> Result<(Editor, EditorInfo), PluginError> {
        Err(PluginError::Unsupported)
    }

    pub fn close(self) {
        match self.never {}
    }

    pub fn focus(&mut self) {}

    pub fn set_title(&mut self, _title: &str) {}

    pub fn close_requested(&self) -> bool {
        false
    }

    pub fn idle(&mut self) {}
}

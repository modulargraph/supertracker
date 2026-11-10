//! macOS only: the container's AppKit main loop ([`app`]) and the window that plug-in
//! editors are shown in ([`editor_window`]). Type-checked from Linux; see the "Audio
//! Units" section of `docs/PLUGIN_CONTAINER.md` for what has to be confirmed on a Mac.

pub mod app;
pub mod editor_window;

pub use editor_window::EditorWindow;

//! Format `"au"`: hosts Audio Units (version 2 components and version 3 extensions)
//! through `AUAudioUnit`. See the "Audio Units" section of `docs/PLUGIN_CONTAINER.md`.
//!
//! The logic that needs no Mac lives in the platform-independent modules below and is
//! tested on Linux; the backend itself exists on macOS only and has so far only been
//! type-checked (`cargo check --target aarch64-apple-darwin`).

pub mod events;
pub mod ident;
pub mod layout;
pub mod params;
pub mod state;
pub mod timing;

pub use ident::{ComponentId, ComponentInfo, ComponentKind, FourCc};

/// Every installed instrument, effect, music-effect and MIDI-effect Audio Unit, sorted by
/// manufacturer and name. Plain data for the plug-in browser; the `plugin_id` of an entry
/// is what `CreateRequest::plugin_id` takes for the format `"au"`.
///
/// Reads the system's component registry (`AVAudioUnitComponentManager`); it does not
/// open any plug-in. On other systems than macOS the list is empty.
pub fn list_components() -> Vec<ComponentInfo> {
    #[cfg(target_os = "macos")]
    {
        listing::list_components()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
mod backend;
#[cfg(target_os = "macos")]
mod editor;
#[cfg(target_os = "macos")]
mod listing;
#[cfg(target_os = "macos")]
mod objc;
#[cfg(target_os = "macos")]
mod observe;
#[cfg(target_os = "macos")]
mod plist;
#[cfg(target_os = "macos")]
mod render;
#[cfg(target_os = "macos")]
mod unit;

#[cfg(target_os = "macos")]
pub use backend::AuBackend;

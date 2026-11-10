//! Plug-in format backends of the `supertracker-plugin-container` executable.
//!
//! * [`test_backend`]: format `"test"`, built-in plug-ins that exercise the transport, the
//!   supervisor and the failure handling (they are what the integration tests load).
//!
//! * [`clap_backend`]: format `"clap"`, CLAP plug-ins.
//!
//! * [`vst3`]: format `"vst3"`, VST3 plug-ins.
//!
//! Further formats (VST3, Audio Units on macOS) are added here as more
//! [`stpc_runtime::PluginBackend`] implementations and registered in `main.rs`.

pub mod au;
pub mod clap_backend;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod test_backend;
pub mod vst3;

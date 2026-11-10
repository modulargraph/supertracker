//! Plug-in format backends of the `supertracker-plugin-container` executable.
//!
//! * [`test_backend`]: format `"test"`, built-in plug-ins that exercise the transport, the
//!   supervisor and the failure handling (they are what the integration tests load).
//!
//! Real formats (CLAP, VST3, Audio Units on macOS) are added here as further
//! [`stpc_runtime::PluginBackend`] implementations and registered in `main.rs`.

pub mod test_backend;

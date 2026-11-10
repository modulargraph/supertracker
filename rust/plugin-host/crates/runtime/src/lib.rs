//! The runtime inside a plug-in container process.
//!
//! A container program (the `supertracker-plugin-container` binary) creates a [`Runtime`]
//! on its main thread, registers one [`PluginBackend`] per plug-in format, connects to the
//! supervisor that spawned it, and then runs the main loop:
//!
//! ```ignore
//! let mut runtime = Runtime::new(RuntimeConfig::from_args(std::env::args().skip(1))?);
//! runtime.register_backend(Box::new(MyBackend));
//! runtime.connect()?;
//! runtime.run();            // or: an OS event loop that calls runtime.run_pending()
//! ```
//!
//! Threads: the main thread executes every control request and every [`Plugin`] method; one
//! I/O thread reads the control channel (so the main thread is never blocked on I/O, and
//! `Ping` is answered even while the main thread is busy); one real-time thread per
//! instance runs [`Processor::process`]. See [`plugin`] for the contract backends rely on.

pub mod plugin;
mod rt;
mod runtime;

pub use plugin::*;
pub use rt::{enable_denormal_flushing, promote_current_thread_to_realtime};
pub use runtime::{abort_on_panic, HostHandle, LoopState, Runtime, RuntimeConfig, RuntimeError};
pub use stpc_transport as transport;

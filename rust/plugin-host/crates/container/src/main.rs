//! `supertracker-plugin-container`: hosts plug-ins for the SuperTracker application in a
//! separate process. Started by the application's plug-in supervisor, never by hand:
//!
//! ```text
//! supertracker-plugin-container --stpc-endpoint <socket path> --stpc-token <token>
//! ```

use stpc_runtime::{Runtime, RuntimeConfig, RuntimeError};

fn main() {
    stpc_runtime::abort_on_panic();
    let mut config = match RuntimeConfig::from_args(std::env::args().skip(1)) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("supertracker-plugin-container: {err}");
            eprintln!("This program is started by SuperTracker to host plug-ins; it has no use on its own.");
            std::process::exit(2);
        }
    };
    // Test hook: announce another protocol version, to exercise the host's rejection.
    if let Some(version) = std::env::var("STPC_TEST_PROTOCOL_VERSION").ok().and_then(|v| v.parse().ok()) {
        config.protocol_version = version;
    }
    let mut runtime = Runtime::new(config);
    runtime.register_backend(Box::new(stpc_container::test_backend::TestBackend));
    if let Err(err) = runtime.connect() {
        eprintln!("supertracker-plugin-container: {err}");
        std::process::exit(if matches!(err, RuntimeError::Rejected(_)) { 3 } else { 4 });
    }
    // The main thread belongs to the plug-ins: every plug-in call except audio processing
    // happens here. A backend with real editor windows replaces this loop by the platform
    // event loop and calls `Runtime::run_pending` from it (see `Runtime::set_main_waker`).
    runtime.run();
}

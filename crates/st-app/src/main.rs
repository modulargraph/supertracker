//! The SuperTracker executable.
//!
//! - Without options it opens the window: the interface in a web view, with native menus and file
//!   choosers (`window.rs`).
//! - With `--serve` it has no window: it serves the interface over HTTP and the bridge over a
//!   WebSocket on 127.0.0.1, for a browser (`serve.rs`).
//!
//! Both run the same host (`st-host`); they differ only in how messages travel.
//!
//! Three more modes have neither window nor host: `--mcp` (the MCP server for AI clients, a
//! proxy to the running application's control socket), `--ctl` (one call through that socket)
//! and `--mk3-probe` (the Maschine MK3 hardware diagnostic).
//!
//! # Ending
//!
//! However the host ends, its services are shut down ([`st_host::areas::services::Services::shutdown`]):
//! the AI control socket is closed, the Maschine MK3 is left dark with Native Instruments' agents
//! restored, external MIDI instruments are silenced. That happens on Quit (both modes) and on
//! SIGINT, SIGTERM and SIGHUP (`signals.rs`), which also write the recovery copy first.

mod cli;
mod menus;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native;
mod serve;
#[cfg(unix)]
mod signals;
mod ui;
mod websocket;
mod window;

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use st_host::{install, Bridge, Dialogs, Host, HostOptions, NoDialogs, ScriptedDialogs};

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args_os().skip(1), |name| std::env::var_os(name)) {
        Ok(cli::Parsed::Run(options)) => options,
        Ok(cli::Parsed::Mcp) => return exit_code(st_host::areas::agent::run_mcp()),
        Ok(cli::Parsed::Control(arguments)) => return exit_code(st_host::areas::agent::run_control(&arguments, std::io::stdout(), std::io::stderr())),
        Ok(cli::Parsed::Mk3Probe { seconds, exclusive, raw }) => {
            return match st_host::support_dir() {
                Ok(support) => exit_code(st_host::areas::mk3::probe(&support, seconds, exclusive, raw)),
                Err(error) => {
                    eprintln!("supertracker: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(cli::Parsed::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(cli::Parsed::Version) => {
            println!("SuperTracker {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("supertracker: {message}\n\n{}", cli::USAGE);
            return ExitCode::from(64);
        }
    };
    // Termination signals are handled by one thread of ours (see `watch_signals`); every thread
    // started from here on inherits the block.
    #[cfg(unix)]
    signals::block();
    let result = match options.serve {
        Some(port) => serve(&options, port),
        None => window::run(&options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("supertracker: {message}");
            ExitCode::FAILURE
        }
    }
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(code.clamp(0, 255) as u8)
}

/// Ends the host in an orderly way when the process is told to terminate (SIGINT, SIGTERM,
/// SIGHUP): the recovery copy is written from the last project the interface published, and the
/// services are shut down. The process then exits with 128 + the signal's number, as a process
/// killed by it would report. A second signal while that is under way exits at once.
pub(crate) fn watch_signals(host: &Arc<Host>) {
    #[cfg(unix)]
    {
        let host = host.clone();
        signals::watch(move |signal| {
            eprintln!("supertracker: terminating on signal {signal}.");
            host.session.stop();
            if let Err(error) = host.session.save_recovery() {
                eprintln!("supertracker: the recovery copy could not be written: {error}");
            }
            host.services.shutdown();
        });
    }
    #[cfg(not(unix))]
    let _ = host;
}

/// The scripted file choosers of `--dialog-answers`, if requested.
fn scripted_dialogs(options: &cli::Options) -> Option<Arc<dyn Dialogs>> {
    options.dialog_answers.as_deref().map(|file| Arc::new(ScriptedDialogs::from_file(file)) as Arc<dyn Dialogs>)
}

fn serve(options: &cli::Options, port: u16) -> Result<(), String> {
    let interface = ui::locate(options.ui.as_deref())?;
    let support = st_host::support_dir().map_err(|error| error.to_string())?;
    let dialogs = scripted_dialogs(options).unwrap_or_else(|| Arc::new(NoDialogs));
    let mut bridge = Bridge::builder();
    // On the main thread: the MIDI service is created here (CoreMIDI wants that).
    let host = Arc::new(install(&mut bridge, HostOptions::new(support, dialogs)).map_err(|error| error.to_string())?);
    watch_signals(&host);
    if let Some(project) = &options.open {
        host.session.request_open(std::path::absolute(project).unwrap_or_else(|_| project.clone()));
    }
    let bridge = Arc::new(bridge.start());
    let server = serve::Server::start(bridge.clone(), interface, port)?;
    // A page can attach from now on: AI control may listen.
    host.services.start_agent();

    // The URL alone on the first line of standard output, for scripts; the rest is for people.
    println!("{}", server.url());
    let _ = std::io::stdout().flush();
    eprintln!("SuperTracker is serving its interface at the address above. Type quit and Enter to stop.");

    // Standard input may be closed or empty (a background process): then the host runs until killed.
    let asked_to_quit = std::io::stdin().lock().lines().map_while(Result::ok).any(|line| line.trim() == "quit");
    if !asked_to_quit {
        loop {
            std::thread::park();
        }
    }
    host.session.stop();
    if let Err(error) = host.session.save_recovery() {
        eprintln!("supertracker: the recovery copy could not be written: {error}");
    }
    host.services.shutdown();
    bridge.shutdown(Duration::from_secs(5));
    server.stop();
    Ok(())
}

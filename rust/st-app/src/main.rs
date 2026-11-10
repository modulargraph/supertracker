//! The SuperTracker executable.
//!
//! - Without options it opens the window: the interface in a web view, with native menus and file
//!   choosers (`window.rs`).
//! - With `--serve` it has no window: it serves the interface over HTTP and the bridge over a
//!   WebSocket on 127.0.0.1, for a browser (`serve.rs`).
//!
//! Both run the same host (`st-host`); they differ only in how messages travel.

mod cli;
mod menus;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native;
mod serve;
mod ui;
mod websocket;
mod window;

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use st_host::{install, Bridge, Dialogs, HostOptions, NoDialogs, ScriptedDialogs};

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args_os().skip(1), |name| std::env::var_os(name)) {
        Ok(cli::Parsed::Run(options)) => options,
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

/// The scripted file choosers of `--dialog-answers`, if requested.
fn scripted_dialogs(options: &cli::Options) -> Option<Arc<dyn Dialogs>> {
    options.dialog_answers.as_deref().map(|file| Arc::new(ScriptedDialogs::from_file(file)) as Arc<dyn Dialogs>)
}

fn serve(options: &cli::Options, port: u16) -> Result<(), String> {
    let interface = ui::locate(options.ui.as_deref())?;
    let support = st_host::support_dir().map_err(|error| error.to_string())?;
    let dialogs = scripted_dialogs(options).unwrap_or_else(|| Arc::new(NoDialogs));
    let mut bridge = Bridge::builder();
    let host = install(&mut bridge, HostOptions::new(support, dialogs)).map_err(|error| error.to_string())?;
    if let Some(project) = &options.open {
        host.session.request_open(std::path::absolute(project).unwrap_or_else(|_| project.clone()));
    }
    let bridge = Arc::new(bridge.start());
    let server = serve::Server::start(bridge.clone(), interface, port)?;

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
    bridge.shutdown(Duration::from_secs(5));
    server.stop();
    Ok(())
}

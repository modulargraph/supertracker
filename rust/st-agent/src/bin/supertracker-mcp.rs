//! `supertracker-mcp`: the MCP server and the scripting command line as a small program of their
//! own, without the application's window and audio code.
//!
//! ```text
//! supertracker-mcp                      serve MCP on standard input and output
//! supertracker-mcp --ctl <method> ['<json params>']
//! ```
//!
//! The control socket is found as the application finds it (`SUPERTRACKER_CONTROL_SOCKET`,
//! `SUPERTRACKER_SUPPORT_DIR`, or the default support folder).

use std::io;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let socket = st_agent::socket_path();
    let code = match arguments.first().map(String::as_str) {
        None | Some("--mcp") => st_agent::mcp::run_stdio(&socket),
        Some("--ctl") => st_agent::mcp::run_control(&arguments[1..], &socket, io::stdout().lock(), io::stderr().lock()),
        Some(_) => {
            eprintln!("Usage: supertracker-mcp [--mcp]\n       supertracker-mcp --ctl <method> ['<json params>']");
            64
        }
    };
    std::process::exit(code);
}

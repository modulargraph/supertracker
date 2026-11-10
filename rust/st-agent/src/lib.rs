//! AI control: a local control socket in the application, and the MCP server that lets Claude (or
//! any Model Context Protocol client) read and edit the open song through it. See `docs/MCP.md` for
//! the user's view and `docs/RUST_ARCHITECTURE.md` for the crate's place in the host.
//!
//! ```text
//! MCP client ──stdio──▶ supertracker --mcp ──control socket──▶ the running application ──bridge──▶ the page
//!                       (mcp::run_stdio)                       (ControlServer)                    (STAgent.dispatch)
//! ```
//!
//! - The **control socket** is a Unix domain socket only this user can open (mode 0600, and the
//!   peer's user id is checked). Its protocol is one JSON-RPC 2.0 request per line; the methods are
//!   the page's agent API (`Resources/Web/agent-api.js`, `help` lists them) plus `app.ping` and
//!   `app.screenshot`, which the host answers itself.
//! - The **application** executes a request by handing it to the page through [`Page`], an
//!   abstraction `st-host` implements over its bridge ([`BridgePage`]), and returns the page's
//!   result. Edits therefore go through the same code as the user's own (undo, engine sync).
//! - The **MCP server** is a separate process without window or audio. Its tool catalogue
//!   ([`mcp::catalogue`], `src/tools.json`) is the public contract.
//!
//! # Guarantees
//!
//! - Lines are bounded ([`framing::MAX_LINE_BYTES`]); a peer that exceeds the bound is cut off.
//! - Each client has its own thread in the application. A client that stalls (stops reading) or
//!   disappears never blocks the application or another client; an unread answer closes the
//!   connection after 30 seconds.
//! - Turning AI control off ([`AgentService::configure`]) closes the socket and every client
//!   connection immediately, and requests are refused while it is off.
//! - The MCP proxy reconnects once when the application was restarted between two calls.
//!
//! # Registering in `st-host`
//!
//! ```ignore
//! let page = st_agent::BridgePage::new(
//!     Box::new({ let events = bridge.events(); move |name, payload| events.emit(name, payload) }),
//!     move || /* an interface is attached */ true,
//!     { let session = session.clone(); move || session.path() },
//! );
//! let agent = st_agent::AgentService::new(st_agent::AgentOptions {
//!     socket_path: st_agent::socket_path(),
//!     page: page.clone(),
//!     settings: settings.get("agent"),
//!     save_settings: Box::new({ let settings = settings.clone(); move |value| settings.set("agent", value).map_err(|e| e.to_string()) }),
//!     events: Box::new({ let events = bridge.events(); move |name, payload| events.emit(name, payload) }),
//! });
//! bridge.commands().with(&agent)
//!     .register("agent.status", st_agent::AgentService::status)
//!     .register("agent.configure", st_agent::AgentService::configure);
//! bridge.commands().with(&page).register("agent.reply", st_agent::BridgePage::reply);
//! agent.start();   // once the interface is attached
//! ```
//!
//! The page's part of [`BridgePage`] (not in the page yet) is to answer the `agent.call` event with
//! `agent.reply`, and to pass `agent.status` events to `STAgent.setConnection`.
//!
//! # Platforms
//!
//! Unix domain sockets on macOS and Linux. On Windows the crate builds, and the socket functions
//! fail with a message: a named pipe would implement [`transport::Channel`] and
//! [`transport::Listener`] in `transport.rs`, and nothing else changes. The macOS code is
//! type-checked from Linux (`cargo check -p st-agent --target aarch64-apple-darwin`) and has not
//! run on a Mac yet.

#![warn(missing_docs)]

pub mod client;
mod error;
pub mod framing;
pub mod mcp;
mod paths;
pub mod server;
mod service;
pub mod transport;

pub use client::{CallOutcome, ControlClient};
pub use error::ControlError;
pub use paths::{fnv1a, max_socket_path_bytes, socket_path, socket_path_for};
pub use server::{ControlServer, Page, ServerState};
pub use service::{AgentOptions, AgentService, AgentSettings, BridgePage, Configure, Error, EventCallback, PageReply, SaveSettings, Status};

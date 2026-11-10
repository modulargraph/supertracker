//! The AI control area: the local control socket for MCP clients (`st-agent`).
//!
//! ```text
//! MCP client ──▶ supertracker --mcp ──control socket──▶ AgentService ──▶ BridgePage ──event `agent.call`──▶ the page
//!                                                                                  ◀─command `agent.reply`──┘
//! ```
//!
//! [`install`] creates the service and registers:
//!
//! | Command | Notes |
//! |---|---|
//! | `agent.status` | `{available, enabled, listening, clients, socket, suspendedForTermination, error?}` |
//! | `agent.configure` | `{enabled?}`: turning AI control off closes the socket and every client connection at once; the choice is the settings section `agent` |
//! | `agent.reply` | `{id, reply}`: the page's answer to an `agent.call` event |
//!
//! Events: `agent.call` `{id, method, params}` (the page runs `STAgent.dispatch(method, params)`
//! and answers with `agent.reply`), and `agent.status` on every change of the listener, the
//! setting, the number of clients or the quit fence (the page passes it to
//! `STAgent.setConnection`).
//!
//! Nothing listens until [`Agent::start`], which the executable calls once its transport can
//! attach the interface. A request that arrives while no interface is ready is refused by the
//! control server with a message.
//!
//! # The socket
//!
//! [`AgentOptions::for_support`]: `control.sock` in the host's support folder (or
//! `SUPERTRACKER_CONTROL_SOCKET`), by the same rule `supertracker --mcp` and `--ctl` use to find
//! it (`st_agent::socket_path`), so a host and its clients agree whenever they agree on the
//! support folder.
//!
//! # Quitting
//!
//! The quit handshake ([`crate::Lifecycle`]) fences AI control: from the moment the interface
//! is asked whether the host may quit, new requests are refused ([`Agent::suspend`]); the page
//! waits for the request that was already running (`STAgent.suspendForTermination`) before it
//! answers. "Stay" opens admission again ([`Agent::resume`]); requests refused meanwhile stay
//! refused. [`Agent::shutdown`] closes the socket.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use st_agent::{AgentService, BridgePage};

use crate::commands::Commands;
use crate::session::Session;
use crate::{EventSink, Settings};

/// How [`crate::install`] sets AI control up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOptions {
    /// The control socket.
    pub socket_path: PathBuf,
}

impl AgentOptions {
    /// The control socket of the host whose support folder is `support`.
    pub fn for_support(support: &Path) -> AgentOptions {
        let support = support.to_string_lossy().into_owned();
        let socket_path = st_agent::socket_path_for(&|name| match name {
            "SUPERTRACKER_SUPPORT_DIR" => Some(support.clone()),
            "SUPERTRACKER_CONTROL_SOCKET" => std::env::var(name).ok(),
            // The folder a too-long socket path falls back to is looked up like any client does.
            _ => std::env::var(name).ok(),
        });
        AgentOptions { socket_path }
    }
}

const SETTINGS_SECTION: &str = "agent";

/// The AI control service of the host.
pub struct Agent {
    service: Arc<AgentService>,
    page: Arc<BridgePage>,
}

impl Agent {
    /// The service (status, the control server).
    pub fn service(&self) -> &Arc<AgentService> {
        &self.service
    }

    /// The interface as the control server calls it.
    pub fn page(&self) -> &Arc<BridgePage> {
        &self.page
    }

    /// Opens the control socket if the setting allows it. Call once the transport is up.
    pub fn start(&self) {
        self.service.start();
    }

    /// The quit handshake began: refuse new requests.
    pub fn suspend(&self) {
        self.service.server().suspend_admission();
    }

    /// Quitting was cancelled: accept requests again.
    pub fn resume(&self) {
        self.service.server().resume_admission();
    }

    /// Lets a request that is still running in the page settle (briefly), then closes the socket
    /// and every client connection (the host is quitting).
    pub fn shutdown(&self) {
        self.service.server().suspend_admission();
        self.service.server().wait_idle(Duration::from_millis(500));
        self.service.shutdown();
    }
}

/// Creates the AI control service and registers the `agent.*` commands. Nothing listens until
/// [`Agent::start`].
pub fn install(commands: &mut Commands, session: &Arc<Session>, settings: &Arc<Settings>, events: EventSink, options: &AgentOptions) -> Arc<Agent> {
    let emit = |events: EventSink| -> st_agent::EventCallback { Box::new(move |name, payload| events.emit(name, payload)) };
    let page = BridgePage::new(
        emit(events.clone()),
        {
            let events = events.clone();
            move || events.has_ready_interface()
        },
        {
            let session = session.clone();
            move || session.path().map(|path| path.to_string_lossy().into_owned())
        },
    );
    let service = AgentService::new(st_agent::AgentOptions {
        socket_path: options.socket_path.clone(),
        page: page.clone(),
        settings: settings.get(SETTINGS_SECTION),
        save_settings: Box::new({
            let settings = settings.clone();
            move |value| settings.set(SETTINGS_SECTION, value).map_err(|error| error.to_string())
        }),
        events: emit(events),
    });
    commands.with(&service).register("agent.status", AgentService::status).register("agent.configure", AgentService::configure);
    commands.with(&page).register("agent.reply", BridgePage::reply);
    Arc::new(Agent { service, page })
}

/// `supertracker --mcp`: serves the Model Context Protocol on standard input and output and
/// passes tool calls to the running application through its control socket (found by
/// `st_agent::socket_path`: `SUPERTRACKER_CONTROL_SOCKET`, else `control.sock` in the support
/// folder). Returns the process's exit code. No host is installed in this process.
pub fn run_mcp() -> i32 {
    st_agent::mcp::run_stdio(&st_agent::socket_path())
}

/// `supertracker --ctl <method> [json]`: calls one control method of the running application
/// and prints its result to `output` (errors to `errors`). Returns the exit code: 0, 1 when
/// the call failed, 64 for wrong usage.
pub fn run_control(arguments: &[String], output: impl std::io::Write, errors: impl std::io::Write) -> i32 {
    st_agent::mcp::run_control(arguments, &st_agent::socket_path(), output, errors)
}

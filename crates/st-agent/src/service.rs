//! The service `st-host` registers: the `agent.*` commands, the stored setting, and the page
//! adapter for the bridge.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::{ControlServer, Page};

/// An error of an `agent.*` command; its text is the reply's `error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    /// The message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// The settings section `agent` (the old host's `ai-control.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    /// Whether local AI clients may control the application. On by default: the socket is local
    /// and only this user can open it.
    pub enabled: bool,
}

impl Default for AgentSettings {
    fn default() -> Self {
        AgentSettings { enabled: true }
    }
}

/// Payload of `agent.configure`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Configure {
    /// Turn AI control on or off; absent leaves it as it is.
    pub enabled: Option<bool>,
}

/// Result of `agent.status` and `agent.configure`, and payload of the `agent.status` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Always true from this service (the page shows "not available" for a host without it).
    pub available: bool,
    /// The user's setting.
    pub enabled: bool,
    /// The control socket is open.
    pub listening: bool,
    /// Connected clients.
    pub clients: usize,
    /// The control socket's path.
    pub socket: String,
    /// Requests are refused because the application is quitting.
    pub suspended_for_termination: bool,
    /// Why the socket could not be opened or the setting not saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Receives the service's events: `(name, payload)`. `st-host` passes `EventSink::emit`.
pub type EventCallback = Box<dyn Fn(&str, Value) + Send + Sync>;
/// Stores the settings section. `st-host` passes `|s| settings.set("agent", s)`.
pub type SaveSettings = Box<dyn Fn(&AgentSettings) -> Result<(), String> + Send + Sync>;

/// What the service is built from.
pub struct AgentOptions {
    /// The control socket; [`crate::socket_path`] unless a test chooses another.
    pub socket_path: PathBuf,
    /// The interface.
    pub page: Arc<dyn Page>,
    /// The stored settings section.
    pub settings: AgentSettings,
    /// Called when the settings change.
    pub save_settings: SaveSettings,
    /// Receives the event `agent.status` (payload: [`Status`]) whenever the status changes.
    pub events: EventCallback,
}

struct Shared {
    settings: Mutex<AgentSettings>,
    save_settings: SaveSettings,
    save_error: Mutex<Option<String>>,
    events: EventCallback,
}

/// AI control as the host uses it: the control socket's lifetime follows the stored setting, and the
/// interface is told about every change.
///
/// Events: `agent.status` with a [`Status`] payload, on every change of the listener, the setting,
/// the number of clients or the quit fence.
pub struct AgentService {
    server: ControlServer,
    shared: Arc<Shared>,
}

impl AgentService {
    /// Creates the service. Nothing listens until [`start`](Self::start).
    pub fn new(options: AgentOptions) -> Arc<Self> {
        let server = ControlServer::new(options.socket_path, options.page);
        server.set_enabled(options.settings.enabled);
        let shared = Arc::new(Shared {
            settings: Mutex::new(options.settings),
            save_settings: options.save_settings,
            save_error: Mutex::new(None),
            events: options.events,
        });
        let for_events = shared.clone();
        server.on_change(move |server| {
            (for_events.events)("agent.status", serde_json::to_value(status_of(server, &for_events)).unwrap_or(Value::Null));
        });
        Arc::new(AgentService { server, shared })
    }

    /// Opens the control socket if the setting allows it. Call once the interface is attached. A
    /// socket that cannot be opened is reported in the status (`error`), not as a failure of the
    /// host.
    pub fn start(&self) {
        let _ = self.server.start();
    }

    /// Closes the socket and every client connection (the application quits).
    pub fn shutdown(&self) {
        self.server.stop();
    }

    /// The control server, for the quit handshake ([`ControlServer::suspend_admission`],
    /// [`ControlServer::wait_idle`], [`ControlServer::resume_admission`]).
    pub fn server(&self) -> &ControlServer {
        &self.server
    }

    /// The current status.
    pub fn current_status(&self) -> Status {
        status_of(&self.server, &self.shared)
    }

    /// Command `agent.status`.
    pub fn status(&self, _: IgnoredAny) -> Result<Status, Error> {
        Ok(self.current_status())
    }

    /// Command `agent.configure`: `{enabled?: bool}`. Turning AI control off closes every client
    /// connection immediately; turning it on opens the socket.
    pub fn configure(&self, request: Configure) -> Result<Status, Error> {
        if let Some(enabled) = request.enabled {
            let settings = {
                let mut settings = lock(&self.shared.settings);
                settings.enabled = enabled;
                settings.clone()
            };
            let saved = (self.shared.save_settings)(&settings);
            *lock(&self.shared.save_error) = saved.err().map(|error| format!("Could not save the setting: {error}"));
            self.server.set_enabled(enabled);
            if enabled {
                let _ = self.server.start();
            } else {
                self.server.stop();
            }
        }
        Ok(self.current_status())
    }
}

impl Drop for AgentService {
    fn drop(&mut self) {
        self.server.stop();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn status_of(server: &ControlServer, shared: &Shared) -> Status {
    let state = server.state();
    Status {
        available: true,
        enabled: lock(&shared.settings).enabled,
        listening: state.listening,
        clients: state.clients,
        socket: server.path().to_string_lossy().into_owned(),
        suspended_for_termination: state.suspended,
        error: state.error.or_else(|| lock(&shared.save_error).clone()),
    }
}

/// Payload of the command `agent.reply`: the page's answer to an `agent.call` event.
#[derive(Debug, Clone, Deserialize)]
pub struct PageReply {
    /// The `id` of the `agent.call` event this answers.
    pub id: u64,
    /// What `STAgent.dispatch` returned: the reply object, or the JSON text of it.
    #[serde(default)]
    pub reply: Value,
}

/// A [`Page`] over the host's bridge.
///
/// A call becomes the event `agent.call` with the payload `{"id": n, "method": "...", "params":
/// {...}}`. The page runs `STAgent.dispatch(method, params)` and sends the outcome back with the
/// command `agent.reply` and the payload `{"id": n, "reply": <what dispatch returned>}`, which the
/// host hands to [`BridgePage::reply`].
pub struct BridgePage {
    emit: EventCallback,
    connected: Box<dyn Fn() -> bool + Send + Sync>,
    project_path: Box<dyn Fn() -> Option<String> + Send + Sync>,
    waiting: Mutex<HashMap<u64, SyncSender<Value>>>,
    next: AtomicU64,
    timeout_millis: AtomicU64,
}

impl BridgePage {
    /// `emit` sends an event to the interface; `connected` tells whether an interface is attached;
    /// `project_path` is the open project's file.
    pub fn new(
        emit: EventCallback,
        connected: impl Fn() -> bool + Send + Sync + 'static,
        project_path: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(BridgePage {
            emit,
            connected: Box::new(connected),
            project_path: Box::new(project_path),
            waiting: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            timeout_millis: AtomicU64::new(crate::client::DEFAULT_TIMEOUT.as_millis() as u64),
        })
    }

    /// Changes how long a call waits for the interface (the default is ten minutes, like the
    /// clients' own limit).
    pub fn set_timeout(&self, timeout: Duration) {
        self.timeout_millis.store(timeout.as_millis().min(u128::from(u64::MAX)) as u64, Ordering::SeqCst);
    }

    /// Command `agent.reply`. An answer nobody waits for (the call timed out) is refused.
    pub fn reply(&self, reply: PageReply) -> Result<(), Error> {
        let waiting = lock(&self.waiting).remove(&reply.id);
        match waiting {
            Some(sender) => {
                let _ = sender.send(reply.reply);
                Ok(())
            }
            None => Err(Error(format!("No AI control call with id {} is waiting for an answer.", reply.id))),
        }
    }

    /// Calls waiting for the interface right now.
    pub fn pending(&self) -> usize {
        lock(&self.waiting).len()
    }
}

impl Page for BridgePage {
    fn connected(&self) -> bool {
        (self.connected)()
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::sync_channel(1);
        lock(&self.waiting).insert(id, sender);
        (self.emit)("agent.call", json!({"id": id, "method": method, "params": params}));
        let outcome = receiver.recv_timeout(Duration::from_millis(self.timeout_millis.load(Ordering::SeqCst)));
        lock(&self.waiting).remove(&id);
        match outcome {
            // `STAgent.dispatch` resolves to JSON text; a page may also send the parsed object.
            Ok(Value::String(text)) => serde_json::from_str(&text).map_err(|_| "the interface returned an unreadable answer.".to_string()),
            Ok(reply) => Ok(reply),
            Err(RecvTimeoutError::Timeout) => Err("the interface did not answer in time.".into()),
            Err(RecvTimeoutError::Disconnected) => Err("the interface went away.".into()),
        }
    }

    fn project_path(&self) -> Option<String> {
        (self.project_path)()
    }
}

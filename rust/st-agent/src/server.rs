//! The application's end of the control socket: accepts local clients of the same user and runs
//! each JSON-RPC request through the interface's agent API.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::framing::{self, LineBuffer};
use crate::transport::{self, Channel, Listener};
use crate::ControlError;

/// The interface (the web page) as the control server sees it. `st-host` implements this over its
/// bridge ([`crate::BridgePage`] does most of it); tests implement it directly.
pub trait Page: Send + Sync {
    /// True while the interface is attached and can run calls.
    fn connected(&self) -> bool;

    /// Runs one method of the page's agent API (`window.STAgent.dispatch(method, params)` in
    /// `Resources/Web/agent-api.js`) and returns its reply as the page produced it:
    /// `{"ok": true, "result": ..., "notices"?: [...]}` or `{"ok": false, "error": "...", "notices"?: [...]}`.
    ///
    /// The call blocks its thread (one of the control server's own) until the page answers. An
    /// `Err` means the page could not be asked or did not answer; its text is shown to the client.
    fn call(&self, method: &str, params: Value) -> Result<Value, String>;

    /// The file of the open project, if it has one. Clients see it in `app.ping` and the page's
    /// `summary` and `save` receive it as `_host.projectPath`.
    fn project_path(&self) -> Option<String> {
        None
    }

    /// A picture of the window for `app.screenshot`:
    /// `{"mimeType": "image/png", "data": <base64>, "width": .., "height": ..}`.
    /// `width` is the requested width in pixels (320 to 2560), if any.
    fn screenshot(&self, _width: Option<u32>) -> Result<Value, String> {
        Err("Screenshots are not available in this version of SuperTracker.".into())
    }
}

/// A snapshot of the control server for `agent.status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerState {
    /// The socket accepts connections.
    pub listening: bool,
    /// Connected clients.
    pub clients: usize,
    /// New requests are refused because the application is quitting.
    pub suspended: bool,
    /// Why the socket could not be opened, if it could not.
    pub error: Option<String>,
}

/// How long a client may leave an answer unread before its connection is closed.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// More simultaneous clients than this are turned away (each one is a thread).
const MAX_CLIENTS: usize = 32;

type ChangeListener = Arc<dyn Fn(&ControlServer) + Send + Sync>;

struct Shared {
    path: PathBuf,
    page: Arc<dyn Page>,
    enabled: AtomicBool,
    suspended: AtomicBool,
    generation: AtomicU64,
    state: Mutex<Connections>,
    running: Mutex<usize>,
    idle: Condvar,
    changed: Mutex<Option<ChangeListener>>,
}

#[derive(Default)]
struct Connections {
    epoch: u64,
    listener: Option<Arc<dyn Listener>>,
    clients: HashMap<u64, Arc<dyn Channel>>,
    next_client: u64,
    error: Option<String>,
}

/// The listener for AI control.
///
/// Every client connection has its own thread that reads requests, runs them one after another
/// through [`Page::call`] and writes the answers. The application's own threads never touch a
/// socket: a client that stalls or disappears costs only its own thread, and [`stop`](Self::stop)
/// closes every connection at once without waiting for anything.
#[derive(Clone)]
pub struct ControlServer {
    shared: Arc<Shared>,
}

impl ControlServer {
    /// A server for the socket at `path`. It does not listen until [`start`](Self::start).
    pub fn new(path: impl Into<PathBuf>, page: Arc<dyn Page>) -> Self {
        ControlServer {
            shared: Arc::new(Shared {
                path: path.into(),
                page,
                enabled: AtomicBool::new(true),
                suspended: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                state: Mutex::new(Connections::default()),
                running: Mutex::new(0),
                idle: Condvar::new(),
                changed: Mutex::new(None),
            }),
        }
    }

    /// Calls `listener` (from the server's threads, with this server) whenever
    /// [`state`](Self::state) changed.
    pub fn on_change(&self, listener: impl Fn(&ControlServer) + Send + Sync + 'static) {
        *lock(&self.shared.changed) = Some(Arc::new(listener));
    }

    /// The socket path.
    pub fn path(&self) -> &Path {
        &self.shared.path
    }

    /// Whether requests are allowed. While false every request is refused with a message, also on
    /// connections that are still open.
    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::SeqCst)
    }

    /// Allows or refuses requests. This is the gate only; [`start`](Self::start) and
    /// [`stop`](Self::stop) open and close the socket.
    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::SeqCst);
    }

    /// The current state.
    pub fn state(&self) -> ServerState {
        let state = lock(&self.shared.state);
        ServerState {
            listening: state.listener.is_some(),
            clients: state.clients.len(),
            suspended: self.shared.suspended.load(Ordering::SeqCst),
            error: state.error.clone(),
        }
    }

    /// Opens the socket and starts accepting clients, unless requests are not allowed or it is
    /// open already. A failure is kept as the state's `error` and returned.
    pub fn start(&self) -> Result<(), ControlError> {
        if !self.enabled() {
            return Ok(());
        }
        let outcome = {
            let mut state = lock(&self.shared.state);
            if state.listener.is_some() {
                return Ok(());
            }
            state.error = None;
            match transport::listen(&self.shared.path) {
                Ok(listener) => {
                    state.epoch += 1;
                    state.listener = Some(listener.clone());
                    let (shared, epoch) = (self.shared.clone(), state.epoch);
                    let spawned = thread::Builder::new().name("ai-control-accept".into()).spawn(move || accept_loop(shared, listener, epoch));
                    match spawned {
                        Ok(_) => Ok(()),
                        Err(error) => {
                            if let Some(listener) = state.listener.take() {
                                listener.close();
                            }
                            Err(ControlError::Transport(format!("Could not start the AI control listener: {error}.")))
                        }
                    }
                }
                Err(error) => Err(error),
            }
            .inspect_err(|error| state.error = Some(error.to_string()))
        };
        notify(&self.shared);
        outcome
    }

    /// Closes the socket and every client connection, now. Requests that are running finish in the
    /// page, but their answers go nowhere.
    pub fn stop(&self) {
        let (listener, clients) = {
            let mut state = lock(&self.shared.state);
            state.epoch += 1;
            (state.listener.take(), std::mem::take(&mut state.clients))
        };
        if let Some(listener) = listener {
            listener.close();
        }
        for client in clients.values() {
            client.close();
        }
        notify(&self.shared);
    }

    /// Refuses requests from now on because the application started quitting, including those that
    /// were received earlier and wait for their turn. Separate from the user's setting.
    pub fn suspend_admission(&self) {
        self.shared.suspended.store(true, Ordering::SeqCst);
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        notify(&self.shared);
    }

    /// Accepts requests again (quitting was cancelled). Requests refused meanwhile stay refused.
    pub fn resume_admission(&self) {
        self.shared.suspended.store(false, Ordering::SeqCst);
        notify(&self.shared);
    }

    /// Waits until no request is running in the page, for at most `timeout`. Returns false when one
    /// still is. Use after [`suspend_admission`](Self::suspend_admission) to let the request that
    /// was already running settle before the application quits.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut running = lock(&self.shared.running);
        while *running > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            running = self.shared.idle.wait_timeout(running, remaining).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
        true
    }

    /// Answers one request line with one response line, as a connection does.
    pub fn respond(&self, line: &[u8]) -> Vec<u8> {
        let admitted = !self.shared.suspended.load(Ordering::SeqCst);
        self.shared.respond(line, self.shared.generation.load(Ordering::SeqCst), admitted)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn accept_loop(shared: Arc<Shared>, listener: Arc<dyn Listener>, epoch: u64) {
    loop {
        match listener.accept() {
            Ok(Some(channel)) => {
                let id = {
                    let mut state = lock(&shared.state);
                    if state.epoch != epoch || state.clients.len() >= MAX_CLIENTS {
                        drop(state);
                        channel.close();
                        continue;
                    }
                    state.next_client += 1;
                    let id = state.next_client;
                    state.clients.insert(id, channel.clone());
                    id
                };
                let (for_thread, for_failure) = (shared.clone(), channel.clone());
                let spawned = thread::Builder::new().name("ai-control-client".into()).spawn(move || serve(for_thread, channel, id));
                if spawned.is_err() {
                    for_failure.close();
                    lock(&shared.state).clients.remove(&id);
                }
                notify(&shared);
            }
            Ok(None) => return,
            Err(error) => {
                let mut state = lock(&shared.state);
                if state.epoch == epoch {
                    state.error = Some(format!("The AI control socket stopped accepting connections: {error}."));
                    if let Some(listener) = state.listener.take() {
                        listener.close();
                    }
                }
                drop(state);
                notify(&shared);
                return;
            }
        }
    }
}

/// One client: requests are answered strictly in the order they were sent.
fn serve(shared: Arc<Shared>, channel: Arc<dyn Channel>, id: u64) {
    let _ = channel.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut buffer = LineBuffer::default();
    let mut chunk = vec![0u8; 64 * 1024];
    'connection: loop {
        let count = match channel.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        let lines = buffer.append(&chunk[..count]);
        if buffer.overflowed() {
            break;
        }
        // Admission is pinned when a line arrives, not when its turn comes: cancelling Quit must
        // not revive requests that were waiting behind the one that was running.
        let generation = shared.generation.load(Ordering::SeqCst);
        let admitted = !shared.suspended.load(Ordering::SeqCst);
        for line in lines {
            let response = shared.respond(&line, generation, admitted);
            if channel.write_all(&response).is_err() {
                break 'connection;
            }
        }
    }
    channel.close();
    if lock(&shared.state).clients.remove(&id).is_some() {
        notify(&shared);
    }
}

struct Running<'a>(&'a Shared);

impl<'a> Running<'a> {
    fn enter(shared: &'a Shared) -> Self {
        *lock(&shared.running) += 1;
        Running(shared)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let mut running = lock(&self.0.running);
        *running -= 1;
        if *running == 0 {
            self.0.idle.notify_all();
        }
    }
}

fn notify(shared: &Arc<Shared>) {
    let listener = lock(&shared.changed).clone();
    if let Some(listener) = listener {
        listener(&ControlServer { shared: shared.clone() });
    }
}

impl Shared {
    fn respond(&self, line: &[u8], generation: u64, admitted: bool) -> Vec<u8> {
        let mut id = None;
        let response = match self.answer(line, generation, admitted, &mut id) {
            Ok((result, notices)) => {
                let mut response = json!({"jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result});
                if !notices.is_empty() {
                    response["notices"] = json!(notices);
                }
                response
            }
            Err(error) => framing::error_response(id.as_ref(), -32000, &error.to_string()),
        };
        framing::encode(&response)
    }

    fn answer(&self, line: &[u8], generation: u64, admitted: bool, id: &mut Option<Value>) -> Result<(Value, Vec<String>), ControlError> {
        let message = framing::decode(line)?;
        *id = message.get("id").cloned();
        if !admitted || generation != self.generation.load(Ordering::SeqCst) {
            return Err(ControlError::Remote(
                "The AI command was canceled because SuperTracker started quitting. Send it again after canceling Quit.".into(),
            ));
        }
        let method = message.get("method").and_then(Value::as_str).ok_or_else(|| ControlError::Protocol("Missing method.".into()))?;
        let params = framing::named_object(message.get("params"), "params")?;
        self.perform(method, params)
    }

    fn perform(&self, method: &str, mut params: Map<String, Value>) -> Result<(Value, Vec<String>), ControlError> {
        let remote = |message: &str| ControlError::Remote(message.to_string());
        if !self.enabled.load(Ordering::SeqCst) {
            return Err(remote("AI control is turned off in SuperTracker Settings."));
        }
        if self.suspended.load(Ordering::SeqCst) {
            return Err(remote("SuperTracker is preparing to quit. Cancel quitting before sending more AI commands."));
        }
        let project = self.page.project_path();
        match method {
            "app.ping" => Ok((
                json!({
                    "app": "SuperTracker",
                    "version": env!("CARGO_PKG_VERSION"),
                    "pid": std::process::id(),
                    "socket": self.path.to_string_lossy(),
                    "project": project,
                }),
                Vec::new(),
            )),
            "app.screenshot" => {
                let width = params.get("width").and_then(Value::as_f64).map(|width| width.clamp(320.0, 2560.0) as u32);
                let _running = Running::enter(self);
                self.page.screenshot(width).map(|image| (image, Vec::new())).map_err(ControlError::Remote)
            }
            _ => {
                if !self.page.connected() {
                    return Err(remote("The SuperTracker window is not open."));
                }
                if method == "save" && !params.contains_key("path") && project.is_none() {
                    return Err(remote("This session has never been saved. Pass path (an absolute path ending in .supertracker)."));
                }
                params.insert("_host".into(), json!({"projectPath": project}));
                let reply = {
                    let _running = Running::enter(self);
                    self.page.call(method, Value::Object(params))
                }
                .map_err(|error| ControlError::Remote(format!("The interface could not run {method}: {error}")))?;
                interpret(reply)
            }
        }
    }
}

/// Turns the page's reply envelope into a result or an error.
fn interpret(reply: Value) -> Result<(Value, Vec<String>), ControlError> {
    let Value::Object(mut reply) = reply else {
        return Err(ControlError::Protocol("The interface returned an unreadable answer.".into()));
    };
    let notices: Vec<String> = reply
        .get("notices")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok((reply.remove("result").unwrap_or(Value::Null), notices));
    }
    let mut text = reply.get("error").and_then(Value::as_str).unwrap_or("The request failed.").to_string();
    for notice in &notices {
        text.push_str("\nApp message: ");
        text.push_str(notice);
    }
    Err(ControlError::Remote(text))
}

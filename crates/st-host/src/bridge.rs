//! The running bridge: takes requests from interface connections, runs their handlers on the right
//! thread, and sends exactly one reply for each; delivers events from any thread.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Display;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Number, Value};

use crate::catalog;
use crate::commands::{Commands, Entry, Lane};
use crate::protocol::{self, Incoming};
use crate::transport::{Outgoing, Transport};

/// Worker threads for blocking commands are created on demand up to this many; further requests
/// wait for a free one. Blocking commands are rare and mostly idle (a dialog waiting for the user),
/// so the limit only bounds a runaway interface.
const MAX_WORKERS: usize = 16;
/// An idle worker thread ends after this long.
const WORKER_IDLE: Duration = Duration::from_secs(30);
/// A quick command that runs longer than this is reported in the log: it belongs on a worker.
const SLOW_ORDERED: Duration = Duration::from_millis(250);

const SHUTTING_DOWN: &str = "SuperTracker is shutting down.";

/// `SUPERTRACKER_TRACE=1` logs every request and its outcome to standard error.
fn tracing() -> bool {
    static TRACING: OnceLock<bool> = OnceLock::new();
    *TRACING.get_or_init(|| std::env::var_os("SUPERTRACKER_TRACE").is_some_and(|value| !value.is_empty() && value != "0"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Handlers run outside these locks, so a poisoned lock still guards consistent data.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Job {
    command: String,
    run: Box<dyn FnOnce() + Send>,
}

/// Everything the bridge's handles share.
struct Core {
    table: OnceLock<BTreeMap<String, Entry>>,
    connections: Mutex<Vec<Arc<ConnectionState>>>,
    pending: Mutex<HashMap<u64, Arc<Slot>>>,
    next_serial: AtomicU64,
    shutting_down: AtomicBool,
    ordered: Mutex<Option<mpsc::Sender<Job>>>,
    pool: Mutex<Pool>,
    pool_work: Condvar,
    pool_quiet: Condvar,
}

#[derive(Default)]
struct Pool {
    queue: VecDeque<Job>,
    threads: usize,
    idle: usize,
    running: usize,
    closed: bool,
}

struct ConnectionState {
    transport: Arc<dyn Transport>,
    ready: AtomicBool,
    open: AtomicBool,
}

impl ConnectionState {
    fn send(&self, json: String, latest_only: Option<String>) {
        if self.open.load(Ordering::SeqCst) {
            self.transport.send(Outgoing { json, latest_only });
        }
    }
}

enum Destination {
    Interface { connection: Arc<ConnectionState>, id: Number },
    Caller(mpsc::Sender<Result<Value, String>>),
}

/// One request that has not been answered yet.
struct Slot {
    serial: u64,
    command: String,
    destination: Destination,
    answered: AtomicBool,
    core: Arc<Core>,
}

impl Slot {
    /// Sends the answer unless one was already sent. This is the only place replies are produced.
    fn answer(&self, result: Result<Value, String>) {
        if self.answered.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.core.pending).remove(&self.serial);
        match &self.destination {
            Destination::Interface { connection, id } => {
                if tracing() {
                    match &result {
                        Ok(_) => eprintln!("st-host: {id} {} -> ok", self.command),
                        Err(message) => eprintln!("st-host: {id} {} -> error: {message}", self.command),
                    }
                }
                let json = match &result {
                    Ok(value) => protocol::reply_ok(id, value),
                    Err(message) => protocol::reply_error(id, message),
                };
                connection.send(json, None);
            }
            Destination::Caller(sender) => {
                let _ = sender.send(result);
            }
        }
    }
}

/// The untyped pending answer to one request. [`crate::Reply`] is the typed form handlers use.
pub(crate) struct RawReply {
    slot: Arc<Slot>,
}

impl RawReply {
    pub(crate) fn command(&self) -> &str {
        &self.slot.command
    }

    pub(crate) fn send(self, result: Result<Value, String>) {
        self.slot.answer(result);
    }

    pub(crate) fn error(self, message: impl Display) {
        self.slot.answer(Err(message.to_string()));
    }
}

impl Drop for RawReply {
    fn drop(&mut self) {
        if self.slot.answered.load(Ordering::SeqCst) {
            return;
        }
        let command = &self.slot.command;
        let message = if self.slot.core.shutting_down.load(Ordering::SeqCst) {
            SHUTTING_DOWN.to_string()
        } else if thread::panicking() {
            format!("`{command}` failed unexpectedly. This is a bug in SuperTracker.")
        } else {
            format!("`{command}` ended without an answer. This is a bug in SuperTracker.")
        };
        self.slot.answer(Err(message));
    }
}

impl Core {
    fn open_slot(self: &Arc<Self>, command: &str, destination: Destination) -> RawReply {
        let slot = Arc::new(Slot {
            serial: self.next_serial.fetch_add(1, Ordering::SeqCst),
            command: command.to_string(),
            destination,
            answered: AtomicBool::new(false),
            core: self.clone(),
        });
        lock(&self.pending).insert(slot.serial, slot.clone());
        RawReply { slot }
    }

    fn entry(&self, command: &str) -> Option<&Entry> {
        self.table.get().and_then(|table| table.get(command))
    }

    fn submit_blocking(self: &Arc<Self>, job: Job) {
        let mut pool = lock(&self.pool);
        if pool.closed {
            drop(pool);
            drop(job); // Its reply answers "shutting down" as it is dropped.
            return;
        }
        pool.queue.push_back(job);
        if pool.queue.len() > pool.idle && pool.threads < MAX_WORKERS {
            pool.threads += 1;
            let core = self.clone();
            let spawned = thread::Builder::new().name("st-host worker".to_string()).spawn(move || core.work());
            if spawned.is_err() {
                pool.threads -= 1;
                if pool.threads == 0 {
                    // Nothing will ever take the job: answer now instead of never.
                    let stranded: Vec<Job> = pool.queue.drain(..).collect();
                    drop(pool);
                    drop(stranded);
                    return;
                }
            }
        }
        drop(pool);
        self.pool_work.notify_one();
    }

    fn work(self: Arc<Self>) {
        let mut pool = lock(&self.pool);
        loop {
            if let Some(job) = pool.queue.pop_front() {
                pool.running += 1;
                drop(pool);
                run_job(job);
                pool = lock(&self.pool);
                pool.running -= 1;
                self.pool_quiet.notify_all();
                continue;
            }
            if pool.closed {
                break;
            }
            pool.idle += 1;
            let (guard, wait) = self.pool_work.wait_timeout(pool, WORKER_IDLE).unwrap_or_else(|poisoned| poisoned.into_inner());
            pool = guard;
            pool.idle -= 1;
            if wait.timed_out() && pool.queue.is_empty() {
                break;
            }
        }
        pool.threads -= 1;
        self.pool_quiet.notify_all();
    }
}

fn run_job(job: Job) {
    // A panic in a handler aborts release builds (`panic = "abort"`). Where panics unwind (tests),
    // the handler's reply answers with an error as it is dropped, and the thread carries on.
    let _ = catch_unwind(AssertUnwindSafe(job.run));
}

/// Collects commands, then starts the bridge.
pub struct BridgeBuilder {
    commands: Commands,
    core: Arc<Core>,
}

impl BridgeBuilder {
    /// The command table to fill.
    pub fn commands(&mut self) -> &mut Commands {
        &mut self.commands
    }

    /// A sink for events, to hand to services before the bridge runs. Events emitted before an
    /// interface is ready are discarded.
    pub fn events(&self) -> EventSink {
        EventSink { core: self.core.clone() }
    }

    /// A handle for calling commands from inside the host. Calls fail until the bridge has started.
    pub fn calls(&self) -> Calls {
        Calls { core: self.core.clone() }
    }

    /// Starts the bridge thread. No command can be added afterwards.
    pub fn start(self) -> Bridge {
        let core = self.core;
        let _ = core.table.set(self.commands.into_entries());
        let (sender, receiver) = mpsc::channel::<Job>();
        *lock(&core.ordered) = Some(sender);
        let worker_core = core.clone();
        let ordered = thread::Builder::new()
            .name("st-host bridge".to_string())
            .spawn(move || {
                for job in receiver {
                    if worker_core.shutting_down.load(Ordering::SeqCst) {
                        continue; // Dropping the job answers "shutting down".
                    }
                    let command = job.command.clone();
                    let started = Instant::now();
                    run_job(job);
                    let elapsed = started.elapsed();
                    if elapsed > SLOW_ORDERED {
                        eprintln!(
                            "st-host: `{command}` held the bridge thread for {} ms; register it with register_blocking",
                            elapsed.as_millis()
                        );
                    }
                }
            })
            .ok();
        if ordered.is_none() {
            // Without the thread, the sender has no receiver and every quick command answers with an error.
            eprintln!("st-host: could not start the bridge thread");
        }
        Bridge { core, ordered: Mutex::new(ordered) }
    }
}

/// The running bridge. Dropping it shuts it down.
pub struct Bridge {
    core: Arc<Core>,
    ordered: Mutex<Option<JoinHandle<()>>>,
}

impl Bridge {
    /// Starts collecting commands.
    pub fn builder() -> BridgeBuilder {
        BridgeBuilder {
            commands: Commands::new(),
            core: Arc::new(Core {
                table: OnceLock::new(),
                connections: Mutex::new(Vec::new()),
                pending: Mutex::new(HashMap::new()),
                next_serial: AtomicU64::new(1),
                shutting_down: AtomicBool::new(false),
                ordered: Mutex::new(None),
                pool: Mutex::new(Pool::default()),
                pool_work: Condvar::new(),
                pool_quiet: Condvar::new(),
            }),
        }
    }

    /// Attaches an interface (one page). Its requests are answered through `transport`; it receives
    /// events once it has sent `ready`.
    pub fn connect(&self, transport: Arc<dyn Transport>) -> Connection {
        let state = Arc::new(ConnectionState { transport, ready: AtomicBool::new(false), open: AtomicBool::new(true) });
        lock(&self.core.connections).push(state.clone());
        Connection { core: self.core.clone(), state }
    }

    /// A sink for events.
    pub fn events(&self) -> EventSink {
        EventSink { core: self.core.clone() }
    }

    /// A handle for calling commands from inside the host.
    pub fn calls(&self) -> Calls {
        Calls { core: self.core.clone() }
    }

    /// The number of requests that have not been answered yet.
    pub fn pending(&self) -> usize {
        lock(&self.core.pending).len()
    }

    /// True when an interface has sent `ready` and is still connected.
    pub fn has_ready_interface(&self) -> bool {
        self.events().has_ready_interface()
    }

    /// Stops the bridge. Requests that arrive from now on, and requests whose handler has not
    /// started, are answered with "SuperTracker is shutting down."; handlers that are running get
    /// `grace` to finish and answer. Whatever is still unanswered after that is answered with an
    /// error too, so every request has its one reply when this returns. Transports stay open: the
    /// caller flushes and closes them.
    pub fn shutdown(&self, grace: Duration) {
        if self.core.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        let deadline = Instant::now() + grace;
        // Quick commands: closing the channel ends the thread after the handler it is in.
        lock(&self.core.ordered).take();
        if let Some(thread) = lock(&self.ordered).take() {
            while !thread.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
        // Blocking commands: drop what has not started, wait for what has.
        let unstarted: Vec<Job> = {
            let mut pool = lock(&self.core.pool);
            pool.closed = true;
            pool.queue.drain(..).collect()
        };
        drop(unstarted);
        self.core.pool_work.notify_all();
        let mut pool = lock(&self.core.pool);
        while pool.running > 0 {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            pool = self.core.pool_quiet.wait_timeout(pool, deadline - now).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
        drop(pool);
        // Handlers that outlived the grace period (and deferred replies nobody will answer now).
        let unanswered: Vec<Arc<Slot>> = lock(&self.core.pending).values().cloned().collect();
        for slot in unanswered {
            slot.answer(Err(format!("SuperTracker shut down before `{}` finished.", slot.command)));
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shutdown(Duration::from_millis(200));
    }
}

/// One attached interface. Dropping it detaches the interface: replies and events for it are
/// discarded from then on.
pub struct Connection {
    core: Arc<Core>,
    state: Arc<ConnectionState>,
}

impl Connection {
    /// Handles one text the interface sent. Returns at once; the reply goes to the transport when
    /// the command's handler has finished.
    pub fn receive(&self, text: &str) {
        let request = match protocol::parse(text) {
            Incoming::Request(request) => request,
            Incoming::Rejected { id, error } => {
                self.state.send(protocol::reply_error(&id, &error), None);
                return;
            }
            Incoming::Unreadable(reason) => {
                eprintln!("st-host: ignored a message from the interface: {reason}");
                return;
            }
        };
        if request.command == "ready" {
            self.state.ready.store(true, Ordering::SeqCst);
        }
        let destination = Destination::Interface { connection: self.state.clone(), id: request.id };
        let reply = self.core.open_slot(&request.command, destination);
        if self.core.shutting_down.load(Ordering::SeqCst) {
            reply.error(SHUTTING_DOWN);
            return;
        }
        let Some(entry) = self.core.entry(&request.command) else {
            reply.error(catalog::missing_message(&request.command));
            return;
        };
        let run = entry.run.clone();
        let payload = request.payload;
        let job = Job { command: request.command, run: Box::new(move || run(payload, reply)) };
        match entry.lane {
            Lane::Ordered => {
                // A send that fails (no bridge thread) drops the job, which answers with an error.
                if let Some(sender) = lock(&self.core.ordered).as_ref() {
                    let _ = sender.send(job);
                }
            }
            Lane::Blocking => self.core.submit_blocking(job),
        }
    }

    /// True once this interface has sent `ready`.
    pub fn is_ready(&self) -> bool {
        self.state.ready.load(Ordering::SeqCst)
    }

    /// Detaches the interface.
    pub fn close(&self) {
        self.state.open.store(false, Ordering::SeqCst);
        self.state.ready.store(false, Ordering::SeqCst);
        lock(&self.core.connections).retain(|connection| !Arc::ptr_eq(connection, &self.state));
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

/// Sends events to the interface. Cheap to clone; usable from any thread, including threads that
/// must not block (sending queues the message and returns).
#[derive(Clone)]
pub struct EventSink {
    core: Arc<Core>,
}

impl EventSink {
    /// Sends the event `name` to every ready interface.
    pub fn emit(&self, name: &str, payload: impl Serialize) {
        self.deliver(name, payload, false);
    }

    /// Like [`emit`](EventSink::emit), for snapshots of which only the newest matters (transport
    /// status, meters, playheads): while an interface has not taken the previous `name` event yet,
    /// this one replaces it instead of queueing behind it.
    pub fn emit_latest(&self, name: &str, payload: impl Serialize) {
        self.deliver(name, payload, true);
    }

    /// True when an interface has sent `ready` and is still connected: someone would receive an event.
    pub fn has_ready_interface(&self) -> bool {
        lock(&self.core.connections).iter().any(|connection| connection.ready.load(Ordering::SeqCst))
    }

    fn deliver(&self, name: &str, payload: impl Serialize, latest_only: bool) {
        let connections: Vec<Arc<ConnectionState>> =
            lock(&self.core.connections).iter().filter(|connection| connection.ready.load(Ordering::SeqCst)).cloned().collect();
        if connections.is_empty() {
            return;
        }
        let payload = match serde_json::to_value(payload) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("st-host: the event `{name}` could not be encoded: {error}");
                return;
            }
        };
        let json = protocol::event(name, &payload);
        for connection in connections {
            connection.send(json.clone(), latest_only.then(|| name.to_string()));
        }
        if tracing() && !latest_only {
            eprintln!("st-host: event {name}");
        }
    }
}

/// Why a call through [`Calls`] produced no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// No handler is registered for the command. The message is the one the interface would get.
    NotImplemented(String),
    /// The handler answered with this error.
    Failed(String),
}

impl Display for CallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotImplemented(message) | CallError::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CallError {}

/// Calls commands from inside the host, so that one area can use another through the same table the
/// interface uses (the `ready` handshake collects `library.roots`, `plugins.list`, ... this way and
/// picks up each area the day it registers its commands).
#[derive(Clone)]
pub struct Calls {
    core: Arc<Core>,
}

impl Calls {
    /// Runs the handler of `command` on the calling thread and returns its answer. For a deferred
    /// command this waits until the answer arrives, so do not call one from the thread that has to
    /// produce that answer.
    pub fn call(&self, command: &str, payload: Value) -> Result<Value, CallError> {
        if self.core.table.get().is_none() {
            return Err(CallError::Failed("The host has not finished starting.".to_string()));
        }
        let Some(entry) = self.core.entry(command) else {
            return Err(CallError::NotImplemented(catalog::missing_message(command)));
        };
        let (sender, receiver) = mpsc::channel();
        let reply = self.core.open_slot(command, Destination::Caller(sender));
        (entry.run)(payload, reply);
        match receiver.recv() {
            Ok(result) => result.map_err(CallError::Failed),
            Err(_) => Err(CallError::Failed(format!("`{command}` ended without an answer."))),
        }
    }

    /// True when a handler is registered for `command`.
    pub fn implements(&self, command: &str) -> bool {
        self.core.entry(command).is_some()
    }

    /// The names of all registered commands, sorted.
    pub fn implemented(&self) -> Vec<String> {
        self.core.table.get().map(|table| table.keys().cloned().collect()).unwrap_or_default()
    }
}

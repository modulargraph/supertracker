//! The supervisor: container processes, instance placement, crash and hang handling,
//! restart with state restore, and the control half of [`Instance`].

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use stpc_transport::channel::{FrameKind, Stream};
use stpc_transport::messages::*;
use stpc_transport::monotonic_ns;
use stpc_transport::region::{Notification, Region, RegionConfig};
use stpc_transport::wake::{self, Notifier, Waiter, WakeMode};
use stpc_transport::wire::{Bytes, Wire};

use crate::connection::Connection;
use crate::instance::{
    DryDelay, Fallback, Instance, InstanceConfig, InstanceShared, InstanceStatus, Link, PluginKind, RtState, Waiting, GATE_IDLE,
};
use crate::spawn;
use crate::{ContainerId, Error, InstanceId, Result};

/// File name of the container executable, looked up next to the host's own executable.
pub const CONTAINER_BINARY_NAME: &str = "supertracker-plugin-container";
/// Environment variable overriding the container executable path.
pub const ENV_CONTAINER_PATH: &str = "STPC_CONTAINER_PATH";

/// How plug-in instances are distributed over container processes (as in Bitwig Studio).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupingPolicy {
    /// All plug-ins in one container: least overhead, one crash takes all plug-ins down.
    Shared,
    /// One container per manufacturer.
    PerManufacturer,
    /// One container per plug-in; all instances of the same plug-in share it.
    PerPlugin,
    /// One container per instance: best isolation, most processes.
    PerInstance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakePreference {
    /// Address waits (futex) when both processes support them, else pipes.
    Auto,
    Address,
    Pipe,
}

#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// The container executable. `None`: `$STPC_CONTAINER_PATH`, else
    /// `supertracker-plugin-container` next to the current executable.
    pub container_path: Option<PathBuf>,
    /// Arguments placed before the ones the supervisor appends.
    pub container_args: Vec<OsString>,
    /// Extra environment for containers.
    pub container_env: Vec<(OsString, OsString)>,
    /// Directory in which the supervisor creates its private socket directory.
    /// `None`: `$XDG_RUNTIME_DIR`, else the system temporary directory.
    pub socket_dir: Option<PathBuf>,
    pub policy: GroupingPolicy,
    pub wake: WakePreference,
    /// How long `process` spins before it sleeps. Spinning shaves the scheduler wake-up
    /// off the round trip at the price of a busy core; 0 (the default) sleeps at once.
    pub spin: Duration,
    /// For a container to start and complete the handshake.
    pub spawn_timeout: Duration,
    /// For ordinary control requests.
    pub request_timeout: Duration,
    /// For creating an instance (plug-ins can take long to load).
    pub create_timeout: Duration,
    /// For state transfers and preset loads.
    pub state_timeout: Duration,
    /// A container that leaves a block unanswered this long is reported as hung.
    pub hang_timeout: Duration,
    /// Same for offline rendering, where `process` waits without deadline. `None`: an
    /// offline block may take forever.
    pub offline_hang_timeout: Option<Duration>,
    /// Kill hung containers (their instances become lost and can be restored).
    pub kill_hung: bool,
    /// Period of the housekeeping thread (notifications, hang detection).
    pub housekeeping_interval: Duration,
    /// Fetch every instance's state this often, so a restore loses little. `None`: the
    /// last known state is whatever the host last read or wrote.
    pub auto_state_interval: Option<Duration>,
    /// For a container to exit after being asked to, before it is killed.
    pub shutdown_timeout: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> SupervisorConfig {
        SupervisorConfig {
            container_path: None,
            container_args: Vec::new(),
            container_env: Vec::new(),
            socket_dir: None,
            policy: GroupingPolicy::PerPlugin,
            wake: WakePreference::Auto,
            spin: Duration::ZERO,
            spawn_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(10),
            create_timeout: Duration::from_secs(120),
            state_timeout: Duration::from_secs(60),
            hang_timeout: Duration::from_secs(3),
            offline_hang_timeout: None,
            kill_hung: true,
            housekeeping_interval: Duration::from_millis(10),
            auto_state_interval: None,
            shutdown_timeout: Duration::from_secs(3),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// Terminated by a signal it raised itself (SIGSEGV, SIGABRT, ...): a crash.
    Crashed { signal: i32 },
    /// Exited on its own with this code, without being asked to.
    Exited { code: i32 },
    /// Killed by the supervisor because it was hung.
    Hung,
    /// Killed on request ([`Supervisor::kill_container`]).
    Killed,
    /// Regular shutdown (no instances left, or the supervisor shut down).
    Shutdown,
}

/// Why an instance was lost: the exit reason of its container.
pub type LossReason = ExitReason;

#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    ContainerStarted {
        container: ContainerId,
        pid: u32,
    },
    /// A container process ended. `lost` lists the instances that went with it (empty for
    /// a regular shutdown).
    ContainerExited {
        container: ContainerId,
        pid: u32,
        reason: ExitReason,
        lost: Vec<InstanceId>,
    },
    /// The instance's container is gone; `process` plays the fallback until it is restored.
    InstanceLost {
        instance: InstanceId,
        container: ContainerId,
        reason: LossReason,
    },
    /// A block missed its deadline (one event per episode).
    DeadlineMissed {
        instance: InstanceId,
    },
    /// The container has not answered a block for the hang timeout. With `kill_hung` the
    /// container is killed next and `InstanceLost` follows.
    InstanceHung {
        instance: InstanceId,
        container: ContainerId,
    },
    InstanceRestored {
        instance: InstanceId,
        container: ContainerId,
    },
    InstanceRestoreFailed {
        instance: InstanceId,
        error: String,
    },
    /// The user grabbed / released a control in the plug-in's editor.
    ParameterGesture {
        instance: InstanceId,
        id: u64,
        begin: bool,
    },
    /// The plug-in changed a parameter itself (editor, program change).
    ParameterChanged {
        instance: InstanceId,
        id: u64,
        value: f64,
    },
    LatencyChanged {
        instance: InstanceId,
        frames: u32,
    },
    TailChanged {
        instance: InstanceId,
        frames: u64,
    },
    /// Parameter metadata and values must be read again.
    ParametersRescan {
        instance: InstanceId,
    },
    StateDirty {
        instance: InstanceId,
    },
    /// The user closed the editor window.
    EditorClosed {
        instance: InstanceId,
    },
    EditorResized {
        instance: InstanceId,
        width: u32,
        height: u32,
    },
    /// The plug-in asks to be deactivated and activated again (e.g. its latency changes).
    /// The host does that when convenient: `deactivate()` then `activate()`.
    RestartRequested {
        instance: InstanceId,
    },
    Log {
        container: ContainerId,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    Dead,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContainerInfo {
    pub id: ContainerId,
    /// The grouping key ("shared", "manufacturer:…", "plugin:…", "instance:…").
    pub key: String,
    pub state: ContainerState,
    /// Process id while running, else 0.
    pub pid: u32,
    /// Instances placed in this container, including lost ones awaiting restore.
    pub instances: Vec<InstanceId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestartReport {
    pub restored: Vec<InstanceId>,
    pub failed: Vec<(InstanceId, String)>,
}

// ---- containers -----------------------------------------------------------------------

struct Process {
    child: Child,
    conn: Arc<Connection>,
}

struct ContainerInner {
    generation: u64,
    process: Option<Process>,
    /// Reader thread of the current (or last) process.
    reader: Option<JoinHandle<()>>,
    /// Set before the supervisor ends the process itself, so the exit is classified.
    intent: Option<ExitReason>,
}

pub(crate) struct Container {
    id: ContainerId,
    key: String,
    inner: Mutex<ContainerInner>,
    /// Signalled when a process of this container has been reaped.
    reaped: Condvar,
    instances: Mutex<Vec<Weak<InstanceShared>>>,
    /// Instances placed here or being placed; guarded by `SupInner::containers`.
    refs: AtomicU32,
}

impl Container {
    fn live_instances(&self) -> Vec<Arc<InstanceShared>> {
        self.instances.lock().unwrap().iter().filter_map(Weak::upgrade).collect()
    }

    fn running(&self) -> Option<(Arc<Connection>, u64)> {
        let inner = self.inner.lock().unwrap();
        inner.process.as_ref().filter(|p| !p.conn.is_dead()).map(|p| (p.conn.clone(), inner.generation))
    }

    /// Ends the process forcibly; the reader thread then runs the death procedure.
    fn kill(&self, reason: ExitReason) -> bool {
        let mut guard = self.inner.lock().unwrap();
        let inner = &mut *guard;
        let Some(process) = inner.process.as_mut() else { return false };
        if inner.intent.is_none() {
            inner.intent = Some(reason);
        }
        // The child is not reaped while `process` is Some, so the pid is still ours.
        let _ = process.child.kill();
        // Requests in flight fail now rather than when the reader thread notices.
        process.conn.fail_all();
        true
    }

    /// If the process is dead or dying (its connection failed, or it does not answer a
    /// ping), waits until the reader thread has completed the death procedure: process
    /// reaped, instances flagged lost. Returns at once for a healthy or stopped container.
    fn settle(&self, timeout: Duration) {
        let conn = self.inner.lock().unwrap().process.as_ref().map(|p| p.conn.clone());
        let Some(conn) = conn else { return };
        if !conn.is_dead() && !matches!(conn.call(&Request::Ping {}, &[], timeout), Err(Error::ContainerDied)) {
            return;
        }
        let deadline = Instant::now() + timeout;
        let mut inner = self.inner.lock().unwrap();
        while inner.process.as_ref().is_some_and(|p| Arc::ptr_eq(&p.conn, &conn)) {
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            inner = self.reaped.wait_timeout(inner, deadline - now).unwrap().0;
        }
    }

    /// Asks the process to exit, waits, and kills it if it does not comply.
    fn shutdown(&self, timeout: Duration, request_timeout: Duration) {
        let conn = {
            let mut inner = self.inner.lock().unwrap();
            let Some(process) = inner.process.as_ref() else { return };
            let conn = process.conn.clone();
            inner.intent = Some(ExitReason::Shutdown);
            conn
        };
        let _ = conn.call(&Request::Shutdown {}, &[], request_timeout.min(timeout));
        conn.close();
        let deadline = Instant::now() + timeout;
        let mut inner = self.inner.lock().unwrap();
        while inner.process.is_some() {
            let now = Instant::now();
            if now >= deadline {
                if let Some(process) = inner.process.as_mut() {
                    let _ = process.child.kill();
                }
                // The reader thread reaps after the kill; wait for it without a limit.
                inner = self.reaped.wait(inner).unwrap();
            } else {
                inner = self.reaped.wait_timeout(inner, deadline - now).unwrap().0;
            }
        }
        // Let the reader thread finish, so that its events precede our return.
        let reader = inner.reader.take();
        drop(inner);
        join_reader(reader);
    }
}

fn join_reader(reader: Option<JoinHandle<()>>) {
    if let Some(reader) = reader {
        // An event callback may run on the reader thread itself: never join oneself.
        if reader.thread().id() != std::thread::current().id() {
            let _ = reader.join();
        }
    }
}

// ---- supervisor -----------------------------------------------------------------------

type Callback = Arc<dyn Fn(&HostEvent) + Send + Sync>;
/// An event callback; see [`Supervisor::set_event_callback`].
pub type EventCallback = Box<dyn Fn(&HostEvent) + Send + Sync>;

pub(crate) struct SupInner {
    config: SupervisorConfig,
    container_path: PathBuf,
    dir: PathBuf,
    policy: Mutex<GroupingPolicy>,
    containers: Mutex<HashMap<String, Arc<Container>>>,
    instances: Mutex<HashMap<InstanceId, Weak<InstanceShared>>>,
    events: Mutex<VecDeque<HostEvent>>,
    callback: Mutex<Option<Callback>>,
    next_instance: AtomicU64,
    next_container: AtomicU32,
    next_socket: AtomicU32,
    closed: AtomicBool,
    housekeeping_stop: Mutex<bool>,
    housekeeping_wake: Condvar,
    housekeeping: Mutex<Option<JoinHandle<()>>>,
}

/// Spawns and watches plug-in containers. Dropping it shuts every container down.
pub struct Supervisor {
    inner: Arc<SupInner>,
}

impl Supervisor {
    pub fn new(config: SupervisorConfig) -> Result<Supervisor> {
        let container_path = spawn::resolve_container_path(config.container_path.as_ref())?;
        let dir = spawn::create_private_dir(config.socket_dir.as_ref())?;
        let inner = Arc::new(SupInner {
            policy: Mutex::new(config.policy),
            config,
            container_path,
            dir,
            containers: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
            events: Mutex::new(VecDeque::new()),
            callback: Mutex::new(None),
            next_instance: AtomicU64::new(1),
            next_container: AtomicU32::new(1),
            next_socket: AtomicU32::new(1),
            closed: AtomicBool::new(false),
            housekeeping_stop: Mutex::new(false),
            housekeeping_wake: Condvar::new(),
            housekeeping: Mutex::new(None),
        });
        let weak = Arc::downgrade(&inner);
        let handle = std::thread::Builder::new()
            .name("stpc-housekeeping".into())
            .spawn(move || housekeeping_thread(weak))
            .map_err(Error::Io)?;
        *inner.housekeeping.lock().unwrap() = Some(handle);
        Ok(Supervisor { inner })
    }

    pub fn policy(&self) -> GroupingPolicy {
        *self.inner.policy.lock().unwrap()
    }

    /// Changes the policy for instances created from now on; existing ones stay put.
    pub fn set_policy(&self, policy: GroupingPolicy) {
        *self.inner.policy.lock().unwrap() = policy;
    }

    /// The supervisor's private socket directory (empty except while a container starts).
    pub fn socket_dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Events are always queued for [`Supervisor::poll_event`]; with a callback they are
    /// also delivered right away, on a supervisor thread (never the audio thread). The
    /// callback must not block for long; it may call back into the supervisor.
    pub fn set_event_callback(&self, callback: Option<EventCallback>) {
        *self.inner.callback.lock().unwrap() = callback.map(Arc::from);
    }

    pub fn poll_event(&self) -> Option<HostEvent> {
        self.inner.events.lock().unwrap().pop_front()
    }

    pub fn containers(&self) -> Vec<ContainerInfo> {
        let containers: Vec<Arc<Container>> = self.inner.containers.lock().unwrap().values().cloned().collect();
        let mut list: Vec<ContainerInfo> = containers
            .iter()
            .map(|container| {
                let inner = container.inner.lock().unwrap();
                let pid = inner.process.as_ref().map_or(0, |p| p.conn.pid);
                ContainerInfo {
                    id: container.id,
                    key: container.key.clone(),
                    state: if inner.process.is_some() { ContainerState::Running } else { ContainerState::Dead },
                    pid,
                    instances: container.live_instances().iter().map(|i| i.id).collect(),
                }
            })
            .collect();
        list.sort_by_key(|info| info.id);
        list
    }

    /// Creates a plug-in instance in the container the policy assigns (spawning it if
    /// needed) and loads `config.initial_state`. The instance starts deactivated.
    pub fn create_instance(&self, config: InstanceConfig) -> Result<Instance> {
        self.inner.create_instance(config)
    }

    /// Kills a container process (e.g. on the user's request for an unresponsive plug-in).
    /// Its instances become lost and can be restored. Returns once the process is reaped
    /// and its instances are flagged.
    pub fn kill_container(&self, container: ContainerId) -> Result<()> {
        let target = self.inner.container_by_id(container)?;
        if target.kill(ExitReason::Killed) {
            target.settle(self.inner.config.shutdown_timeout);
        }
        Ok(())
    }

    /// Starts a dead container again and rebuilds its lost instances from their
    /// configuration and last known state.
    pub fn restart_container(&self, container: ContainerId) -> Result<RestartReport> {
        let target = self.inner.container_by_id(container)?;
        target.settle(self.inner.config.request_timeout);
        let mut report = RestartReport::default();
        for shared in target.live_instances() {
            if shared.status() != InstanceStatus::Lost {
                continue;
            }
            match self.inner.restore(&shared) {
                Ok(()) => report.restored.push(shared.id),
                Err(err) => report.failed.push((shared.id, err.to_string())),
            }
        }
        Ok(report)
    }

    /// Shuts all containers down. Instances that still exist become `Closed`.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.inner.shutdown();
    }
}

impl SupInner {
    fn emit(&self, event: HostEvent) {
        let callback = self.callback.lock().unwrap().clone();
        if let Some(callback) = &callback {
            callback(&event);
        }
        let mut events = self.events.lock().unwrap();
        if events.len() >= 65536 {
            events.pop_front(); // nobody polls: keep the newest
        }
        events.push_back(event);
    }

    fn container_by_id(&self, id: ContainerId) -> Result<Arc<Container>> {
        self.containers
            .lock()
            .unwrap()
            .values()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("no container {id}")))
    }

    fn key_for(&self, config: &InstanceConfig, id: InstanceId) -> String {
        match *self.policy.lock().unwrap() {
            GroupingPolicy::Shared => "shared".to_string(),
            GroupingPolicy::PerManufacturer => format!("manufacturer:{}", config.manufacturer),
            GroupingPolicy::PerPlugin => format!("plugin:{}:{}", config.format, config.plugin_id),
            GroupingPolicy::PerInstance => format!("instance:{id}"),
        }
    }

    fn acquire_container(&self, key: String) -> Arc<Container> {
        let mut containers = self.containers.lock().unwrap();
        let container = containers.entry(key.clone()).or_insert_with(|| {
            Arc::new(Container {
                id: self.next_container.fetch_add(1, Ordering::Relaxed),
                key,
                inner: Mutex::new(ContainerInner { generation: 0, process: None, reader: None, intent: None }),
                reaped: Condvar::new(),
                instances: Mutex::new(Vec::new()),
                refs: AtomicU32::new(0),
            })
        });
        container.refs.fetch_add(1, Ordering::Relaxed);
        container.clone()
    }

    /// Drops one placement; the container process ends with its last instance.
    fn release_container(&self, container: &Arc<Container>) {
        let last = {
            let mut containers = self.containers.lock().unwrap();
            let last = container.refs.fetch_sub(1, Ordering::Relaxed) == 1;
            if last {
                containers.remove(&container.key);
            }
            last
        };
        if last {
            container.shutdown(self.config.shutdown_timeout, self.config.request_timeout);
        }
    }

    /// Returns the container's connection, starting the process if it is not running.
    fn ensure_running(self: &Arc<Self>, container: &Arc<Container>) -> Result<(Arc<Connection>, u64)> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let mut inner = container.inner.lock().unwrap();
        // A dead connection means the reader thread is about to reap the process: wait.
        let deadline = Instant::now() + self.config.shutdown_timeout;
        while inner.process.as_ref().is_some_and(|p| p.conn.is_dead()) {
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::ContainerDied);
            }
            inner = container.reaped.wait_timeout(inner, deadline - now).unwrap().0;
        }
        if let Some(process) = inner.process.as_ref() {
            return Ok((process.conn.clone(), inner.generation));
        }
        // The previous process's reader thread is ending (it cleared `process`).
        join_reader(inner.reader.take());
        let generation = inner.generation + 1;
        let socket = format!("c{}-{}.sock", container.id, self.next_socket.fetch_add(1, Ordering::Relaxed));
        let (mut child, stream, hello) = spawn::launch(&spawn::Launch {
            container_path: &self.container_path,
            args: &self.config.container_args,
            env: &self.config.container_env,
            dir: &self.dir,
            socket: &socket,
            timeout: self.config.spawn_timeout,
        })?;
        let reject = |child: &mut Child, error: Error| -> Error {
            let _ = child.kill();
            let _ = child.wait();
            error
        };
        let setup = stream.set_write_timeout(Some(self.config.state_timeout)).and_then(|_| stream.try_clone());
        let reader_stream = match setup {
            Ok(reader) => reader,
            Err(err) => return Err(reject(&mut child, Error::Io(err))),
        };
        let pid = child.id();
        let conn = Arc::new(Connection::new(stream, hello.wake_support, pid));
        let reader = {
            let conn = conn.clone();
            let container = container.clone();
            let sup = Arc::downgrade(self);
            std::thread::Builder::new()
                .name(format!("stpc-reader-{}", container.id))
                .spawn(move || reader_thread(reader_stream, conn, container, sup, generation))
        };
        let reader = match reader {
            Ok(reader) => reader,
            Err(err) => return Err(reject(&mut child, Error::Io(err))),
        };
        inner.generation = generation;
        inner.intent = None;
        inner.process = Some(Process { child, conn: conn.clone() });
        inner.reader = Some(reader);
        drop(inner);
        self.emit(HostEvent::ContainerStarted { container: container.id, pid });
        Ok((conn, generation))
    }

    fn create_instance(self: &Arc<Self>, config: InstanceConfig) -> Result<Instance> {
        validate(&config)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let id = self.next_instance.fetch_add(1, Ordering::Relaxed);
        let container = self.acquire_container(self.key_for(&config, id));
        let pass_through = match config.fallback {
            Fallback::Auto => config.kind == PluginKind::Effect,
            Fallback::Silence => false,
            Fallback::PassThrough => true,
        };
        let shared = Arc::new(InstanceShared {
            id,
            gate: AtomicU32::new(GATE_IDLE),
            status: AtomicU32::new(InstanceStatus::Lost as u32),
            active: AtomicBool::new(false),
            client_bypass: AtomicBool::new(false),
            offline: AtomicBool::new(false),
            latency: AtomicU32::new(0),
            tail: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            late_since_ns: AtomicU64::new(0),
            late_seq: AtomicU32::new(0),
            wait_started_ns: AtomicU64::new(0),
            blocks: AtomicU64::new(0),
            fallback_blocks: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            resyncs: AtomicU64::new(0),
            reported_misses: AtomicU64::new(0),
            hang_reported: AtomicBool::new(false),
            deadline_ns: AtomicU64::new(config.deadline.map_or(0, |d| (d.as_nanos() as u64).max(1))),
            spin_ns: self.config.spin.as_nanos() as u64,
            sample_rate_bits: AtomicU64::new(config.sample_rate.to_bits()),
            pass_through,
            main_in: config.input_buses.first().copied().unwrap_or(0) as usize,
            main_out: config.output_buses.first().copied().unwrap_or(0) as usize,
            total_in: config.input_buses.iter().sum::<u32>() as usize,
            total_out: config.output_buses.iter().sum::<u32>() as usize,
            rt: std::cell::UnsafeCell::new(RtState::new()),
            waiting: std::cell::UnsafeCell::new(Waiting::new(config.event_capacity as usize)),
            held_blocks: AtomicU32::new(0),
            region_ref: Mutex::new(None),
            dirty: Mutex::new(HashMap::new()),
            control: Mutex::new(Control {
                sample_rate: config.sample_rate,
                max_frames: config.max_frames,
                last_state: config.initial_state.clone().map(Arc::new),
                config,
                container: container.clone(),
                conn: None,
                info: InstanceInfo::default(),
                last_state_at: None,
                bypass: false,
                offline: false,
                editor_title: None,
            }),
        });
        let attached = {
            let mut control = shared.control.lock().unwrap();
            self.attach(&shared, &mut control)
        };
        if let Err(err) = attached {
            shared.status.store(InstanceStatus::Closed as u32, Ordering::Release);
            self.release_container(&container);
            return Err(err);
        }
        container.instances.lock().unwrap().push(Arc::downgrade(&shared));
        self.instances.lock().unwrap().insert(id, Arc::downgrade(&shared));
        // The container may have died between the attach and the registration above, in
        // which case its death procedure did not see this instance.
        if container.running().map(|(_, generation)| generation) != Some(shared.generation.load(Ordering::Acquire)) {
            shared.mark_lost(InstanceStatus::Lost);
        }
        Ok(Instance { shared, sup: self.clone() })
    }

    /// Builds the remote instance in the (running or restarted) container and installs
    /// the attachment: region, wake-up primitives, state, parameters, activation.
    fn attach(self: &Arc<Self>, shared: &Arc<InstanceShared>, control: &mut Control) -> Result<()> {
        let (conn, generation) = self.ensure_running(&control.container)?;
        let mode = match self.config.wake {
            WakePreference::Pipe => WakeMode::Pipe,
            WakePreference::Address => WakeMode::Address,
            WakePreference::Auto => WakeMode::Address,
        };
        let both = wake::supported_mask() & conn.wake_support;
        let mode = if both & mode.bit() != 0 {
            mode
        } else if self.config.wake == WakePreference::Auto && both & WakeMode::Pipe.bit() != 0 {
            WakeMode::Pipe
        } else {
            return Err(Error::Invalid(format!("wake mode {mode:?} is not available on both sides")));
        };
        let (link, fds) = new_link(control, control.max_frames, mode)?;
        let params = CreateParams {
            format: control.config.format.clone(),
            plugin_id: control.config.plugin_id.clone(),
            name: control.config.name.clone(),
            options: control.config.options.clone(),
            sample_rate: control.sample_rate,
            max_frames: control.max_frames,
            in_buses: control.config.input_buses.clone(),
            out_buses: control.config.output_buses.clone(),
            event_capacity: control.config.event_capacity,
            out_event_capacity: control.config.out_event_capacity,
            queue_capacity: control.config.notification_capacity,
            wake_mode: mode as u32,
        };
        let borrowed: Vec<BorrowedFd<'_>> = fds.iter().map(|fd| fd.as_fd()).collect();
        let request = Request::CreateInstance { instance: shared.id, params };
        let info = match conn.call(&request, &borrowed, self.config.create_timeout) {
            Ok(Response::Info { info }) => info,
            Ok(other) => return Err(unexpected(&other)),
            Err(err) => {
                if matches!(err, Error::Timeout(_)) {
                    // The plug-in is still loading. Queue its destruction behind the
                    // creation (not waiting for the answer), so nothing is left behind.
                    let _ = conn.call(&Request::DestroyInstance { instance: shared.id }, &[], Duration::ZERO);
                }
                return Err(err);
            }
        };
        drop(borrowed);
        drop(fds); // the container holds its own copies now
        let configure = || -> Result<()> {
            if let Some(state) = control.last_state.clone() {
                let request = Request::SetState { instance: shared.id, data: Bytes(state.as_ref().clone()) };
                expect_ok(conn.call(&request, &[], self.config.state_timeout)?)?;
            }
            let dirty: Vec<(u64, f64)> = shared.dirty.lock().unwrap().iter().map(|(k, v)| (*k, *v)).collect();
            for (id, value) in dirty {
                // Best effort: a parameter the plug-in refuses must not fail the restore.
                let _ = conn.call(
                    &Request::SetParameter { instance: shared.id, id, value },
                    &[],
                    self.config.request_timeout,
                );
            }
            if control.offline {
                expect_ok(conn.call(
                    &Request::SetOffline { instance: shared.id, offline: true },
                    &[],
                    self.config.request_timeout,
                )?)?;
            }
            if control.bypass && !shared.client_bypass.load(Ordering::Relaxed) {
                expect_ok(conn.call(
                    &Request::SetBypass { instance: shared.id, bypass: true },
                    &[],
                    self.config.request_timeout,
                )?)?;
            }
            if shared.active.load(Ordering::Relaxed) {
                expect_ok(conn.call(&Request::Activate { instance: shared.id }, &[], self.config.create_timeout)?)?;
            }
            Ok(())
        };
        if let Err(err) = configure() {
            let _ = conn.call(&Request::DestroyInstance { instance: shared.id }, &[], self.config.request_timeout);
            return Err(err);
        }
        // The latency may differ after the state was applied.
        let info = match conn.call(&Request::GetInfo { instance: shared.id }, &[], self.config.request_timeout) {
            Ok(Response::Info { info }) => info,
            _ => info,
        };
        shared.latency.store(info.latency, Ordering::Relaxed);
        shared.tail.store(info.tail, Ordering::Relaxed);
        let dry = DryDelay::new(
            if shared.pass_through { shared.main_in } else { 0 },
            control.config.max_dry_delay_frames as usize,
            control.max_frames as usize,
        );
        shared.lock_gate();
        *shared.region_ref.lock().unwrap() = Some(link.region.clone());
        // SAFETY: the gate is held exclusively.
        unsafe { (*shared.rt.get()).install(link, dry) };
        shared.late_since_ns.store(0, Ordering::Release);
        shared.wait_started_ns.store(0, Ordering::Relaxed);
        shared.hang_reported.store(false, Ordering::Relaxed);
        shared.generation.store(generation, Ordering::Release);
        shared.status.store(InstanceStatus::Running as u32, Ordering::Release);
        shared.unlock_gate();
        control.conn = Some(conn);
        control.info = info;
        Ok(())
    }

    fn restore(self: &Arc<Self>, shared: &Arc<InstanceShared>) -> Result<()> {
        let mut control = shared.control.lock().unwrap();
        // The audio thread can see a crash before the supervisor has dealt with it: let
        // the container's death settle so that the status below is final.
        control.container.settle(self.config.request_timeout);
        match shared.status() {
            InstanceStatus::Running | InstanceStatus::Late => return Ok(()),
            InstanceStatus::Closed => return Err(Error::Closed),
            InstanceStatus::Lost => {}
        }
        let container = control.container.id;
        match self.attach(shared, &mut control) {
            Ok(()) => {
                drop(control);
                self.emit(HostEvent::InstanceRestored { instance: shared.id, container });
                Ok(())
            }
            Err(err) => {
                drop(control);
                self.emit(HostEvent::InstanceRestoreFailed { instance: shared.id, error: err.to_string() });
                Err(err)
            }
        }
    }

    fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        *self.housekeeping_stop.lock().unwrap() = true;
        self.housekeeping_wake.notify_all();
        let handle = self.housekeeping.lock().unwrap().take();
        if let Some(handle) = handle {
            if handle.thread().id() != std::thread::current().id() {
                let _ = handle.join();
            }
        }
        let containers: Vec<Arc<Container>> = self.containers.lock().unwrap().values().cloned().collect();
        for container in &containers {
            for shared in container.live_instances() {
                shared.mark_lost(InstanceStatus::Closed);
            }
        }
        // Ask all of them first, then wait: shutdown time does not add up per container.
        let workers: Vec<_> = containers
            .into_iter()
            .map(|container| {
                let (timeout, request_timeout) = (self.config.shutdown_timeout, self.config.request_timeout);
                std::thread::spawn(move || container.shutdown(timeout, request_timeout))
            })
            .collect();
        for worker in workers {
            let _ = worker.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn validate(config: &InstanceConfig) -> Result<()> {
    let region = RegionConfig {
        max_frames: config.max_frames,
        in_buses: config.input_buses.clone(),
        out_buses: config.output_buses.clone(),
        event_capacity: config.event_capacity,
        out_event_capacity: config.out_event_capacity,
        queue_capacity: config.notification_capacity,
    };
    stpc_transport::region::Layout::new(&region).map_err(|e| Error::Invalid(e.to_string()))?;
    if !(config.sample_rate.is_finite() && config.sample_rate > 0.0) {
        return Err(Error::Invalid("sample rate must be positive".into()));
    }
    if config.format.is_empty() {
        return Err(Error::Invalid("format is empty".into()));
    }
    Ok(())
}

/// Creates a region and wake-up primitives; returns the host half and the descriptors
/// that go to the container (region first).
fn new_link(control: &Control, max_frames: u32, mode: WakeMode) -> Result<(Link, Vec<OwnedFd>)> {
    let region_config = RegionConfig {
        max_frames,
        in_buses: control.config.input_buses.clone(),
        out_buses: control.config.output_buses.clone(),
        event_capacity: control.config.event_capacity,
        out_event_capacity: control.config.out_event_capacity,
        queue_capacity: control.config.notification_capacity,
    };
    let (region, region_fd) = Region::create(&region_config).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut fds = vec![region_fd];
    let (notifier, waiter) = match mode {
        WakeMode::Address => (Notifier::address(), Waiter::address()),
        WakeMode::Pipe => {
            let (req_read, req_write) = wake::pipe_pair().map_err(Error::Io)?;
            let (rsp_read, rsp_write) = wake::pipe_pair().map_err(Error::Io)?;
            fds.push(req_read);
            fds.push(rsp_write);
            (Notifier::pipe(req_write), Waiter::pipe(rsp_read))
        }
    };
    Ok((Link { region: Arc::new(region), notifier, waiter }, fds))
}

fn unexpected(response: &Response) -> Error {
    Error::Protocol(format!("unexpected response {}", response.name()))
}

fn expect_ok(response: Response) -> Result<()> {
    match response {
        Response::Ok {} => Ok(()),
        other => Err(unexpected(&other)),
    }
}

// ---- reader thread and death procedure --------------------------------------------------

fn reader_thread(
    stream: Stream,
    conn: Arc<Connection>,
    container: Arc<Container>,
    sup: Weak<SupInner>,
    generation: u64,
) {
    loop {
        let frame = match stream.recv() {
            Ok(frame) => frame,
            Err(_) => break,
        };
        match frame.kind {
            FrameKind::Response => match Response::decode(&frame.payload) {
                Ok(response) => conn.dispatch(frame.id, response),
                Err(_) => break, // a garbled stream cannot be resynchronised
            },
            FrameKind::Notification => {
                if let (Ok(ContainerNotice::Log { message, .. }), Some(sup)) =
                    (ContainerNotice::decode(&frame.payload), sup.upgrade())
                {
                    sup.emit(HostEvent::Log { container: container.id, message });
                }
            }
            _ => break,
        }
    }
    // The connection is gone: the process is dead, dying, or misbehaving. Make it dead.
    conn.fail_all();
    let mut lost = Vec::new();
    let reason = {
        let mut inner = container.inner.lock().unwrap();
        if inner.generation != generation {
            return;
        }
        let Some(mut process) = inner.process.take() else { return };
        let _ = process.child.kill();
        let status = process.child.wait().ok();
        let reason = inner.intent.take().unwrap_or_else(|| match status {
            Some(status) => match status.signal() {
                Some(signal) => ExitReason::Crashed { signal },
                None => ExitReason::Exited { code: status.code().unwrap_or(-1) },
            },
            None => ExitReason::Exited { code: -1 },
        });
        // Flag the instances before anyone waiting on `reaped` can look: whoever sees
        // "no process" also sees the final instance statuses.
        for shared in container.live_instances() {
            if shared.generation.load(Ordering::Acquire) != generation || shared.status() == InstanceStatus::Closed {
                continue;
            }
            if reason == ExitReason::Shutdown {
                shared.mark_lost(InstanceStatus::Closed);
            } else {
                shared.mark_lost(InstanceStatus::Lost);
                lost.push(shared.id);
            }
        }
        container.reaped.notify_all();
        reason
    };
    if let Some(sup) = sup.upgrade() {
        for &instance in &lost {
            sup.emit(HostEvent::InstanceLost { instance, container: container.id, reason });
        }
        sup.emit(HostEvent::ContainerExited { container: container.id, pid: conn.pid, reason, lost });
    }
}

// ---- housekeeping -------------------------------------------------------------------------

fn housekeeping_thread(sup: Weak<SupInner>) {
    loop {
        let Some(sup) = sup.upgrade() else { return };
        {
            let stop = sup.housekeeping_stop.lock().unwrap();
            if *stop {
                return;
            }
            let (stop, _) = sup.housekeeping_wake.wait_timeout(stop, sup.config.housekeeping_interval).unwrap();
            if *stop {
                return;
            }
        }
        let instances: Vec<Arc<InstanceShared>> =
            sup.instances.lock().unwrap().values().filter_map(Weak::upgrade).collect();
        for shared in &instances {
            housekeep(&sup, shared);
        }
    }
}

fn housekeep(sup: &Arc<SupInner>, shared: &Arc<InstanceShared>) {
    let region = shared.region_ref.lock().unwrap().clone();
    let Some(region) = region else { return };
    let status = shared.status();
    if status == InstanceStatus::Closed {
        return;
    }
    let instance = shared.id;

    // Notifications from the plug-in.
    while let Some(notification) = region.pop_notification() {
        let event = match notification {
            Notification::ParamGestureBegin { id } => HostEvent::ParameterGesture { instance, id, begin: true },
            Notification::ParamGestureEnd { id } => HostEvent::ParameterGesture { instance, id, begin: false },
            Notification::ParamValue { id, value } => {
                shared.dirty.lock().unwrap().insert(id, value);
                HostEvent::ParameterChanged { instance, id, value }
            }
            Notification::LatencyChanged { frames } => {
                shared.latency.store(frames, Ordering::Relaxed);
                HostEvent::LatencyChanged { instance, frames }
            }
            Notification::TailChanged { frames } => {
                shared.tail.store(frames, Ordering::Relaxed);
                HostEvent::TailChanged { instance, frames }
            }
            Notification::ParamsRescan => HostEvent::ParametersRescan { instance },
            Notification::StateDirty => HostEvent::StateDirty { instance },
            Notification::EditorClosed => {
                if let Ok(mut control) = shared.control.try_lock() {
                    control.editor_title = None;
                }
                HostEvent::EditorClosed { instance }
            }
            Notification::EditorResized { width, height } => HostEvent::EditorResized { instance, width, height },
            Notification::RestartRequested => HostEvent::RestartRequested { instance },
        };
        sup.emit(event);
    }

    // Deadline misses: one event per episode.
    let misses = shared.deadline_misses.load(Ordering::Relaxed);
    if shared.reported_misses.swap(misses, Ordering::Relaxed) != misses {
        sup.emit(HostEvent::DeadlineMissed { instance });
    }

    // Hang detection: a block the container has owed for too long.
    let now = monotonic_ns();
    let late_since = shared.late_since_ns.load(Ordering::Acquire);
    let waiting_since = shared.wait_started_ns.load(Ordering::Relaxed);
    let offline = shared.offline.load(Ordering::Relaxed);
    let mut hung = false;
    if status == InstanceStatus::Late && late_since != 0 {
        let owed = shared.late_seq.load(Ordering::Relaxed);
        let caught_up = region.header().rsp.seq.load(Ordering::SeqCst) == owed;
        hung = !caught_up && now.saturating_sub(late_since) > sup.config.hang_timeout.as_nanos() as u64;
    } else if status == InstanceStatus::Running && waiting_since != 0 {
        // Waiting without (or with a long) deadline: offline rendering.
        let limit = if offline { sup.config.offline_hang_timeout } else { Some(sup.config.hang_timeout) };
        hung = limit.is_some_and(|limit| now.saturating_sub(waiting_since) > limit.as_nanos() as u64);
    }
    if hung {
        if !shared.hang_reported.swap(true, Ordering::Relaxed) {
            let container = shared.control.try_lock().ok().map(|c| c.container.clone());
            if let Some(container) = container {
                sup.emit(HostEvent::InstanceHung { instance, container: container.id });
                if sup.config.kill_hung {
                    container.kill(ExitReason::Hung);
                }
            } else {
                shared.hang_reported.store(false, Ordering::Relaxed); // control is busy: next tick
            }
        }
    } else if status == InstanceStatus::Running && waiting_since == 0 {
        shared.hang_reported.store(false, Ordering::Relaxed);
    }

    // Periodic state snapshot.
    if let Some(interval) = sup.config.auto_state_interval {
        if status == InstanceStatus::Running {
            if let Ok(mut control) = shared.control.try_lock() {
                let due = control.last_state_at.is_none_or(|at| at.elapsed() >= interval);
                if due {
                    control.last_state_at = Some(Instant::now()); // also paces retries after a failure
                    let _ = fetch_state(sup, shared, &mut control);
                }
            }
        }
    }
}

// ---- control half of Instance -------------------------------------------------------------

pub(crate) struct Control {
    pub config: InstanceConfig,
    pub container: Arc<Container>,
    pub conn: Option<Arc<Connection>>,
    pub info: InstanceInfo,
    pub last_state: Option<Arc<Vec<u8>>>,
    pub last_state_at: Option<Instant>,
    pub sample_rate: f64,
    pub max_frames: u32,
    pub bypass: bool,
    pub offline: bool,
    pub editor_title: Option<String>,
}

fn call(shared: &InstanceShared, control: &Control, request: &Request, timeout: Duration) -> Result<Response> {
    match shared.status() {
        InstanceStatus::Lost => return Err(Error::InstanceLost),
        InstanceStatus::Closed => return Err(Error::Closed),
        _ => {}
    }
    let conn = control.conn.as_ref().ok_or(Error::InstanceLost)?;
    let result = conn.call(request, &[], timeout);
    if matches!(result, Err(Error::ContainerDied)) {
        // Return only once the death is fully processed: the instance is flagged lost and
        // the events are out, so the caller can restore right away.
        control.container.settle(Duration::from_secs(5));
    }
    result
}

fn fetch_state(sup: &SupInner, shared: &InstanceShared, control: &mut Control) -> Result<Arc<Vec<u8>>> {
    // Parameter changes made from now on are newer than the snapshot: forget older ones first.
    let before: HashMap<u64, f64> = std::mem::take(&mut *shared.dirty.lock().unwrap());
    match call(shared, control, &Request::GetState { instance: shared.id }, sup.config.state_timeout) {
        Ok(Response::State { data }) => {
            let state = Arc::new(data.0);
            control.last_state = Some(state.clone());
            control.last_state_at = Some(Instant::now());
            Ok(state)
        }
        other => {
            // Keep the changes the failed snapshot would have covered.
            let mut dirty = shared.dirty.lock().unwrap();
            for (id, value) in before {
                dirty.entry(id).or_insert(value);
            }
            Err(match other {
                Ok(response) => unexpected(&response),
                Err(err) => err,
            })
        }
    }
}

impl Instance {
    fn simple(&self, request: Request, timeout: Duration) -> Result<()> {
        let control = self.shared.control.lock().unwrap();
        expect_ok(call(&self.shared, &control, &request, timeout)?)
    }

    fn timeout(&self) -> Duration {
        self.sup.config.request_timeout
    }

    /// The container this instance is placed in.
    pub fn container(&self) -> ContainerId {
        self.shared.control.lock().unwrap().container.id
    }

    /// Name, capabilities, latency, tail and bus usage, as reported at creation or the
    /// last refresh.
    pub fn info(&self) -> InstanceInfo {
        let mut info = self.shared.control.lock().unwrap().info.clone();
        info.latency = self.latency();
        info.tail = self.tail();
        info
    }

    /// Reads latency, tail and capabilities from the plug-in again.
    pub fn refresh_info(&self) -> Result<InstanceInfo> {
        let mut control = self.shared.control.lock().unwrap();
        match call(&self.shared, &control, &Request::GetInfo { instance: self.shared.id }, self.timeout())? {
            Response::Info { info } => {
                self.shared.latency.store(info.latency, Ordering::Relaxed);
                self.shared.tail.store(info.tail, Ordering::Relaxed);
                control.info = info.clone();
                Ok(info)
            }
            other => Err(unexpected(&other)),
        }
    }

    /// Prepares the plug-in for processing. Until then `process` plays the fallback.
    pub fn activate(&self) -> Result<()> {
        let control = self.shared.control.lock().unwrap();
        let request = Request::Activate { instance: self.shared.id };
        expect_ok(call(&self.shared, &control, &request, self.sup.config.create_timeout)?)?;
        self.shared.active.store(true, Ordering::Release);
        Ok(())
    }

    pub fn deactivate(&self) -> Result<()> {
        let control = self.shared.control.lock().unwrap();
        self.shared.active.store(false, Ordering::Release);
        if self.shared.status() == InstanceStatus::Lost {
            return Ok(()); // remembered for the restore
        }
        expect_ok(call(&self.shared, &control, &Request::Deactivate { instance: self.shared.id }, self.timeout())?)
    }

    pub fn is_active(&self) -> bool {
        self.shared.active.load(Ordering::Relaxed)
    }

    /// Clears the plug-in's delay lines, voices and envelopes.
    pub fn reset(&self) -> Result<()> {
        self.simple(Request::Reset { instance: self.shared.id }, self.timeout())
    }

    /// Offline rendering: `process` waits without deadline (exports never drop a plug-in),
    /// and the plug-in is told to render offline.
    pub fn set_offline(&self, offline: bool) -> Result<()> {
        let mut control = self.shared.control.lock().unwrap();
        control.offline = offline;
        self.shared.offline.store(offline, Ordering::Release);
        if self.shared.status() == InstanceStatus::Lost {
            return Ok(());
        }
        expect_ok(call(
            &self.shared,
            &control,
            &Request::SetOffline { instance: self.shared.id, offline },
            self.timeout(),
        )?)
    }

    /// Bypasses the plug-in. Uses the plug-in's own bypass when it has one; otherwise the
    /// client bypasses by playing the fallback (returns `Ok(false)` in that case).
    pub fn set_bypass(&self, bypass: bool) -> Result<bool> {
        let mut control = self.shared.control.lock().unwrap();
        control.bypass = bypass;
        if self.shared.status() == InstanceStatus::Lost {
            return Ok(false);
        }
        match call(&self.shared, &control, &Request::SetBypass { instance: self.shared.id, bypass }, self.timeout()) {
            Ok(response) => {
                expect_ok(response)?;
                self.shared.client_bypass.store(false, Ordering::Release);
                Ok(true)
            }
            Err(Error::Unsupported) => {
                self.shared.client_bypass.store(bypass, Ordering::Release);
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    /// New sample rate and/or maximum block size. `process` plays the fallback meanwhile;
    /// an active instance is reactivated with the new values.
    pub fn reconfigure(&self, sample_rate: f64, max_frames: u32) -> Result<()> {
        let mut control = self.shared.control.lock().unwrap();
        let probe = InstanceConfig { sample_rate, max_frames, ..control.config.clone() };
        validate(&probe)?;
        if self.shared.status() == InstanceStatus::Lost {
            control.sample_rate = sample_rate;
            control.max_frames = max_frames;
            return Ok(());
        }
        let conn = control.conn.clone().ok_or(Error::InstanceLost)?;
        // SAFETY: only the wake mode is read, through the gate.
        self.shared.lock_gate();
        let mode = unsafe { (*self.shared.rt.get()).link.as_ref().map(|link| link.waiter.mode()) }
            .unwrap_or(WakeMode::Address);
        let result = (|| -> Result<InstanceInfo> {
            let (link, fds) = new_link(&control, max_frames, mode)?;
            let borrowed: Vec<BorrowedFd<'_>> = fds.iter().map(|fd| fd.as_fd()).collect();
            let request = Request::Reconfigure { instance: self.shared.id, sample_rate, max_frames };
            let info = match conn.call(&request, &borrowed, self.sup.config.create_timeout)? {
                Response::Info { info } => info,
                other => return Err(unexpected(&other)),
            };
            let dry = DryDelay::new(
                if self.shared.pass_through { self.shared.main_in } else { 0 },
                control.config.max_dry_delay_frames as usize,
                max_frames as usize,
            );
            *self.shared.region_ref.lock().unwrap() = Some(link.region.clone());
            // SAFETY: the gate is held exclusively.
            unsafe { (*self.shared.rt.get()).install(link, dry) };
            Ok(info)
        })();
        if result.is_ok() {
            self.shared.sample_rate_bits.store(sample_rate.to_bits(), Ordering::Relaxed);
        }
        self.shared.unlock_gate();
        let info = result?;
        self.shared.latency.store(info.latency, Ordering::Relaxed);
        self.shared.tail.store(info.tail, Ordering::Relaxed);
        control.sample_rate = sample_rate;
        control.max_frames = max_frames;
        control.info = info;
        // Reconfiguring deactivates the plug-in; bring it back if it was active.
        if self.shared.active.load(Ordering::Relaxed) {
            let request = Request::Activate { instance: self.shared.id };
            let activated = call(&self.shared, &control, &request, self.sup.config.create_timeout).and_then(expect_ok);
            if activated.is_err() {
                self.shared.active.store(false, Ordering::Release);
            }
            activated?;
        }
        Ok(())
    }

    // ---- parameters ----

    pub fn parameters(&self) -> Result<Vec<ParameterInfo>> {
        let control = self.shared.control.lock().unwrap();
        match call(&self.shared, &control, &Request::GetParameters { instance: self.shared.id }, self.timeout())? {
            Response::Parameters { list } => Ok(list),
            other => Err(unexpected(&other)),
        }
    }

    /// Current values of all parameters (e.g. after a state or preset load).
    pub fn parameter_values(&self) -> Result<Vec<ParameterValue>> {
        let control = self.shared.control.lock().unwrap();
        match call(&self.shared, &control, &Request::GetParameterValues { instance: self.shared.id }, self.timeout())? {
            Response::Values { values } => Ok(values),
            other => Err(unexpected(&other)),
        }
    }

    /// `None` if the plug-in has no such parameter.
    pub fn parameter(&self, id: u64) -> Result<Option<f64>> {
        let control = self.shared.control.lock().unwrap();
        match call(&self.shared, &control, &Request::GetParameter { instance: self.shared.id, id }, self.timeout())? {
            Response::Value { value } => Ok(value),
            other => Err(unexpected(&other)),
        }
    }

    /// Sets a parameter from a control thread (untimed). Sample-accurate changes go
    /// through `process` as events. `value` is in the parameter's own range.
    pub fn set_parameter(&self, id: u64, value: f64) -> Result<()> {
        let control = self.shared.control.lock().unwrap();
        let request = Request::SetParameter { instance: self.shared.id, id, value };
        expect_ok(call(&self.shared, &control, &request, self.timeout())?)?;
        self.shared.dirty.lock().unwrap().insert(id, value);
        Ok(())
    }

    /// Display texts for values of a parameter.
    pub fn format_parameter(&self, id: u64, values: &[f64]) -> Result<Vec<String>> {
        let control = self.shared.control.lock().unwrap();
        let request = Request::FormatParameter { instance: self.shared.id, id, values: values.to_vec() };
        match call(&self.shared, &control, &request, self.timeout())? {
            Response::Strings { strings } => Ok(strings),
            other => Err(unexpected(&other)),
        }
    }

    pub fn parse_parameter(&self, id: u64, text: &str) -> Result<Option<f64>> {
        let control = self.shared.control.lock().unwrap();
        let request = Request::ParseParameter { instance: self.shared.id, id, text: text.to_string() };
        match call(&self.shared, &control, &request, self.timeout())? {
            Response::Value { value } => Ok(value),
            other => Err(unexpected(&other)),
        }
    }

    // ---- state and presets ----

    /// Fetches the plug-in's state. The client keeps a copy as the last known state.
    pub fn state(&self) -> Result<Vec<u8>> {
        let mut control = self.shared.control.lock().unwrap();
        fetch_state(&self.sup, &self.shared, &mut control).map(|state| state.as_ref().clone())
    }

    /// Loads a state into the plug-in; it becomes the last known state.
    pub fn set_state(&self, data: &[u8]) -> Result<()> {
        let mut control = self.shared.control.lock().unwrap();
        let state = Arc::new(data.to_vec());
        if self.shared.status() == InstanceStatus::Lost {
            // Applied by the next restore.
            control.last_state = Some(state);
            self.shared.dirty.lock().unwrap().clear();
            return Ok(());
        }
        let request = Request::SetState { instance: self.shared.id, data: Bytes(data.to_vec()) };
        expect_ok(call(&self.shared, &control, &request, self.sup.config.state_timeout)?)?;
        control.last_state = Some(state);
        control.last_state_at = Some(Instant::now());
        self.shared.dirty.lock().unwrap().clear();
        Ok(())
    }

    /// What a restore would load: the state last fetched from or given to the plug-in.
    /// Works for lost instances.
    pub fn last_known_state(&self) -> Option<Arc<Vec<u8>>> {
        self.shared.control.lock().unwrap().last_state.clone()
    }

    pub fn presets(&self) -> Result<Vec<PresetInfo>> {
        let control = self.shared.control.lock().unwrap();
        match call(&self.shared, &control, &Request::GetPresets { instance: self.shared.id }, self.timeout())? {
            Response::Presets { list } => Ok(list),
            other => Err(unexpected(&other)),
        }
    }

    /// Loads a factory preset, then snapshots the resulting state (best effort).
    pub fn load_preset(&self, id: u32) -> Result<()> {
        let mut control = self.shared.control.lock().unwrap();
        let request = Request::LoadPreset { instance: self.shared.id, id };
        expect_ok(call(&self.shared, &control, &request, self.sup.config.state_timeout)?)?;
        let _ = fetch_state(&self.sup, &self.shared, &mut control);
        Ok(())
    }

    // ---- editor ----

    /// Opens the plug-in's editor in a window owned by the container process.
    pub fn open_editor(&self, title: &str) -> Result<EditorInfo> {
        let mut control = self.shared.control.lock().unwrap();
        let request = Request::OpenEditor { instance: self.shared.id, title: title.to_string() };
        match call(&self.shared, &control, &request, self.sup.config.create_timeout)? {
            Response::Editor { info } => {
                control.editor_title = Some(title.to_string());
                Ok(info)
            }
            other => Err(unexpected(&other)),
        }
    }

    pub fn close_editor(&self) -> Result<()> {
        let mut control = self.shared.control.lock().unwrap();
        control.editor_title = None;
        if self.shared.status() == InstanceStatus::Lost {
            return Ok(());
        }
        expect_ok(call(&self.shared, &control, &Request::CloseEditor { instance: self.shared.id }, self.timeout())?)
    }

    /// Brings the editor window to the front.
    pub fn focus_editor(&self) -> Result<()> {
        self.simple(Request::FocusEditor { instance: self.shared.id }, self.timeout())
    }

    pub fn set_editor_title(&self, title: &str) -> Result<()> {
        let mut control = self.shared.control.lock().unwrap();
        let request = Request::SetEditorTitle { instance: self.shared.id, title: title.to_string() };
        expect_ok(call(&self.shared, &control, &request, self.timeout())?)?;
        if control.editor_title.is_some() {
            control.editor_title = Some(title.to_string());
        }
        Ok(())
    }

    /// Title of the editor if the host opened it and it was not closed since (also after
    /// the instance was lost, so the host can reopen it after a restore).
    pub fn editor_title(&self) -> Option<String> {
        self.shared.control.lock().unwrap().editor_title.clone()
    }

    // ---- health ----

    /// Round trip to the container's I/O thread (`main_thread == false`) or through its
    /// main thread, which also proves the plug-ins' main thread is responsive.
    pub fn ping(&self, main_thread: bool, timeout: Duration) -> Result<Duration> {
        let control = self.shared.control.lock().unwrap();
        let started = Instant::now();
        let request = if main_thread { Request::PingMain {} } else { Request::Ping {} };
        expect_ok(call(&self.shared, &control, &request, timeout)?)?;
        Ok(started.elapsed())
    }

    /// Rebuilds a lost instance: restarts its container if necessary, creates the plug-in
    /// again, loads the last known state, replays parameter changes made since, and
    /// restores activation, bypass and offline mode. No-op for a healthy instance.
    pub fn restore(&self) -> Result<()> {
        self.sup.restore(&self.shared)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let container = {
            let mut control = self.shared.control.lock().unwrap();
            if matches!(self.shared.status(), InstanceStatus::Running | InstanceStatus::Late) {
                let request = Request::DestroyInstance { instance: self.shared.id };
                let _ = call(&self.shared, &control, &request, self.sup.config.request_timeout);
            }
            self.shared.status.store(InstanceStatus::Closed as u32, Ordering::Release);
            control.conn = None;
            control.container.clone()
        };
        self.sup.instances.lock().unwrap().remove(&self.shared.id);
        container.instances.lock().unwrap().retain(|weak| weak.upgrade().is_some_and(|i| i.id != self.shared.id));
        self.sup.release_container(&container);
    }
}

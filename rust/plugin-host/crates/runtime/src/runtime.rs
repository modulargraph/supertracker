//! Control loop, instance table and real-time threads of a container process.

use std::cell::UnsafeCell;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use stpc_transport::channel::{Frame, FrameKind, Stream};
use stpc_transport::messages::*;
use stpc_transport::region::{
    Event, Notification, Region, RegionConfig, REQ_DISCONTINUITY, REQ_OFFLINE, RSP_BUSY, RSP_ERROR, RSP_INACTIVE,
    RSP_OUT_EVENTS_DROPPED, RSP_PROCESSED,
};
use stpc_transport::wake::{self, Notifier, WaitOutcome, Waiter, WakeMode};
use stpc_transport::wire::{Bytes, Wire};
use stpc_transport::{monotonic_ns, ARG_ENDPOINT, ARG_TOKEN, HELLO_MAGIC, PROTOCOL_VERSION};

use crate::plugin::{BusSpan, CreateRequest, Plugin, PluginBackend, PluginError, ProcessData, ProcessStatus, Processor};

#[derive(Debug)]
pub enum RuntimeError {
    Arguments(String),
    Io(std::io::Error),
    /// The supervisor refused the handshake (protocol version or token mismatch).
    Rejected(String),
    Protocol(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::Arguments(msg) => write!(f, "bad arguments: {msg}"),
            RuntimeError::Io(err) => write!(f, "control channel: {err}"),
            RuntimeError::Rejected(msg) => write!(f, "rejected by the host: {msg}"),
            RuntimeError::Protocol(msg) => write!(f, "protocol error: {msg}"),
        }
    }
}
impl std::error::Error for RuntimeError {}

#[derive(Clone)]
pub struct RuntimeConfig {
    /// Control endpoint the supervisor listens on.
    pub endpoint: String,
    /// Token proving this process is the one the supervisor spawned.
    pub token: String,
    /// Call `Plugin::idle` on every instance at this interval (None: only on request).
    pub idle_interval: Option<Duration>,
    /// Try to give real-time threads a real-time scheduling class.
    pub realtime_priority: bool,
    /// Protocol version announced in the handshake. Only tests change it.
    pub protocol_version: u32,
}

impl RuntimeConfig {
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> RuntimeConfig {
        RuntimeConfig {
            endpoint: endpoint.into(),
            token: token.into(),
            idle_interval: None,
            realtime_priority: true,
            protocol_version: PROTOCOL_VERSION,
        }
    }

    /// Reads `--stpc-endpoint <path>` and `--stpc-token <token>` (the options the
    /// supervisor appends); other arguments are ignored.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<RuntimeConfig, RuntimeError> {
        let mut endpoint = None;
        let mut token = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == ARG_ENDPOINT {
                endpoint = args.next();
            } else if arg == ARG_TOKEN {
                token = args.next();
            }
        }
        match (endpoint, token) {
            (Some(endpoint), Some(token)) => Ok(RuntimeConfig::new(endpoint, token)),
            _ => Err(RuntimeError::Arguments(format!("{ARG_ENDPOINT} and {ARG_TOKEN} are required"))),
        }
    }
}

/// Makes a panic on any thread terminate the process (after printing it), so that a bug in
/// a backend is a container crash the host detects, not a silently dead thread.
pub fn abort_on_panic() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        std::process::abort();
    }));
}

// ---- shared between threads -------------------------------------------------------------

enum Task {
    Request { id: u64, request: Request, fds: Vec<OwnedFd> },
    BadRequest { id: u64, message: String },
}

struct Shared {
    tasks: Mutex<VecDeque<Task>>,
    cond: Condvar,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    writer: Mutex<Option<Stream>>,
    /// Some instance asked for `Plugin::idle`.
    idle_requested: AtomicBool,
}

impl Shared {
    fn wake_main(&self) {
        self.cond.notify_one();
        if let Some(waker) = self.waker.lock().unwrap().clone() {
            waker();
        }
    }

    fn send(&self, kind: FrameKind, id: u64, payload: &[u8]) {
        if let Some(stream) = self.writer.lock().unwrap().as_ref() {
            // A failed write means the host is gone; the I/O thread ends the process.
            let _ = stream.send(kind, id, payload, &[]);
        }
    }

    fn respond(&self, id: u64, response: &Response) {
        self.send(FrameKind::Response, id, &response.encode());
    }
}

const GATE_IDLE: u32 = 0;
const GATE_PROCESSING: u32 = 1;
const GATE_EXCLUSIVE: u32 = 2;

/// What an instance's real-time thread shares with the main thread.
struct RtShared {
    region: Arc<Region>,
    waiter: Waiter,
    notifier: Notifier,
    /// Excludes `process` from main-thread operations that must not overlap it.
    gate: AtomicU32,
    stop: AtomicBool,
    exited: AtomicBool,
    /// Accessed only by the holder of the gate.
    processor: UnsafeCell<Option<Box<dyn Processor>>>,
    in_buses: Vec<u32>,
    out_buses: Vec<u32>,
    period_ns: u64,
    realtime_priority: bool,
}

// SAFETY: `processor` is only touched by the thread that holds `gate` (PROCESSING: the
// real-time thread; EXCLUSIVE: the main thread). Everything else is Sync already.
unsafe impl Sync for RtShared {}
unsafe impl Send for RtShared {}

struct HostLink {
    instance: u64,
    /// The region notifications go to; null once the instance is closed.
    current: AtomicPtr<Region>,
    /// Keeps every region this link ever pointed to alive as long as a handle exists.
    keep: Mutex<Vec<Arc<Region>>>,
    idle_requested: AtomicBool,
    shared: Arc<Shared>,
}

/// A plug-in's line to the host. Cheap to clone, usable from any thread.
#[derive(Clone)]
pub struct HostHandle(Arc<HostLink>);

impl HostHandle {
    /// Queues a notification for the host. Lock-free and real-time safe. Returns false if
    /// the queue is full or the instance is closed.
    pub fn notify(&self, notification: Notification) -> bool {
        let region = self.0.current.load(Ordering::Acquire);
        if region.is_null() {
            return false;
        }
        // SAFETY: `keep` holds an Arc to every region `current` has pointed to.
        unsafe { (*region).push_notification(notification) }
    }

    /// Asks for [`Plugin::idle`] to run soon on the main thread. Callable from any thread;
    /// from the real-time thread it costs one uncontended wake-up system call.
    pub fn request_main_callback(&self) {
        self.0.idle_requested.store(true, Ordering::Release);
        self.0.shared.idle_requested.store(true, Ordering::Release);
        self.0.shared.cond.notify_one();
        if let Ok(waker) = self.0.shared.waker.try_lock() {
            if let Some(waker) = waker.as_ref() {
                waker();
            }
        }
    }

    pub fn instance_id(&self) -> u64 {
        self.0.instance
    }

    fn set_region(&self, region: &Arc<Region>) {
        self.0.keep.lock().unwrap().push(region.clone());
        self.0.current.store(Arc::as_ptr(region) as *mut Region, Ordering::Release);
    }

    fn close(&self) {
        self.0.current.store(std::ptr::null_mut(), Ordering::Release);
    }
}

struct Slot {
    plugin: Box<dyn Plugin>,
    link: HostHandle,
    rt: Arc<RtShared>,
    thread: Option<JoinHandle<()>>,
    params: CreateParams,
    active: bool,
    editor_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopState {
    Running,
    /// The host asked the container to shut down; all instances are destroyed. Leave the
    /// event loop and return from `main`.
    Exit,
}

/// The container runtime. Create it on the main thread and keep it there.
pub struct Runtime {
    config: RuntimeConfig,
    backends: HashMap<String, Box<dyn PluginBackend>>,
    shared: Arc<Shared>,
    slots: HashMap<u64, Slot>,
    exit: bool,
    last_idle: Instant,
}

impl Runtime {
    pub fn new(config: RuntimeConfig) -> Runtime {
        Runtime {
            config,
            backends: HashMap::new(),
            shared: Arc::new(Shared {
                tasks: Mutex::new(VecDeque::new()),
                cond: Condvar::new(),
                waker: Mutex::new(None),
                writer: Mutex::new(None),
                idle_requested: AtomicBool::new(false),
            }),
            slots: HashMap::new(),
            exit: false,
            last_idle: Instant::now(),
        }
    }

    /// Registers the backend for its format; a later registration replaces an earlier one.
    pub fn register_backend(&mut self, backend: Box<dyn PluginBackend>) {
        self.backends.insert(backend.format().to_string(), backend);
    }

    /// For programs whose main thread runs an OS event loop (AppKit, Win32) instead of
    /// [`Runtime::run`]: `waker` is called from other threads whenever work is pending and
    /// must cause [`Runtime::run_pending`] to be called on the main thread soon (e.g. by
    /// signalling a run-loop source). It must not block.
    pub fn set_main_waker(&mut self, waker: Arc<dyn Fn() + Send + Sync>) {
        *self.shared.waker.lock().unwrap() = Some(waker);
    }

    /// Connects to the supervisor, performs the handshake and starts the I/O thread.
    pub fn connect(&mut self) -> Result<(), RuntimeError> {
        let stream = Stream::connect(&self.config.endpoint).map_err(RuntimeError::Io)?;
        let mut formats: Vec<String> = self.backends.keys().cloned().collect();
        formats.sort();
        let hello = Hello {
            magic: HELLO_MAGIC,
            protocol_version: self.config.protocol_version,
            pid: std::process::id(),
            token: self.config.token.clone(),
            wake_support: wake::supported_mask(),
            runtime: format!("stpc-runtime {}", env!("CARGO_PKG_VERSION")),
            formats,
        };
        stream.send(FrameKind::Handshake, 0, &hello.encode(), &[]).map_err(RuntimeError::Io)?;
        stream.set_read_timeout(Some(Duration::from_secs(20))).map_err(RuntimeError::Io)?;
        let frame = stream.recv().map_err(RuntimeError::Io)?;
        stream.set_read_timeout(None).map_err(RuntimeError::Io)?;
        if frame.kind != FrameKind::Handshake {
            return Err(RuntimeError::Protocol("expected the handshake answer".into()));
        }
        let welcome = Welcome::decode(&frame.payload).map_err(|e| RuntimeError::Protocol(e.to_string()))?;
        if !welcome.accepted {
            return Err(RuntimeError::Rejected(welcome.reason));
        }
        if welcome.protocol_version != self.config.protocol_version {
            return Err(RuntimeError::Rejected(format!(
                "host speaks protocol {}, this container {}",
                welcome.protocol_version, self.config.protocol_version
            )));
        }
        let reader = stream.try_clone().map_err(RuntimeError::Io)?;
        *self.shared.writer.lock().unwrap() = Some(stream);
        let shared = self.shared.clone();
        std::thread::Builder::new()
            .name("stpc-io".into())
            .spawn(move || io_thread(reader, shared))
            .map_err(RuntimeError::Io)?;
        Ok(())
    }

    /// The built-in main loop: executes requests until the host asks for shutdown.
    pub fn run(&mut self) {
        loop {
            if self.run_pending() == LoopState::Exit {
                return;
            }
            let guard = self.shared.tasks.lock().unwrap();
            if guard.is_empty() && !self.shared.idle_requested.load(Ordering::Acquire) {
                let timeout = self.config.idle_interval.unwrap_or(Duration::from_millis(100));
                let _ = self.shared.cond.wait_timeout(guard, timeout).unwrap();
            }
        }
    }

    /// Executes everything that is pending, without blocking. Main thread only.
    pub fn run_pending(&mut self) -> LoopState {
        loop {
            let task = self.shared.tasks.lock().unwrap().pop_front();
            let Some(task) = task else { break };
            match task {
                Task::Request { id, request, fds } => {
                    let response = self.handle(request, fds);
                    self.shared.respond(id, &response);
                }
                Task::BadRequest { id, message } => {
                    self.shared.respond(id, &Response::Error { code: ERR_PROTOCOL, message });
                }
            }
            if self.exit {
                return LoopState::Exit;
            }
        }
        let periodic = self.config.idle_interval.is_some_and(|interval| self.last_idle.elapsed() >= interval);
        if periodic || self.shared.idle_requested.swap(false, Ordering::AcqRel) {
            self.last_idle = Instant::now();
            for slot in self.slots.values_mut() {
                if slot.link.0.idle_requested.swap(false, Ordering::AcqRel) || periodic {
                    slot.plugin.idle();
                }
            }
        }
        LoopState::Running
    }

    /// Sends a log line to the host (it surfaces as a host event).
    pub fn log(&self, message: impl Into<String>) {
        let notice = ContainerNotice::Log { level: 0, message: message.into() };
        self.shared.send(FrameKind::Notification, 0, &notice.encode());
    }

    // ---- request handling (main thread) -------------------------------------------------

    fn handle(&mut self, request: Request, fds: Vec<OwnedFd>) -> Response {
        let result = match request {
            Request::Ping {} | Request::PingMain {} => Ok(Response::Ok {}),
            Request::Shutdown {} => {
                let ids: Vec<u64> = self.slots.keys().copied().collect();
                for id in ids {
                    let _ = self.destroy(id);
                }
                self.exit = true;
                Ok(Response::Ok {})
            }
            Request::CreateInstance { instance, params } => self.create(instance, params, fds),
            Request::DestroyInstance { instance } => self.destroy(instance),
            Request::Activate { instance } => self.with_slot(instance, |slot, _| {
                activate(slot)?;
                Ok(Response::Ok {})
            }),
            Request::Deactivate { instance } => self.with_slot(instance, |slot, _| {
                deactivate(slot)?;
                Ok(Response::Ok {})
            }),
            Request::Reset { instance } => self.with_slot(instance, |slot, _| {
                let guard = Exclusive::lock(&slot.rt)?;
                if let Some(processor) = guard.processor() {
                    processor.reset();
                }
                Ok(Response::Ok {})
            }),
            Request::SetOffline { instance, offline } => self.with_slot(instance, |slot, _| {
                let _guard = Exclusive::lock(&slot.rt)?;
                slot.plugin.set_offline(offline);
                Ok(Response::Ok {})
            }),
            Request::SetBypass { instance, bypass } => self.with_slot(instance, |slot, _| {
                slot.plugin.set_bypass(bypass).map_err(plugin_error)?;
                Ok(Response::Ok {})
            }),
            Request::Reconfigure { instance, sample_rate, max_frames } => {
                let config = self.config.clone();
                self.with_slot(instance, move |slot, _| reconfigure(slot, sample_rate, max_frames, fds, &config))
            }
            Request::GetInfo { instance } => {
                self.with_slot(instance, |slot, _| Ok(Response::Info { info: slot.plugin.info() }))
            }
            Request::GetParameters { instance } => {
                self.with_slot(instance, |slot, _| Ok(Response::Parameters { list: slot.plugin.parameters() }))
            }
            Request::GetParameterValues { instance } => self.with_slot(instance, |slot, _| {
                let ids: Vec<u64> = slot.plugin.parameters().iter().map(|p| p.id).collect();
                let values = ids
                    .into_iter()
                    .filter_map(|id| slot.plugin.parameter_value(id).map(|value| ParameterValue { id, value }))
                    .collect();
                Ok(Response::Values { values })
            }),
            Request::GetParameter { instance, id } => {
                self.with_slot(instance, |slot, _| Ok(Response::Value { value: slot.plugin.parameter_value(id) }))
            }
            Request::SetParameter { instance, id, value } => self.with_slot(instance, |slot, _| {
                slot.plugin.set_parameter(id, value).map_err(plugin_error)?;
                Ok(Response::Ok {})
            }),
            Request::FormatParameter { instance, id, values } => self.with_slot(instance, |slot, _| {
                let strings =
                    values.into_iter().map(|v| slot.plugin.format_parameter(id, v).unwrap_or_default()).collect();
                Ok(Response::Strings { strings })
            }),
            Request::ParseParameter { instance, id, text } => self
                .with_slot(instance, |slot, _| Ok(Response::Value { value: slot.plugin.parse_parameter(id, &text) })),
            Request::GetState { instance } => self.with_slot(instance, |slot, _| {
                Ok(Response::State { data: Bytes(slot.plugin.state().map_err(plugin_error)?) })
            }),
            Request::SetState { instance, data } => self.with_slot(instance, |slot, _| {
                slot.plugin.set_state(&data.0).map_err(plugin_error)?;
                Ok(Response::Ok {})
            }),
            Request::GetPresets { instance } => {
                self.with_slot(instance, |slot, _| Ok(Response::Presets { list: slot.plugin.presets() }))
            }
            Request::LoadPreset { instance, id } => self.with_slot(instance, |slot, _| {
                slot.plugin.load_preset(id).map_err(plugin_error)?;
                Ok(Response::Ok {})
            }),
            Request::OpenEditor { instance, title } => self.with_slot(instance, |slot, _| {
                let info = slot.plugin.open_editor(&title).map_err(plugin_error)?;
                slot.editor_open = true;
                Ok(Response::Editor { info })
            }),
            Request::CloseEditor { instance } => self.with_slot(instance, |slot, _| {
                if slot.editor_open {
                    slot.plugin.close_editor();
                    slot.editor_open = false;
                }
                Ok(Response::Ok {})
            }),
            Request::FocusEditor { instance } => self.with_slot(instance, |slot, _| {
                slot.plugin.focus_editor();
                Ok(Response::Ok {})
            }),
            Request::SetEditorTitle { instance, title } => self.with_slot(instance, |slot, _| {
                slot.plugin.set_editor_title(&title);
                Ok(Response::Ok {})
            }),
        };
        result.unwrap_or_else(|(code, message)| Response::Error { code, message })
    }

    fn with_slot(&mut self, instance: u64, f: impl FnOnce(&mut Slot, &Arc<Shared>) -> Handled) -> Handled {
        match self.slots.get_mut(&instance) {
            Some(slot) => f(slot, &self.shared),
            None => Err((ERR_UNKNOWN_INSTANCE, format!("no instance {instance}"))),
        }
    }

    fn create(&mut self, instance: u64, params: CreateParams, fds: Vec<OwnedFd>) -> Handled {
        if self.slots.contains_key(&instance) {
            return Err((ERR_PROTOCOL, format!("instance {instance} already exists")));
        }
        let backend = self.backends.get(&params.format).ok_or_else(|| {
            (ERR_UNKNOWN_FORMAT, format!("this container has no backend for the format \"{}\"", params.format))
        })?;
        let link = HostHandle(Arc::new(HostLink {
            instance,
            current: AtomicPtr::new(std::ptr::null_mut()),
            keep: Mutex::new(Vec::new()),
            idle_requested: AtomicBool::new(false),
            shared: self.shared.clone(),
        }));
        // Map the region first, so the plug-in can notify from inside `create`.
        let rt = build_rt(&params, params.sample_rate, params.max_frames, fds, &self.config)?;
        link.set_region(&rt.region);
        let request = CreateRequest {
            plugin_id: &params.plugin_id,
            name: &params.name,
            options: &params.options,
            sample_rate: params.sample_rate,
            max_frames: params.max_frames,
            input_buses: &params.in_buses,
            output_buses: &params.out_buses,
        };
        let mut plugin = match backend.create(&request, link.clone()) {
            Ok(plugin) => plugin,
            Err(err) => {
                link.close();
                return Err(plugin_error(err));
            }
        };
        let info = plugin.info();
        let thread = spawn_rt_thread(instance, &rt)?;
        self.slots.insert(
            instance,
            Slot { plugin, link, rt, thread: Some(thread), params, active: false, editor_open: false },
        );
        Ok(Response::Info { info })
    }

    fn destroy(&mut self, instance: u64) -> Handled {
        let Some(mut slot) = self.slots.remove(&instance) else {
            return Err((ERR_UNKNOWN_INSTANCE, format!("no instance {instance}")));
        };
        if slot.editor_open {
            slot.plugin.close_editor();
        }
        slot.link.close();
        if !stop_rt_thread(&mut slot) {
            // The real-time thread is stuck inside the plug-in. Dismantling the plug-in
            // under it would be undefined behaviour: leak it. The host kills hung containers.
            std::mem::forget(slot);
            return Err((ERR_INVALID_STATE, "the instance is hung in its process call".into()));
        }
        // SAFETY: the real-time thread has exited; nobody else touches the processor.
        if let Some(processor) = unsafe { (*slot.rt.processor.get()).take() } {
            slot.plugin.deactivate(processor);
        }
        drop(slot);
        Ok(Response::Ok {})
    }
}

type Handled = Result<Response, (u32, String)>;

fn plugin_error(err: PluginError) -> (u32, String) {
    match err {
        PluginError::Unsupported => (ERR_UNSUPPORTED, err.to_string()),
        PluginError::Failed(message) => (ERR_PLUGIN, message),
    }
}

/// Main-thread hold on an instance's gate: the real-time thread answers "busy" meanwhile.
struct Exclusive<'a> {
    rt: &'a RtShared,
}

impl<'a> Exclusive<'a> {
    fn lock(rt: &'a RtShared) -> Result<Exclusive<'a>, (u32, String)> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if rt.gate.compare_exchange(GATE_IDLE, GATE_EXCLUSIVE, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                return Ok(Exclusive { rt });
            }
            if Instant::now() >= deadline {
                return Err((ERR_INVALID_STATE, "the instance did not leave its process call".into()));
            }
            std::thread::sleep(Duration::from_micros(50));
        }
    }

    #[allow(clippy::mut_from_ref)]
    fn processor(&self) -> Option<&mut Box<dyn Processor>> {
        // SAFETY: this guard holds the gate exclusively.
        unsafe { (*self.rt.processor.get()).as_mut() }
    }

    fn slot(&self) -> &mut Option<Box<dyn Processor>> {
        // SAFETY: this guard holds the gate exclusively.
        unsafe { &mut *self.rt.processor.get() }
    }
}

impl Drop for Exclusive<'_> {
    fn drop(&mut self) {
        self.rt.gate.store(GATE_IDLE, Ordering::Release);
    }
}

fn activate(slot: &mut Slot) -> Result<(), (u32, String)> {
    if slot.active {
        return Ok(());
    }
    let guard = Exclusive::lock(&slot.rt)?;
    let processor = slot.plugin.activate(slot.params.sample_rate, slot.params.max_frames).map_err(plugin_error)?;
    *guard.slot() = Some(processor);
    slot.active = true;
    Ok(())
}

fn deactivate(slot: &mut Slot) -> Result<(), (u32, String)> {
    if !slot.active {
        return Ok(());
    }
    let guard = Exclusive::lock(&slot.rt)?;
    if let Some(processor) = guard.slot().take() {
        slot.plugin.deactivate(processor);
    }
    slot.active = false;
    Ok(())
}

fn build_rt(
    params: &CreateParams,
    sample_rate: f64,
    max_frames: u32,
    mut fds: Vec<OwnedFd>,
    config: &RuntimeConfig,
) -> Result<Arc<RtShared>, (u32, String)> {
    let protocol = |message: String| (ERR_PROTOCOL, message);
    let mode = WakeMode::from_u32(params.wake_mode).ok_or_else(|| protocol("unknown wake mode".into()))?;
    let expected = if mode == WakeMode::Pipe { 3 } else { 1 };
    if fds.len() != expected {
        return Err(protocol(format!("{} descriptors attached, {expected} expected", fds.len())));
    }
    if mode == WakeMode::Address && !wake::address::supported() {
        return Err(protocol("address waits are not available in this process".into()));
    }
    let region_config = RegionConfig {
        max_frames,
        in_buses: params.in_buses.clone(),
        out_buses: params.out_buses.clone(),
        event_capacity: params.event_capacity,
        out_event_capacity: params.out_event_capacity,
        queue_capacity: params.queue_capacity,
    };
    let region = Region::attach(fds[0].as_fd(), &region_config).map_err(|e| protocol(e.to_string()))?;
    let (waiter, notifier) = match mode {
        WakeMode::Address => (Waiter::address(), Notifier::address()),
        WakeMode::Pipe => {
            let rsp_write = fds.pop().unwrap();
            let req_read = fds.pop().unwrap();
            for fd in [&req_read, &rsp_write] {
                wake::set_nonblocking_cloexec(std::os::fd::AsRawFd::as_raw_fd(fd))
                    .map_err(|e| protocol(e.to_string()))?;
            }
            (Waiter::pipe(req_read), Notifier::pipe(rsp_write))
        }
    };
    Ok(Arc::new(RtShared {
        region: Arc::new(region),
        waiter,
        notifier,
        gate: AtomicU32::new(GATE_IDLE),
        stop: AtomicBool::new(false),
        exited: AtomicBool::new(false),
        processor: UnsafeCell::new(None),
        in_buses: params.in_buses.clone(),
        out_buses: params.out_buses.clone(),
        period_ns: (max_frames.min(1024) as f64 / sample_rate.max(1.0) * 1e9) as u64,
        realtime_priority: config.realtime_priority,
    }))
}

fn spawn_rt_thread(instance: u64, rt: &Arc<RtShared>) -> Result<JoinHandle<()>, (u32, String)> {
    let rt = rt.clone();
    std::thread::Builder::new()
        .name(format!("stpc-rt-{instance}"))
        .spawn(move || rt_thread(rt))
        .map_err(|e| (ERR_PLUGIN, format!("cannot start the real-time thread: {e}")))
}

/// Stops and joins the real-time thread. False if it is stuck inside the plug-in.
fn stop_rt_thread(slot: &mut Slot) -> bool {
    let Some(thread) = slot.thread.take() else { return true };
    slot.rt.stop.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !slot.rt.exited.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            return false;
        }
        if slot.rt.waiter.mode() == WakeMode::Address {
            wake::address::wake(&slot.rt.region.header().req.seq);
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    let _ = thread.join();
    true
}

fn reconfigure(slot: &mut Slot, sample_rate: f64, max_frames: u32, fds: Vec<OwnedFd>, config: &RuntimeConfig) -> Handled {
    let new_rt = build_rt(&slot.params, sample_rate, max_frames, fds, config)?;
    let was_active = slot.active;
    if !stop_rt_thread(slot) {
        return Err((ERR_INVALID_STATE, "the instance is hung in its process call".into()));
    }
    // SAFETY: the real-time thread has exited; nobody else touches the processor.
    if let Some(processor) = unsafe { (*slot.rt.processor.get()).take() } {
        slot.plugin.deactivate(processor);
    }
    slot.active = false;
    slot.params.sample_rate = sample_rate;
    slot.params.max_frames = max_frames;
    slot.link.set_region(&new_rt.region);
    slot.rt = new_rt;
    slot.thread = Some(spawn_rt_thread(slot.link.instance_id(), &slot.rt)?);
    if was_active {
        activate(slot)?;
    }
    Ok(Response::Info { info: slot.plugin.info() })
}

// ---- I/O thread ---------------------------------------------------------------------------

fn io_thread(reader: Stream, shared: Arc<Shared>) {
    loop {
        let Frame { kind, id, payload, fds } = match reader.recv() {
            Ok(frame) => frame,
            Err(_) => {
                // The host closed the channel or died. Nothing here is worth saving, and
                // plug-in destructors may hang: leave at once.
                // SAFETY: _exit is always safe to call.
                unsafe { libc::_exit(0) }
            }
        };
        if kind != FrameKind::Request {
            continue;
        }
        let task = match Request::decode(&payload) {
            Ok(Request::Ping {}) => {
                shared.respond(id, &Response::Ok {});
                continue;
            }
            Ok(request) => Task::Request { id, request, fds },
            Err(err) => Task::BadRequest { id, message: err.to_string() },
        };
        shared.tasks.lock().unwrap().push_back(task);
        shared.wake_main();
    }
}

// ---- real-time thread ---------------------------------------------------------------------

fn spans(buses: &[u32]) -> Vec<BusSpan> {
    let mut first = 0;
    buses
        .iter()
        .map(|&channels| {
            let span = BusSpan { first, channels: channels as usize };
            first += channels as usize;
            span
        })
        .collect()
}

fn rt_thread(rt: Arc<RtShared>) {
    if rt.realtime_priority {
        crate::rt::promote_current_thread_to_realtime(rt.period_ns);
    }
    crate::rt::enable_denormal_flushing();
    let region = &*rt.region;
    let header = region.header();
    let layout = region.layout().clone();
    let in_spans = spans(&rt.in_buses);
    let out_spans = spans(&rt.out_buses);
    let in_ptrs: Vec<*const f32> = (0..layout.in_channels).map(|c| region.input(c) as *const f32).collect();
    let out_ptrs: Vec<*mut f32> = (0..layout.out_channels).map(|c| region.output(c)).collect();
    // Pipe mode has no way to be woken for `stop` other than the timeout.
    let idle_wait_ns = if rt.waiter.mode() == WakeMode::Pipe { 20_000_000 } else { 500_000_000 };

    // Start from the last completed block: a request published before this thread started
    // is still served.
    let mut last = header.rsp.seq.load(Ordering::SeqCst);
    loop {
        let seq = loop {
            if rt.stop.load(Ordering::Acquire) {
                rt.exited.store(true, Ordering::Release);
                return;
            }
            let seq = header.req.seq.load(Ordering::SeqCst);
            if seq != last {
                break seq;
            }
            if rt.waiter.wait(&header.req.seq, last, Some(idle_wait_ns)) == WaitOutcome::PeerGone {
                // The host closed its end: the I/O thread is about to end the process.
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        last = seq;

        let mut flags;
        if rt.gate.compare_exchange(GATE_IDLE, GATE_PROCESSING, Ordering::AcqRel, Ordering::Relaxed).is_err() {
            flags = RSP_BUSY;
        } else {
            // SAFETY: this thread holds the gate.
            match unsafe { (*rt.processor.get()).as_mut() } {
                None => flags = RSP_INACTIVE,
                Some(processor) => {
                    let frames = (header.req.frames.load(Ordering::Relaxed) as usize).min(layout.max_frames);
                    let num_events = (header.req.num_events.load(Ordering::Relaxed) as usize).min(layout.event_capacity);
                    let req_flags = header.req.flags.load(Ordering::Relaxed);
                    for &ptr in &out_ptrs {
                        // SAFETY: each output channel holds max_frames >= frames samples.
                        unsafe { std::ptr::write_bytes(ptr, 0, frames) };
                    }
                    // SAFETY: the request block and the event list belong to this side
                    // until the response is published below.
                    let (transport, events) = unsafe {
                        (header.req.transport(), std::slice::from_raw_parts(region.events() as *const Event, num_events))
                    };
                    let mut data = ProcessData {
                        frames,
                        offline: req_flags & REQ_OFFLINE != 0,
                        discontinuity: req_flags & REQ_DISCONTINUITY != 0,
                        transport,
                        events,
                        input_silence: header.req.input_silence.load(Ordering::Relaxed),
                        output_silence: 0,
                        in_spans: &in_spans,
                        out_spans: &out_spans,
                        in_ptrs: &in_ptrs,
                        out_ptrs: &out_ptrs,
                        out_events: region.out_events(),
                        out_event_capacity: layout.out_event_capacity,
                        out_event_count: 0,
                        out_events_dropped: false,
                    };
                    let started = monotonic_ns();
                    let status = processor.process(&mut data);
                    header.rsp.process_ns.store(monotonic_ns() - started, Ordering::Relaxed);
                    flags = RSP_PROCESSED;
                    if data.out_events_dropped {
                        flags |= RSP_OUT_EVENTS_DROPPED;
                    }
                    let mut silence = data.output_silence;
                    let mut out_count = data.out_event_count;
                    if status == ProcessStatus::Failed {
                        flags |= RSP_ERROR;
                        for &ptr in &out_ptrs {
                            // SAFETY: as above.
                            unsafe { std::ptr::write_bytes(ptr, 0, frames) };
                        }
                        silence = u64::MAX;
                        out_count = 0;
                    }
                    header.rsp.output_silence.store(silence, Ordering::Relaxed);
                    header.rsp.num_out_events.store(out_count as u32, Ordering::Relaxed);
                }
            }
            rt.gate.store(GATE_IDLE, Ordering::Release);
        }
        header.rsp.flags.store(flags, Ordering::Relaxed);
        header.rsp.seq.store(seq, Ordering::SeqCst);
        rt.notifier.notify(&header.rsp.seq);
    }
}

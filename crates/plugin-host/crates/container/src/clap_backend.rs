//! Format `"clap"`: hosts CLAP plug-ins (<https://github.com/free-audio/clap>).
//!
//! Instance configuration on the host side: `format = "clap"`, `plugin_id` = the CLAP
//! plug-in id (e.g. `com.vendor.synth`), `options` = path of the `.clap` file (on macOS
//! the bundle directory).
//!
//! What is implemented: plug-in life cycle with the CLAP threading rules, 32-bit audio on
//! all ports (ports the host did not allot get scratch buffers), note and MIDI input
//! (see below), sample-accurate parameter events, transport, output events (MIDI, parameter changes
//! and gestures made in the plug-in), `clap.params`, `clap.state`, `clap.latency`,
//! `clap.tail`, `clap.audio-ports`, `clap.note-ports`, bypass through the parameter
//! flagged `CLAP_PARAM_IS_BYPASS`, `request_callback` / `request_restart` /
//! `params.request_flush`, `clap.thread-check`, `clap.log`, and **floating** editor
//! windows through `clap.gui`. Not implemented: embedded editors (the container would
//! have to create the parent window), 64-bit audio, sysex, MIDI 2, note expressions other
//! than those below, polyphonic modulation, timers and fd support.
//!
//! # Notes, controllers and pressure
//!
//! A host event is MIDI bytes; what the plug-in gets depends on the dialects of its note
//! port (`clap.note-ports`):
//!
//! | Message | Port takes the MIDI dialect | Port takes the CLAP dialect only |
//! |---|---|---|
//! | note on / off | CLAP note events when the port takes them too, else MIDI | CLAP note events |
//! | control change, program change | MIDI | dropped (CLAP has no event for them) |
//! | channel pressure | MIDI | note expression *pressure* for every key of the channel (`key` -1) |
//! | poly pressure | MIDI | note expression *pressure* for that key |
//! | pitch bend | MIDI | note expression *tuning* for every key of the channel, two semitones either way |
//!
//! Verified by running on Linux against `stpc-clap-test-plugin`. The macOS specifics
//! (bundle layout, the `cocoa` window API name) are type-checked only.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::gui::*;
use clap_sys::ext::latency::*;
use clap_sys::ext::log::*;
use clap_sys::ext::note_ports::*;
use clap_sys::ext::params::*;
use clap_sys::ext::state::*;
use clap_sys::ext::tail::*;
use clap_sys::ext::thread_check::*;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR;
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::*;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{clap_version_is_compatible, CLAP_VERSION};

use stpc_runtime::*;

/// A loaded `.clap` library. Never unloaded: plug-ins tend to leave threads and static
/// state behind, and the container process is short-lived by design.
struct Library {
    factory: *const clap_plugin_factory,
}

#[derive(Default)]
pub struct ClapBackend {
    libraries: RefCell<HashMap<PathBuf, Library>>,
}

impl ClapBackend {
    pub fn new() -> ClapBackend {
        ClapBackend::default()
    }

    fn library(&self, path: &Path) -> Result<*const clap_plugin_factory, PluginError> {
        if let Some(library) = self.libraries.borrow().get(path) {
            return Ok(library.factory);
        }
        // macOS: a .clap is a bundle; the code is in Contents/MacOS/<name>. (By reading.)
        let binary = if path.is_dir() {
            let stem = path.file_stem().ok_or_else(|| PluginError::failed("bad CLAP bundle path"))?;
            path.join("Contents").join("MacOS").join(stem)
        } else {
            path.to_path_buf()
        };
        let c_binary =
            CString::new(binary.to_string_lossy().as_bytes()).map_err(|_| PluginError::failed("bad path"))?;
        let c_path = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| PluginError::failed("bad path"))?;
        // SAFETY: loading a plug-in runs its initialisers; that is what this process is for.
        let factory = unsafe {
            let handle = libc::dlopen(c_binary.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if handle.is_null() {
                let reason = libc::dlerror();
                let reason =
                    if reason.is_null() { "unknown error".into() } else { CStr::from_ptr(reason).to_string_lossy() };
                return Err(PluginError::failed(format!("cannot load {}: {reason}", binary.display())));
            }
            let entry = libc::dlsym(handle, c"clap_entry".as_ptr()) as *const clap_plugin_entry;
            if entry.is_null() {
                return Err(PluginError::failed(format!("{} is not a CLAP plug-in (no clap_entry)", binary.display())));
            }
            if !clap_version_is_compatible((*entry).clap_version) {
                return Err(PluginError::failed("incompatible CLAP version"));
            }
            let (Some(init), Some(get_factory)) = ((*entry).init, (*entry).get_factory) else {
                return Err(PluginError::failed("incomplete clap_entry"));
            };
            if !init(c_path.as_ptr()) {
                return Err(PluginError::failed("the CLAP library failed to initialise"));
            }
            let factory = get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
            if factory.is_null() {
                return Err(PluginError::failed("the CLAP library has no plug-in factory"));
            }
            factory
        };
        self.libraries.borrow_mut().insert(path.to_path_buf(), Library { factory });
        Ok(factory)
    }
}

thread_local! {
    /// Set while this thread is inside a plug-in's audio-thread function.
    static ON_AUDIO_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// What the plug-in's `clap_host` points to. Boxed, so its address is stable.
#[repr(C)]
struct HostData {
    host: clap_host,
    handle: HostHandle,
    plugin: Cell<*const clap_plugin>,
    main_thread: std::thread::ThreadId,
    callback_requested: AtomicBool,
    flush_requested: AtomicBool,
    restart_requested: AtomicBool,
    latency_changed: AtomicBool,
}

unsafe fn host_data<'a>(host: *const clap_host) -> &'a HostData {
    &*((*host).host_data as *const HostData)
}

unsafe extern "C" fn host_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_THREAD_CHECK {
        &HOST_THREAD_CHECK as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &HOST_PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &HOST_STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &HOST_LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_TAIL {
        &HOST_TAIL as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI {
        &HOST_GUI as *const _ as *const c_void
    } else if id == CLAP_EXT_LOG {
        &HOST_LOG as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    let data = host_data(host);
    if !data.restart_requested.swap(true, Ordering::AcqRel) {
        data.handle.notify(Notification::RestartRequested);
    }
}

unsafe extern "C" fn host_request_process(_host: *const clap_host) {
    // The host processes every block it renders; there is nothing to wake.
}

unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    let data = host_data(host);
    data.callback_requested.store(true, Ordering::Release);
    data.handle.request_main_callback();
}

unsafe extern "C" fn host_is_main_thread(host: *const clap_host) -> bool {
    std::thread::current().id() == host_data(host).main_thread
}
unsafe extern "C" fn host_is_audio_thread(_host: *const clap_host) -> bool {
    ON_AUDIO_THREAD.with(|flag| flag.get())
}
static HOST_THREAD_CHECK: clap_host_thread_check =
    clap_host_thread_check { is_main_thread: Some(host_is_main_thread), is_audio_thread: Some(host_is_audio_thread) };

unsafe extern "C" fn host_params_rescan(host: *const clap_host, _flags: clap_param_rescan_flags) {
    host_data(host).handle.notify(Notification::ParamsRescan);
}
unsafe extern "C" fn host_params_clear(_host: *const clap_host, _id: u32, _flags: clap_param_clear_flags) {}
unsafe extern "C" fn host_params_request_flush(host: *const clap_host) {
    let data = host_data(host);
    data.flush_requested.store(true, Ordering::Release);
    data.handle.request_main_callback();
}
static HOST_PARAMS: clap_host_params = clap_host_params {
    rescan: Some(host_params_rescan),
    clear: Some(host_params_clear),
    request_flush: Some(host_params_request_flush),
};

unsafe extern "C" fn host_state_mark_dirty(host: *const clap_host) {
    host_data(host).handle.notify(Notification::StateDirty);
}
static HOST_STATE: clap_host_state = clap_host_state { mark_dirty: Some(host_state_mark_dirty) };

unsafe extern "C" fn host_latency_changed(host: *const clap_host) {
    // Called from within activate(): the new value is read after activation completes.
    let data = host_data(host);
    data.latency_changed.store(true, Ordering::Release);
    data.handle.request_main_callback();
}
static HOST_LATENCY: clap_host_latency = clap_host_latency { changed: Some(host_latency_changed) };

unsafe extern "C" fn host_tail_changed(host: *const clap_host) {
    let data = host_data(host);
    let plugin = data.plugin.get();
    if plugin.is_null() {
        return;
    }
    let tail: *const clap_plugin_tail = extension(plugin, CLAP_EXT_TAIL);
    if let Some(get) = tail.as_ref().and_then(|tail| tail.get) {
        data.handle.notify(Notification::TailChanged { frames: tail_frames(get(plugin)) });
    }
}
static HOST_TAIL: clap_host_tail = clap_host_tail { changed: Some(host_tail_changed) };

unsafe extern "C" fn host_gui_resize_hints_changed(_host: *const clap_host) {}
unsafe extern "C" fn host_gui_request_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    // Floating windows belong to the plug-in: it may resize itself.
    host_data(host).handle.notify(Notification::EditorResized { width, height });
    true
}
unsafe extern "C" fn host_gui_request_show(_host: *const clap_host) -> bool {
    false
}
unsafe extern "C" fn host_gui_request_hide(_host: *const clap_host) -> bool {
    false
}
unsafe extern "C" fn host_gui_closed(host: *const clap_host, _was_destroyed: bool) {
    host_data(host).handle.notify(Notification::EditorClosed);
}
static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(host_gui_resize_hints_changed),
    request_resize: Some(host_gui_request_resize),
    request_show: Some(host_gui_request_show),
    request_hide: Some(host_gui_request_hide),
    closed: Some(host_gui_closed),
};

unsafe extern "C" fn host_log(_host: *const clap_host, severity: clap_log_severity, message: *const c_char) {
    if severity >= CLAP_LOG_WARNING && !message.is_null() {
        eprintln!("[clap plug-in] {}", CStr::from_ptr(message).to_string_lossy());
    }
}
static HOST_LOG: clap_host_log = clap_host_log { log: Some(host_log) };

unsafe fn extension<T>(plugin: *const clap_plugin, id: &CStr) -> *const T {
    match (*plugin).get_extension {
        Some(get_extension) => get_extension(plugin, id.as_ptr()) as *const T,
        None => std::ptr::null(),
    }
}

fn tail_frames(tail: u32) -> u64 {
    if tail == u32::MAX {
        u64::MAX
    } else {
        tail as u64
    }
}

unsafe fn string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

fn array_string(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(target_os = "macos")]
const WINDOW_API: &CStr = CLAP_WINDOW_API_COCOA;
#[cfg(not(target_os = "macos"))]
const WINDOW_API: &CStr = CLAP_WINDOW_API_X11;

impl PluginBackend for ClapBackend {
    fn format(&self) -> &str {
        "clap"
    }

    fn create(&self, request: &CreateRequest<'_>, handle: HostHandle) -> Result<Box<dyn Plugin>, PluginError> {
        if request.options.is_empty() {
            return Err(PluginError::failed("the CLAP backend needs the path of the .clap file in `options`"));
        }
        let factory = self.library(Path::new(request.options))?;
        let id = CString::new(request.plugin_id).map_err(|_| PluginError::failed("bad plug-in id"))?;
        let host = Box::new(HostData {
            host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: c"SuperTracker".as_ptr(),
                vendor: c"SuperTracker".as_ptr(),
                url: c"".as_ptr(),
                version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
                get_extension: Some(host_get_extension),
                request_restart: Some(host_request_restart),
                request_process: Some(host_request_process),
                request_callback: Some(host_request_callback),
            },
            handle,
            plugin: Cell::new(std::ptr::null()),
            main_thread: std::thread::current().id(),
            callback_requested: AtomicBool::new(false),
            flush_requested: AtomicBool::new(false),
            restart_requested: AtomicBool::new(false),
            latency_changed: AtomicBool::new(false),
        });
        let host = Box::into_raw(host);
        // SAFETY: `host` stays allocated until `ClapPlugin::drop` (or the error paths
        // below) frees it, after the plug-in is destroyed. All plug-in calls follow the
        // CLAP threading rules: this is the main thread.
        unsafe {
            (*host).host.host_data = host.cast();
            let fail = |message: String| {
                drop(Box::from_raw(host));
                Err(PluginError::failed(message))
            };
            let Some(create_plugin) = (*factory).create_plugin else { return fail("incomplete CLAP factory".into()) };
            let plugin = create_plugin(factory, &(*host).host, id.as_ptr());
            if plugin.is_null() {
                return fail(format!("{} has no plug-in \"{}\"", request.options, request.plugin_id));
            }
            (*host).plugin.set(plugin);
            if !(*plugin).init.is_some_and(|init| init(plugin)) {
                if let Some(destroy) = (*plugin).destroy {
                    destroy(plugin);
                }
                return fail("the CLAP plug-in failed to initialise".into());
            }
            let mut this = ClapPlugin {
                host,
                plugin,
                params: extension(plugin, CLAP_EXT_PARAMS),
                state: extension(plugin, CLAP_EXT_STATE),
                latency: extension(plugin, CLAP_EXT_LATENCY),
                tail: extension(plugin, CLAP_EXT_TAIL),
                gui: extension(plugin, CLAP_EXT_GUI),
                in_ports: Vec::new(),
                out_ports: Vec::new(),
                note_dialects: 0,
                note_outputs: false,
                host_in: request.input_buses.to_vec(),
                host_out: request.output_buses.to_vec(),
                event_capacity: request.event_capacity as usize,
                queue: Arc::new(ParamQueue::new()),
                active: false,
                editor_created: false,
                bypass_param: None,
            };
            this.in_ports = this.audio_ports(true);
            this.out_ports = this.audio_ports(false);
            let notes: *const clap_plugin_note_ports = extension(plugin, CLAP_EXT_NOTE_PORTS);
            if let Some(notes) = notes.as_ref() {
                if let (Some(count), Some(get)) = (notes.count, notes.get) {
                    if count(plugin, true) > 0 {
                        let mut info: clap_note_port_info = std::mem::zeroed();
                        if get(plugin, 0, true, &mut info) {
                            this.note_dialects = info.supported_dialects;
                        }
                    }
                    this.note_outputs = count(plugin, false) > 0;
                }
            }
            this.bypass_param =
                this.raw_parameters().into_iter().find(|p| p.flags & CLAP_PARAM_IS_BYPASS != 0).map(|p| p.id);
            Ok(Box::new(this))
        }
    }
}

#[derive(Clone)]
struct Port {
    name: String,
    channels: u32,
    main: bool,
}

/// Parameter changes from the main thread on their way to the audio thread (single
/// producer, single consumer, lock-free).
struct ParamQueue {
    slots: Vec<(AtomicU64, AtomicU64)>,
    head: AtomicU32,
    tail: AtomicU32,
}

impl ParamQueue {
    const CAPACITY: u32 = 1024;

    fn new() -> ParamQueue {
        ParamQueue {
            slots: (0..Self::CAPACITY).map(|_| (AtomicU64::new(0), AtomicU64::new(0))).collect(),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    fn push(&self, id: u64, value: f64) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail.wrapping_sub(self.head.load(Ordering::Acquire)) >= Self::CAPACITY {
            return false;
        }
        let slot = &self.slots[(tail % Self::CAPACITY) as usize];
        slot.0.store(id, Ordering::Relaxed);
        slot.1.store(value.to_bits(), Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<(u64, f64)> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        let slot = &self.slots[(head % Self::CAPACITY) as usize];
        let item = (slot.0.load(Ordering::Relaxed), f64::from_bits(slot.1.load(Ordering::Relaxed)));
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(item)
    }
}

/// Storage for one input event of any supported type; the header comes first in each.
#[repr(C)]
union EventSlot {
    header: clap_event_header,
    note: clap_event_note,
    expression: clap_event_note_expression,
    midi: clap_event_midi,
    param: clap_event_param_value,
}

/// Semitones a full pitch bend moves a plug-in that gets it as a tuning expression.
const BEND_RANGE: f64 = 2.0;

fn param_event(time: u32, id: u64, value: f64) -> EventSlot {
    EventSlot {
        param: clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: id as u32,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        },
    }
}

struct InputList {
    events: *const EventSlot,
    count: u32,
}

unsafe extern "C" fn input_size(list: *const clap_input_events) -> u32 {
    (*((*list).ctx as *const InputList)).count
}

unsafe extern "C" fn input_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let input = &*((*list).ctx as *const InputList);
    if index >= input.count {
        return std::ptr::null();
    }
    &(*input.events.add(index as usize)).header
}

/// Where events coming out of the plug-in go.
struct OutputSink {
    handle: *const HostHandle,
    /// Null outside `process` (parameter flush on the main thread): no MIDI out then.
    data: *mut c_void,
}

unsafe extern "C" fn output_try_push(list: *const clap_output_events, header: *const clap_event_header) -> bool {
    let sink = &*((*list).ctx as *const OutputSink);
    let handle = &*sink.handle;
    if (*header).space_id != CLAP_CORE_EVENT_SPACE_ID {
        return true;
    }
    let midi_out = |event: Event| {
        if !sink.data.is_null() {
            // SAFETY: `data` is the `ProcessData` of the block being processed.
            let data = &mut *(sink.data as *mut ProcessData<'_>);
            data.push_event(event);
        }
    };
    match (*header).type_ {
        CLAP_EVENT_PARAM_VALUE => {
            let event = &*(header as *const clap_event_param_value);
            handle.notify(Notification::ParamValue { id: event.param_id as u64, value: event.value });
        }
        CLAP_EVENT_PARAM_GESTURE_BEGIN => {
            let event = &*(header as *const clap_event_param_gesture);
            handle.notify(Notification::ParamGestureBegin { id: event.param_id as u64 });
        }
        CLAP_EVENT_PARAM_GESTURE_END => {
            let event = &*(header as *const clap_event_param_gesture);
            handle.notify(Notification::ParamGestureEnd { id: event.param_id as u64 });
        }
        CLAP_EVENT_MIDI => {
            let event = &*(header as *const clap_event_midi);
            let mut out = Event::midi((*header).time, &event.data);
            out.port = event.port_index as u8;
            midi_out(out);
        }
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
            let event = &*(header as *const clap_event_note);
            let on = (*header).type_ == CLAP_EVENT_NOTE_ON;
            let status = if on { 0x90 } else { 0x80 } | (event.channel.clamp(0, 15) as u8);
            let velocity = (event.velocity * 127.0).round().clamp(if on { 1.0 } else { 0.0 }, 127.0) as u8;
            let mut out = Event::midi((*header).time, &[status, event.key.clamp(0, 127) as u8, velocity]);
            out.port = event.port_index.max(0) as u8;
            out.aux = event.note_id as u32;
            midi_out(out);
        }
        _ => {}
    }
    true
}

struct ClapPlugin {
    host: *mut HostData,
    plugin: *const clap_plugin,
    params: *const clap_plugin_params,
    state: *const clap_plugin_state,
    latency: *const clap_plugin_latency,
    tail: *const clap_plugin_tail,
    gui: *const clap_plugin_gui,
    in_ports: Vec<Port>,
    out_ports: Vec<Port>,
    /// Dialects of the first note input port (0: the plug-in takes no notes).
    note_dialects: clap_note_dialect,
    note_outputs: bool,
    host_in: Vec<u32>,
    host_out: Vec<u32>,
    event_capacity: usize,
    queue: Arc<ParamQueue>,
    active: bool,
    editor_created: bool,
    bypass_param: Option<u32>,
}

impl ClapPlugin {
    fn host(&self) -> &HostData {
        // SAFETY: allocated in `create`, freed in `drop`.
        unsafe { &*self.host }
    }

    unsafe fn audio_ports(&self, input: bool) -> Vec<Port> {
        let ports: *const clap_plugin_audio_ports = extension(self.plugin, CLAP_EXT_AUDIO_PORTS);
        let Some(ports) = ports.as_ref() else { return Vec::new() };
        let (Some(count), Some(get)) = (ports.count, ports.get) else { return Vec::new() };
        (0..count(self.plugin, input))
            .filter_map(|index| {
                let mut info: clap_audio_port_info = std::mem::zeroed();
                get(self.plugin, index, input, &mut info).then(|| Port {
                    name: array_string(&info.name),
                    channels: info.channel_count,
                    main: info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0,
                })
            })
            .collect()
    }

    fn raw_parameters(&self) -> Vec<clap_param_info> {
        // SAFETY: main thread; `params` is null or the plug-in's params extension.
        unsafe {
            let Some(params) = self.params.as_ref() else { return Vec::new() };
            let (Some(count), Some(get_info)) = (params.count, params.get_info) else { return Vec::new() };
            (0..count(self.plugin))
                .filter_map(|index| {
                    let mut info: clap_param_info = std::mem::zeroed();
                    get_info(self.plugin, index, &mut info).then_some(info)
                })
                .collect()
        }
    }

    /// Hands events to the plug-in outside of processing (`clap_plugin_params.flush`).
    fn flush(&mut self, events: &[EventSlot]) -> Result<(), PluginError> {
        // SAFETY: main thread, plug-in not active (so `process` cannot run concurrently).
        unsafe {
            let flush = self.params.as_ref().and_then(|params| params.flush).ok_or(PluginError::Unsupported)?;
            let list = InputList { events: events.as_ptr(), count: events.len() as u32 };
            let input = clap_input_events {
                ctx: &list as *const _ as *mut c_void,
                size: Some(input_size),
                get: Some(input_get),
            };
            let sink = OutputSink { handle: &self.host().handle, data: std::ptr::null_mut() };
            let output = clap_output_events { ctx: &sink as *const _ as *mut c_void, try_push: Some(output_try_push) };
            flush(self.plugin, &input, &output);
        }
        Ok(())
    }

    fn bus_infos(ports: &[Port], host: &[u32], input: bool) -> Vec<BusInfo> {
        host.iter()
            .enumerate()
            .map(|(index, &channels)| match ports.get(index) {
                Some(port) => {
                    BusInfo { name: port.name.clone(), channels: channels.min(port.channels), auxiliary: !port.main }
                }
                None => BusInfo {
                    name: if input { "Unused input".into() } else { "Unused output".into() },
                    channels: 0,
                    auxiliary: index > 0,
                },
            })
            .collect()
    }
}

impl Plugin for ClapPlugin {
    fn info(&mut self) -> InstanceInfo {
        // SAFETY: main thread; descriptor strings live as long as the plug-in.
        unsafe {
            let desc = &*(*self.plugin).desc;
            let mut features = Vec::new();
            let mut feature = desc.features;
            while !feature.is_null() && !(*feature).is_null() {
                features.push(string(*feature));
                feature = feature.add(1);
            }
            let mut flags = 0;
            if features.iter().any(|f| f == "instrument") {
                flags |= INFO_INSTRUMENT;
            }
            if self.note_dialects != 0 {
                flags |= INFO_WANTS_MIDI;
            }
            if self.note_outputs {
                flags |= INFO_PRODUCES_MIDI;
            }
            if self.bypass_param.is_some() {
                flags |= INFO_HAS_BYPASS;
            }
            let floating = self
                .gui
                .as_ref()
                .and_then(|gui| gui.is_api_supported)
                .is_some_and(|f| f(self.plugin, WINDOW_API.as_ptr(), true));
            if floating {
                flags |= INFO_HAS_EDITOR;
            }
            InstanceInfo {
                name: string(desc.name),
                vendor: string(desc.vendor),
                version: string(desc.version),
                flags,
                latency: self.latency.as_ref().and_then(|l| l.get).map_or(0, |get| get(self.plugin)),
                tail: self.tail.as_ref().and_then(|t| t.get).map_or(0, |get| tail_frames(get(self.plugin))),
                inputs: Self::bus_infos(&self.in_ports, &self.host_in, true),
                outputs: Self::bus_infos(&self.out_ports, &self.host_out, false),
                parameter_count: self.raw_parameters().len() as u32,
            }
        }
    }

    fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<Box<dyn Processor>, PluginError> {
        // SAFETY: main thread, plug-in inactive.
        unsafe {
            let activate = (*self.plugin).activate.ok_or_else(|| PluginError::failed("plug-in has no activate"))?;
            self.host().restart_requested.store(false, Ordering::Release);
            if !activate(self.plugin, sample_rate, 1, max_frames) {
                return Err(PluginError::failed("the CLAP plug-in refused to activate"));
            }
        }
        self.active = true;
        if self.host().latency_changed.swap(false, Ordering::AcqRel) {
            // SAFETY: main thread.
            let frames = unsafe { self.latency.as_ref().and_then(|l| l.get).map_or(0, |get| get(self.plugin)) };
            self.host().handle.notify(Notification::LatencyChanged { frames });
        }
        let frames = max_frames as usize;
        let port_buffers = |ports: &[Port]| -> Vec<PortBuffers> {
            ports
                .iter()
                .map(|port| PortBuffers {
                    pointers: vec![std::ptr::null_mut(); port.channels as usize],
                    scratch: vec![0.0; port.channels as usize * frames],
                })
                .collect()
        };
        let inputs = port_buffers(&self.in_ports);
        let outputs = port_buffers(&self.out_ports);
        let empty = clap_audio_buffer {
            data32: std::ptr::null_mut(),
            data64: std::ptr::null_mut(),
            channel_count: 0,
            latency: 0,
            constant_mask: 0,
        };
        Ok(Box::new(ClapProcessor {
            plugin: self.plugin,
            handle: self.host().handle.clone(),
            queue: self.queue.clone(),
            started: false,
            max_frames: frames,
            steady_time: 0,
            midi_dialect: self.note_dialects & CLAP_NOTE_DIALECT_MIDI != 0,
            note_dialect: self.note_dialects & CLAP_NOTE_DIALECT_CLAP != 0,
            events: Vec::with_capacity(self.event_capacity + ParamQueue::CAPACITY as usize),
            in_buffers: vec![empty; inputs.len()],
            out_buffers: vec![empty; outputs.len()],
            inputs,
            outputs,
        }))
    }

    fn deactivate(&mut self, processor: Box<dyn Processor>) {
        // Dropping the processor stops processing (the audio thread is excluded here).
        drop(processor);
        // SAFETY: main thread, processing stopped.
        unsafe {
            if let Some(deactivate) = (*self.plugin).deactivate {
                deactivate(self.plugin);
            }
        }
        self.active = false;
    }

    fn set_bypass(&mut self, bypass: bool) -> Result<(), PluginError> {
        let id = self.bypass_param.ok_or(PluginError::Unsupported)?;
        self.set_parameter(id as u64, if bypass { 1.0 } else { 0.0 })
    }

    fn parameters(&mut self) -> Vec<ParameterInfo> {
        let raw = self.raw_parameters();
        raw.iter()
            .map(|info| {
                let stepped = info.flags & CLAP_PARAM_IS_STEPPED != 0;
                let steps = if stepped { (info.max_value - info.min_value).round().max(0.0) as u32 } else { 0 };
                let mut flags = 0;
                for (clap, ours) in [
                    (CLAP_PARAM_IS_AUTOMATABLE, PARAM_AUTOMATABLE),
                    (CLAP_PARAM_IS_READONLY, PARAM_READ_ONLY),
                    (CLAP_PARAM_IS_HIDDEN, PARAM_HIDDEN),
                    (CLAP_PARAM_IS_STEPPED, PARAM_STEPPED),
                    (CLAP_PARAM_IS_BYPASS, PARAM_BYPASS),
                ] {
                    if info.flags & clap != 0 {
                        flags |= ours;
                    }
                }
                if stepped && steps == 1 {
                    flags |= PARAM_BOOLEAN;
                }
                let name = array_string(&info.name);
                // Enumerations: fetch the labels, as hosts show them in menus.
                let value_strings = if info.flags & CLAP_PARAM_IS_ENUM != 0 && steps <= 256 {
                    (0..=steps)
                        .map(|step| {
                            self.format_parameter(info.id as u64, info.min_value + step as f64).unwrap_or_default()
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                ParameterInfo {
                    id: info.id as u64,
                    short_name: name.chars().take(8).collect(),
                    name,
                    identifier: String::new(),
                    group: array_string(&info.module),
                    unit_label: String::new(),
                    flags,
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    step_count: steps,
                    value_strings,
                }
            })
            .collect()
    }

    fn parameter_value(&mut self, id: u64) -> Option<f64> {
        // SAFETY: main thread.
        unsafe {
            let get_value = self.params.as_ref()?.get_value?;
            let mut value = 0.0;
            get_value(self.plugin, id as u32, &mut value).then_some(value)
        }
    }

    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), PluginError> {
        if self.params.is_null() {
            return Err(PluginError::Unsupported);
        }
        if self.active {
            // CLAP delivers parameter changes through process(): queue it for the next block.
            if self.queue.push(id, value) {
                Ok(())
            } else {
                Err(PluginError::failed("too many pending parameter changes"))
            }
        } else {
            self.flush(&[param_event(0, id, value)])
        }
    }

    fn format_parameter(&mut self, id: u64, value: f64) -> Option<String> {
        // SAFETY: main thread; the buffer is NUL-terminated whatever the plug-in writes.
        unsafe {
            let value_to_text = self.params.as_ref()?.value_to_text?;
            let mut buffer = [0 as c_char; 256];
            if !value_to_text(self.plugin, id as u32, value, buffer.as_mut_ptr(), buffer.len() as u32 - 1) {
                return None;
            }
            Some(array_string(&buffer))
        }
    }

    fn parse_parameter(&mut self, id: u64, text: &str) -> Option<f64> {
        let text = CString::new(text).ok()?;
        // SAFETY: main thread.
        unsafe {
            let text_to_value = self.params.as_ref()?.text_to_value?;
            let mut value = 0.0;
            text_to_value(self.plugin, id as u32, text.as_ptr(), &mut value).then_some(value)
        }
    }

    fn state(&mut self) -> Result<Vec<u8>, PluginError> {
        unsafe extern "C" fn write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
            let out = &mut *((*stream).ctx as *mut Vec<u8>);
            out.extend_from_slice(std::slice::from_raw_parts(buffer as *const u8, size as usize));
            size as i64
        }
        let mut out: Vec<u8> = Vec::new();
        // SAFETY: main thread; the stream context outlives the call.
        unsafe {
            let save = self.state.as_ref().and_then(|state| state.save).ok_or(PluginError::Unsupported)?;
            let stream = clap_ostream { ctx: &mut out as *mut Vec<u8> as *mut c_void, write: Some(write) };
            if !save(self.plugin, &stream) {
                return Err(PluginError::failed("the CLAP plug-in failed to save its state"));
            }
        }
        Ok(out)
    }

    fn set_state(&mut self, data: &[u8]) -> Result<(), PluginError> {
        struct Cursor<'a> {
            data: &'a [u8],
            pos: usize,
        }
        unsafe extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
            let cursor = &mut *((*stream).ctx as *mut Cursor<'_>);
            let count = (cursor.data.len() - cursor.pos).min(size as usize);
            std::ptr::copy_nonoverlapping(cursor.data.as_ptr().add(cursor.pos), buffer as *mut u8, count);
            cursor.pos += count;
            count as i64
        }
        let mut cursor = Cursor { data, pos: 0 };
        // SAFETY: main thread; the stream context outlives the call.
        unsafe {
            let load = self.state.as_ref().and_then(|state| state.load).ok_or(PluginError::Unsupported)?;
            let stream = clap_istream { ctx: &mut cursor as *mut Cursor<'_> as *mut c_void, read: Some(read) };
            if !load(self.plugin, &stream) {
                return Err(PluginError::failed("the CLAP plug-in rejected the state"));
            }
        }
        Ok(())
    }

    fn open_editor(&mut self, title: &str) -> Result<EditorInfo, PluginError> {
        let title = CString::new(title).unwrap_or_default();
        // SAFETY: main thread; calls in the order clap.gui prescribes for floating windows.
        unsafe {
            let gui = self.gui.as_ref().ok_or(PluginError::Unsupported)?;
            let supported = gui.is_api_supported.is_some_and(|f| f(self.plugin, WINDOW_API.as_ptr(), true));
            if !supported {
                // Embedded-only editor: needs a parent window made by the container.
                return Err(PluginError::Unsupported);
            }
            if !self.editor_created {
                if !gui.create.is_some_and(|create| create(self.plugin, WINDOW_API.as_ptr(), true)) {
                    return Err(PluginError::failed("the CLAP plug-in failed to create its editor"));
                }
                self.editor_created = true;
            }
            if let Some(suggest_title) = gui.suggest_title {
                suggest_title(self.plugin, title.as_ptr());
            }
            let (mut width, mut height) = (0, 0);
            if let Some(get_size) = gui.get_size {
                get_size(self.plugin, &mut width, &mut height);
            }
            if !gui.show.is_some_and(|show| show(self.plugin)) {
                return Err(PluginError::failed("the CLAP plug-in failed to show its editor"));
            }
            let resizable = gui.can_resize.is_some_and(|can_resize| can_resize(self.plugin));
            Ok(EditorInfo { width, height, resizable, window_id: 0 })
        }
    }

    fn close_editor(&mut self) {
        if !self.editor_created {
            return;
        }
        // SAFETY: main thread.
        unsafe {
            if let Some(gui) = self.gui.as_ref() {
                if let Some(hide) = gui.hide {
                    hide(self.plugin);
                }
                if let Some(destroy) = gui.destroy {
                    destroy(self.plugin);
                }
            }
        }
        self.editor_created = false;
    }

    fn focus_editor(&mut self) {
        // SAFETY: main thread. CLAP has no "raise": showing again is the closest request.
        unsafe {
            if let (true, Some(show)) = (self.editor_created, self.gui.as_ref().and_then(|gui| gui.show)) {
                show(self.plugin);
            }
        }
    }

    fn set_editor_title(&mut self, title: &str) {
        let title = CString::new(title).unwrap_or_default();
        // SAFETY: main thread.
        unsafe {
            if let (true, Some(suggest_title)) =
                (self.editor_created, self.gui.as_ref().and_then(|gui| gui.suggest_title))
            {
                suggest_title(self.plugin, title.as_ptr());
            }
        }
    }

    fn idle(&mut self) {
        if self.host().callback_requested.swap(false, Ordering::AcqRel) {
            // SAFETY: main thread.
            unsafe {
                if let Some(on_main_thread) = (*self.plugin).on_main_thread {
                    on_main_thread(self.plugin);
                }
            }
        }
        if self.host().flush_requested.swap(false, Ordering::AcqRel) && !self.active {
            let _ = self.flush(&[]);
        }
        if self.host().latency_changed.swap(false, Ordering::AcqRel) {
            // SAFETY: main thread.
            let frames = unsafe { self.latency.as_ref().and_then(|l| l.get).map_or(0, |get| get(self.plugin)) };
            self.host().handle.notify(Notification::LatencyChanged { frames });
        }
    }
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        self.close_editor();
        // SAFETY: main thread; the runtime deactivated the plug-in before dropping it
        // (the fallback covers a backend bug only). The host data outlives the plug-in.
        unsafe {
            if self.active {
                if let Some(deactivate) = (*self.plugin).deactivate {
                    deactivate(self.plugin);
                }
            }
            if let Some(destroy) = (*self.plugin).destroy {
                destroy(self.plugin);
            }
            drop(Box::from_raw(self.host));
        }
    }
}

/// Channel pointer table and scratch memory of one plug-in audio port.
struct PortBuffers {
    pointers: Vec<*mut f32>,
    /// One `max_frames` buffer per channel, for channels the host did not allot.
    scratch: Vec<f32>,
}

struct ClapProcessor {
    plugin: *const clap_plugin,
    handle: HostHandle,
    queue: Arc<ParamQueue>,
    started: bool,
    max_frames: usize,
    steady_time: i64,
    midi_dialect: bool,
    note_dialect: bool,
    events: Vec<EventSlot>,
    inputs: Vec<PortBuffers>,
    outputs: Vec<PortBuffers>,
    in_buffers: Vec<clap_audio_buffer>,
    out_buffers: Vec<clap_audio_buffer>,
}

// SAFETY: the processor is used by one thread at a time (the runtime's gate); the raw
// pointers refer to the plug-in, which outlives the processor, and to the processor's own
// buffers.
unsafe impl Send for ClapProcessor {}

impl ClapProcessor {
    fn push_input_event(&mut self, event: &Event) {
        if self.events.len() == self.events.capacity() {
            return; // never reallocate on the audio thread
        }
        if event.kind == EVENT_PARAM {
            self.events.push(param_event(event.offset, event.id, event.value));
            return;
        }
        if event.kind != EVENT_MIDI {
            return;
        }
        let bytes = event.midi_bytes();
        let Some(&status) = bytes.first() else { return };
        let flags = if event.flags & EVENT_FLAG_SEQUENCED != 0 { 0 } else { CLAP_EVENT_IS_LIVE };
        let kind = status & 0xF0;
        let is_note = (kind == 0x90 || kind == 0x80) && bytes.len() == 3;
        if is_note && self.note_dialect {
            let on = kind == 0x90 && bytes[2] > 0;
            self.events.push(EventSlot {
                note: clap_event_note {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note>() as u32,
                        time: event.offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: if on { CLAP_EVENT_NOTE_ON } else { CLAP_EVENT_NOTE_OFF },
                        flags,
                    },
                    note_id: if event.aux == u32::MAX { -1 } else { event.aux as i32 },
                    port_index: event.port as i16,
                    channel: (status & 0x0F) as i16,
                    key: bytes[1] as i16,
                    velocity: bytes[2] as f64 / 127.0,
                },
            });
        } else if !self.midi_dialect && self.note_dialect {
            // No MIDI on this port: pressure and pitch bend have note expressions.
            let data1 = bytes.get(1).copied().unwrap_or(0) & 0x7F;
            let data2 = bytes.get(2).copied().unwrap_or(0) & 0x7F;
            let (expression_id, key, value) = match kind {
                0xA0 => (CLAP_NOTE_EXPRESSION_PRESSURE, data1 as i16, data2 as f64 / 127.0),
                0xD0 => (CLAP_NOTE_EXPRESSION_PRESSURE, -1, data1 as f64 / 127.0),
                0xE0 => {
                    let bend = (data1 as i32 | (data2 as i32) << 7) - 8192;
                    (CLAP_NOTE_EXPRESSION_TUNING, -1, bend as f64 / 8192.0 * BEND_RANGE)
                }
                _ => return,
            };
            self.events.push(EventSlot {
                expression: clap_event_note_expression {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note_expression>() as u32,
                        time: event.offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_NOTE_EXPRESSION,
                        flags,
                    },
                    expression_id,
                    note_id: -1,
                    port_index: event.port as i16,
                    channel: (status & 0x0F) as i16,
                    key,
                    value,
                },
            });
        } else if self.midi_dialect {
            let mut data = [0u8; 3];
            data[..bytes.len()].copy_from_slice(bytes);
            self.events.push(EventSlot {
                midi: clap_event_midi {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_midi>() as u32,
                        time: event.offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_MIDI,
                        flags,
                    },
                    port_index: event.port as u16,
                    data,
                },
            });
        }
    }
}

impl Processor for ClapProcessor {
    fn process(&mut self, data: &mut ProcessData<'_>) -> ProcessStatus {
        let frames = data.frames;
        if frames > self.max_frames {
            return ProcessStatus::Failed;
        }
        ON_AUDIO_THREAD.with(|flag| flag.set(true));
        // SAFETY: this is the plug-in's audio thread in CLAP terms: the runtime calls
        // `process` and `reset` for this plug-in from one thread at a time.
        let status = unsafe {
            if !self.started {
                self.started = (*self.plugin).start_processing.is_none_or(|start| start(self.plugin));
                if !self.started {
                    ON_AUDIO_THREAD.with(|flag| flag.set(false));
                    return ProcessStatus::Failed;
                }
            }
            // Events: queued main-thread parameter changes first (time 0), then the block's.
            self.events.clear();
            while self.events.len() < self.events.capacity() {
                let Some((id, value)) = self.queue.pop() else { break };
                self.events.push(param_event(0, id, value));
            }
            for index in 0..data.events.len() {
                let event = data.events[index];
                self.push_input_event(&event);
            }

            // Audio: region channels where the host allotted them, scratch elsewhere.
            for (port, buffers) in self.inputs.iter_mut().enumerate() {
                let host_channels = data.input_channels(port);
                let mut constant = 0u64;
                for (channel, pointer) in buffers.pointers.iter_mut().enumerate() {
                    if channel < host_channels {
                        *pointer = data.input_ptrs(port)[channel] as *mut f32;
                        let flat = data.input_flat_index(port) + channel;
                        if flat < 64 && data.input_silence & (1 << flat) != 0 {
                            constant |= 1 << channel;
                        }
                    } else {
                        // Silence (the scratch of input ports is never written to).
                        *pointer = buffers.scratch.as_mut_ptr().add(channel * self.max_frames);
                        constant |= 1u64.checked_shl(channel as u32).unwrap_or(0);
                    }
                }
                self.in_buffers[port] = clap_audio_buffer {
                    data32: buffers.pointers.as_mut_ptr(),
                    data64: std::ptr::null_mut(),
                    channel_count: buffers.pointers.len() as u32,
                    latency: 0,
                    constant_mask: constant,
                };
            }
            for (port, buffers) in self.outputs.iter_mut().enumerate() {
                let host_channels = data.output_channels(port);
                for (channel, pointer) in buffers.pointers.iter_mut().enumerate() {
                    *pointer = if channel < host_channels {
                        data.output_ptrs(port)[channel]
                    } else {
                        buffers.scratch.as_mut_ptr().add(channel * self.max_frames)
                    };
                }
                self.out_buffers[port] = clap_audio_buffer {
                    data32: buffers.pointers.as_mut_ptr(),
                    data64: std::ptr::null_mut(),
                    channel_count: buffers.pointers.len() as u32,
                    latency: 0,
                    constant_mask: 0,
                };
            }

            let t = &data.transport;
            let mut flags = 0;
            for (ours, clap) in [
                (TRANSPORT_PLAYING, CLAP_TRANSPORT_IS_PLAYING),
                (TRANSPORT_RECORDING, CLAP_TRANSPORT_IS_RECORDING),
                (TRANSPORT_LOOPING, CLAP_TRANSPORT_IS_LOOP_ACTIVE),
                (TRANSPORT_TEMPO_VALID, CLAP_TRANSPORT_HAS_TEMPO),
                (TRANSPORT_TIME_SIG_VALID, CLAP_TRANSPORT_HAS_TIME_SIGNATURE),
                (TRANSPORT_PPQ_VALID, CLAP_TRANSPORT_HAS_BEATS_TIMELINE),
            ] {
                if t.flags & ours != 0 {
                    flags |= clap;
                }
            }
            let beats = |quarters: f64| (quarters * CLAP_BEATTIME_FACTOR as f64).round() as i64;
            let transport = clap_event_transport {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_transport>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_TRANSPORT,
                    flags: 0,
                },
                flags,
                song_pos_beats: beats(t.ppq_position),
                song_pos_seconds: 0,
                tempo: t.tempo_bpm,
                tempo_inc: 0.0,
                loop_start_beats: beats(t.loop_start_ppq),
                loop_end_beats: beats(t.loop_end_ppq),
                loop_start_seconds: 0,
                loop_end_seconds: 0,
                bar_start: beats(t.bar_start_ppq),
                bar_number: 0,
                tsig_num: t.time_sig_numerator,
                tsig_denom: t.time_sig_denominator,
            };
            let list = InputList { events: self.events.as_ptr(), count: self.events.len() as u32 };
            let input = clap_input_events {
                ctx: &list as *const _ as *mut c_void,
                size: Some(input_size),
                get: Some(input_get),
            };
            let sink = OutputSink { handle: &self.handle, data: data as *mut ProcessData<'_> as *mut c_void };
            let output = clap_output_events { ctx: &sink as *const _ as *mut c_void, try_push: Some(output_try_push) };
            let process = clap_process {
                steady_time: self.steady_time,
                frames_count: frames as u32,
                transport: &transport,
                audio_inputs: self.in_buffers.as_ptr(),
                audio_outputs: self.out_buffers.as_mut_ptr(),
                audio_inputs_count: self.in_buffers.len() as u32,
                audio_outputs_count: self.out_buffers.len() as u32,
                in_events: &input,
                out_events: &output,
            };
            self.steady_time += frames as i64;
            match (*self.plugin).process {
                Some(process_fn) => process_fn(self.plugin, &process),
                None => CLAP_PROCESS_ERROR,
            }
        };
        ON_AUDIO_THREAD.with(|flag| flag.set(false));
        if status == CLAP_PROCESS_ERROR {
            ProcessStatus::Failed
        } else {
            ProcessStatus::Ok
        }
    }

    fn reset(&mut self) {
        // SAFETY: processing is excluded while this runs (audio-thread function in CLAP terms).
        unsafe {
            if let Some(reset) = (*self.plugin).reset {
                reset(self.plugin);
            }
        }
    }
}

impl Drop for ClapProcessor {
    fn drop(&mut self) {
        if self.started {
            // SAFETY: dropped in `Plugin::deactivate`, with processing excluded.
            unsafe {
                if let Some(stop) = (*self.plugin).stop_processing {
                    stop(self.plugin);
                }
            }
        }
    }
}

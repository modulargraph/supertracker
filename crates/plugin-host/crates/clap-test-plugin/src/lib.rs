//! A small CLAP plug-in library for testing the container's CLAP backend end to end.
//!
//! Written directly against the CLAP ABI (`clap-sys`), like a C plug-in would be. Two
//! plug-ins:
//!
//! * [`GAIN_ID`]: stereo effect. Parameters: Gain, Bypass (the CLAP bypass flag), Crash
//!   (1: abort, 2: segfault, inside `process`), Latency (the latency it *reports*; changing
//!   it while active asks the host for a restart), and three read-only probes (editor
//!   state, editor title length, number of `on_main_thread` calls). State, floating GUI
//!   (bookkeeping only: no window is created), latency extension.
//! * [`SINE_ID`]: instrument with one note port (CLAP and MIDI dialects) and a stereo
//!   output. Parameter: Level. MIDI CC 7 sets the level and reports it to the host as an
//!   output parameter event. Channel pressure scales the level ([`pressure_gain`]). Four
//!   read-only probes show the last control change, channel pressure, poly pressure and
//!   pitch bend it received.
//! * [`NOTE_SINE_ID`]: the same instrument with a note port that takes the CLAP dialect
//!   only: pressure and pitch bend reach it as note expressions (and fill the same probes),
//!   control changes cannot reach it at all.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::gui::*;
use clap_sys::ext::latency::*;
use clap_sys::ext::note_ports::*;
use clap_sys::ext::params::*;
use clap_sys::ext::state::*;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::{clap_id, CLAP_INVALID_ID};
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::*;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

pub const GAIN_ID: &str = "org.supertracker.test.gain";
pub const SINE_ID: &str = "org.supertracker.test.sine";
pub const NOTE_SINE_ID: &str = "org.supertracker.test.sine.notes";

pub const PARAM_GAIN: u32 = 1;
pub const PARAM_BYPASS: u32 = 2;
pub const PARAM_CRASH: u32 = 3;
pub const PARAM_LATENCY: u32 = 4;
/// Read-only: 0 no editor, 1 created, 2 shown.
pub const PARAM_EDITOR_STATE: u32 = 10;
/// Read-only: length of the title the host suggested for the editor.
pub const PARAM_EDITOR_TITLE_LEN: u32 = 11;
/// Read-only: how often the host called `on_main_thread`.
pub const PARAM_MAIN_CALLS: u32 = 12;
pub const PARAM_LEVEL: u32 = 1;
/// Read-only probes of the sine instruments: what arrived last.
/// Control change: `controller * 128 + value`.
pub const PARAM_PROBE_CONTROLLER: u32 = 20;
/// Channel pressure, 0..=127.
pub const PARAM_PROBE_PRESSURE: u32 = 21;
/// Poly pressure: `key * 128 + pressure`.
pub const PARAM_PROBE_POLY_PRESSURE: u32 = 22;
/// Pitch bend, 0..=16383 (8192: centre).
pub const PARAM_PROBE_BEND: u32 = 23;

/// The factor channel pressure (0..=127) multiplies the sine instruments' level by.
pub fn pressure_gain(pressure: u8) -> f64 {
    1.0 + pressure as f64 / 127.0
}

/// The sample the sine instrument produces `k` frames after a note-on (same formula as
/// the built-in test backend, so the tests can compare bit for bit).
pub fn sine_sample(note: u8, k: u64, sample_rate: f64, level: f64) -> f32 {
    let frequency = 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
    (level * (std::f64::consts::TAU * frequency * k as f64 / sample_rate).sin()) as f32
}

macro_rules! c {
    ($s:literal) => {
        concat!($s, "\0").as_ptr() as *const c_char
    };
}

struct SyncPtrs<const N: usize>([*const c_char; N]);
// SAFETY: pointers to string literals.
unsafe impl<const N: usize> Sync for SyncPtrs<N> {}

static GAIN_FEATURES: SyncPtrs<3> = SyncPtrs([c!("audio-effect"), c!("stereo"), std::ptr::null()]);
static SINE_FEATURES: SyncPtrs<3> = SyncPtrs([c!("instrument"), c!("stereo"), std::ptr::null()]);

static GAIN_DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c!("org.supertracker.test.gain"),
    name: c!("ST Test Gain"),
    vendor: c!("SuperTracker Tests"),
    url: c!(""),
    manual_url: c!(""),
    support_url: c!(""),
    version: c!("1.2.3"),
    description: c!("Gain effect for testing"),
    features: GAIN_FEATURES.0.as_ptr(),
};

static SINE_DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c!("org.supertracker.test.sine"),
    name: c!("ST Test Sine"),
    vendor: c!("SuperTracker Tests"),
    url: c!(""),
    manual_url: c!(""),
    support_url: c!(""),
    version: c!("1.2.3"),
    description: c!("Sine instrument for testing"),
    features: SINE_FEATURES.0.as_ptr(),
};

static NOTE_SINE_DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c!("org.supertracker.test.sine.notes"),
    name: c!("ST Test Sine (CLAP notes)"),
    vendor: c!("SuperTracker Tests"),
    url: c!(""),
    manual_url: c!(""),
    support_url: c!(""),
    version: c!("1.2.3"),
    description: c!("Sine instrument without MIDI input, for testing"),
    features: SINE_FEATURES.0.as_ptr(),
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Gain,
    Sine,
}

#[repr(C)]
struct Instance {
    plugin: clap_plugin,
    host: *const clap_host,
    kind: Kind,
    sample_rate: f64,
    active: AtomicBool,
    /// Gain, or the sine's level.
    value: AtomicU64,
    bypass: AtomicBool,
    latency: AtomicU32,
    reported_latency: AtomicU32,
    editor_state: AtomicU32,
    editor_title_len: AtomicU32,
    main_calls: AtomicU32,
    /// The sine's note port takes MIDI besides CLAP note events.
    midi_dialect: bool,
    /// The probes of the sine: controller, channel pressure, poly pressure, pitch bend.
    probes: [AtomicU32; 4],
    // audio thread only
    note: Option<u8>,
    note_age: u64,
}

impl Instance {
    fn value(&self) -> f64 {
        f64::from_bits(self.value.load(Ordering::Relaxed))
    }
    fn set_value(&self, value: f64) {
        self.value.store(value.to_bits(), Ordering::Relaxed);
    }
    fn probe(&self, id: clap_id) -> u32 {
        self.probes[(id - PARAM_PROBE_CONTROLLER) as usize].load(Ordering::Relaxed)
    }
    fn set_probe(&self, id: clap_id, value: u32) {
        self.probes[(id - PARAM_PROBE_CONTROLLER) as usize].store(value, Ordering::Relaxed);
    }
}

unsafe fn this<'a>(plugin: *const clap_plugin) -> &'a mut Instance {
    &mut *((*plugin).plugin_data as *mut Instance)
}

fn crash(how: u32) -> ! {
    if how == 2 {
        // SAFETY: deliberately not safe: the segfault a test asked for.
        #[allow(clippy::manual_dangling_ptr)]
        unsafe {
            std::ptr::write_volatile(8 as *mut u64, 0xDEAD)
        };
    }
    std::process::abort()
}

// ---- plug-in life cycle ---------------------------------------------------------------------

unsafe extern "C" fn plugin_init(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    drop(Box::from_raw((*plugin).plugin_data as *mut Instance));
}

unsafe extern "C" fn plugin_activate(plugin: *const clap_plugin, sample_rate: f64, _min: u32, _max: u32) -> bool {
    let instance = this(plugin);
    instance.sample_rate = sample_rate;
    instance.active.store(true, Ordering::Relaxed);
    // A latency change may only be announced while activating.
    let latency = instance.latency.load(Ordering::Relaxed);
    if instance.reported_latency.swap(latency, Ordering::Relaxed) != latency {
        let host = instance.host;
        if let Some(get_extension) = (*host).get_extension {
            let ext = get_extension(host, CLAP_EXT_LATENCY.as_ptr()) as *const clap_host_latency;
            if !ext.is_null() {
                if let Some(changed) = (*ext).changed {
                    changed(host);
                }
            }
        }
    }
    true
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    this(plugin).active.store(false, Ordering::Relaxed);
}

unsafe extern "C" fn plugin_start_processing(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    let instance = this(plugin);
    instance.note = None;
    instance.note_age = 0;
}

unsafe extern "C" fn plugin_on_main_thread(plugin: *const clap_plugin) {
    this(plugin).main_calls.fetch_add(1, Ordering::Relaxed);
}

/// Applies one event. Returns a parameter change to report back to the host, if any.
unsafe fn handle_event(instance: &mut Instance, header: *const clap_event_header) -> Option<(clap_id, f64)> {
    if (*header).space_id != CLAP_CORE_EVENT_SPACE_ID {
        return None;
    }
    match (*header).type_ {
        CLAP_EVENT_PARAM_VALUE => {
            let event = &*(header as *const clap_event_param_value);
            match (instance.kind, event.param_id) {
                (Kind::Gain, PARAM_GAIN) | (Kind::Sine, PARAM_LEVEL) => instance.set_value(event.value),
                (Kind::Gain, PARAM_BYPASS) => instance.bypass.store(event.value >= 0.5, Ordering::Relaxed),
                (Kind::Gain, PARAM_CRASH) if event.value >= 1.0 => crash(event.value as u32),
                (Kind::Gain, PARAM_LATENCY) => {
                    instance.latency.store(event.value as u32, Ordering::Relaxed);
                    if instance.active.load(Ordering::Relaxed) {
                        if let Some(request_restart) = (*instance.host).request_restart {
                            request_restart(instance.host);
                        }
                    }
                }
                _ => {}
            }
        }
        CLAP_EVENT_NOTE_ON if instance.kind == Kind::Sine => {
            let event = &*(header as *const clap_event_note);
            instance.note = Some(event.key as u8);
            instance.note_age = 0;
        }
        CLAP_EVENT_NOTE_OFF if instance.kind == Kind::Sine => {
            let event = &*(header as *const clap_event_note);
            if instance.note == Some(event.key as u8) {
                instance.note = None;
            }
        }
        CLAP_EVENT_NOTE_EXPRESSION if instance.kind == Kind::Sine => {
            let event = &*(header as *const clap_event_note_expression);
            match event.expression_id {
                CLAP_NOTE_EXPRESSION_PRESSURE => {
                    let pressure = (event.value.clamp(0.0, 1.0) * 127.0).round() as u32;
                    if event.key < 0 {
                        instance.set_probe(PARAM_PROBE_PRESSURE, pressure);
                    } else {
                        instance.set_probe(PARAM_PROBE_POLY_PRESSURE, event.key as u32 * 128 + pressure);
                    }
                }
                CLAP_NOTE_EXPRESSION_TUNING => {
                    // The host bends by two semitones either way.
                    let bend = (8192.0 + event.value / 2.0 * 8192.0).round().clamp(0.0, 16383.0);
                    instance.set_probe(PARAM_PROBE_BEND, bend as u32);
                }
                _ => {}
            }
        }
        CLAP_EVENT_MIDI if instance.kind == Kind::Sine && instance.midi_dialect => {
            let event = &*(header as *const clap_event_midi);
            match event.data[0] & 0xF0 {
                0x90 if event.data[2] > 0 => {
                    instance.note = Some(event.data[1]);
                    instance.note_age = 0;
                }
                0x80 | 0x90 => {
                    if instance.note == Some(event.data[1]) {
                        instance.note = None;
                    }
                }
                0xB0 => {
                    instance.set_probe(PARAM_PROBE_CONTROLLER, event.data[1] as u32 * 128 + event.data[2] as u32);
                    if event.data[1] == 7 {
                        let level = event.data[2] as f64 / 127.0;
                        instance.set_value(level);
                        return Some((PARAM_LEVEL, level));
                    }
                }
                0xD0 => instance.set_probe(PARAM_PROBE_PRESSURE, event.data[1] as u32),
                0xA0 => instance.set_probe(PARAM_PROBE_POLY_PRESSURE, event.data[1] as u32 * 128 + event.data[2] as u32),
                0xE0 => instance.set_probe(PARAM_PROBE_BEND, event.data[1] as u32 | (event.data[2] as u32) << 7),
                _ => {}
            }
        }
        _ => {}
    }
    None
}

unsafe fn report_param(out: *const clap_output_events, time: u32, id: clap_id, value: f64) {
    let event = clap_event_param_value {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_param_value>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: id,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    };
    if let Some(try_push) = (*out).try_push {
        try_push(out, &event.header);
    }
}

unsafe extern "C" fn plugin_process(plugin: *const clap_plugin, process: *const clap_process) -> clap_process_status {
    let instance = this(plugin);
    let process = &*process;
    let frames = process.frames_count as usize;
    let events = process.in_events;
    let event_count = (*events).size.map_or(0, |size| size(events));
    let get = (*events).get.unwrap();
    let mut next_event = 0;

    let out_buffer = &*process.audio_outputs;
    let outputs = std::slice::from_raw_parts(out_buffer.data32, out_buffer.channel_count as usize);
    let inputs: &[*mut f32] = if instance.kind == Kind::Gain && process.audio_inputs_count > 0 {
        let in_buffer = &*process.audio_inputs;
        std::slice::from_raw_parts(in_buffer.data32, in_buffer.channel_count as usize)
    } else {
        &[]
    };

    for frame in 0..frames {
        while next_event < event_count {
            let header = get(events, next_event);
            if (*header).time as usize > frame {
                break;
            }
            if let Some((id, value)) = handle_event(instance, header) {
                report_param(process.out_events, (*header).time, id, value);
            }
            next_event += 1;
        }
        match instance.kind {
            Kind::Gain => {
                let gain = if instance.bypass.load(Ordering::Relaxed) { 1.0 } else { instance.value() as f32 };
                for (channel, &output) in outputs.iter().enumerate() {
                    let sample = inputs.get(channel).map_or(0.0, |&input| *input.add(frame));
                    *output.add(frame) = sample * gain;
                }
            }
            Kind::Sine => {
                let sample = match instance.note {
                    Some(note) => {
                        let level = match instance.probe(PARAM_PROBE_PRESSURE) {
                            0 => instance.value(),
                            pressure => instance.value() * pressure_gain(pressure as u8),
                        };
                        let sample = sine_sample(note, instance.note_age, instance.sample_rate, level);
                        instance.note_age += 1;
                        sample
                    }
                    None => 0.0,
                };
                for &output in outputs {
                    *output.add(frame) = sample;
                }
            }
        }
    }
    CLAP_PROCESS_CONTINUE
}

// ---- extensions -------------------------------------------------------------------------------

struct ParamSpec {
    id: clap_id,
    name: &'static str,
    module: &'static str,
    min: f64,
    max: f64,
    default: f64,
    flags: clap_param_info_flags,
}

const GAIN_PARAMS: [ParamSpec; 7] = [
    ParamSpec {
        id: PARAM_GAIN,
        name: "Gain",
        module: "Main",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        flags: CLAP_PARAM_IS_AUTOMATABLE,
    },
    ParamSpec {
        id: PARAM_BYPASS,
        name: "Bypass",
        module: "Main",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        flags: CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_BYPASS,
    },
    ParamSpec {
        id: PARAM_CRASH,
        name: "Crash",
        module: "Test",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        flags: CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM,
    },
    ParamSpec {
        id: PARAM_LATENCY,
        name: "Latency",
        module: "Test",
        min: 0.0,
        max: 1024.0,
        default: 0.0,
        flags: CLAP_PARAM_IS_STEPPED,
    },
    ParamSpec {
        id: PARAM_EDITOR_STATE,
        name: "Editor State",
        module: "Test/Probe",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        flags: CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_STEPPED,
    },
    ParamSpec {
        id: PARAM_EDITOR_TITLE_LEN,
        name: "Editor Title Length",
        module: "Test/Probe",
        min: 0.0,
        max: 1024.0,
        default: 0.0,
        flags: CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_HIDDEN,
    },
    ParamSpec {
        id: PARAM_MAIN_CALLS,
        name: "Main Calls",
        module: "Test/Probe",
        min: 0.0,
        max: 1e9,
        default: 0.0,
        flags: CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_HIDDEN,
    },
];

const fn probe(id: clap_id, name: &'static str, max: f64, default: f64) -> ParamSpec {
    ParamSpec { id, name, module: "Test/Probe", min: 0.0, max, default, flags: CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_STEPPED }
}

const SINE_PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        id: PARAM_LEVEL,
        name: "Level",
        module: "",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        flags: CLAP_PARAM_IS_AUTOMATABLE,
    },
    probe(PARAM_PROBE_CONTROLLER, "Last Controller", 16383.0, 0.0),
    probe(PARAM_PROBE_PRESSURE, "Channel Pressure", 127.0, 0.0),
    probe(PARAM_PROBE_POLY_PRESSURE, "Poly Pressure", 16383.0, 0.0),
    probe(PARAM_PROBE_BEND, "Pitch Bend", 16383.0, 8192.0),
];

fn specs(kind: Kind) -> &'static [ParamSpec] {
    match kind {
        Kind::Gain => &GAIN_PARAMS,
        Kind::Sine => &SINE_PARAMS,
    }
}

fn copy_str(dst: &mut [c_char], src: &str) {
    let len = src.len().min(dst.len() - 1);
    for (slot, byte) in dst.iter_mut().zip(src.bytes().take(len)) {
        *slot = byte as c_char;
    }
    dst[len] = 0;
}

unsafe extern "C" fn params_count(plugin: *const clap_plugin) -> u32 {
    specs(this(plugin).kind).len() as u32
}

unsafe extern "C" fn params_get_info(plugin: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let Some(spec) = specs(this(plugin).kind).get(index as usize) else { return false };
    let info = &mut *info;
    info.id = spec.id;
    info.flags = spec.flags;
    info.cookie = std::ptr::null_mut();
    copy_str(&mut info.name, spec.name);
    copy_str(&mut info.module, spec.module);
    info.min_value = spec.min;
    info.max_value = spec.max;
    info.default_value = spec.default;
    true
}

unsafe extern "C" fn params_get_value(plugin: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    let instance = this(plugin);
    let value = match (instance.kind, id) {
        (Kind::Gain, PARAM_GAIN) | (Kind::Sine, PARAM_LEVEL) => instance.value(),
        (Kind::Gain, PARAM_BYPASS) => instance.bypass.load(Ordering::Relaxed) as u8 as f64,
        (Kind::Gain, PARAM_CRASH) => 0.0,
        (Kind::Gain, PARAM_LATENCY) => instance.latency.load(Ordering::Relaxed) as f64,
        (Kind::Gain, PARAM_EDITOR_STATE) => instance.editor_state.load(Ordering::Relaxed) as f64,
        (Kind::Gain, PARAM_EDITOR_TITLE_LEN) => instance.editor_title_len.load(Ordering::Relaxed) as f64,
        (Kind::Gain, PARAM_MAIN_CALLS) => instance.main_calls.load(Ordering::Relaxed) as f64,
        (Kind::Sine, PARAM_PROBE_CONTROLLER..=PARAM_PROBE_BEND) => instance.probe(id) as f64,
        _ => return false,
    };
    *out = value;
    true
}

unsafe extern "C" fn params_value_to_text(
    plugin: *const clap_plugin,
    id: clap_id,
    value: f64,
    out: *mut c_char,
    capacity: u32,
) -> bool {
    let text = match (this(plugin).kind, id) {
        (Kind::Gain, PARAM_GAIN) => format!("{value:.2} x"),
        (Kind::Gain, PARAM_BYPASS) => (if value >= 0.5 { "On" } else { "Off" }).to_string(),
        (Kind::Gain, PARAM_CRASH) => {
            ["No", "Abort", "Segfault"].get(value as usize).copied().unwrap_or("?").to_string()
        }
        (Kind::Sine, PARAM_LEVEL) => format!("{:.0} %", value * 100.0),
        _ => return false,
    };
    copy_str(std::slice::from_raw_parts_mut(out, capacity as usize), &text);
    true
}

unsafe extern "C" fn params_text_to_value(
    plugin: *const clap_plugin,
    id: clap_id,
    text: *const c_char,
    out: *mut f64,
) -> bool {
    let text = CStr::from_ptr(text).to_string_lossy();
    let value = match (this(plugin).kind, id) {
        (Kind::Gain, PARAM_GAIN) => text.trim().trim_end_matches('x').trim().parse::<f64>().ok(),
        (Kind::Gain, PARAM_BYPASS) => match text.as_ref() {
            "On" => Some(1.0),
            "Off" => Some(0.0),
            _ => None,
        },
        _ => None,
    };
    match value {
        Some(value) => {
            *out = value;
            true
        }
        None => false,
    }
}

unsafe extern "C" fn params_flush(
    plugin: *const clap_plugin,
    input: *const clap_input_events,
    out: *const clap_output_events,
) {
    let instance = this(plugin);
    let count = (*input).size.map_or(0, |size| size(input));
    let get = (*input).get.unwrap();
    for index in 0..count {
        let header = get(input, index);
        if let Some((id, value)) = handle_event(instance, header) {
            report_param(out, 0, id, value);
        }
    }
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

const STATE_MAGIC: &[u8; 4] = b"STCT";

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let instance = this(plugin);
    let mut data = STATE_MAGIC.to_vec();
    data.push(instance.kind as u8);
    data.extend_from_slice(&instance.value().to_le_bytes());
    data.push(instance.bypass.load(Ordering::Relaxed) as u8);
    data.extend_from_slice(&instance.latency.load(Ordering::Relaxed).to_le_bytes());
    let write = (*stream).write.unwrap();
    let mut written = 0;
    // Deliberately in small pieces: a stream may accept less than offered.
    while written < data.len() {
        let chunk = (data.len() - written).min(5);
        let n = write(stream, data[written..].as_ptr().cast(), chunk as u64);
        if n <= 0 {
            return false;
        }
        written += n as usize;
    }
    true
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let instance = this(plugin);
    let read = (*stream).read.unwrap();
    let mut data = Vec::new();
    let mut chunk = [0u8; 7];
    loop {
        let n = read(stream, chunk.as_mut_ptr().cast(), chunk.len() as u64);
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n as usize]);
    }
    if data.len() != 18 || &data[..4] != STATE_MAGIC || data[4] != instance.kind as u8 {
        return false;
    }
    instance.set_value(f64::from_le_bytes(data[5..13].try_into().unwrap()));
    instance.bypass.store(data[13] != 0, Ordering::Relaxed);
    instance.latency.store(u32::from_le_bytes(data[14..18].try_into().unwrap()), Ordering::Relaxed);
    // Exercise the host's main-thread callback.
    if let Some(request_callback) = (*instance.host).request_callback {
        request_callback(instance.host);
    }
    true
}

static STATE: clap_plugin_state = clap_plugin_state { save: Some(state_save), load: Some(state_load) };

unsafe extern "C" fn audio_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    match (this(plugin).kind, is_input) {
        (Kind::Sine, true) => 0,
        _ => 1,
    }
}

unsafe extern "C" fn audio_ports_get(
    plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if index >= audio_ports_count(plugin, is_input) {
        return false;
    }
    let info = &mut *info;
    info.id = if is_input { 100 } else { 200 };
    copy_str(&mut info.name, if is_input { "Audio In" } else { "Audio Out" });
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = CLAP_INVALID_ID;
    true
}

static AUDIO_PORTS: clap_plugin_audio_ports =
    clap_plugin_audio_ports { count: Some(audio_ports_count), get: Some(audio_ports_get) };

unsafe extern "C" fn note_ports_count(_plugin: *const clap_plugin, is_input: bool) -> u32 {
    is_input as u32
}

unsafe extern "C" fn note_ports_get(
    plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    if !is_input || index != 0 {
        return false;
    }
    let info = &mut *info;
    info.id = 300;
    info.supported_dialects =
        if this(plugin).midi_dialect { CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI } else { CLAP_NOTE_DIALECT_CLAP };
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    copy_str(&mut info.name, "Notes");
    true
}

static NOTE_PORTS: clap_plugin_note_ports =
    clap_plugin_note_ports { count: Some(note_ports_count), get: Some(note_ports_get) };

unsafe extern "C" fn latency_get(plugin: *const clap_plugin) -> u32 {
    this(plugin).latency.load(Ordering::Relaxed)
}

static LATENCY: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };

// A "floating" editor without a window: enough to verify the host's call sequence.
unsafe extern "C" fn gui_is_api_supported(_plugin: *const clap_plugin, _api: *const c_char, is_floating: bool) -> bool {
    is_floating
}
unsafe extern "C" fn gui_get_preferred_api(
    _plugin: *const clap_plugin,
    _api: *mut *const c_char,
    is_floating: *mut bool,
) -> bool {
    *is_floating = true;
    false
}
unsafe extern "C" fn gui_create(plugin: *const clap_plugin, _api: *const c_char, is_floating: bool) -> bool {
    if !is_floating {
        return false;
    }
    this(plugin).editor_state.store(1, Ordering::Relaxed);
    true
}
unsafe extern "C" fn gui_destroy(plugin: *const clap_plugin) {
    this(plugin).editor_state.store(0, Ordering::Relaxed);
}
unsafe extern "C" fn gui_set_scale(_plugin: *const clap_plugin, _scale: f64) -> bool {
    false
}
unsafe extern "C" fn gui_get_size(_plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
    *width = 320;
    *height = 200;
    true
}
unsafe extern "C" fn gui_can_resize(_plugin: *const clap_plugin) -> bool {
    false
}
unsafe extern "C" fn gui_get_resize_hints(_plugin: *const clap_plugin, _hints: *mut clap_gui_resize_hints) -> bool {
    false
}
unsafe extern "C" fn gui_adjust_size(_plugin: *const clap_plugin, _width: *mut u32, _height: *mut u32) -> bool {
    false
}
unsafe extern "C" fn gui_set_size(_plugin: *const clap_plugin, _width: u32, _height: u32) -> bool {
    false
}
unsafe extern "C" fn gui_set_parent(_plugin: *const clap_plugin, _window: *const clap_window) -> bool {
    false
}
unsafe extern "C" fn gui_set_transient(_plugin: *const clap_plugin, _window: *const clap_window) -> bool {
    false
}
unsafe extern "C" fn gui_suggest_title(plugin: *const clap_plugin, title: *const c_char) {
    this(plugin).editor_title_len.store(CStr::from_ptr(title).to_bytes().len() as u32, Ordering::Relaxed);
}
unsafe extern "C" fn gui_show(plugin: *const clap_plugin) -> bool {
    this(plugin).editor_state.store(2, Ordering::Relaxed);
    true
}
unsafe extern "C" fn gui_hide(plugin: *const clap_plugin) -> bool {
    this(plugin).editor_state.store(1, Ordering::Relaxed);
    true
}

static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: Some(gui_get_preferred_api),
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: Some(gui_get_size),
    can_resize: Some(gui_can_resize),
    get_resize_hints: Some(gui_get_resize_hints),
    adjust_size: Some(gui_adjust_size),
    set_size: Some(gui_set_size),
    set_parent: Some(gui_set_parent),
    set_transient: Some(gui_set_transient),
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

unsafe extern "C" fn plugin_get_extension(plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    let kind = this(plugin).kind;
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &AUDIO_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS && kind == Kind::Sine {
        &NOTE_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY && kind == Kind::Gain {
        &LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI && kind == Kind::Gain {
        &GUI as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

// ---- factory and entry ------------------------------------------------------------------------

unsafe extern "C" fn factory_count(_factory: *const clap_plugin_factory) -> u32 {
    3
}

unsafe extern "C" fn factory_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    match index {
        0 => &GAIN_DESCRIPTOR,
        1 => &SINE_DESCRIPTOR,
        2 => &NOTE_SINE_DESCRIPTOR,
        _ => std::ptr::null(),
    }
}

unsafe extern "C" fn factory_create(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    let id = CStr::from_ptr(id).to_bytes();
    let (kind, descriptor, initial) = if id == GAIN_ID.as_bytes() {
        (Kind::Gain, &GAIN_DESCRIPTOR, 1.0f64)
    } else if id == SINE_ID.as_bytes() {
        (Kind::Sine, &SINE_DESCRIPTOR, 0.5f64)
    } else if id == NOTE_SINE_ID.as_bytes() {
        (Kind::Sine, &NOTE_SINE_DESCRIPTOR, 0.5f64)
    } else {
        return std::ptr::null();
    };
    let instance = Box::into_raw(Box::new(Instance {
        plugin: clap_plugin {
            desc: descriptor,
            plugin_data: std::ptr::null_mut(),
            init: Some(plugin_init),
            destroy: Some(plugin_destroy),
            activate: Some(plugin_activate),
            deactivate: Some(plugin_deactivate),
            start_processing: Some(plugin_start_processing),
            stop_processing: Some(plugin_stop_processing),
            reset: Some(plugin_reset),
            process: Some(plugin_process),
            get_extension: Some(plugin_get_extension),
            on_main_thread: Some(plugin_on_main_thread),
        },
        host,
        kind,
        sample_rate: 48_000.0,
        active: AtomicBool::new(false),
        value: AtomicU64::new(initial.to_bits()),
        bypass: AtomicBool::new(false),
        latency: AtomicU32::new(0),
        reported_latency: AtomicU32::new(0),
        editor_state: AtomicU32::new(0),
        editor_title_len: AtomicU32::new(0),
        main_calls: AtomicU32::new(0),
        midi_dialect: id != NOTE_SINE_ID.as_bytes(),
        probes: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(8192)],
        note: None,
        note_age: 0,
    }));
    (*instance).plugin.plugin_data = instance.cast();
    &(*instance).plugin
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    if CStr::from_ptr(id) == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

/// The symbol a CLAP host looks up after loading the library.
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};

//! A VST3 plug-in module for testing the container's VST3 backend and the scanner.
//!
//! Two plug-ins in one module:
//!
//! * **ST Test Gain VST3** ([`GAIN_CLASS_ID`]): an effect whose processor and edit
//!   controller are **separate classes** connected through `IConnectionPoint`. Gain with
//!   sample-accurate automation, a bypass parameter, a latency parameter (announced with
//!   `restartComponent(kLatencyChanged)`, effective at the next `setActive(true)`), a
//!   sidechain input bus, a program list, an output parameter (peak meter), read-only
//!   probe parameters that echo the process context, component state and controller
//!   state, and deliberate crash and hang triggers.
//! * **ST Test Sine VST3** ([`SINE_CLASS_ID`]): a **single-component** instrument (one
//!   object is processor and controller): a one-voice sine driven by sample-accurate note
//!   events, MIDI controller mapping (`IMidiMapping`), note and legacy-MIDI output events.
//!
//! Parameter points are applied as steps at their sample offset (not as ramps), so the
//! tests can predict every sample exactly.
//!
//! Behaviour that depends on the file name of the loaded library, for scanner tests: a
//! name containing `CrashOnLoad` aborts in the module entry, `EntryFails` makes the entry
//! return false, `HangOnScan` never returns from `GetPluginFactory`, `CrashInFactory`
//! faults there, `NoFactory` returns no factory.
//!
//! With `STPC_VST3_TEST_MARKER=<file>` in the environment the module appends a line per
//! module entry and exit (with the number of plug-in objects still alive at exit).
// The casts of SDK constants are needed where the bindings give an enumeration another
// integer type on another platform.
#![allow(non_snake_case, non_upper_case_globals, clippy::missing_safety_doc, clippy::unnecessary_cast)]

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{uid, Class, ComPtr, ComRef, ComWrapper, Interface};

// ---- identities ---------------------------------------------------------------------------

const GAIN_UID: [u32; 4] = [0x5354_5633, 0x4761_696E, 0x5072_6F63, 0x0000_0001];
const GAIN_CONTROLLER_UID: [u32; 4] = [0x5354_5633, 0x4761_696E, 0x4374_726C, 0x0000_0002];
const SINE_UID: [u32; 4] = [0x5354_5633, 0x5369_6E65, 0x5379_6E74, 0x0000_0003];

/// Class id of the gain effect's processor, as hosts write it (32 hex digits).
pub const GAIN_CLASS_ID: &str = "535456334761696E50726F6300000001";
pub const GAIN_CONTROLLER_CLASS_ID: &str = "535456334761696E4374726C00000002";
pub const SINE_CLASS_ID: &str = "5354563353696E6553796E7400000003";

pub const VENDOR: &str = "SuperTracker Tests";
pub const VERSION: &str = "1.2.3";
pub const GAIN_NAME: &str = "ST Test Gain VST3";
/// The Unicode class info carries a name the 8-bit class info cannot.
pub const SINE_NAME: &str = "ST Test Sine VST3 \u{266A}";
pub const SINE_NAME_ASCII: &str = "ST Test Sine VST3";

fn tuid(parts: [u32; 4]) -> TUID {
    uid(parts[0], parts[1], parts[2], parts[3])
}

// ---- parameters ---------------------------------------------------------------------------

/// Gain effect. Normalized 0..1 maps to a factor of 0..2 ([`gain_factor`]).
pub const PARAM_GAIN: u32 = 100;
pub const PARAM_BYPASS: u32 = 101;
/// Normalized 0..1 maps to 0..1024 frames ([`latency_frames`]).
pub const PARAM_LATENCY: u32 = 102;
/// List: No, Abort, Segfault. Takes effect inside `process`.
pub const PARAM_CRASH: u32 = 103;
/// List: No, Process (never returns from `process`), Main (never returns from the
/// controller's `setParamNormalized`).
pub const PARAM_HANG: u32 = 104;
/// How much of the sidechain input is added to the output (0..1).
pub const PARAM_SIDECHAIN: u32 = 105;
/// Program change parameter bound to the program list ([`PROGRAMS`]).
pub const PARAM_PROGRAM: u32 = 110;
/// Exists in the controller only; saved in the controller state.
pub const PARAM_UI_ZOOM: u32 = 120;
/// Setting it to `v` makes the controller act like its editor: `beginEdit(PARAM_GAIN)`,
/// `performEdit(PARAM_GAIN, v)`, `endEdit(PARAM_GAIN)`, then `setDirty(true)`.
pub const PARAM_EDIT_TRIGGER: u32 = 121;
/// Output parameter: peak of the first output channel in the last block.
pub const PARAM_METER: u32 = 200;
/// Output parameter: 1 while blocks arrive with `kOffline`.
pub const PARAM_OFFLINE: u32 = 201;
/// Output parameters echoing the process context: state flags / 2^20, tempo / 1000,
/// musical position / 1e6, sample position / 2^40, (numerator * 256 + denominator) /
/// 65536, bar start / 1e6, cycle start / 1e6, cycle end / 1e6.
pub const PARAM_CTX_STATE: u32 = 210;
pub const PARAM_CTX_TEMPO: u32 = 211;
pub const PARAM_CTX_PPQ: u32 = 212;
pub const PARAM_CTX_SAMPLES: u32 = 213;
pub const PARAM_CTX_SIGNATURE: u32 = 214;
pub const PARAM_CTX_BAR: u32 = 215;
pub const PARAM_CTX_CYCLE_START: u32 = 216;
pub const PARAM_CTX_CYCLE_END: u32 = 217;
/// Reserved for probes of an editor view (the plug-in has none yet): read-only, always 0.
pub const PARAM_EDITOR_STATE: u32 = 220;
pub const PARAM_EDITOR_WIDTH: u32 = 221;

/// Sine instrument.
pub const PARAM_LEVEL: u32 = 1;
/// Echo note events to the event output bus; pitch bend comes back as a legacy MIDI event.
pub const PARAM_THRU: u32 = 2;
/// Mapped to MIDI pitch bend through `IMidiMapping`.
pub const PARAM_BEND: u32 = 3;
/// Mapped to channel pressure through `IMidiMapping`; scales the level ([`pressure_gain`]).
pub const PARAM_PRESSURE: u32 = 4;
/// Mapped to controller 74 (brightness) through `IMidiMapping`; no effect on the sound.
pub const PARAM_BRIGHTNESS: u32 = 5;
/// Read-only: the last poly pressure event, `(pitch * 128 + pressure 0..=127) / 16383`.
pub const PARAM_POLY_PRESSURE: u32 = 6;

/// Programs of the gain effect: name and the normalized gain each one sets.
pub const PROGRAMS: [(&str, f64); 3] = [("Unity", 0.5), ("Half", 0.25), ("Double", 1.0)];
pub const PROGRAM_LIST_ID: i32 = 7;

const CAN_AUTOMATE: i32 = ParameterInfo_::ParameterFlags_::kCanAutomate as i32;
const READ_ONLY: i32 = ParameterInfo_::ParameterFlags_::kIsReadOnly as i32;
const IS_LIST: i32 = ParameterInfo_::ParameterFlags_::kIsList as i32;
const IS_HIDDEN: i32 = ParameterInfo_::ParameterFlags_::kIsHidden as i32;
const IS_PROGRAM: i32 = ParameterInfo_::ParameterFlags_::kIsProgramChange as i32;
const IS_BYPASS: i32 = ParameterInfo_::ParameterFlags_::kIsBypass as i32;

const UNIT_ROOT: i32 = 0;
const UNIT_TEST: i32 = 1;
const UNIT_PROBE: i32 = 2;

struct ParamDef {
    id: u32,
    title: &'static str,
    short: &'static str,
    units: &'static str,
    steps: i32,
    default: f64,
    flags: i32,
    unit: i32,
}

#[allow(clippy::too_many_arguments)]
const fn def(
    id: u32,
    title: &'static str,
    short: &'static str,
    units: &'static str,
    steps: i32,
    default: f64,
    flags: i32,
    unit: i32,
) -> ParamDef {
    ParamDef { id, title, short, units, steps, default, flags, unit }
}

const PROBE: i32 = READ_ONLY | IS_HIDDEN;

static GAIN_PARAMS: [ParamDef; 21] = [
    def(PARAM_GAIN, "Gain", "Gn", "x", 0, 0.5, CAN_AUTOMATE, UNIT_ROOT),
    def(PARAM_BYPASS, "Bypass", "Byp", "", 1, 0.0, CAN_AUTOMATE | IS_BYPASS, UNIT_ROOT),
    def(PARAM_LATENCY, "Latency", "Lat", "smp", 0, 0.0, 0, UNIT_ROOT),
    def(PARAM_CRASH, "Crash", "Crash", "", 2, 0.0, IS_LIST, UNIT_TEST),
    def(PARAM_HANG, "Hang", "Hang", "", 2, 0.0, IS_LIST, UNIT_TEST),
    def(PARAM_SIDECHAIN, "Sidechain", "SC", "", 0, 0.0, CAN_AUTOMATE, UNIT_ROOT),
    def(PARAM_PROGRAM, "Program", "Prg", "", 2, 0.0, CAN_AUTOMATE | IS_LIST | IS_PROGRAM, UNIT_ROOT),
    def(PARAM_UI_ZOOM, "UI Zoom", "Zoom", "", 0, 0.5, IS_HIDDEN, UNIT_TEST),
    def(PARAM_EDIT_TRIGGER, "Edit Trigger", "Edit", "", 0, 0.0, IS_HIDDEN, UNIT_TEST),
    def(PARAM_METER, "Meter", "Mtr", "", 0, 0.0, READ_ONLY, UNIT_ROOT),
    def(PARAM_OFFLINE, "Offline", "Offl", "", 1, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_STATE, "Context State", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_TEMPO, "Context Tempo", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_PPQ, "Context PPQ", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_SAMPLES, "Context Samples", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_SIGNATURE, "Context Signature", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_BAR, "Context Bar", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_CYCLE_START, "Context Cycle Start", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_CTX_CYCLE_END, "Context Cycle End", "", "", 0, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_EDITOR_STATE, "Editor State", "", "", 2, 0.0, PROBE, UNIT_PROBE),
    def(PARAM_EDITOR_WIDTH, "Editor Width", "", "", 0, 0.0, PROBE, UNIT_PROBE),
];

static SINE_PARAMS: [ParamDef; 6] = [
    def(PARAM_LEVEL, "Level", "Lvl", "%", 0, 0.5, CAN_AUTOMATE, UNIT_ROOT),
    def(PARAM_THRU, "Thru", "Thru", "", 1, 0.0, 0, UNIT_ROOT),
    def(PARAM_BEND, "Bend", "Bend", "", 0, 0.5, CAN_AUTOMATE, UNIT_ROOT),
    def(PARAM_PRESSURE, "Pressure", "Prs", "", 0, 0.0, CAN_AUTOMATE, UNIT_ROOT),
    def(PARAM_BRIGHTNESS, "Brightness", "Brt", "", 0, 0.0, CAN_AUTOMATE, UNIT_ROOT),
    def(PARAM_POLY_PRESSURE, "Poly Pressure", "Poly", "", 0, 0.0, READ_ONLY, UNIT_ROOT),
];

/// The factor the sine instrument's Pressure parameter (normalized) multiplies its level by.
pub fn pressure_gain(normalized: f64) -> f64 {
    1.0 + normalized
}

/// The factor the gain effect multiplies by for a normalized parameter value.
pub fn gain_factor(normalized: f64) -> f32 {
    (2.0 * normalized) as f32
}

/// The latency in frames the gain effect reports for a normalized parameter value.
pub fn latency_frames(normalized: f64) -> u32 {
    (normalized.clamp(0.0, 1.0) * 1024.0).round() as u32
}

/// The sample the sine instrument produces `k` frames after a note-on (the formula of the
/// other test plug-ins, so the tests can compare bit for bit).
pub fn sine_sample(note: u8, k: u64, sample_rate: f64, level: f64) -> f32 {
    let frequency = 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
    (level * (std::f64::consts::TAU * frequency * k as f64 / sample_rate).sin()) as f32
}

/// Values of a parameter table, shared between threads.
struct Values<const N: usize> {
    defs: &'static [ParamDef; N],
    values: [AtomicU64; N],
}

impl<const N: usize> Values<N> {
    fn new(defs: &'static [ParamDef; N]) -> Values<N> {
        Values { defs, values: std::array::from_fn(|index| AtomicU64::new(defs[index].default.to_bits())) }
    }
    fn slot(&self, id: u32) -> Option<usize> {
        self.defs.iter().position(|def| def.id == id)
    }
    fn get(&self, id: u32) -> f64 {
        self.slot(id).map_or(0.0, |slot| f64::from_bits(self.values[slot].load(Ordering::Relaxed)))
    }
    /// Stores the value; true if it differs from the previous one.
    fn set(&self, id: u32, value: f64) -> bool {
        match self.slot(id) {
            Some(slot) => self.values[slot].swap(value.to_bits(), Ordering::Relaxed) != value.to_bits(),
            None => false,
        }
    }
}

fn list_index(normalized: f64, steps: i32) -> i32 {
    ((normalized.clamp(0.0, 1.0) * steps as f64).round() as i32).clamp(0, steps)
}

// ---- strings and streams --------------------------------------------------------------------

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let bytes = src.as_bytes();
    let count = bytes.len().min(dst.len() - 1);
    for (target, &byte) in dst.iter_mut().zip(&bytes[..count]) {
        *target = byte as c_char;
    }
    dst[count] = 0;
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for unit in src.encode_utf16().take(dst.len() - 1) {
        dst[len] = unit as TChar;
        len += 1;
    }
    dst[len] = 0;
}

unsafe fn wide_string(ptr: *const TChar) -> String {
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr as *const u16, len))
}

unsafe fn stream_write(stream: *mut IBStream, bytes: &[u8]) -> bool {
    let Some(stream) = ComRef::from_raw(stream) else { return false };
    let mut written = 0;
    stream.write(bytes.as_ptr() as *mut c_void, bytes.len() as i32, &mut written) == kResultOk
        && written == bytes.len() as i32
}

/// Reads up to `max` bytes from the current position.
unsafe fn stream_read(stream: *mut IBStream, max: usize) -> Vec<u8> {
    let Some(stream) = ComRef::from_raw(stream) else { return Vec::new() };
    let mut out = vec![0u8; max];
    let mut total = 0;
    while total < max {
        let mut got = 0;
        let result = stream.read(out[total..].as_mut_ptr() as *mut c_void, (max - total) as i32, &mut got);
        if result != kResultOk || got <= 0 {
            break;
        }
        total += got as usize;
    }
    out.truncate(total);
    out
}

fn f64_at(bytes: &[u8], offset: usize) -> f64 {
    f64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

// ---- module bookkeeping -----------------------------------------------------------------------

/// Plug-in objects (processors, controllers, views) currently alive.
static LIVE_OBJECTS: AtomicUsize = AtomicUsize::new(0);

fn marker(line: &str) {
    if let Some(path) = std::env::var_os("STPC_VST3_TEST_MARKER") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// File name of this library as it was loaded (the tests copy it under telling names).
fn module_file_name() -> String {
    #[cfg(unix)]
    // SAFETY: dladdr only reads; the info strings belong to the loader.
    unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(GetPluginFactory as *const c_void, &mut info) != 0 && !info.dli_fname.is_null() {
            let path = CStr::from_ptr(info.dli_fname).to_string_lossy().into_owned();
            return path.rsplit('/').next().unwrap_or("").to_string();
        }
    }
    String::new()
}

fn crash(kind: i32) {
    match kind {
        1 => std::process::abort(),
        // SAFETY: none; this is the deliberate fault.
        2 => unsafe { std::ptr::null_mut::<u32>().write_volatile(0xDEAD) },
        _ => {}
    }
}

fn hang() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn module_entry() -> bool {
    let name = module_file_name();
    if name.contains("CrashOnLoad") {
        std::process::abort();
    }
    marker("entry");
    !name.contains("EntryFails")
}

fn module_exit() -> bool {
    marker(&format!("exit live={}", LIVE_OBJECTS.load(Ordering::SeqCst)));
    true
}

// ---- the gain effect: processor ---------------------------------------------------------------

const GAIN_STATE_MAGIC: &[u8; 4] = b"STG3";
/// Size of the gain effect's component state.
pub const GAIN_STATE_LEN: usize = 48;
const GAIN_CONTROLLER_MAGIC: &[u8; 4] = b"STGC";
/// Size of the gain effect's controller state.
pub const GAIN_CONTROLLER_STATE_LEN: usize = 16;

struct GainProcessor {
    values: Values<21>,
    /// Last value sent for each output parameter (NaN: never).
    sent: [AtomicU64; 21],
    latency: AtomicU32,
    latency_pending: AtomicU32,
    active: AtomicBool,
    offline: AtomicBool,
    main_arrangement: AtomicU64,
    sidechain_arrangement: AtomicU64,
    sidechain_active: AtomicBool,
    peer: RefCell<Option<ComPtr<IConnectionPoint>>>,
}

impl Class for GainProcessor {
    type Interfaces = (IComponent, IAudioProcessor, IConnectionPoint);
}

impl Drop for GainProcessor {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn channels_of(arrangement: SpeakerArrangement) -> i32 {
    arrangement.count_ones() as i32
}

impl GainProcessor {
    fn new() -> GainProcessor {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        GainProcessor {
            values: Values::new(&GAIN_PARAMS),
            sent: std::array::from_fn(|_| AtomicU64::new(f64::NAN.to_bits())),
            latency: AtomicU32::new(0),
            latency_pending: AtomicU32::new(0),
            active: AtomicBool::new(false),
            offline: AtomicBool::new(false),
            main_arrangement: AtomicU64::new(SpeakerArr::kStereo),
            sidechain_arrangement: AtomicU64::new(SpeakerArr::kStereo),
            sidechain_active: AtomicBool::new(false),
            peer: RefCell::new(None),
        }
    }

    fn set_latency(&self, frames: u32) {
        self.latency_pending.store(frames, Ordering::Relaxed);
        if !self.active.load(Ordering::Relaxed) {
            self.latency.store(frames, Ordering::Relaxed);
        }
    }

    /// A parameter value arrives (from a queue in `process`, or from a state).
    fn apply(&self, id: u32, value: f64, in_process: bool) {
        self.values.set(id, value);
        match id {
            PARAM_LATENCY => self.set_latency(latency_frames(value)),
            PARAM_PROGRAM => {
                self.values.set(PARAM_GAIN, PROGRAMS[list_index(value, 2) as usize].1);
            }
            PARAM_CRASH if in_process => crash(list_index(value, 2)),
            PARAM_HANG if in_process && list_index(value, 2) == 1 => hang(),
            _ => {}
        }
    }

    unsafe fn send_output(&self, changes: *mut IParameterChanges, id: u32, value: f64, offset: i32) {
        let Some(slot) = self.values.slot(id) else { return };
        if self.sent[slot].load(Ordering::Relaxed) == value.to_bits() {
            return;
        }
        let Some(changes) = ComRef::from_raw(changes) else { return };
        let mut index = 0;
        if let Some(queue) = ComRef::from_raw(changes.addParameterData(&id, &mut index)) {
            if queue.addPoint(offset, value, &mut index) == kResultOk {
                self.sent[slot].store(value.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

impl IPluginBaseTrait for GainProcessor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        *self.peer.borrow_mut() = None;
        kResultOk
    }
}

unsafe fn fill_bus(bus: *mut BusInfo, media: MediaTypes, input: bool, channels: i32, name: &str, main: bool) {
    let bus = &mut *bus;
    bus.mediaType = media as MediaType;
    bus.direction = if input { BusDirections_::kInput } else { BusDirections_::kOutput } as BusDirection;
    bus.channelCount = channels;
    copy_wstring(name, &mut bus.name);
    bus.busType = if main { BusTypes_::kMain } else { BusTypes_::kAux } as BusType;
    bus.flags = if main { BusInfo_::BusFlags_::kDefaultActive as u32 } else { 0 };
}

impl IComponentTrait for GainProcessor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        *class_id = tuid(GAIN_CONTROLLER_UID);
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kNotImplemented
    }

    unsafe fn getBusCount(&self, media: MediaType, direction: BusDirection) -> i32 {
        if media != MediaTypes_::kAudio as MediaType {
            return 0;
        }
        if direction == BusDirections_::kInput as BusDirection {
            2
        } else {
            1
        }
    }

    unsafe fn getBusInfo(&self, media: MediaType, direction: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if media != MediaTypes_::kAudio as MediaType {
            return kInvalidArgument;
        }
        let main = channels_of(self.main_arrangement.load(Ordering::Relaxed));
        let input = direction == BusDirections_::kInput as BusDirection;
        match (input, index) {
            (true, 0) => fill_bus(bus, MediaTypes_::kAudio, true, main, "Audio In", true),
            (true, 1) => {
                let channels = channels_of(self.sidechain_arrangement.load(Ordering::Relaxed));
                fill_bus(bus, MediaTypes_::kAudio, true, channels, "Sidechain", false)
            }
            (false, 0) => fill_bus(bus, MediaTypes_::kAudio, false, main, "Audio Out", true),
            _ => return kInvalidArgument,
        }
        kResultOk
    }

    unsafe fn getRoutingInfo(&self, _in_info: *mut RoutingInfo, _out_info: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(&self, media: MediaType, direction: BusDirection, index: i32, state: TBool) -> tresult {
        if media != MediaTypes_::kAudio as MediaType {
            return kInvalidArgument;
        }
        if direction == BusDirections_::kInput as BusDirection && index == 1 {
            self.sidechain_active.store(state != 0, Ordering::Relaxed);
        }
        kResultOk
    }

    unsafe fn setActive(&self, state: TBool) -> tresult {
        self.active.store(state != 0, Ordering::Relaxed);
        if state != 0 {
            // The latency a host was told about with kLatencyChanged becomes effective here.
            self.latency.store(self.latency_pending.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let bytes = stream_read(state, GAIN_STATE_LEN + 1);
        if bytes.len() != GAIN_STATE_LEN || &bytes[..4] != GAIN_STATE_MAGIC || bytes[4..8] != 1u32.to_le_bytes() {
            return kResultFalse;
        }
        for (index, id) in [PARAM_GAIN, PARAM_BYPASS, PARAM_LATENCY, PARAM_SIDECHAIN, PARAM_PROGRAM].iter().enumerate()
        {
            let value = f64_at(&bytes, 8 + index * 8);
            if *id == PARAM_PROGRAM {
                self.values.set(*id, value); // the program's gain is in the state already
            } else {
                self.apply(*id, value, false);
            }
        }
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let mut bytes = Vec::with_capacity(GAIN_STATE_LEN);
        bytes.extend_from_slice(GAIN_STATE_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for id in [PARAM_GAIN, PARAM_BYPASS, PARAM_LATENCY, PARAM_SIDECHAIN, PARAM_PROGRAM] {
            bytes.extend_from_slice(&self.values.get(id).to_le_bytes());
        }
        if stream_write(state, &bytes) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

const MAX_QUEUES: usize = 32;

/// Cursor over the parameter queues of a block, yielding points in offset order.
struct QueueReader<'a> {
    queues: [Option<ComRef<'a, IParamValueQueue>>; MAX_QUEUES],
    ids: [u32; MAX_QUEUES],
    counts: [i32; MAX_QUEUES],
    cursors: [i32; MAX_QUEUES],
    next: [i32; MAX_QUEUES],
    len: usize,
}

impl<'a> QueueReader<'a> {
    unsafe fn new(changes: *mut IParameterChanges) -> QueueReader<'a> {
        let mut reader = QueueReader {
            queues: [None; MAX_QUEUES],
            ids: [0; MAX_QUEUES],
            counts: [0; MAX_QUEUES],
            cursors: [0; MAX_QUEUES],
            next: [i32::MAX; MAX_QUEUES],
            len: 0,
        };
        let Some(changes) = ComRef::<'a, IParameterChanges>::from_raw(changes) else { return reader };
        for index in 0..changes.getParameterCount() {
            if reader.len == MAX_QUEUES {
                break;
            }
            let Some(queue) = ComRef::<'a, IParamValueQueue>::from_raw(changes.getParameterData(index)) else {
                continue;
            };
            let slot = reader.len;
            reader.ids[slot] = queue.getParameterId();
            reader.counts[slot] = queue.getPointCount();
            reader.queues[slot] = Some(queue);
            reader.len += 1;
            reader.peek(slot);
        }
        reader
    }

    unsafe fn peek(&mut self, slot: usize) {
        self.next[slot] = i32::MAX;
        if self.cursors[slot] < self.counts[slot] {
            let (mut offset, mut value) = (0, 0.0);
            if self.queues[slot].unwrap().getPoint(self.cursors[slot], &mut offset, &mut value) == kResultOk {
                self.next[slot] = offset;
            }
        }
    }

    /// Calls `apply(id, value)` for every point at or before `frame` (`None`: all that is left).
    unsafe fn advance(&mut self, frame: Option<i32>, mut apply: impl FnMut(u32, f64)) -> bool {
        let mut any = false;
        for slot in 0..self.len {
            while self.cursors[slot] < self.counts[slot] && frame.is_none_or(|frame| self.next[slot] <= frame) {
                let (mut offset, mut value) = (0, 0.0);
                if self.queues[slot].unwrap().getPoint(self.cursors[slot], &mut offset, &mut value) == kResultOk {
                    apply(self.ids[slot], value);
                    any = true;
                }
                self.cursors[slot] += 1;
                self.peek(slot);
            }
        }
        any
    }
}

unsafe fn bus_channels<'a>(buses: *mut AudioBusBuffers, count: i32, index: usize) -> &'a [*mut f32] {
    if buses.is_null() || index >= count.max(0) as usize {
        return &[];
    }
    let bus = &*buses.add(index);
    if bus.numChannels <= 0 || bus.__field0.channelBuffers32.is_null() {
        return &[];
    }
    std::slice::from_raw_parts(bus.__field0.channelBuffers32, bus.numChannels as usize)
}

impl IAudioProcessorTrait for GainProcessor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        if num_ins != 2 || num_outs != 1 {
            return kResultFalse;
        }
        let (main, sidechain, out) = (*inputs, *inputs.add(1), *outputs);
        let allowed = |arrangement| arrangement == SpeakerArr::kMono || arrangement == SpeakerArr::kStereo;
        if !allowed(main) || !allowed(sidechain) || out != main {
            return kResultFalse;
        }
        self.main_arrangement.store(main, Ordering::Relaxed);
        self.sidechain_arrangement.store(sidechain, Ordering::Relaxed);
        kResultTrue
    }

    unsafe fn getBusArrangement(&self, direction: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        let input = direction == BusDirections_::kInput as BusDirection;
        *arr = match (input, index) {
            (_, 0) => self.main_arrangement.load(Ordering::Relaxed),
            (true, 1) => self.sidechain_arrangement.load(Ordering::Relaxed),
            _ => return kInvalidArgument,
        };
        kResultOk
    }

    unsafe fn canProcessSampleSize(&self, symbolic_sample_size: i32) -> tresult {
        if symbolic_sample_size == SymbolicSampleSizes_::kSample32 as i32 {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        self.latency.load(Ordering::Relaxed)
    }

    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        if self.active.load(Ordering::Relaxed) {
            return kResultFalse; // a host must not set up an active component
        }
        self.offline.store((*setup).processMode == ProcessModes_::kOffline as i32, Ordering::Relaxed);
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = &mut *data;
        if !self.active.load(Ordering::Relaxed) {
            return kResultFalse;
        }
        let frames = data.numSamples.max(0) as usize;
        let mut reader = QueueReader::new(data.inputParameterChanges);
        let inputs = bus_channels(data.inputs, data.numInputs, 0);
        let sidechain = bus_channels(data.inputs, data.numInputs, 1);
        let outputs = bus_channels(data.outputs, data.numOutputs, 0);
        let mut peak = 0.0f32;
        let (mut gain, mut bypass, mut mix) = (0.0f32, false, 0.0f32);
        let mut refresh = true;
        for frame in 0..frames {
            refresh |= reader.advance(Some(frame as i32), |id, value| self.apply(id, value, true));
            if refresh {
                gain = gain_factor(self.values.get(PARAM_GAIN));
                bypass = self.values.get(PARAM_BYPASS) >= 0.5;
                mix = if self.sidechain_active.load(Ordering::Relaxed) && !sidechain.is_empty() {
                    self.values.get(PARAM_SIDECHAIN) as f32
                } else {
                    0.0
                };
                refresh = false;
            }
            for (channel, &out) in outputs.iter().enumerate() {
                let dry = if inputs.is_empty() { 0.0 } else { *inputs[channel.min(inputs.len() - 1)].add(frame) };
                let mut sample = if bypass { dry } else { dry * gain };
                if !bypass && mix != 0.0 {
                    sample += *sidechain[channel.min(sidechain.len() - 1)].add(frame) * mix;
                }
                *out.add(frame) = sample;
                if channel == 0 {
                    peak = peak.max(sample.abs());
                }
            }
        }
        // Points past the block (and everything in a zero-sample flush).
        reader.advance(None, |id, value| self.apply(id, value, true));
        if data.numOutputs > 0 && !data.outputs.is_null() {
            (*data.outputs).silenceFlags = 0;
        }

        if frames > 0 {
            let last = frames as i32 - 1;
            let out = data.outputParameterChanges;
            self.send_output(out, PARAM_METER, (peak as f64).min(1.0), last);
            let offline = data.processMode == ProcessModes_::kOffline as i32 && self.offline.load(Ordering::Relaxed);
            self.send_output(out, PARAM_OFFLINE, offline as u8 as f64, 0);
            if let Some(context) = data.processContext.as_ref() {
                self.send_output(out, PARAM_CTX_STATE, context.state as f64 / (1u64 << 20) as f64, 0);
                self.send_output(out, PARAM_CTX_TEMPO, context.tempo / 1000.0, 0);
                self.send_output(out, PARAM_CTX_PPQ, context.projectTimeMusic / 1e6, 0);
                self.send_output(out, PARAM_CTX_SAMPLES, context.projectTimeSamples as f64 / (1u64 << 40) as f64, 0);
                let signature = (context.timeSigNumerator * 256 + context.timeSigDenominator) as f64 / 65536.0;
                self.send_output(out, PARAM_CTX_SIGNATURE, signature, 0);
                self.send_output(out, PARAM_CTX_BAR, context.barPositionMusic / 1e6, 0);
                self.send_output(out, PARAM_CTX_CYCLE_START, context.cycleStartMusic / 1e6, 0);
                self.send_output(out, PARAM_CTX_CYCLE_END, context.cycleEndMusic / 1e6, 0);
            }
        }
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        kNoTail
    }
}

impl IConnectionPointTrait for GainProcessor {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        *self.peer.borrow_mut() = ComRef::from_raw(other).map(|other| other.to_com_ptr());
        kResultOk
    }

    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        *self.peer.borrow_mut() = None;
        kResultOk
    }

    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        let Some(message) = ComRef::from_raw(message) else { return kInvalidArgument };
        let id = message.getMessageID();
        if id.is_null() || CStr::from_ptr(id) != c"latency" {
            return kResultFalse;
        }
        let Some(attributes) = ComRef::from_raw(message.getAttributes()) else { return kResultFalse };
        let mut frames = 0i64;
        if attributes.getInt(c"frames".as_ptr(), &mut frames) != kResultOk {
            return kResultFalse;
        }
        self.set_latency(frames.clamp(0, 1 << 20) as u32);
        kResultOk
    }
}

// ---- the gain effect: controller ------------------------------------------------------------

struct GainController {
    values: Values<21>,
    host: RefCell<Option<ComPtr<IHostApplication>>>,
    handler: RefCell<Option<ComPtr<IComponentHandler>>>,
    peer: RefCell<Option<ComPtr<IConnectionPoint>>>,
}

impl Class for GainController {
    type Interfaces = (IEditController, IConnectionPoint, IUnitInfo);
}

impl Drop for GainController {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl GainController {
    fn new() -> GainController {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        GainController {
            values: Values::new(&GAIN_PARAMS),
            host: RefCell::new(None),
            handler: RefCell::new(None),
            peer: RefCell::new(None),
        }
    }

    fn handler(&self) -> Option<ComPtr<IComponentHandler>> {
        self.handler.borrow().clone()
    }

    unsafe fn restart(&self, flags: i32) {
        if let Some(handler) = self.handler() {
            handler.restartComponent(flags);
        }
    }

    /// Tells the processor its new latency through the host's message objects.
    unsafe fn send_latency(&self, frames: u32) {
        let (Some(host), Some(peer)) = (self.host.borrow().clone(), self.peer.borrow().clone()) else { return };
        let mut class_id = IMessage_iid;
        let mut interface_id = IMessage_iid;
        let mut object: *mut c_void = std::ptr::null_mut();
        if host.createInstance(&mut class_id, &mut interface_id, &mut object) != kResultOk {
            return;
        }
        let Some(message) = ComPtr::<IMessage>::from_raw(object as *mut IMessage) else { return };
        message.setMessageID(c"latency".as_ptr());
        if let Some(attributes) = ComRef::from_raw(message.getAttributes()) {
            attributes.setInt(c"frames".as_ptr(), frames as i64);
            peer.notify(message.as_ptr());
        }
    }

    fn text(&self, id: u32, value: f64) -> Option<String> {
        let def = &GAIN_PARAMS[self.values.slot(id)?];
        Some(match id {
            PARAM_GAIN => format!("{:.2} x", 2.0 * value),
            PARAM_LATENCY => format!("{} smp", latency_frames(value)),
            PARAM_BYPASS | PARAM_OFFLINE => if value >= 0.5 { "On" } else { "Off" }.to_string(),
            PARAM_CRASH => ["No", "Abort", "Segfault"][list_index(value, 2) as usize].to_string(),
            PARAM_HANG => ["No", "Process", "Main"][list_index(value, 2) as usize].to_string(),
            PARAM_PROGRAM => PROGRAMS[list_index(value, 2) as usize].0.to_string(),
            _ if def.steps > 0 => list_index(value, def.steps).to_string(),
            _ => format!("{value:.3}"),
        })
    }
}

impl IPluginBaseTrait for GainController {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        *self.host.borrow_mut() = ComRef::from_raw(context).and_then(|context| context.cast::<IHostApplication>());
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        *self.host.borrow_mut() = None;
        *self.handler.borrow_mut() = None;
        *self.peer.borrow_mut() = None;
        kResultOk
    }
}

unsafe fn fill_parameter(def: &ParamDef, info: *mut ParameterInfo) {
    let info = &mut *info;
    info.id = def.id;
    copy_wstring(def.title, &mut info.title);
    copy_wstring(def.short, &mut info.shortTitle);
    copy_wstring(def.units, &mut info.units);
    info.stepCount = def.steps;
    info.defaultNormalizedValue = def.default;
    info.unitId = def.unit;
    info.flags = def.flags;
}

impl IEditControllerTrait for GainController {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let bytes = stream_read(state, GAIN_STATE_LEN + 1);
        if bytes.len() != GAIN_STATE_LEN || &bytes[..4] != GAIN_STATE_MAGIC {
            return kResultFalse;
        }
        let mut latency_changed = false;
        for (index, id) in [PARAM_GAIN, PARAM_BYPASS, PARAM_LATENCY, PARAM_SIDECHAIN, PARAM_PROGRAM].iter().enumerate()
        {
            let changed = self.values.set(*id, f64_at(&bytes, 8 + index * 8));
            latency_changed |= changed && *id == PARAM_LATENCY;
        }
        if latency_changed {
            self.restart(RestartFlags_::kLatencyChanged as i32);
        }
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let bytes = stream_read(state, GAIN_CONTROLLER_STATE_LEN + 1);
        if bytes.len() != GAIN_CONTROLLER_STATE_LEN || &bytes[..4] != GAIN_CONTROLLER_MAGIC {
            return kResultFalse;
        }
        self.values.set(PARAM_UI_ZOOM, f64_at(&bytes, 8));
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let mut bytes = Vec::with_capacity(GAIN_CONTROLLER_STATE_LEN);
        bytes.extend_from_slice(GAIN_CONTROLLER_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&self.values.get(PARAM_UI_ZOOM).to_le_bytes());
        if stream_write(state, &bytes) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getParameterCount(&self) -> i32 {
        GAIN_PARAMS.len() as i32
    }

    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        match GAIN_PARAMS.get(index as usize) {
            Some(def) if index >= 0 => {
                fill_parameter(def, info);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getParamStringByValue(&self, id: u32, value: f64, string: *mut String128) -> tresult {
        match self.text(id, value) {
            Some(text) => {
                copy_wstring(&text, &mut *string);
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn getParamValueByString(&self, id: u32, string: *mut TChar, value: *mut f64) -> tresult {
        let text = wide_string(string);
        let parsed = match id {
            PARAM_GAIN => text.strip_suffix(" x").and_then(|number| number.trim().parse::<f64>().ok()).map(|x| x / 2.0),
            PARAM_LATENCY => {
                text.strip_suffix(" smp").and_then(|number| number.trim().parse::<f64>().ok()).map(|x| x / 1024.0)
            }
            _ => None,
        };
        match parsed {
            Some(parsed) if (0.0..=1.0).contains(&parsed) => {
                *value = parsed;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn normalizedParamToPlain(&self, id: u32, value: f64) -> f64 {
        match self.values.slot(id).map(|slot| &GAIN_PARAMS[slot]) {
            Some(def) if def.steps > 0 => list_index(value, def.steps) as f64,
            Some(_) if id == PARAM_GAIN => 2.0 * value,
            Some(_) if id == PARAM_LATENCY => latency_frames(value) as f64,
            _ => value,
        }
    }

    unsafe fn plainParamToNormalized(&self, id: u32, plain: f64) -> f64 {
        match self.values.slot(id).map(|slot| &GAIN_PARAMS[slot]) {
            Some(def) if def.steps > 0 => plain / def.steps as f64,
            Some(_) if id == PARAM_GAIN => plain / 2.0,
            Some(_) if id == PARAM_LATENCY => plain / 1024.0,
            _ => plain,
        }
    }

    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        self.values.get(id)
    }

    unsafe fn setParamNormalized(&self, id: u32, value: f64) -> tresult {
        if self.values.slot(id).is_none() {
            return kInvalidArgument;
        }
        let changed = self.values.set(id, value);
        match id {
            PARAM_LATENCY if changed => {
                self.send_latency(latency_frames(value));
                self.restart(RestartFlags_::kLatencyChanged as i32);
            }
            PARAM_PROGRAM if changed => {
                self.values.set(PARAM_GAIN, PROGRAMS[list_index(value, 2) as usize].1);
                self.restart(RestartFlags_::kParamValuesChanged as i32);
            }
            PARAM_HANG if list_index(value, 2) == 2 => hang(),
            PARAM_EDIT_TRIGGER if changed => {
                // What an editor does when the user drags the gain knob.
                if let Some(handler) = self.handler() {
                    handler.beginEdit(PARAM_GAIN);
                    self.values.set(PARAM_GAIN, value);
                    handler.performEdit(PARAM_GAIN, value);
                    handler.endEdit(PARAM_GAIN);
                    if let Some(handler2) = handler.cast::<IComponentHandler2>() {
                        handler2.setDirty(1);
                    }
                }
            }
            _ => {}
        }
        kResultOk
    }

    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        *self.handler.borrow_mut() = ComRef::from_raw(handler).map(|handler| handler.to_com_ptr());
        kResultOk
    }

    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

impl IConnectionPointTrait for GainController {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        *self.peer.borrow_mut() = ComRef::from_raw(other).map(|other| other.to_com_ptr());
        kResultOk
    }
    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        *self.peer.borrow_mut() = None;
        kResultOk
    }
    unsafe fn notify(&self, _message: *mut IMessage) -> tresult {
        kResultFalse
    }
}

impl IUnitInfoTrait for GainController {
    unsafe fn getUnitCount(&self) -> i32 {
        3
    }

    unsafe fn getUnitInfo(&self, index: i32, info: *mut UnitInfo) -> tresult {
        let (id, parent, name, list) = match index {
            0 => (UNIT_ROOT, kNoParentUnitId, "Root", PROGRAM_LIST_ID),
            1 => (UNIT_TEST, UNIT_ROOT, "Test", kNoProgramListId),
            2 => (UNIT_PROBE, UNIT_TEST, "Probe", kNoProgramListId),
            _ => return kInvalidArgument,
        };
        let info = &mut *info;
        info.id = id;
        info.parentUnitId = parent;
        copy_wstring(name, &mut info.name);
        info.programListId = list;
        kResultOk
    }

    unsafe fn getProgramListCount(&self) -> i32 {
        1
    }

    unsafe fn getProgramListInfo(&self, index: i32, info: *mut ProgramListInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.id = PROGRAM_LIST_ID;
        copy_wstring("Factory", &mut info.name);
        info.programCount = PROGRAMS.len() as i32;
        kResultOk
    }

    unsafe fn getProgramName(&self, list: ProgramListID, index: i32, name: *mut String128) -> tresult {
        match PROGRAMS.get(index as usize) {
            Some(program) if list == PROGRAM_LIST_ID && index >= 0 => {
                copy_wstring(program.0, &mut *name);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getProgramInfo(
        &self,
        _list: ProgramListID,
        _index: i32,
        _attribute: *const c_char,
        _value: *mut String128,
    ) -> tresult {
        kResultFalse
    }
    unsafe fn hasProgramPitchNames(&self, _list: ProgramListID, _index: i32) -> tresult {
        kResultFalse
    }
    unsafe fn getProgramPitchName(
        &self,
        _list: ProgramListID,
        _index: i32,
        _pitch: i16,
        _name: *mut String128,
    ) -> tresult {
        kResultFalse
    }
    unsafe fn getSelectedUnit(&self) -> UnitID {
        UNIT_ROOT
    }
    unsafe fn selectUnit(&self, _unit: UnitID) -> tresult {
        kResultOk
    }
    unsafe fn getUnitByBus(
        &self,
        _media: MediaType,
        _direction: BusDirection,
        _bus: i32,
        _channel: i32,
        _unit: *mut UnitID,
    ) -> tresult {
        kResultFalse
    }
    unsafe fn setUnitProgramData(&self, _list_or_unit: i32, _index: i32, _data: *mut IBStream) -> tresult {
        kNotImplemented
    }
}

// ---- the sine instrument: one object for processor and controller ----------------------------

const SINE_STATE_MAGIC: &[u8; 4] = b"STS3";
/// Size of the sine instrument's component state.
pub const SINE_STATE_LEN: usize = 24;
/// Tail the sine instrument reports (the VST3 "infinite tail" value).
pub const SINE_TAIL: u32 = kInfiniteTail;

struct SineSynth {
    /// What the processor half uses.
    values: Values<6>,
    /// What the controller half shows (a host keeps the two in step).
    shown: Values<6>,
    sample_rate: AtomicU64,
    active: AtomicBool,
    /// Sounding note, or -1.
    note: AtomicI32,
    age: AtomicU64,
    mono: AtomicBool,
    handler: RefCell<Option<ComPtr<IComponentHandler>>>,
    initialized: AtomicU32,
}

impl Class for SineSynth {
    type Interfaces = (IComponent, IAudioProcessor, IEditController, IMidiMapping);
}

impl Drop for SineSynth {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SineSynth {
    fn new() -> SineSynth {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        SineSynth {
            values: Values::new(&SINE_PARAMS),
            shown: Values::new(&SINE_PARAMS),
            sample_rate: AtomicU64::new(48_000f64.to_bits()),
            active: AtomicBool::new(false),
            note: AtomicI32::new(-1),
            age: AtomicU64::new(0),
            mono: AtomicBool::new(false),
            handler: RefCell::new(None),
            initialized: AtomicU32::new(0),
        }
    }
}

impl IPluginBaseTrait for SineSynth {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        // One object: a host must initialize it once, not once per interface.
        if self.initialized.fetch_add(1, Ordering::Relaxed) == 0 {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn terminate(&self) -> tresult {
        *self.handler.borrow_mut() = None;
        kResultOk
    }
}

impl IComponentTrait for SineSynth {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kNotImplemented
    }

    unsafe fn getBusCount(&self, media: MediaType, direction: BusDirection) -> i32 {
        let input = direction == BusDirections_::kInput as BusDirection;
        if media == MediaTypes_::kAudio as MediaType {
            !input as i32
        } else {
            1
        }
    }

    unsafe fn getBusInfo(&self, media: MediaType, direction: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let input = direction == BusDirections_::kInput as BusDirection;
        if media == MediaTypes_::kAudio as MediaType {
            if input {
                return kInvalidArgument;
            }
            let channels = if self.mono.load(Ordering::Relaxed) { 1 } else { 2 };
            fill_bus(bus, MediaTypes_::kAudio, false, channels, "Audio Out", true);
        } else {
            fill_bus(bus, MediaTypes_::kEvent, input, 16, if input { "MIDI In" } else { "MIDI Out" }, true);
        }
        kResultOk
    }

    unsafe fn getRoutingInfo(&self, _in_info: *mut RoutingInfo, _out_info: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(&self, _media: MediaType, _direction: BusDirection, index: i32, _state: TBool) -> tresult {
        if index == 0 {
            kResultOk
        } else {
            kInvalidArgument
        }
    }

    unsafe fn setActive(&self, state: TBool) -> tresult {
        self.active.store(state != 0, Ordering::Relaxed);
        self.note.store(-1, Ordering::Relaxed);
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let bytes = stream_read(state, SINE_STATE_LEN + 1);
        if bytes.len() != SINE_STATE_LEN || &bytes[..4] != SINE_STATE_MAGIC {
            return kResultFalse;
        }
        self.values.set(PARAM_LEVEL, f64_at(&bytes, 8));
        self.values.set(PARAM_THRU, f64_at(&bytes, 16));
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let mut bytes = Vec::with_capacity(SINE_STATE_LEN);
        bytes.extend_from_slice(SINE_STATE_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&self.values.get(PARAM_LEVEL).to_le_bytes());
        bytes.extend_from_slice(&self.values.get(PARAM_THRU).to_le_bytes());
        if stream_write(state, &bytes) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IAudioProcessorTrait for SineSynth {
    unsafe fn setBusArrangements(
        &self,
        _inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        if num_ins != 0 || num_outs != 1 {
            return kResultFalse;
        }
        match *outputs {
            SpeakerArr::kMono => self.mono.store(true, Ordering::Relaxed),
            SpeakerArr::kStereo => self.mono.store(false, Ordering::Relaxed),
            _ => return kResultFalse,
        }
        kResultTrue
    }

    unsafe fn getBusArrangement(&self, direction: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if direction != BusDirections_::kOutput as BusDirection || index != 0 {
            return kInvalidArgument;
        }
        *arr = if self.mono.load(Ordering::Relaxed) { SpeakerArr::kMono } else { SpeakerArr::kStereo };
        kResultOk
    }

    unsafe fn canProcessSampleSize(&self, symbolic_sample_size: i32) -> tresult {
        if symbolic_sample_size == SymbolicSampleSizes_::kSample32 as i32 {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }

    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        self.sample_rate.store((*setup).sampleRate.to_bits(), Ordering::Relaxed);
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        // Optional in VST3; hosts must treat this answer as "fine".
        kNotImplemented
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = &mut *data;
        let frames = data.numSamples.max(0) as usize;
        let rate = f64::from_bits(self.sample_rate.load(Ordering::Relaxed));
        let mut reader = QueueReader::new(data.inputParameterChanges);
        let events = ComRef::from_raw(data.inputEvents);
        let out_events = ComRef::from_raw(data.outputEvents);
        let event_count = events.map_or(0, |events| events.getEventCount());
        let mut next_event = 0;
        let outputs = bus_channels(data.outputs, data.numOutputs, 0);
        let mut note = self.note.load(Ordering::Relaxed);
        let mut age = self.age.load(Ordering::Relaxed);
        let mut sounded = false;

        let on_parameter = |id: u32, value: f64, offset: i32| {
            self.values.set(id, value);
            if id == PARAM_BEND && self.values.get(PARAM_THRU) >= 0.5 {
                if let Some(out) = out_events {
                    let bend = (value.clamp(0.0, 1.0) * 16383.0).round() as u32;
                    let mut event: Event = std::mem::zeroed();
                    event.sampleOffset = offset;
                    event.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
                    event.__field0.midiCCOut = LegacyMIDICCOutEvent {
                        controlNumber: ControllerNumbers_::kPitchBend as u8,
                        channel: 0,
                        value: (bend & 0x7F) as i8,
                        value2: ((bend >> 7) & 0x7F) as i8,
                    };
                    out.addEvent(&mut event);
                }
            }
        };

        for frame in 0..=frames {
            let limit = if frame == frames { None } else { Some(frame as i32) };
            reader.advance(limit, |id, value| on_parameter(id, value, frame.min(frames.saturating_sub(1)) as i32));
            while next_event < event_count {
                let mut event: Event = std::mem::zeroed();
                if events.unwrap().getEvent(next_event, &mut event) != kResultOk {
                    next_event += 1;
                    continue;
                }
                if frame < frames && event.sampleOffset > frame as i32 {
                    break;
                }
                next_event += 1;
                let kind = event.r#type as u32;
                if kind == Event_::EventTypes_::kNoteOnEvent as u32 {
                    note = event.__field0.noteOn.pitch as i32;
                    age = 0;
                } else if kind == Event_::EventTypes_::kNoteOffEvent as u32 {
                    if note == event.__field0.noteOff.pitch as i32 {
                        note = -1;
                    }
                } else if kind == Event_::EventTypes_::kPolyPressureEvent as u32 {
                    // One object is processor and controller: the probe is shown at once.
                    let pressure = event.__field0.polyPressure;
                    let packed = pressure.pitch.clamp(0, 127) as u32 * 128 + (pressure.pressure.clamp(0.0, 1.0) * 127.0).round() as u32;
                    self.values.set(PARAM_POLY_PRESSURE, packed as f64 / 16383.0);
                    self.shown.set(PARAM_POLY_PRESSURE, packed as f64 / 16383.0);
                    continue;
                } else {
                    continue;
                }
                if self.values.get(PARAM_THRU) >= 0.5 {
                    if let Some(out) = out_events {
                        out.addEvent(&mut event);
                    }
                }
            }
            if frame == frames {
                break;
            }
            let sample = if note >= 0 {
                let level = match self.values.get(PARAM_PRESSURE) {
                    pressure if pressure > 0.0 => self.values.get(PARAM_LEVEL) * pressure_gain(pressure),
                    _ => self.values.get(PARAM_LEVEL),
                };
                let sample = sine_sample(note as u8, age, rate, level);
                age += 1;
                sample
            } else {
                0.0
            };
            sounded |= sample != 0.0;
            for &out in outputs {
                *out.add(frame) = sample;
            }
        }
        self.note.store(note, Ordering::Relaxed);
        self.age.store(age, Ordering::Relaxed);
        if data.numOutputs > 0 && !data.outputs.is_null() {
            let all = (1u64 << outputs.len().min(63)) - 1;
            (*data.outputs).silenceFlags = if !sounded && frames > 0 { all } else { 0 };
        }
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        SINE_TAIL
    }
}

impl IEditControllerTrait for SineSynth {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let bytes = stream_read(state, SINE_STATE_LEN + 1);
        if bytes.len() != SINE_STATE_LEN || &bytes[..4] != SINE_STATE_MAGIC {
            return kResultFalse;
        }
        self.shown.set(PARAM_LEVEL, f64_at(&bytes, 8));
        self.shown.set(PARAM_THRU, f64_at(&bytes, 16));
        kResultOk
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    /// No controller state of its own, like the SDK's base controller.
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    unsafe fn getParameterCount(&self) -> i32 {
        SINE_PARAMS.len() as i32
    }

    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        match SINE_PARAMS.get(index as usize) {
            Some(def) if index >= 0 => {
                fill_parameter(def, info);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getParamStringByValue(&self, id: u32, value: f64, string: *mut String128) -> tresult {
        let text = match id {
            PARAM_LEVEL => format!("{:.0} %", value * 100.0),
            PARAM_THRU => if value >= 0.5 { "On" } else { "Off" }.to_string(),
            PARAM_BEND => format!("{:+.2}", value * 2.0 - 1.0),
            PARAM_PRESSURE | PARAM_BRIGHTNESS => format!("{:.0}", value * 127.0),
            PARAM_POLY_PRESSURE => format!("{:.0}", value * 16383.0),
            _ => return kInvalidArgument,
        };
        copy_wstring(&text, &mut *string);
        kResultOk
    }

    unsafe fn getParamValueByString(&self, id: u32, string: *mut TChar, value: *mut f64) -> tresult {
        let text = wide_string(string);
        match (id, text.strip_suffix(" %").and_then(|number| number.trim().parse::<f64>().ok())) {
            (PARAM_LEVEL, Some(percent)) => {
                *value = percent / 100.0;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn normalizedParamToPlain(&self, _id: u32, value: f64) -> f64 {
        value
    }
    unsafe fn plainParamToNormalized(&self, _id: u32, plain: f64) -> f64 {
        plain
    }

    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        self.shown.get(id)
    }

    unsafe fn setParamNormalized(&self, id: u32, value: f64) -> tresult {
        if self.shown.slot(id).is_none() {
            return kInvalidArgument;
        }
        self.shown.set(id, value);
        kResultOk
    }

    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        *self.handler.borrow_mut() = ComRef::from_raw(handler).map(|handler| handler.to_com_ptr());
        kResultOk
    }

    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

impl IMidiMappingTrait for SineSynth {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: i32,
        _channel: i16,
        controller: CtrlNumber,
        id: *mut ParamID,
    ) -> tresult {
        if bus != 0 {
            return kResultFalse;
        }
        if controller == ControllerNumbers_::kCtrlVolume as CtrlNumber {
            *id = PARAM_LEVEL;
        } else if controller == ControllerNumbers_::kPitchBend as CtrlNumber {
            *id = PARAM_BEND;
        } else if controller == ControllerNumbers_::kAfterTouch as CtrlNumber {
            *id = PARAM_PRESSURE;
        } else if controller == 74 {
            *id = PARAM_BRIGHTNESS;
        } else {
            // Some plug-ins scribble over the out-parameter and report failure: a host
            // must go by the result, not by the value.
            *id = 0;
            return kResultFalse;
        }
        kResultOk
    }
}

// ---- factory ------------------------------------------------------------------------------------

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory3,);
}

struct ClassEntry {
    uid: [u32; 4],
    category: &'static str,
    name: &'static str,
    wide_name: &'static str,
    sub_categories: &'static str,
}

static CLASSES: [ClassEntry; 3] = [
    ClassEntry {
        uid: GAIN_UID,
        category: "Audio Module Class",
        name: GAIN_NAME,
        wide_name: GAIN_NAME,
        sub_categories: "Fx|Dynamics",
    },
    ClassEntry {
        uid: GAIN_CONTROLLER_UID,
        category: "Component Controller Class",
        name: "ST Test Gain VST3 Controller",
        wide_name: "ST Test Gain VST3 Controller",
        sub_categories: "",
    },
    ClassEntry {
        uid: SINE_UID,
        category: "Audio Module Class",
        name: SINE_NAME_ASCII,
        wide_name: SINE_NAME,
        sub_categories: "Instrument|Synth",
    },
];

const SDK_VERSION: &str = "VST 3.8.0";
const MANY_INSTANCES: i32 = PClassInfo_::ClassCardinality_::kManyInstances as i32;

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = &mut *info;
        copy_cstring(VENDOR, &mut info.vendor);
        copy_cstring("https://example.invalid/supertracker", &mut info.url);
        copy_cstring("mailto:tests@example.invalid", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as i32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> i32 {
        CLASSES.len() as i32
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let Some(class) = CLASSES.get(index as usize).filter(|_| index >= 0) else { return kInvalidArgument };
        let info = &mut *info;
        info.cid = tuid(class.uid);
        info.cardinality = MANY_INSTANCES;
        copy_cstring(class.category, &mut info.category);
        copy_cstring(class.name, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(&self, cid: FIDString, iid: FIDString, obj: *mut *mut c_void) -> tresult {
        if obj.is_null() || cid.is_null() || iid.is_null() {
            return kInvalidArgument;
        }
        *obj = std::ptr::null_mut();
        let cid = *(cid as *const TUID);
        let instance = if cid == tuid(GAIN_UID) {
            ComWrapper::new(GainProcessor::new()).to_com_ptr::<FUnknown>()
        } else if cid == tuid(GAIN_CONTROLLER_UID) {
            ComWrapper::new(GainController::new()).to_com_ptr::<FUnknown>()
        } else if cid == tuid(SINE_UID) {
            ComWrapper::new(SineSynth::new()).to_com_ptr::<FUnknown>()
        } else {
            None
        };
        match instance {
            Some(instance) => {
                let ptr = instance.as_ptr();
                ((*(*ptr).vtbl).queryInterface)(ptr, iid as *const TUID, obj)
            }
            None => kNoInterface,
        }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let Some(class) = CLASSES.get(index as usize).filter(|_| index >= 0) else { return kInvalidArgument };
        let info = &mut *info;
        info.cid = tuid(class.uid);
        info.cardinality = MANY_INSTANCES;
        copy_cstring(class.category, &mut info.category);
        copy_cstring(class.name, &mut info.name);
        info.classFlags = 0;
        copy_cstring(class.sub_categories, &mut info.subCategories);
        copy_cstring("", &mut info.vendor); // empty: hosts fall back to the factory's vendor
        copy_cstring(VERSION, &mut info.version);
        copy_cstring(SDK_VERSION, &mut info.sdkVersion);
        kResultOk
    }
}

impl IPluginFactory3Trait for Factory {
    unsafe fn getClassInfoUnicode(&self, index: i32, info: *mut PClassInfoW) -> tresult {
        let Some(class) = CLASSES.get(index as usize).filter(|_| index >= 0) else { return kInvalidArgument };
        let info = &mut *info;
        info.cid = tuid(class.uid);
        info.cardinality = MANY_INSTANCES;
        copy_cstring(class.category, &mut info.category);
        copy_wstring(class.wide_name, &mut info.name);
        info.classFlags = 0;
        copy_cstring(class.sub_categories, &mut info.subCategories);
        copy_wstring("", &mut info.vendor);
        copy_wstring(VERSION, &mut info.version);
        copy_wstring(SDK_VERSION, &mut info.sdkVersion);
        kResultOk
    }

    unsafe fn setHostContext(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
}

// ---- module entry points --------------------------------------------------------------------

#[cfg(target_os = "windows")]
#[no_mangle]
pub extern "system" fn InitDll() -> bool {
    module_entry()
}

#[cfg(target_os = "windows")]
#[no_mangle]
pub extern "system" fn ExitDll() -> bool {
    module_exit()
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub extern "system" fn bundleEntry(_bundle: *mut c_void) -> bool {
    module_entry()
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub extern "system" fn bundleExit() -> bool {
    module_exit()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[no_mangle]
pub extern "system" fn ModuleEntry(_library: *mut c_void) -> bool {
    module_entry()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[no_mangle]
pub extern "system" fn ModuleExit() -> bool {
    module_exit()
}

#[no_mangle]
pub extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    let name = module_file_name();
    if name.contains("HangOnScan") {
        hang();
    }
    if name.contains("CrashInFactory") {
        crash(2);
    }
    if name.contains("NoFactory") {
        return std::ptr::null_mut();
    }
    ComWrapper::new(Factory).to_com_ptr::<IPluginFactory>().unwrap().into_raw()
}

const _: () = assert!(IComponent::IID.len() == 16);

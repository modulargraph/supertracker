//! Format `"test"`: small deterministic plug-ins for the integration tests.
//!
//! | id         | kind       | behaviour                                                                  |
//! |------------|------------|----------------------------------------------------------------------------|
//! | `copy`     | effect     | flat output channel j = flat input channel j mod inputs, bit for bit       |
//! | `gain`     | effect     | out = in x gain, sample-accurate gain events, bypass, presets, fake editor |
//! | `delay`    | effect     | out[n] = in[n - L]; reports latency L (option `latency=`, parameter 1)     |
//! | `ducker`   | effect     | out = main x (1 - min(1, abs(sidechain)))                                  |
//! | `sine`     | instrument | one sine voice driven by MIDI notes; echoes MIDI in to MIDI out            |
//! | `probe`    | effect     | writes what it was told about the block into output channel 0; can stall   |
//! | `bigstate` | effect     | large state blob (option `size=`), 64 parameters with metadata             |
//! | `crash`    | effect     | pass-through that aborts / segfaults / exits on request                    |
//! | `hang`     | effect     | pass-through that stalls or hangs on request                               |
//! | `slow`     | effect     | `gain` that takes `ms=` milliseconds to instantiate                        |
//! | `events`   | instrument | keeps a log of the events it was given, readable as parameters; can stall  |
//! |            |            | and can keep its container from processing for a while                     |

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use stpc_runtime::*;

pub struct TestBackend;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Copy,
    Gain,
    Delay,
    Ducker,
    Sine,
    Probe,
    BigState,
    Crash,
    Hang,
    Events,
}

/// Parameter ids of `gain` (and `slow`).
pub mod gain {
    pub const GAIN: u64 = 1;
    pub const MODE: u64 = 2;
    /// Setting this acts like the user dragging the gain knob in the editor to the value.
    pub const SIMULATE_EDIT: u64 = 100;
    /// Setting this acts like the user closing the editor window.
    pub const SIMULATE_CLOSE: u64 = 101;
}
/// Parameter ids of `delay`.
pub mod delay {
    /// Delay and reported latency, in frames.
    pub const FRAMES: u64 = 1;
    pub const MAX_FRAMES: usize = 16384;
}
/// Parameter ids of `sine`.
pub mod sine {
    pub const LEVEL: u64 = 1;
}
/// Parameters and state layout of `bigstate`.
pub mod bigstate {
    pub const PARAMS: u64 = 64;
    /// Setting this regenerates the blob from the value as a seed.
    pub const SEED: u64 = 1000;
    /// The blob a given seed and size produce.
    pub fn blob(seed: u64, size: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..size)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }
}
/// Triggers of `crash`.
pub mod crash {
    /// Main thread (set_parameter): 1 abort, 2 segfault, 3 exit(7).
    pub const MAIN: u64 = 1;
    /// Real-time thread (parameter event): 1 abort, 2 segfault.
    pub const PROCESS: u64 = 2;
    pub const NOTE_ABORT: u8 = 60;
    pub const NOTE_SEGFAULT: u8 = 61;
}
/// Triggers of `hang`.
pub mod hang {
    /// Parameter event: hang forever inside process.
    pub const PROCESS_FOREVER: u64 = 1;
    /// Parameter event: sleep `value` milliseconds inside process, once.
    pub const PROCESS_SLEEP_MS: u64 = 2;
    /// set_parameter: hang the main thread forever.
    pub const MAIN_FOREVER: u64 = 3;
    /// set_parameter: block the main thread for `value` milliseconds.
    pub const MAIN_SLEEP_MS: u64 = 4;
    pub const NOTE_FOREVER: u8 = 62;
}

/// `events`: the log and what makes the plug-in miss blocks.
pub mod events {
    /// Parameter event: sleep `value` milliseconds inside process, once (the block is late).
    pub const PROCESS_SLEEP_MS: u64 = 2;
    /// set_parameter: keep the real-time thread out of `process` for `value` milliseconds
    /// (blocks requested meanwhile are answered "busy", unprocessed).
    pub const EXCLUDE_MS: u64 = 5;
    /// Read-only: how many events the plug-in has been given (the log holds the first 256).
    pub const COUNT: u64 = 10;
    /// Read-only: entry `n` of the log is parameter `LOG + n`.
    pub const LOG: u64 = 1000;
    pub const LOG_CAPACITY: usize = 256;

    /// The log entry of a MIDI event (up to three bytes) at `offset`.
    pub fn midi(offset: u32, bytes: &[u8]) -> f64 {
        let payload = bytes.iter().take(3).enumerate().fold(0u64, |payload, (index, byte)| payload | u64::from(*byte) << (8 * index));
        (1u64 << 50 | u64::from(offset.min(8191)) << 37 | payload) as f64
    }

    /// The log entry of a parameter event at `offset` (`id` below 32, a whole `value`).
    pub fn param(offset: u32, id: u64, value: f64) -> f64 {
        (2u64 << 50 | u64::from(offset.min(8191)) << 37 | (id & 31) << 32 | u64::from(value as u32)) as f64
    }
}

/// The sample `sine` produces `k` frames after a note-on, shared with the tests so that
/// they can compare bit for bit.
pub fn sine_sample(note: u8, k: u64, sample_rate: f64, level: f64) -> f32 {
    let frequency = 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
    (level * (std::f64::consts::TAU * frequency * k as f64 / sample_rate).sin()) as f32
}

fn option<'a>(options: &'a str, key: &str) -> Option<&'a str> {
    options.split(',').find_map(|part| part.trim().strip_prefix(key)?.strip_prefix('='))
}

fn has_flag(options: &str, flag: &str) -> bool {
    options.split(',').any(|part| part.trim() == flag)
}

fn crash_now(how: u32) -> ! {
    match how {
        2 => {
            // SAFETY: deliberately not safe: this is the segfault the tests ask for.
            #[allow(clippy::manual_dangling_ptr)]
            unsafe {
                std::ptr::write_volatile(8 as *mut u64, 0xDEAD)
            };
            std::process::abort()
        }
        3 => std::process::exit(7),
        _ => std::process::abort(),
    }
}

/// State shared between the main-thread and real-time halves of a test plug-in.
struct Shared {
    kind: Kind,
    sample_rate: AtomicU64,
    /// Gain (gain), delay in frames (delay), level (sine).
    value: AtomicU64,
    bypass: AtomicBool,
    mode: AtomicU32,
    /// `events`: the events received (`events::midi`, `events::param`) and their count.
    log: Vec<AtomicU64>,
    log_count: AtomicU32,
}

impl Shared {
    fn value(&self) -> f64 {
        f64::from_bits(self.value.load(Ordering::Relaxed))
    }
    fn set_value(&self, value: f64) {
        self.value.store(value.to_bits(), Ordering::Relaxed);
    }
}

struct TestPlugin {
    shared: Arc<Shared>,
    host: HostHandle,
    name: String,
    in_buses: Vec<u32>,
    out_buses: Vec<u32>,
    editor_open: bool,
    editor_title: String,
    params: Vec<f64>,
    blob: Vec<u8>,
    crash_on_activate: bool,
}

impl PluginBackend for TestBackend {
    fn format(&self) -> &str {
        "test"
    }

    fn create(&self, request: &CreateRequest<'_>, host: HostHandle) -> Result<Box<dyn Plugin>, PluginError> {
        let options = request.options;
        let (kind, initial) = match request.plugin_id {
            "copy" => (Kind::Copy, 0.0),
            "gain" => (Kind::Gain, 1.0),
            "slow" => {
                let ms = option(options, "ms").and_then(|v| v.parse().ok()).unwrap_or(1000);
                std::thread::sleep(Duration::from_millis(ms));
                (Kind::Gain, 1.0)
            }
            "delay" => {
                let latency: usize = option(options, "latency").and_then(|v| v.parse().ok()).unwrap_or(64);
                (Kind::Delay, latency.min(delay::MAX_FRAMES) as f64)
            }
            "ducker" => (Kind::Ducker, 0.0),
            "sine" => (Kind::Sine, 0.5),
            "probe" => (Kind::Probe, 0.0),
            "bigstate" => (Kind::BigState, 0.0),
            "crash" => (Kind::Crash, 0.0),
            "hang" => (Kind::Hang, 0.0),
            "events" => (Kind::Events, 0.0),
            other => return Err(PluginError::failed(format!("the test backend has no plug-in \"{other}\""))),
        };
        if has_flag(options, "crash_on_create") {
            crash_now(1);
        }
        if has_flag(options, "fail_on_create") {
            return Err(PluginError::failed("this plug-in refuses to load (fail_on_create)"));
        }
        let blob = if kind == Kind::BigState {
            let size = option(options, "size").and_then(|v| v.parse().ok()).unwrap_or(4 << 20);
            bigstate::blob(1, size)
        } else {
            Vec::new()
        };
        Ok(Box::new(TestPlugin {
            shared: Arc::new(Shared {
                kind,
                sample_rate: AtomicU64::new(request.sample_rate.to_bits()),
                value: AtomicU64::new(initial.to_bits()),
                bypass: AtomicBool::new(false),
                mode: AtomicU32::new(0),
                log: (0..if kind == Kind::Events { events::LOG_CAPACITY } else { 0 }).map(|_| AtomicU64::new(0)).collect(),
                log_count: AtomicU32::new(0),
            }),
            host,
            name: request.plugin_id.to_string(),
            in_buses: request.input_buses.to_vec(),
            out_buses: request.output_buses.to_vec(),
            editor_open: false,
            editor_title: String::new(),
            params: (0..bigstate::PARAMS).map(|i| i as f64 / 64.0).collect(),
            blob,
            crash_on_activate: has_flag(options, "crash_on_activate"),
        }))
    }
}

fn simple_param(id: u64, name: &str, min: f64, max: f64, default: f64, unit: &str) -> ParameterInfo {
    ParameterInfo {
        id,
        name: name.into(),
        short_name: name.chars().take(4).collect(),
        identifier: name.to_lowercase().replace(' ', "_"),
        unit_label: unit.into(),
        flags: PARAM_AUTOMATABLE,
        min,
        max,
        default,
        ..ParameterInfo::default()
    }
}

impl Plugin for TestPlugin {
    fn info(&mut self) -> InstanceInfo {
        let kind = self.shared.kind;
        let mut flags = 0;
        if kind == Kind::Sine {
            flags |= INFO_INSTRUMENT | INFO_WANTS_MIDI | INFO_PRODUCES_MIDI;
        }
        if kind == Kind::Events {
            flags |= INFO_INSTRUMENT | INFO_WANTS_MIDI;
        }
        if kind == Kind::Gain {
            flags |= INFO_HAS_EDITOR | INFO_HAS_BYPASS;
        }
        if matches!(kind, Kind::Crash | Kind::Hang) {
            flags |= INFO_WANTS_MIDI;
        }
        let latency = if kind == Kind::Delay { self.shared.value() as u32 } else { 0 };
        let bus = |index: usize, channels: u32, input: bool| BusInfo {
            name: match (index, input) {
                (0, _) => "Main".to_string(),
                (_, true) => format!("Sidechain {index}"),
                (_, false) => format!("Aux {index}"),
            },
            channels,
            auxiliary: index > 0,
        };
        InstanceInfo {
            name: self.name.clone(),
            manufacturer: "SuperTracker Tests".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            flags,
            latency,
            tail: latency as u64,
            inputs: self.in_buses.iter().enumerate().map(|(i, &c)| bus(i, c, true)).collect(),
            outputs: self.out_buses.iter().enumerate().map(|(i, &c)| bus(i, c, false)).collect(),
            parameter_count: self.parameters().len() as u32,
        }
    }

    fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<Box<dyn Processor>, PluginError> {
        if self.crash_on_activate {
            crash_now(1);
        }
        self.shared.sample_rate.store(sample_rate.to_bits(), Ordering::Relaxed);
        let channels = self.in_buses.first().copied().unwrap_or(0) as usize;
        Ok(Box::new(TestProcessor {
            shared: self.shared.clone(),
            sample_rate,
            gain: self.shared.value(),
            line: if self.shared.kind == Kind::Delay { vec![0.0; channels * delay::MAX_FRAMES] } else { Vec::new() },
            line_pos: 0,
            note: None,
            note_age: 0,
            level: self.shared.value(),
            scratch: vec![0.0; max_frames as usize],
        }))
    }

    fn deactivate(&mut self, processor: Box<dyn Processor>) {
        drop(processor);
    }

    fn set_bypass(&mut self, bypass: bool) -> Result<(), PluginError> {
        if self.shared.kind != Kind::Gain {
            return Err(PluginError::Unsupported);
        }
        self.shared.bypass.store(bypass, Ordering::Relaxed);
        Ok(())
    }

    fn parameters(&mut self) -> Vec<ParameterInfo> {
        match self.shared.kind {
            Kind::Gain => vec![
                simple_param(gain::GAIN, "Gain", 0.0, 4.0, 1.0, "x"),
                ParameterInfo {
                    flags: PARAM_AUTOMATABLE | PARAM_STEPPED,
                    step_count: 2,
                    value_strings: vec!["Clean".into(), "Warm".into(), "Hot".into()],
                    group: "Character".into(),
                    ..simple_param(gain::MODE, "Mode", 0.0, 2.0, 0.0, "")
                },
            ],
            Kind::Delay => vec![ParameterInfo {
                flags: PARAM_STEPPED,
                step_count: delay::MAX_FRAMES as u32,
                ..simple_param(delay::FRAMES, "Delay", 0.0, delay::MAX_FRAMES as f64, 64.0, "frames")
            }],
            Kind::Sine => vec![simple_param(sine::LEVEL, "Level", 0.0, 1.0, 0.5, "")],
            Kind::BigState => (1..=bigstate::PARAMS)
                .map(|id| ParameterInfo {
                    group: format!("Bank {}/Page {}", (id - 1) / 16 + 1, (id - 1) / 4 % 4 + 1),
                    flags: if id % 8 == 0 {
                        PARAM_AUTOMATABLE | PARAM_BOOLEAN | PARAM_STEPPED
                    } else {
                        PARAM_AUTOMATABLE
                    },
                    step_count: if id % 8 == 0 { 1 } else { 0 },
                    ..simple_param(id, &format!("Param {id:02}"), 0.0, 1.0, (id - 1) as f64 / 64.0, "dB")
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn parameter_value(&mut self, id: u64) -> Option<f64> {
        match (self.shared.kind, id) {
            (Kind::Gain, gain::GAIN) | (Kind::Delay, delay::FRAMES) | (Kind::Sine, sine::LEVEL) => {
                Some(self.shared.value())
            }
            (Kind::Gain, gain::MODE) => Some(self.shared.mode.load(Ordering::Relaxed) as f64),
            (Kind::BigState, id) if (1..=bigstate::PARAMS).contains(&id) => Some(self.params[id as usize - 1]),
            (Kind::Events, events::COUNT) => Some(self.shared.log_count.load(Ordering::Acquire) as f64),
            (Kind::Events, id) if id >= events::LOG => {
                self.shared.log.get((id - events::LOG) as usize).map(|entry| f64::from_bits(entry.load(Ordering::Acquire)))
            }
            _ => None,
        }
    }

    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), PluginError> {
        match (self.shared.kind, id) {
            (Kind::Gain, gain::GAIN) | (Kind::Sine, sine::LEVEL) => self.shared.set_value(value),
            (Kind::Gain, gain::MODE) => self.shared.mode.store(value.round().clamp(0.0, 2.0) as u32, Ordering::Relaxed),
            (Kind::Gain, gain::SIMULATE_EDIT) => {
                self.host.notify(Notification::ParamGestureBegin { id: gain::GAIN });
                self.shared.set_value(value);
                self.host.notify(Notification::ParamValue { id: gain::GAIN, value });
                self.host.notify(Notification::ParamGestureEnd { id: gain::GAIN });
            }
            (Kind::Gain, gain::SIMULATE_CLOSE) => {
                self.editor_open = false;
                self.host.notify(Notification::EditorClosed);
            }
            (Kind::Delay, delay::FRAMES) => {
                let frames = value.round().clamp(0.0, delay::MAX_FRAMES as f64);
                self.shared.set_value(frames);
                self.host.notify(Notification::LatencyChanged { frames: frames as u32 });
                self.host.notify(Notification::TailChanged { frames: frames as u64 });
            }
            (Kind::BigState, bigstate::SEED) => {
                self.blob = bigstate::blob(value as u64, self.blob.len());
                self.host.notify(Notification::StateDirty);
            }
            (Kind::BigState, id) if (1..=bigstate::PARAMS).contains(&id) => self.params[id as usize - 1] = value,
            (Kind::Crash, crash::MAIN) => crash_now(value as u32),
            (Kind::Hang, hang::MAIN_FOREVER) => loop {
                std::thread::sleep(Duration::from_secs(3600));
            },
            (Kind::Hang, hang::MAIN_SLEEP_MS) => std::thread::sleep(Duration::from_millis(value as u64)),
            (Kind::Events, events::EXCLUDE_MS) => {
                self.host.with_processing_excluded(|| std::thread::sleep(Duration::from_millis(value as u64)));
            }
            _ => return Err(PluginError::failed(format!("no parameter {id}"))),
        }
        Ok(())
    }

    fn format_parameter(&mut self, id: u64, value: f64) -> Option<String> {
        match (self.shared.kind, id) {
            (Kind::Gain, gain::GAIN) => Some(format!("{:.1} dB", 20.0 * value.max(1e-9).log10())),
            (Kind::Gain, gain::MODE) => ["Clean", "Warm", "Hot"].get(value.round() as usize).map(|s| s.to_string()),
            (Kind::BigState, _) => Some(format!("{:.2} dB", value * 24.0 - 12.0)),
            (Kind::Delay, _) => Some(format!("{} frames", value.round())),
            _ => None,
        }
    }

    fn parse_parameter(&mut self, id: u64, text: &str) -> Option<f64> {
        match (self.shared.kind, id) {
            (Kind::Gain, gain::GAIN) => {
                let db: f64 = text.trim().trim_end_matches("dB").trim().parse().ok()?;
                Some(10f64.powf(db / 20.0))
            }
            (Kind::Gain, gain::MODE) => ["Clean", "Warm", "Hot"].iter().position(|s| *s == text).map(|i| i as f64),
            _ => None,
        }
    }

    fn state(&mut self) -> Result<Vec<u8>, PluginError> {
        match self.shared.kind {
            Kind::BigState => {
                let mut out = Vec::with_capacity(self.params.len() * 8 + self.blob.len());
                for value in &self.params {
                    out.extend_from_slice(&value.to_le_bytes());
                }
                out.extend_from_slice(&self.blob);
                Ok(out)
            }
            Kind::Copy | Kind::Probe | Kind::Ducker => Ok(Vec::new()),
            _ => {
                let mut out = self.shared.value().to_le_bytes().to_vec();
                out.extend_from_slice(&self.shared.mode.load(Ordering::Relaxed).to_le_bytes());
                Ok(out)
            }
        }
    }

    fn set_state(&mut self, data: &[u8]) -> Result<(), PluginError> {
        if data == b"crash" {
            crash_now(1);
        }
        match self.shared.kind {
            Kind::BigState => {
                let head = self.params.len() * 8;
                if data.len() < head {
                    return Err(PluginError::failed("state too short"));
                }
                for (value, raw) in self.params.iter_mut().zip(data.chunks_exact(8)) {
                    *value = f64::from_le_bytes(raw.try_into().unwrap());
                }
                self.blob = data[head..].to_vec();
                Ok(())
            }
            Kind::Copy | Kind::Probe | Kind::Ducker => Ok(()),
            _ => {
                if data.len() != 12 {
                    return Err(PluginError::failed(format!("bad state: {} bytes, 12 expected", data.len())));
                }
                let value = f64::from_le_bytes(data[..8].try_into().unwrap());
                self.shared.set_value(value);
                self.shared.mode.store(u32::from_le_bytes(data[8..].try_into().unwrap()), Ordering::Relaxed);
                if self.shared.kind == Kind::Delay {
                    self.host.notify(Notification::LatencyChanged { frames: value as u32 });
                }
                Ok(())
            }
        }
    }

    fn presets(&mut self) -> Vec<PresetInfo> {
        if self.shared.kind != Kind::Gain {
            return Vec::new();
        }
        ["Unity", "Half", "Double"]
            .iter()
            .enumerate()
            .map(|(id, name)| PresetInfo { id: id as u32, name: name.to_string() })
            .collect()
    }

    fn load_preset(&mut self, id: u32) -> Result<(), PluginError> {
        if self.shared.kind != Kind::Gain {
            return Err(PluginError::Unsupported);
        }
        let value = *[1.0, 0.5, 2.0].get(id as usize).ok_or_else(|| PluginError::failed("no such preset"))?;
        self.shared.set_value(value);
        self.host.notify(Notification::ParamsRescan);
        Ok(())
    }

    fn open_editor(&mut self, title: &str) -> Result<EditorInfo, PluginError> {
        if self.shared.kind != Kind::Gain {
            return Err(PluginError::Unsupported);
        }
        // No window system in the tests: the "editor" is its bookkeeping.
        self.editor_open = true;
        self.editor_title = title.to_string();
        Ok(EditorInfo { width: 400, height: 300, resizable: false, window_id: title.len() as u64 })
    }

    fn close_editor(&mut self) {
        self.editor_open = false;
    }

    fn set_editor_title(&mut self, title: &str) {
        self.editor_title = title.to_string();
    }
}

struct TestProcessor {
    shared: Arc<Shared>,
    sample_rate: f64,
    gain: f64,
    line: Vec<f32>,
    line_pos: usize,
    note: Option<u8>,
    note_age: u64,
    level: f64,
    scratch: Vec<f32>,
}

impl TestProcessor {
    fn pass_through(data: &mut ProcessData<'_>) {
        let channels = data.input_channels(0).min(data.output_channels(0));
        for channel in 0..channels {
            let (input, output) = data.io(0, channel, 0, channel);
            output.copy_from_slice(input);
        }
    }
}

impl Processor for TestProcessor {
    fn reset(&mut self) {
        self.line.fill(0.0);
        self.line_pos = 0;
        self.note = None;
        self.note_age = 0;
    }

    fn process(&mut self, data: &mut ProcessData<'_>) -> ProcessStatus {
        let frames = data.frames;
        match self.shared.kind {
            Kind::Copy => {
                let inputs: usize = (0..data.input_bus_count()).map(|b| data.input_channels(b)).sum();
                if inputs == 0 {
                    return ProcessStatus::Ok;
                }
                let mut flat_out = 0;
                for out_bus in 0..data.output_bus_count() {
                    for out_channel in 0..data.output_channels(out_bus) {
                        let mut source = flat_out % inputs;
                        let mut in_bus = 0;
                        while source >= data.input_channels(in_bus) {
                            source -= data.input_channels(in_bus);
                            in_bus += 1;
                        }
                        let (input, output) = data.io(in_bus, source, out_bus, out_channel);
                        output.copy_from_slice(input);
                        flat_out += 1;
                    }
                }
            }
            Kind::Gain => {
                if self.shared.bypass.load(Ordering::Relaxed) {
                    Self::pass_through(data);
                    return ProcessStatus::Ok;
                }
                // A change made from the main thread applies from the block start; timed
                // events then apply at their offsets.
                let main_value = self.shared.value();
                if data.events.iter().all(|e| !(e.kind == EVENT_PARAM && e.id == gain::GAIN)) {
                    self.gain = main_value;
                }
                let channels = data.input_channels(0).min(data.output_channels(0));
                let start_gain = self.gain;
                for channel in 0..channels {
                    let mut current = start_gain;
                    let mut next_event = 0;
                    let events = data.events;
                    let (input, output) = data.io(0, channel, 0, channel);
                    for frame in 0..frames {
                        while next_event < events.len() && events[next_event].offset as usize <= frame {
                            let event = &events[next_event];
                            if event.kind == EVENT_PARAM && event.id == gain::GAIN {
                                current = event.value;
                            }
                            next_event += 1;
                        }
                        output[frame] = input[frame] * current as f32;
                    }
                    self.gain = current;
                }
                if channels == 0 {
                    if let Some(event) = data.events.iter().rev().find(|e| e.kind == EVENT_PARAM && e.id == gain::GAIN)
                    {
                        self.gain = event.value;
                    }
                }
                self.shared.set_value(self.gain);
            }
            Kind::Delay => {
                let delay = (self.shared.value() as usize).min(delay::MAX_FRAMES - 1);
                let channels = data.input_channels(0).min(data.output_channels(0));
                let size = delay::MAX_FRAMES;
                for channel in 0..channels {
                    let line = &mut self.line[channel * size..(channel + 1) * size];
                    let mut pos = self.line_pos;
                    let (input, output) = data.io(0, channel, 0, channel);
                    for frame in 0..frames {
                        line[pos] = input[frame];
                        output[frame] = line[(pos + size - delay) % size];
                        pos = (pos + 1) % size;
                    }
                }
                self.line_pos = (self.line_pos + frames) % size;
            }
            Kind::Ducker => {
                let channels = data.input_channels(0).min(data.output_channels(0));
                let key_channels = data.input_channels(1);
                for channel in 0..channels {
                    if key_channels > 0 {
                        self.scratch[..frames].copy_from_slice(data.input(1, channel % key_channels));
                    } else {
                        self.scratch[..frames].fill(0.0);
                    }
                    let key = &self.scratch[..frames];
                    let (input, output) = data.io(0, channel, 0, channel);
                    for frame in 0..frames {
                        output[frame] = input[frame] * (1.0 - key[frame].abs().min(1.0));
                    }
                }
            }
            Kind::Sine => {
                let events = data.events;
                let mut next_event = 0;
                for frame in 0..frames {
                    while next_event < events.len() && events[next_event].offset as usize <= frame {
                        let event = events[next_event];
                        next_event += 1;
                        if event.kind == EVENT_PARAM && event.id == sine::LEVEL {
                            self.level = event.value;
                        } else if event.kind == EVENT_MIDI {
                            let bytes = event.midi_bytes();
                            match bytes.first().map(|status| status & 0xF0) {
                                Some(0x90) if bytes.len() == 3 && bytes[2] > 0 => {
                                    self.note = Some(bytes[1]);
                                    self.note_age = 0;
                                }
                                Some(0x80) | Some(0x90) if bytes.len() >= 2 && self.note == Some(bytes[1]) => {
                                    self.note = None;
                                }
                                _ => {}
                            }
                            data.push_event(event); // MIDI thru, same offset
                        }
                    }
                    let sample = match self.note {
                        Some(note) => {
                            let sample = sine_sample(note, self.note_age, self.sample_rate, self.level);
                            self.note_age += 1;
                            sample
                        }
                        None => 0.0,
                    };
                    self.scratch[frame] = sample;
                }
                for channel in 0..data.output_channels(0) {
                    data.output(0, channel).copy_from_slice(&self.scratch[..frames]);
                }
                if self.note.is_none() && data.events.is_empty() {
                    data.output_silence = u64::MAX;
                }
            }
            Kind::Probe => {
                // Like `hang`: a stall on request, to observe what follows a dropped block.
                for event in data.events {
                    if event.kind == EVENT_PARAM && event.id == hang::PROCESS_SLEEP_MS {
                        std::thread::sleep(Duration::from_millis(event.value as u64));
                    }
                }
                let report = [
                    frames as f32,
                    data.transport.tempo_bpm as f32,
                    data.transport.sample_position as f32,
                    data.transport.flags as f32,
                    data.offline as u8 as f32,
                    data.events.len() as f32,
                    data.discontinuity as u8 as f32,
                    data.input_silence as f32,
                    data.transport.ppq_position as f32,
                    data.transport.time_sig_numerator as f32,
                    data.transport.time_sig_denominator as f32,
                    self.sample_rate as f32,
                ];
                if data.output_channels(0) > 0 {
                    let output = data.output(0, 0);
                    let count = report.len().min(frames);
                    output[..count].copy_from_slice(&report[..count]);
                }
            }
            Kind::BigState => Self::pass_through(data),
            Kind::Events => {
                for event in data.events {
                    let entry = if event.kind == EVENT_PARAM { events::param(event.offset, event.id, event.value) } else { events::midi(event.offset, event.midi_bytes()) };
                    let count = self.shared.log_count.load(Ordering::Relaxed);
                    if let Some(slot) = self.shared.log.get(count as usize) {
                        slot.store(entry.to_bits(), Ordering::Release);
                        self.shared.log_count.store(count + 1, Ordering::Release);
                    }
                }
                // The stall comes after the log: what a late block carried is on record.
                for event in data.events {
                    if event.kind == EVENT_PARAM && event.id == events::PROCESS_SLEEP_MS {
                        std::thread::sleep(Duration::from_millis(event.value as u64));
                    }
                }
            }
            Kind::Crash => {
                for event in data.events {
                    if event.kind == EVENT_PARAM && event.id == crash::PROCESS {
                        crash_now(event.value as u32);
                    }
                    if event.kind == EVENT_MIDI && event.midi_bytes().first().map(|s| s & 0xF0) == Some(0x90) {
                        match event.midi_bytes().get(1) {
                            Some(&crash::NOTE_ABORT) => crash_now(1),
                            Some(&crash::NOTE_SEGFAULT) => crash_now(2),
                            _ => {}
                        }
                    }
                }
                Self::pass_through(data);
            }
            Kind::Hang => {
                for event in data.events {
                    let note = (event.kind == EVENT_MIDI && event.midi_bytes().first().map(|s| s & 0xF0) == Some(0x90))
                        .then(|| event.midi_bytes().get(1).copied())
                        .flatten();
                    if (event.kind == EVENT_PARAM && event.id == hang::PROCESS_FOREVER)
                        || note == Some(hang::NOTE_FOREVER)
                    {
                        loop {
                            std::thread::sleep(Duration::from_secs(3600));
                        }
                    }
                    if event.kind == EVENT_PARAM && event.id == hang::PROCESS_SLEEP_MS {
                        std::thread::sleep(Duration::from_millis(event.value as u64));
                    }
                }
                Self::pass_through(data);
            }
        }
        ProcessStatus::Ok
    }
}

//! [`AudioEngine`]: the control side of the audio core, as one object the host talks to.
//!
//! It owns the engine's control half, the project-to-engine synchronizer, the graph compiler and
//! the output device, and shares the [`Renderer`] with the device thread. Every method takes
//! `&self` and may be called from any control thread; a mutex serializes them (the engine has
//! one control thread at a time, as its design requires). Nothing here runs on the audio thread.
//!
//! What crosses to the audio thread, and how, is listed in [`crate::graph::render`].

use crate::device::{self, null::NullDevice, system, DeviceInfo, OutputStream, SharedRenderer, NULL_DEVICE};
use crate::graph::insert::{encode_builtin_state, InsertFactory};
use crate::graph::meters::{MeterBank, MeterDisplay};
use crate::graph::plan::{Compiler, InsertStatus};
use crate::graph::render::{Message, RenderShared, Renderer};
use crate::graph::spec::{Destination, GraphSpec, StripKind, MASTER};
use crate::project_sync::{engine_step, live_instrument_target, ApplyReport, ProjectSync};
use crate::samples::SampleSource;
use serde_json::{json, Value};
use st_project::model::{Project, Step};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};
use tracker_engine::engine::abi::*;
use tracker_engine::engine::host::{self, ControlHalf, StatusHandle};
use tracker_engine::engine::tables::MAX_BLOCK;

/// Which output the engine renders to.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceChoice {
    /// No device: the owner calls [`AudioEngine::render_offline`].
    None,
    /// The null device.
    Null,
    /// The system's default output; the null device while there is none.
    SystemDefault,
    /// The output with this `uid`; the null device while it is not connected.
    Named(String),
}

pub struct EngineOptions {
    /// The engine's sample rate. A device that cannot run at it makes the engine restart at the
    /// device's rate.
    pub sample_rate: f64,
    /// Frames per device block.
    pub buffer_size: u32,
    pub samples: Arc<dyn SampleSource>,
    pub inserts: Arc<dyn InsertFactory>,
    pub device: DeviceChoice,
}

impl EngineOptions {
    /// 48 kHz, 512 frames, no samples, built-in effects, no device (offline).
    pub fn offline() -> EngineOptions {
        EngineOptions {
            sample_rate: 48_000.0,
            buffer_size: 512,
            samples: Arc::new(crate::samples::NoSamples),
            inserts: Arc::new(crate::graph::insert::BuiltinFactory),
            device: DeviceChoice::None,
        }
    }
}

/// `transport.play`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayOptions {
    /// Song mode (follow the sequence) rather than looping the active pattern.
    pub song: bool,
    /// Start from the first row of the active pattern / the song's start.
    pub from_start: bool,
    /// Pattern mode: loop these rows (inclusive).
    pub pattern_loop: Option<(i32, i32)>,
    /// Bars of count-in clicks before playback starts, 0..=4.
    pub count_in: i32,
}

/// One live key edge to be acknowledged by the audio thread (`captureInput`).
#[derive(Debug, Clone, PartialEq)]
pub struct InputEdge {
    pub owner: String,
    pub on: bool,
    pub track: i32,
    pub instrument: i32,
    pub note: i32,
    pub velocity: i32,
    pub apply_note: bool,
}

type InputCompletion = Box<dyn FnOnce(Value) + Send>;

struct PendingInput {
    edge: InputEdge,
    context: Value,
    patterns: Vec<i64>,
    generation: u64,
    deadline: Instant,
    completion: InputCompletion,
}

/// Everything tied to one engine instance (and so to one sample rate).
struct Core {
    control: ControlHalf,
    status: StatusHandle,
    shared: Arc<RenderShared>,
    compiler: Compiler,
    sync: ProjectSync,
    spec: GraphSpec,
    meters: Arc<MeterBank>,
}

impl Core {
    fn new(sample_rate: f64, samples: Arc<dyn SampleSource>, inserts: Arc<dyn InsertFactory>) -> Result<(Core, Renderer), String> {
        let (mut control, render, status) = host::split(sample_rate).ok_or_else(|| format!("The audio engine cannot run at {sample_rate} Hz."))?;
        {
            // The host mixes from the first block on.
            let (edit, shared, rate) = control.parts();
            edit.begin_update(shared);
            edit.set_host_mixing_tracks(true, &[], true);
            edit.commit(shared, rate)?;
        }
        let mut compiler = Compiler::new(inserts, sample_rate, MAX_BLOCK);
        let spec = GraphSpec::empty();
        let plan = compiler.compile(&spec)?;
        let meters = plan.meters().clone();
        let shared = Arc::new(RenderShared::new());
        let renderer = Renderer::new(render, status.clone(), shared.clone(), plan);
        Ok((Core { control, status, shared, compiler, sync: ProjectSync::new(samples), spec, meters }, renderer))
    }
}

struct State {
    core: Core,
    project: Option<Project>,
    display: MeterDisplay,
    stream: Option<Box<dyn OutputStream>>,
    wanted: DeviceChoice,
    buffer_size: u32,
    /// Why the wanted device is not the one in use.
    device_error: Option<String>,
    last_device_check: Instant,
    record_armed: bool,
    /// Destination of the last live note (the default for the next one).
    live_track: i32,
    live_instrument: i32,
    next_input: u64,
    pending_inputs: HashMap<u64, PendingInput>,
    /// Bumped when the project or the record arm changes: older input edges no longer count.
    input_generation: u64,
    last_report: ApplyReport,
}

/// The audio core. See the module documentation.
pub struct AudioEngine {
    state: Mutex<State>,
    renderer: SharedRenderer,
    samples: Mutex<Arc<dyn SampleSource>>,
    inserts: Arc<dyn InsertFactory>,
    service_stop: Arc<std::sync::atomic::AtomicBool>,
}

/// What `audio.devices` answers.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesStatus {
    pub devices: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    pub default_input: Option<u32>,
    pub default_output: Option<u32>,
    /// The output in use.
    pub output_device: u32,
    pub output_uid: String,
    pub engine_device: u32,
    pub input_device: Option<u32>,
    pub sample_rate: f64,
    pub buffer_size: u32,
    pub buffer_sizes: Vec<u32>,
    pub running: bool,
    /// True while the null device stands in for the device that was asked for.
    pub fallback: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_error: Option<String>,
    pub input_channels: Vec<u32>,
    pub input_metering: bool,
    pub monitoring: bool,
    pub monitor_volume: f64,
    pub input_acknowledgements: Value,
    pub latency_compensation: Value,
}

/// `audio.configure`: what to change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceRequest {
    /// Select the output with this id (`DeviceInfo::id`; 0 is the null device).
    pub output_id: Option<u32>,
    /// Select the output with this `uid`.
    pub output_uid: Option<String>,
    /// Follow the system's default output.
    pub system_default: bool,
    pub buffer_size: Option<u32>,
    pub sample_rate: Option<f64>,
}

fn input_capability() -> Value {
    json!({ "version": 1, "supported": true })
}

impl AudioEngine {
    /// Creates the engine and opens its device.
    pub fn new(options: EngineOptions) -> Result<Arc<AudioEngine>, String> {
        let (core, renderer) = Core::new(options.sample_rate, options.samples.clone(), options.inserts.clone())?;
        let engine = Arc::new(AudioEngine {
            state: Mutex::new(State {
                core,
                project: None,
                display: MeterDisplay::default(),
                stream: None,
                wanted: options.device.clone(),
                buffer_size: options.buffer_size.clamp(16, 8192),
                device_error: None,
                last_device_check: Instant::now(),
                record_armed: false,
                live_track: 0,
                live_instrument: 0,
                next_input: 0,
                pending_inputs: HashMap::new(),
                input_generation: 0,
                last_report: ApplyReport::default(),
            }),
            renderer: Arc::new(Mutex::new(renderer)),
            samples: Mutex::new(options.samples),
            inserts: options.inserts,
            service_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        engine.open_device(&mut engine.lock())?;
        Ok(engine)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn sample_rate(&self) -> f64 {
        self.lock().core.compiler.sample_rate()
    }

    // ---- project -------------------------------------------------------------------------

    /// Applies a published project to the engine and the mixer. On an error both keep what
    /// they had.
    pub fn publish(&self, project: &Project) -> Result<ApplyReport, String> {
        let mut state = self.lock();
        let report = Self::publish_locked(&mut state, project)?;
        state.project = Some(project.clone());
        state.input_generation += 1;
        state.last_report = report.clone();
        Ok(report)
    }

    fn publish_locked(state: &mut State, project: &Project) -> Result<ApplyReport, String> {
        let core = &mut state.core;
        let spec = GraphSpec::from_project(project);
        // Compile first: routing that cannot be rendered must not reach the engine either.
        let plan = if spec != core.spec { Some(core.compiler.compile(&spec)?) } else { None };
        let Core { control, compiler, sync, .. } = core;
        let compiler = &*compiler;
        let defaults = |destination: &Destination, address: u64| -> Option<f64> {
            match destination {
                Destination::Mix(owner) => spec.strip(owner).map(|strip| if address == 0 { strip.volume as f64 } else { strip.pan as f64 }),
                Destination::Route(id) => spec.sends.iter().chain(&spec.sidechains).find(|route| route.id == *id).map(|route| route.gain as f64),
                Destination::Insert(key) => {
                    let insert = spec.strips.iter().flat_map(|strip| &strip.inserts).find(|insert| insert.key == *key)?;
                    compiler.insert_values(insert).into_iter().find(|(known, _)| *known == address).map(|(_, value)| value)
                }
            }
        };
        let report = sync.apply(control, project, &spec, &defaults)?;
        if let Some(plan) = plan {
            core.meters = plan.meters().clone();
            core.shared.publish(plan);
            core.spec = spec;
        }
        core.shared.automation_changed();
        Ok(report)
    }

    /// The sample source has new or changed PCM: install it (re-applies the current project).
    pub fn samples_changed(&self) -> Result<ApplyReport, String> {
        let mut state = self.lock();
        let Some(project) = state.project.clone() else { return Ok(ApplyReport::default()) };
        let report = Self::publish_locked(&mut state, &project)?;
        state.last_report = report.clone();
        Ok(report)
    }

    /// Replaces the sample source and installs everything from it.
    pub fn set_sample_source(&self, samples: Arc<dyn SampleSource>) -> Result<ApplyReport, String> {
        *self.samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = samples.clone();
        self.lock().core.sync.set_samples(samples);
        self.samples_changed()
    }

    /// What the last publication did (patterns sent and reused, samples installed or missing).
    pub fn last_report(&self) -> ApplyReport {
        self.lock().last_report.clone()
    }

    /// The published project, if any.
    pub fn project(&self) -> Option<Project> {
        self.lock().project.clone()
    }

    /// Song (or active pattern) duration in seconds, as the engine computes it.
    pub fn duration(&self, song: bool) -> f64 {
        let mut state = self.lock();
        let (control, _, rate) = state.core.control.parts();
        control.duration(rate, song)
    }

    // ---- transport -----------------------------------------------------------------------

    pub fn play(&self, options: &PlayOptions) {
        let mut state = self.lock();
        let active = state.project.as_ref().map_or(0, |project| project.active_pattern() as i32);
        let (control, shared, _) = state.core.control.parts();
        if options.from_start {
            control.seek(shared, active, 0, 0);
        }
        // The loop range rides with each Play; a Play without one clears it.
        let (start, end) = options.pattern_loop.filter(|_| !options.song).unwrap_or((-1, -1));
        control.set_pattern_loop(shared, start, end);
        match options.count_in.clamp(0, 4) {
            0 => control.play(shared, options.song as i32),
            bars => control.play_count_in(shared, options.song, bars),
        }
    }

    pub fn stop(&self) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        control.stop(shared);
    }

    /// `pattern` defaults to the project's active pattern.
    pub fn seek(&self, pattern: Option<i32>, row: i32, sequence: i32) {
        let mut state = self.lock();
        let active = state.project.as_ref().map_or(0, |project| project.active_pattern() as i32);
        let (control, shared, _) = state.core.control.parts();
        control.seek(shared, pattern.unwrap_or(active), row, sequence);
    }

    /// Loop rows `start..=end` of the pattern in Pattern mode; `None` clears the loop.
    pub fn set_pattern_loop(&self, range: Option<(i32, i32)>) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        let (start, end) = range.unwrap_or((-1, -1));
        control.set_pattern_loop(shared, start, end);
    }

    /// Queue a jump for the next pattern boundary; negative values clear it. With `mode`
    /// (`Some(true)` = Song) the playback mode changes at that boundary too.
    pub fn queue(&self, pattern: i32, sequence: i32, mode: Option<bool>) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        match mode {
            Some(song) => control.queue_mode(shared, song, pattern, sequence),
            None => control.queue_jump(shared, pattern, sequence),
        }
    }

    /// Arms or disarms note recording: the engine stamps input acknowledgements accordingly.
    pub fn record_arm(&self, armed: bool) -> bool {
        let mut state = self.lock();
        if state.record_armed != armed {
            state.input_generation += 1;
        }
        state.record_armed = armed;
        let (control, shared, _) = state.core.control.parts();
        control.set_recording(shared, armed);
        armed
    }

    /// All sound off: voices, effect tails, compensation delays.
    pub fn panic(&self) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        control.panic(shared);
        state.core.shared.request_clear_tails();
    }

    // ---- live notes ----------------------------------------------------------------------

    /// The channel and instrument a live note goes to: the given ones, else those of the last
    /// note, clamped to the project.
    pub fn note_destination(&self, track: Option<i64>, instrument: Option<i64>) -> (i32, i32) {
        let mut state = self.lock();
        let (tracks, instruments) = state.project.as_ref().map_or((1, 1), |project| (project.track_list().len().max(1), project.instrument_list().len().max(1)));
        let track = track.unwrap_or(state.live_track as i64).clamp(0, tracks as i64 - 1) as i32;
        let instrument = instrument.unwrap_or(state.live_instrument as i64).clamp(0, instruments as i64 - 1) as i32;
        state.live_track = track;
        state.live_instrument = instrument;
        (track, instrument)
    }

    pub fn note_on(&self, track: i32, instrument: i32, note: i32, velocity: i32) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        control.note_on(shared, track, instrument, note, velocity);
    }

    pub fn note_off(&self, track: i32, note: i32) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        control.note_off(shared, track, note);
    }

    /// A live key edge the audio thread acknowledges with the transport position it landed on
    /// (for recording). `completion` gets the acknowledgement, from [`AudioEngine::poll_inputs`]
    /// or at once when the edge cannot be queued (then the note is played or released directly
    /// if `apply_note` asks for it).
    pub fn input_edge(&self, edge: InputEdge, completion: InputCompletion) {
        let mut state = self.lock();
        let context = state.project.as_ref().map_or(Value::Null, recording_context);
        let refuse = |edge: &InputEdge, context: &Value, id: Option<u64>, reason: &str| {
            let mut result = input_base(edge, context, id);
            result["status"] = json!("unavailable");
            result["reason"] = json!(reason);
            for key in ["applied", "playing", "recording", "mapped"] {
                result[key] = json!(false);
            }
            result
        };
        let fallback = |state: &mut State, edge: &InputEdge| {
            if edge.apply_note {
                let (control, shared, _) = state.core.control.parts();
                if edge.on {
                    control.note_on(shared, edge.track, edge.instrument, edge.note, edge.velocity);
                } else {
                    control.note_off(shared, edge.track, edge.note);
                }
            }
        };
        if edge.owner.is_empty() || edge.owner.len() > 256 {
            fallback(&mut state, &edge);
            drop(state);
            return completion(refuse(&edge, &context, None, "Input recording requires a bounded correlation ID."));
        }
        if state.pending_inputs.len() >= ST_INPUT_ACK_CAPACITY {
            fallback(&mut state, &edge);
            drop(state);
            return completion(refuse(&edge, &context, None, "The input recording queue is full."));
        }
        state.next_input += 1;
        let id = state.next_input;
        let (control, shared, _) = state.core.control.parts();
        let kind = if edge.on { ST_INPUT_NOTE_ON } else { ST_INPUT_NOTE_OFF };
        if !control.input_note(shared, id, kind, edge.track, edge.instrument, edge.note, edge.velocity, edge.apply_note as i32) {
            fallback(&mut state, &edge);
            drop(state);
            return completion(refuse(&edge, &context, Some(id), "The audio engine could not accept this recording edge."));
        }
        let patterns = state.project.as_ref().map_or_else(Vec::new, |project| project.pattern_list().iter().enumerate().map(|(index, pattern)| pattern.number(index)).collect());
        let generation = state.input_generation;
        state.pending_inputs.insert(id, PendingInput { edge, context, patterns, generation, deadline: Instant::now() + Duration::from_secs(1), completion });
    }

    /// Delivers the acknowledgements the audio thread has produced and expires edges that got
    /// none within a second. Call often (the service thread does, every 10 ms).
    pub fn poll_inputs(&self) {
        let mut finished: Vec<(InputCompletion, Value)> = Vec::new();
        {
            let mut state = self.lock();
            if state.pending_inputs.is_empty() {
                return;
            }
            let generation = state.input_generation;
            while let Some(ack) = state.core.control.pop_input_ack() {
                let Some(item) = state.pending_inputs.remove(&ack.request_id) else { continue };
                let mut result = input_base(&item.edge, &item.context, Some(ack.request_id));
                let current = item.generation == generation;
                let applied = current && ack.status == ST_INPUT_ACK_APPLIED && item.patterns.contains(&(ack.pattern as i64));
                result["status"] = json!(if !current {
                    "unavailable"
                } else if applied {
                    "applied"
                } else if ack.status == ST_INPUT_ACK_DISCONTINUITY {
                    "discontinuity"
                } else if ack.status == ST_INPUT_ACK_CONTROL_FLUSH {
                    "controlFlush"
                } else {
                    "unavailable"
                });
                if !current {
                    result["reason"] = json!("The recording session or arm intent changed.");
                }
                result["applied"] = json!(applied);
                result["playing"] = json!(applied && ack.flags & ST_INPUT_CLOCK_PLAYING != 0);
                result["recording"] = json!(applied && ack.flags & ST_INPUT_CLOCK_RECORDING != 0);
                result["mapped"] = json!(applied && ack.flags & ST_INPUT_CLOCK_MAPPED != 0);
                result["pattern"] = json!(ack.pattern);
                result["row"] = json!(ack.row);
                result["sequence"] = json!(ack.sequence);
                result["early"] = json!(ack.early != 0);
                result["renderFrame"] = json!(ack.render_frame.to_string());
                if ack.graph_frame != i64::MIN {
                    result["graphFrame"] = json!(ack.graph_frame.to_string());
                }
                if applied && ack.flags & ST_INPUT_CLOCK_PLAYING != 0 && ack.distance_beats.is_finite() {
                    result["recordingClock"] = json!({ "distanceBeats": ack.distance_beats, "runID": ack.run_id.to_string(), "timingRevision": ack.timing_revision.to_string() });
                }
                finished.push((item.completion, result));
            }
            let now = Instant::now();
            let expired: Vec<u64> = state.pending_inputs.iter().filter(|(_, item)| now >= item.deadline || item.generation != generation).map(|(id, _)| *id).collect();
            for id in expired {
                let Some(item) = state.pending_inputs.remove(&id) else { continue };
                let mut result = input_base(&item.edge, &item.context, Some(id));
                let current = item.generation == generation;
                result["status"] = json!("unavailable");
                result["reason"] = json!(if current { "The audio engine did not acknowledge the input edge in time." } else { "The recording session or arm intent changed before the input was applied." });
                result["ownershipCurrent"] = json!(current);
                for key in ["applied", "playing", "recording", "mapped"] {
                    result[key] = json!(false);
                }
                finished.push((item.completion, result));
            }
        }
        for (completion, result) in finished {
            completion(result);
        }
    }

    /// Auditions one tracker step on `track` for `seconds` without moving the transport. A step
    /// without a note stops the previous audition. Returns whether something sounds.
    pub fn preview_step(&self, track: i32, step: &Step, seconds: f32) -> bool {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        let engine = engine_step(step);
        if engine.note < 0 {
            control.preview_step(shared, track, engine, None, 0.03);
            return false;
        }
        let chord: Vec<i32> = step.chord.as_deref().unwrap_or(&[]).iter().take(ST_MAX_CHORD).map(|note| *note as i32).collect();
        control.preview_step(shared, track, engine, (!chord.is_empty()).then_some(&chord[..]), seconds);
        true
    }

    pub fn pitch_bend(&self, track: i32, semitones: f32) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        control.pitch_bend(shared, track, semitones);
    }

    pub fn mod_wheel(&self, track: i32, value: f32) {
        let mut state = self.lock();
        let (control, shared, _) = state.core.control.parts();
        control.mod_wheel(shared, track, value);
    }

    /// A knob drag heard on sounding voices at once, without republishing the project.
    pub fn instrument_parameter(&self, instrument: i64, name: &str, value: f64) -> Result<bool, String> {
        let mut state = self.lock();
        let count = state.project.as_ref().map_or(0, |project| project.instrument_list().len()) as i64;
        let target = live_instrument_target(name).filter(|_| (0..count).contains(&instrument) && value.is_finite());
        let Some(target) = target else { return Err(format!("Unsupported live instrument parameter {name}.")) };
        let (control, shared, _) = state.core.control.parts();
        Ok(control.set_instrument_parameter(shared, instrument as i32, target, value as f32))
    }

    /// Sets a parameter of an insert now (a knob being turned), by the insert's key
    /// ([`crate::graph::spec::InsertSpec::key`]). False when the mixer has no such insert.
    pub fn set_insert_parameter(&self, key: &str, address: u64, normalized: f64) -> bool {
        let mut state = self.lock();
        let core = &mut state.core;
        match core.compiler.set_live_value(key, address, normalized.clamp(0.0, 1.0)) {
            Some(target) if address <= u32::MAX as u64 => core.shared.send(Message { serial: core.compiler.serial(), target, address: address as u32, value: normalized as f32 }),
            _ => false,
        }
    }

    // ---- status --------------------------------------------------------------------------

    /// Housekeeping, on the status timer: frees what the audio thread has retired, advances the
    /// meters by one reading, and looks after the device (see [`AudioEngine::watch_device`]).
    /// Returns true when the device in use changed.
    pub fn tick(&self) -> bool {
        let mut state = self.lock();
        let core = &mut state.core;
        core.shared.collect();
        let (control, shared, _) = core.control.parts();
        control.collect(shared);
        let meters = core.meters.clone();
        state.display.read(&meters);
        self.watch_device(&mut state)
    }

    /// The status the interface animates from (`status` events and transport replies).
    pub fn status(&self) -> Value {
        let state = self.lock();
        let core = &state.core;
        let mut engine = core.status.status();
        // A Play/Stop queued for the audio thread is reported as requested, not as the stale
        // rendered state.
        let playing = if engine.transport_pending != 0 { engine.requested_playing != 0 } else { engine.playing != 0 };
        let count_in = if engine.transport_pending != 0 { 0 } else { engine.count_in };
        if engine.transport_pending != 0 {
            engine.queued_pattern = -1;
            engine.queued_sequence = -1;
        }
        let (peak_l, peak_r) = state.display.peak(MASTER);
        let track_peaks: Vec<f32> = core.spec.strips.iter().filter(|strip| matches!(strip.kind, StripKind::Track(_))).map(|strip| { let (l, r) = state.display.peak(&strip.id); l.max(r) }).collect();
        let bus_peaks: serde_json::Map<String, Value> = state.display.all().iter().map(|(owner, (l, r))| (owner.clone(), json!([l, r]))).collect();
        let dropped_mix = core.shared.dropped_events();
        let sample_rate = core.compiler.sample_rate();
        let latency = core.shared.latency_frames();
        json!({
            "playing": playing, "songMode": engine.song_mode != 0, "pattern": engine.pattern, "row": engine.row, "sequence": engine.sequence,
            "position": engine.position, "seconds": engine.seconds, "beat": engine.beat, "bpm": engine.bpm, "bar": engine.bar,
            "barStartBeat": engine.bar_start_beat, "numerator": engine.numerator, "denominator": engine.denominator,
            "timingRevision": engine.timing_revision.to_string(), "transportSeekRevision": engine.transport_seek_revision.to_string(),
            "peakL": peak_l, "peakR": peak_r, "inputPeakLeft": 0.0, "inputPeakRight": 0.0, "inputMetering": false,
            "trackPeaks": track_peaks, "busPeaks": bus_peaks,
            "routing": routing_status(core), "routingFailure": Value::Null,
            "cpu": (core.shared.load() as f64 * 100.0), "sampleRate": sample_rate,
            "recordArmSupported": true, "inputAcknowledgements": input_capability(), "noteRecordArmed": state.record_armed,
            "transportRecording": engine.recording != 0, "queuedPattern": engine.queued_pattern, "queuedSequence": engine.queued_sequence,
            "countIn": count_in, "transportPending": engine.transport_pending != 0,
            "recording": false, "recordingStopping": false, "recordingDuration": 0.0, "exporting": false,
            "running": state.stream.as_ref().is_some_and(|stream| stream.is_alive()),
            "droppedCommands": engine.dropped_commands, "droppedEvents": engine.dropped_events + dropped_mix, "engineDroppedEvents": engine.dropped_events,
            "latencyCompensation": { "frames": latency, "seconds": latency as f64 / sample_rate },
            "pluginsRestoring": false, "playPreparing": false, "previewPreparing": false, "graphMutationsPending": 0,
        })
    }

    /// The mixer as compiled: strips, inserts (with the unavailable ones) and unconnected sidechains.
    pub fn routing_status(&self) -> Value {
        routing_status(&self.lock().core)
    }

    /// The inserts of the mixer in use.
    pub fn inserts(&self) -> Vec<InsertStatus> {
        self.lock().core.compiler.insert_report().to_vec()
    }

    /// The project as the engine holds it, with the state of every insert captured now
    /// (`engine.snapshot`). Built-in effects are captured; plug-ins keep the state they carry.
    pub fn snapshot(&self) -> Value {
        let state = self.lock();
        let Some(project) = &state.project else { return json!({ "audioPlugins": [] }) };
        let mut value = project.to_value();
        let spec = &state.core.spec;
        let mut plugins = Vec::new();
        for descriptor in project.plugin_list() {
            let mut item = serde_json::to_value(descriptor_value(descriptor)).unwrap_or(Value::Null);
            let instance = descriptor.instance_id.as_deref().unwrap_or_default();
            let insert = spec.strips.iter().flat_map(|strip| &strip.inserts).find(|insert| !instance.is_empty() && insert.instance_id == instance);
            if let Some((insert, effect)) = insert.and_then(|insert| insert.builtin_id().map(|effect| (insert, effect))) {
                let names = builtin_parameter_ids(effect);
                let values: Vec<(String, f64)> = state.core.compiler.insert_values(insert).into_iter().filter_map(|(address, value)| names.get(address as usize).map(|name| (name.clone(), value))).collect();
                if !values.is_empty() {
                    item["state"] = json!(encode_builtin_state(effect, &values));
                }
            }
            plugins.push(item);
        }
        value["audioPlugins"] = Value::Array(plugins);
        value
    }

    // ---- devices -------------------------------------------------------------------------

    fn open_device(&self, state: &mut State) -> Result<(), String> {
        state.stream = None;
        state.device_error = None;
        let rate = state.core.compiler.sample_rate();
        let wanted = match &state.wanted {
            DeviceChoice::None => return Ok(()),
            DeviceChoice::Null => None,
            DeviceChoice::SystemDefault => system::list().default_output,
            DeviceChoice::Named(uid) => Some(uid.clone()),
        };
        if let Some(uid) = wanted {
            let opened = system::output_rate(&uid, rate).and_then(|device_rate| {
                if device_rate != rate {
                    self.restart_engine(state, device_rate)?;
                }
                system::SystemOutput::start(self.renderer.clone(), &uid, device_rate, state.buffer_size)
            });
            match opened {
                Ok(stream) => {
                    state.stream = Some(Box::new(stream));
                    return Ok(());
                }
                Err(error) => state.device_error = Some(error),
            }
        } else if state.wanted == DeviceChoice::SystemDefault {
            state.device_error = Some("No audio output device is connected.".to_string());
        }
        let rate = state.core.compiler.sample_rate();
        state.stream = Some(Box::new(NullDevice::start(self.renderer.clone(), rate, state.buffer_size)?));
        Ok(())
    }

    /// Replaces the engine by one at `sample_rate` and applies the current project to it. The
    /// device must be stopped (no stream).
    fn restart_engine(&self, state: &mut State, sample_rate: f64) -> Result<(), String> {
        let samples = self.samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let (core, renderer) = Core::new(sample_rate, samples, self.inserts.clone())?;
        // The old renderer (and with it the old engine's render half) is dropped here, on the
        // control thread; no device thread exists at this point.
        *self.renderer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = renderer;
        state.core = core;
        state.display.reset();
        state.pending_inputs.clear();
        if let Some(project) = state.project.clone() {
            state.last_report = Self::publish_locked(state, &project)?;
        }
        let armed = state.record_armed;
        let (control, shared, _) = state.core.control.parts();
        control.set_recording(shared, armed);
        Ok(())
    }

    /// Device loss and return. A stream that died is replaced (by the wanted device if it still
    /// exists, else by the null device); while the null device stands in for a wanted device,
    /// the wanted one is tried again every two seconds. Returns true when the device changed.
    fn watch_device(&self, state: &mut State) -> bool {
        if state.wanted == DeviceChoice::None {
            return false;
        }
        let dead = state.stream.as_ref().is_some_and(|stream| !stream.is_alive());
        let standing_in = state.stream.as_ref().is_some_and(|stream| stream.uid() == NULL_DEVICE) && state.wanted != DeviceChoice::Null;
        let due = state.last_device_check.elapsed() >= Duration::from_secs(2);
        if !dead && !(standing_in && due) {
            return false;
        }
        state.last_device_check = Instant::now();
        if standing_in && !dead {
            // Only leave the null device when the wanted one is really there.
            if !system::may_have_outputs() {
                return false;
            }
            let devices = system::list();
            let available = match &state.wanted {
                DeviceChoice::Named(uid) => devices.outputs.iter().any(|device| device.uid == *uid),
                _ => devices.default_output.is_some(),
            };
            if !available {
                return false;
            }
        }
        let before = state.stream.as_ref().map(|stream| stream.uid().to_string());
        let _ = self.open_device(state);
        before != state.stream.as_ref().map(|stream| stream.uid().to_string())
    }

    /// `audio.devices`.
    pub fn devices(&self) -> DevicesStatus {
        let state = self.lock();
        Self::devices_locked(&state)
    }

    fn devices_locked(state: &State) -> DevicesStatus {
        let rate = state.core.compiler.sample_rate();
        let listed = if state.wanted == DeviceChoice::None { system::SystemDevices::default() } else { system::list() };
        let mut outputs = listed.outputs.clone();
        outputs.push(device::null_device_info(rate, state.buffer_size));
        let mut devices = outputs.clone();
        for input in &listed.inputs {
            if !devices.iter().any(|device| device.uid == input.uid) {
                devices.push(input.clone());
            }
        }
        let uid = state.stream.as_ref().map_or(NULL_DEVICE.to_string(), |stream| stream.uid().to_string());
        let id = device::device_id(&uid);
        let latency = state.core.shared.latency_frames();
        DevicesStatus {
            devices,
            inputs: listed.inputs.clone(),
            outputs,
            default_input: listed.default_input.as_deref().map(device::device_id),
            default_output: listed.default_output.as_deref().map(device::device_id),
            output_device: id,
            output_uid: uid.clone(),
            engine_device: id,
            input_device: listed.default_input.as_deref().map(device::device_id),
            sample_rate: rate,
            buffer_size: state.stream.as_ref().map_or(state.buffer_size, |stream| stream.buffer_size()),
            buffer_sizes: device::BUFFER_SIZES.to_vec(),
            running: state.stream.as_ref().is_some_and(|stream| stream.is_alive()),
            fallback: uid == NULL_DEVICE && !matches!(state.wanted, DeviceChoice::Null | DeviceChoice::None),
            device_error: state.device_error.clone(),
            input_channels: Vec::new(),
            input_metering: false,
            monitoring: false,
            monitor_volume: 0.0,
            input_acknowledgements: input_capability(),
            latency_compensation: json!({ "frames": latency, "seconds": latency as f64 / rate }),
        }
    }

    /// `audio.configure`: selects the output, the buffer size and the sample rate. A device that
    /// cannot be opened leaves the null device running and its reason in `deviceError`.
    pub fn configure(&self, request: &DeviceRequest) -> Result<DevicesStatus, String> {
        let mut state = self.lock();
        if state.wanted == DeviceChoice::None {
            return Err("This audio engine renders offline and has no device.".to_string());
        }
        let mut reopen = false;
        if let Some(size) = request.buffer_size {
            if !(16..=8192).contains(&size) {
                return Err("Choose a buffer size from 16 to 8192 samples.".to_string());
            }
            reopen |= state.buffer_size != size;
            state.buffer_size = size;
        }
        let chosen = if request.system_default {
            Some(DeviceChoice::SystemDefault)
        } else if let Some(uid) = request.output_uid.as_deref() {
            Some(if uid == NULL_DEVICE { DeviceChoice::Null } else { DeviceChoice::Named(uid.to_string()) })
        } else if let Some(id) = request.output_id {
            if id == 0 {
                Some(DeviceChoice::Null)
            } else {
                let found = system::list().outputs.into_iter().find(|device| device.id == id);
                Some(DeviceChoice::Named(found.ok_or("That audio output is not connected.")?.uid))
            }
        } else {
            None
        };
        if let Some(choice) = chosen {
            reopen |= state.wanted != choice;
            state.wanted = choice;
        }
        let rate = request.sample_rate.filter(|rate| *rate != state.core.compiler.sample_rate());
        if let Some(rate) = rate {
            if !(8_000.0..=384_000.0).contains(&rate) {
                return Err("Choose a sample rate from 8 to 384 kHz.".to_string());
            }
            state.stream = None;
            self.restart_engine(&mut state, rate)?;
            reopen = true;
        }
        if reopen || state.stream.is_none() {
            self.open_device(&mut state)?;
        }
        Ok(Self::devices_locked(&state))
    }

    /// The device choice in effect (for the settings file).
    pub fn device_choice(&self) -> (DeviceChoice, u32) {
        let state = self.lock();
        (state.wanted.clone(), state.buffer_size)
    }

    // ---- offline -------------------------------------------------------------------------

    /// Renders the next `frames` frames in blocks of `block` frames, as fast as the machine
    /// allows, through the same renderer a device would call. For an engine without a device.
    pub fn render_offline(&self, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0f32; frames], vec![0.0f32; frames]);
        let mut renderer = self.renderer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let block = block.max(1);
        let mut at = 0;
        while at < frames {
            let end = (at + block).min(frames);
            renderer.render(&mut left[at..end], &mut right[at..end]);
            at = end;
        }
        (left, right)
    }

    /// The renderer, for callers that drive it themselves (benchmarks, the allocation test).
    pub fn renderer(&self) -> SharedRenderer {
        self.renderer.clone()
    }

    // ---- service thread ------------------------------------------------------------------

    /// Starts the engine's service thread: every 10 ms it delivers input acknowledgements, every
    /// 50 ms (20 times a second) it runs [`AudioEngine::tick`] and then calls `on_status` with
    /// the engine and whether the device changed. The thread ends when the engine is dropped or
    /// [`AudioEngine::stop_service`] is called.
    pub fn start_service(self: &Arc<Self>, on_status: impl Fn(&AudioEngine, bool) + Send + 'static) {
        let engine: Weak<AudioEngine> = Arc::downgrade(self);
        let stop = self.service_stop.clone();
        let _ = std::thread::Builder::new().name("audio-service".to_string()).spawn(move || {
            let mut count = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
                let Some(engine) = engine.upgrade() else { break };
                engine.poll_inputs();
                count += 1;
                if count % 5 == 0 {
                    let changed = engine.tick();
                    on_status(&engine, changed);
                }
            }
        });
    }

    pub fn stop_service(&self) {
        self.service_stop.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Stops the device and the service thread (the host is shutting down).
    pub fn shutdown(&self) {
        self.stop_service();
        let mut state = self.lock();
        state.stream = None;
        state.wanted = DeviceChoice::None;
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.service_stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn routing_status(core: &Core) -> Value {
    let unavailable: Vec<Value> = core.compiler.unavailable_routes().iter().map(|(id, reason)| json!({ "id": id, "reason": reason })).collect();
    json!({
        "standard": true,
        "strips": core.spec.strips.iter().map(|strip| strip.id.clone()).collect::<Vec<_>>(),
        "inserts": core.compiler.insert_report(),
        "unavailableSidechains": unavailable,
        "latencyFrames": core.shared.latency_frames(),
    })
}

fn descriptor_value(descriptor: &st_project::model::PluginDescriptor) -> Value {
    // A descriptor has no serializer of its own outside a project: go through one.
    let project = Project { audio_plugins: Some(vec![descriptor.clone()]), ..Project::default() };
    project.to_value()["audioPlugins"][0].clone()
}

fn builtin_parameter_ids(effect: &str) -> Vec<String> {
    let Ok(id) = std::ffi::CString::new(effect) else { return Vec::new() };
    tracker_engine::effects::find(&id).map_or_else(Vec::new, |descriptor| descriptor.params.iter().map(|info| info.id().to_string_lossy().into_owned()).collect())
}

fn input_base(edge: &InputEdge, context: &Value, id: Option<u64>) -> Value {
    let mut result = json!({
        "version": 1, "inputOwner": edge.owner, "on": edge.on, "track": edge.track, "instrument": edge.instrument,
        "note": edge.note, "velocity": edge.velocity, "applyNote": edge.apply_note, "recordingContext": context,
    });
    if let Some(id) = id {
        result["requestID"] = json!(id.to_string());
    }
    result
}

/// What an input acknowledgement says about the project it was recorded against: the timing
/// and shape the interface needs to place the note, without musical content.
pub fn recording_context(project: &Project) -> Value {
    let patterns: Vec<Value> = project.pattern_list().iter().enumerate().map(|(index, pattern)| json!({ "id": pattern.number(index), "length": pattern.length().clamp(1, 256) })).collect();
    let clips: Vec<Value> = project
        .clip_list()
        .iter()
        .map(|clip| {
            json!({
                "id": clip.id.clone().unwrap_or_default(), "track": clip.track(), "startBeat": clip.start_beat(), "durationBeats": clip.duration_beats(),
                "path": clip.file().unwrap_or_default(), "sourceOffset": clip.source_offset().clamp(0.0, 1e6), "muted": clip.muted(),
            })
        })
        .collect();
    json!({
        "version": 1, "linesPerBeat": project.lines_per_beat().clamp(1, 32), "swing": (project.swing() as f32).clamp(0.0, 0.95) as f64,
        "loopSong": project.loop_song(), "bpm": project.bpm().clamp(20.0, 999.0), "timing": Value::Null,
        "patterns": patterns, "sequence": project.sequence.clone().unwrap_or_default(), "clips": clips,
    })
}

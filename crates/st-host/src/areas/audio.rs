//! The audio area: the commands of `st-audio`'s playback core.
//!
//! [`install`] creates the [`AudioEngine`], attaches it to project publications
//! ([`ProjectAudio`]), registers the commands below and starts the engine's service thread,
//! which sends the `status` event twenty times a second (`emit_latest`: a page that is busy
//! gets the newest one only).
//!
//! | Command | Notes |
//! |---|---|
//! | `engine.project` | through [`ProjectAudio`]: the engine and the mixer follow every publication; the reply is the status |
//! | `transport.play`, `.stop`, `.seek`, `.loop`, `.queue`, `.recordArm` | the reply of the first four is the status |
//! | `note.on`, `note.off` | with `captureInput` the reply waits for the audio thread's acknowledgement (`inputAcknowledgement`) |
//! | `step.preview`, `engine.panic`, `engine.instrumentParameter`, `midi.expression` | `midi.expression` reaches the engine (pitch bend, mod wheel, sustain); plug-ins and MIDI outputs do not exist yet |
//! | `engine.snapshot` | the project with built-in effect state captured; plug-in descriptors keep the state they carry |
//! | `audio.devices`, `audio.configure` | outputs; the choice is remembered in the settings section `audio.devices` |
//! | `builtin.effects`, `routing.status` | |
//!
//! Events: `status` (20 Hz), `audio.devices` (the output changed by itself: a device was lost
//! or came back), `sample.error` (a sample of the published project could not be loaded).
//!
//! Samples come from `st-samples` through [`st_audio::DecodedSamples`], which decodes on its own
//! worker threads; the sample commands and the events of finished decodes are in
//! [`crate::areas::samples`].

use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use st_audio::graph::insert::{builtin_effects_metadata, BuiltinFactory};
use st_audio::{AudioEngine, DecodedSamples, DeviceChoice, DeviceRequest, EngineOptions, InputEdge, PlayOptions};
use st_project::model::Step;
use st_project::Project;

use crate::commands::{Commands, NoPayload, Reply};
use crate::session::{ProjectAudio, PublishOptions};
use crate::{fail, Error, EventSink, Result, Settings};

/// Which output the host starts on.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioOutput {
    /// What the settings say; the system's default output otherwise. The null device stands in
    /// while no output is connected.
    System,
    /// The null device, whatever the settings say (tests, servers).
    Null,
}

/// How [`crate::install`] sets the audio engine up.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioOptions {
    pub output: AudioOutput,
}

impl AudioOptions {
    /// The system's devices, unless the environment variable `SUPERTRACKER_AUDIO_DEVICE` is
    /// `null`.
    pub fn from_environment() -> AudioOptions {
        let null = std::env::var("SUPERTRACKER_AUDIO_DEVICE").is_ok_and(|value| value == "null");
        AudioOptions { output: if null { AudioOutput::Null } else { AudioOutput::System } }
    }
}

/// The settings section `audio.devices`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
struct DeviceSettings {
    /// `uid` of the chosen output; absent to follow the system's default.
    output: Option<String>,
    buffer_size: Option<u32>,
    sample_rate: Option<f64>,
}

const SETTINGS_SECTION: &str = "audio.devices";

/// The audio service of the host.
pub struct Audio {
    engine: Arc<AudioEngine>,
    /// Where the engine's samples come from (decoded on worker threads).
    samples: Arc<DecodedSamples>,
    events: EventSink,
    settings: Arc<Settings>,
    /// Sustain pedal of the interface's keyboard: down, and the notes whose release it holds.
    sustain: Mutex<(bool, Vec<(i32, i32)>)>,
    /// Sample problems already reported, so a publication repeats none.
    reported: Mutex<Vec<st_audio::SampleProblem>>,
    /// Called on `transport.stop` and `engine.panic`: Stop also cancels a running export.
    stop_hooks: Mutex<Vec<Box<dyn Fn() + Send + Sync>>>,
}

impl ProjectAudio for Audio {
    fn prepare(&self, project: &Project, options: &PublishOptions) -> Result<Value> {
        // An opened or replaced project loads every plug-in's saved state again.
        let report = self.engine.publish_with(project, options.force_plugin_state).map_err(Error::new)?;
        self.report_samples(report);
        crate::areas::samples::retain_for(&self.samples, project);
        Ok(self.engine.status())
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PlayPayload {
    mode: Option<String>,
    song_mode: Option<bool>,
    from_start: bool,
    #[serde(rename = "loop")]
    range: Value,
    count_in: f64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SeekPayload {
    pattern: Option<f64>,
    row: f64,
    sequence: f64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QueuePayload {
    pattern: Option<f64>,
    sequence: Option<f64>,
    mode: Option<String>,
    clear: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct NotePayload {
    note: Option<f64>,
    velocity: Option<f64>,
    track: Option<f64>,
    instrument: Option<f64>,
    capture_input: bool,
    input_owner: Option<String>,
    apply_note: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PreviewPayload {
    step: Option<Value>,
    track: Option<f64>,
    instrument: Option<f64>,
    duration: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct InstrumentParameterPayload {
    instrument: Option<f64>,
    target: String,
    value: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ExpressionPayload {
    kind: String,
    track: Option<f64>,
    controller: Option<f64>,
    value: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ConfigurePayload {
    output: Option<String>,
    output_device: Option<f64>,
    output_system_default: bool,
    buffer_size: Option<f64>,
    sample_rate: Option<f64>,
    /// The input: its `uid`, or its id; the source channels (one or two); monitoring; the meter.
    input: Option<String>,
    input_device: Option<f64>,
    input_channels: Option<Vec<f64>>,
    monitor: Option<bool>,
    monitor_volume: Option<f64>,
    input_meter: Option<bool>,
}

#[derive(Deserialize)]
struct ArmPayload {
    armed: bool,
}

/// `{start, end}` as an inclusive row range, when it is one.
fn pattern_loop(value: &Value) -> Option<(i32, i32)> {
    let (start, end) = (value.get("start")?.as_f64()?, value.get("end")?.as_f64()?);
    (start >= 0.0 && end >= start && start < 256.0).then_some((start as i32, end.min(255.0) as i32))
}

fn whole(value: Option<f64>) -> Option<i64> {
    value.filter(|value| value.is_finite()).map(|value| value.clamp(-1e9, 1e9) as i64)
}

impl Audio {
    /// The engine, for other areas of the host (samples, export, plug-ins).
    pub fn engine(&self) -> &Arc<AudioEngine> {
        &self.engine
    }

    /// The sample cache the engine reads from.
    pub fn samples(&self) -> &Arc<DecodedSamples> {
        &self.samples
    }

    /// Tells the interface about samples that cannot be loaded, each once: `sample.error` for an
    /// instrument's sample or zone, `audio.clipError` for a timeline clip's file.
    fn report_samples(&self, report: st_audio::ApplyReport) {
        let mut reported = self.reported.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for problem in &report.problems {
            if let (Some(error), false) = (&problem.error, reported.contains(problem)) {
                match problem.instrument {
                    Some(instrument) => self.events.emit("sample.error", json!({ "instrument": instrument, "path": problem.path, "error": error })),
                    None => self.events.emit("audio.clipError", json!({ "path": problem.path, "error": error })),
                }
            }
        }
        // What stays reported: the failures of this report, and earlier failures of samples this
        // report (made while they were decoded again, or before their failure was known to it)
        // lists as pending.
        let mut kept: Vec<st_audio::SampleProblem> = reported.iter().filter(|known| report.problems.iter().any(|problem| problem.error.is_none() && problem.instrument == known.instrument && problem.path == known.path)).cloned().collect();
        kept.extend(report.problems.into_iter().filter(|problem| problem.error.is_some()));
        *reported = kept;
    }

    /// Decoded samples arrived (or changed on disk): installs them in the engine and reports
    /// the ones that failed.
    pub fn samples_changed(&self) -> Result<()> {
        let report = self.engine.samples_changed().map_err(Error::new)?;
        self.report_samples(report);
        Ok(())
    }

    fn play(&self, payload: PlayPayload) -> Result<Value> {
        let song = payload.mode.as_deref() != Some("pattern") && payload.song_mode != Some(false);
        let count_in = if payload.count_in.is_finite() { payload.count_in.clamp(0.0, 4.0) as i32 } else { 0 };
        self.engine.play(&PlayOptions { song, from_start: payload.from_start, pattern_loop: pattern_loop(&payload.range), count_in });
        Ok(self.engine.status())
    }

    /// Adds something Stop (and panic) also does: cancelling a running export.
    pub fn on_stop(&self, hook: impl Fn() + Send + Sync + 'static) {
        self.stop_hooks.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(Box::new(hook));
    }

    fn run_stop_hooks(&self) {
        for hook in self.stop_hooks.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter() {
            hook();
        }
    }

    fn stop(&self, _: NoPayload) -> Result<Value> {
        self.run_stop_hooks();
        self.engine.stop();
        Ok(self.engine.status())
    }

    fn seek(&self, payload: SeekPayload) -> Result<Value> {
        self.engine.seek(whole(payload.pattern).map(|pattern| pattern as i32), whole(Some(payload.row)).unwrap_or(0) as i32, whole(Some(payload.sequence)).unwrap_or(0) as i32);
        Ok(self.engine.status())
    }

    /// `transport.loop`: `{loop: {start, end}}`, the range itself, or anything else to clear.
    fn set_loop(&self, payload: Value) -> Result<Value> {
        self.engine.set_pattern_loop(pattern_loop(payload.get("loop").unwrap_or(&payload)));
        Ok(self.engine.status())
    }

    fn queue(&self, payload: QueuePayload) -> Result<Value> {
        let pattern = if payload.clear { -1 } else { whole(payload.pattern).unwrap_or(-1) };
        let sequence = if payload.clear { -1 } else { whole(payload.sequence).unwrap_or(-1) };
        if !payload.clear && pattern < 0 && sequence < 0 {
            fail!("Queue a pattern or sequence position.");
        }
        let playing = self.engine.status()["playing"] == true;
        if let (false, Some(mode)) = (payload.clear, payload.mode.as_deref()) {
            if mode != "song" && mode != "pattern" {
                fail!("Queue mode must be song or pattern.");
            }
            if (mode == "song" && sequence < 0) || (mode == "pattern" && pattern < 0) {
                fail!("Queue a sequence position for Song mode or a pattern for Pattern mode.");
            }
            self.engine.queue(pattern as i32, sequence as i32, Some(mode == "song"));
            return Ok(json!({ "queued": playing, "mode": mode, "pattern": pattern, "sequence": sequence }));
        }
        self.engine.queue(pattern as i32, sequence as i32, None);
        Ok(json!({ "queued": playing && !payload.clear, "pattern": pattern, "sequence": sequence }))
    }

    fn record_arm(&self, payload: ArmPayload) -> Result<Value> {
        Ok(json!({ "armed": self.engine.record_arm(payload.armed) }))
    }

    fn panic(&self, _: NoPayload) -> Result<Value> {
        *self.sustain.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = (false, Vec::new());
        self.run_stop_hooks();
        self.engine.panic();
        self.engine.live().preview().stop(true);
        Ok(self.engine.status())
    }

    fn note_on(&self, payload: NotePayload, reply: Reply<Value>) {
        let (track, instrument) = self.engine.note_destination(whole(payload.track), whole(payload.instrument));
        let note = whole(payload.note).unwrap_or(60).clamp(0, 127) as i32;
        let velocity = whole(payload.velocity).unwrap_or(100).clamp(0, 127) as i32;
        // A key struck again while the pedal holds its release is no longer waiting to be released.
        self.sustain.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).1.retain(|held| *held != (track, note));
        if payload.capture_input {
            let edge = InputEdge { owner: payload.input_owner.unwrap_or_default(), on: true, track, instrument, note, velocity, apply_note: payload.apply_note.unwrap_or(true) };
            return self.engine.input_edge(edge, Box::new(move |acknowledgement| reply.ok(json!({ "ok": true, "inputAcknowledgement": acknowledgement }))));
        }
        self.engine.note_on(track, instrument, note, velocity);
        reply.ok(json!({ "ok": true }));
    }

    fn note_off(&self, payload: NotePayload, reply: Reply<Value>) {
        let (track, instrument) = self.engine.note_destination(whole(payload.track), whole(payload.instrument));
        let note = whole(payload.note).unwrap_or(60).clamp(0, 127) as i32;
        let apply = payload.apply_note.unwrap_or(true);
        // A held sustain pedal defers the sounding release; the key release is still captured.
        let sustained = apply && {
            let mut sustain = self.sustain.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let down = sustain.0;
            if down && !sustain.1.contains(&(track, note)) {
                sustain.1.push((track, note));
            }
            down
        };
        if payload.capture_input {
            let velocity = whole(payload.velocity).unwrap_or(0).clamp(0, 127) as i32;
            let edge = InputEdge { owner: payload.input_owner.unwrap_or_default(), on: false, track, instrument, note, velocity, apply_note: apply && !sustained };
            return self.engine.input_edge(edge, Box::new(move |acknowledgement| reply.ok(json!({ "ok": true, "inputAcknowledgement": acknowledgement }))));
        }
        if sustained {
            return reply.ok(json!({ "ok": true, "sustained": true }));
        }
        self.engine.note_off(track, note);
        reply.ok(json!({ "ok": true }));
    }

    fn preview(&self, payload: PreviewPayload) -> Result<Value> {
        let Some(step) = payload.step.filter(Value::is_object) else { fail!("Choose a tracker step to audition.") };
        let step: Step = Project::from_value(json!({ "patterns": [{ "steps": [[step]] }] }))?.pattern_list().first().and_then(|pattern| pattern.step(0, 0)).cloned().unwrap_or_default();
        let (track, _) = self.engine.note_destination(whole(payload.track), whole(payload.instrument));
        let seconds = payload.duration.filter(|seconds| seconds.is_finite()).unwrap_or(0.35) as f32;
        Ok(json!({ "auditioning": self.engine.preview_step(track, &step, seconds) }))
    }

    fn instrument_parameter(&self, payload: InstrumentParameterPayload) -> Result<Value> {
        let applied = self.engine.instrument_parameter(whole(payload.instrument).unwrap_or(-1), &payload.target, payload.value.unwrap_or(f64::NAN)).map_err(Error::new)?;
        Ok(json!({ "ok": applied }))
    }

    /// `midi.expression`, as far as the engine goes: pitch bend (±2 semitones), the mod wheel
    /// and the sustain pedal. Other controllers have nowhere to go until plug-ins and MIDI
    /// outputs exist; they answer `sent: false`.
    fn expression(&self, payload: ExpressionPayload) -> Result<Value> {
        let Some(value) = payload.value.filter(|value| value.is_finite()) else { fail!("Expression value must be a finite number.") };
        let (track, instrument) = self.engine.note_destination(whole(payload.track), None);
        let controller = whole(payload.controller).unwrap_or(1).clamp(0, 127);
        let (kind, controller) = if payload.kind == "sustain" { ("cc", 64) } else { (payload.kind.as_str(), controller) };
        let sent = match (kind, controller) {
            ("pitchBend", _) => {
                self.engine.pitch_bend(track, value.clamp(-1.0, 1.0) as f32 * 2.0);
                true
            }
            ("cc", 1) => {
                self.engine.mod_wheel(track, value.clamp(0.0, 1.0) as f32);
                true
            }
            ("cc", 64) => {
                let released = {
                    let mut sustain = self.sustain.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    sustain.0 = value >= 0.5;
                    if sustain.0 { Vec::new() } else { std::mem::take(&mut sustain.1) }
                };
                for (track, note) in released {
                    self.engine.note_off(track, note);
                }
                true
            }
            ("cc", controller) if controller >= 120 => fail!("Use engine.panic for channel-mode messages."),
            ("cc", _) | ("aftertouch", _) | ("polyAftertouch", _) => false,
            (other, _) => fail!("Unsupported expression kind: {other}."),
        };
        Ok(json!({ "sent": sent, "track": track, "instrument": instrument }))
    }

    fn snapshot(&self, _: NoPayload) -> Result<Value> {
        Ok(self.engine.snapshot())
    }

    fn routing_status(&self, _: NoPayload) -> Result<Value> {
        let mut status = self.engine.routing_status();
        status["projectUnavailable"] = json!(false);
        status["failure"] = Value::Null;
        Ok(status)
    }

    fn effects(&self, _: NoPayload) -> Result<Value> {
        Ok(builtin_effects_metadata())
    }

    fn devices_value(&self, status: st_audio::DevicesStatus) -> Value {
        // The input follows the engine's format; the null input is offered with the null output.
        let live = self.engine.live();
        live.inputs().set_format(status.sample_rate, status.buffer_size, status.output_uid == st_audio::device::NULL_DEVICE);
        let mut value = serde_json::to_value(status).unwrap_or(Value::Null);
        live.decorate_devices(&mut value);
        // Line inputs into track strips are not implemented yet; the interface reads these keys.
        value["lineInputs"] = json!([]);
        value["lineInputError"] = Value::Null;
        value
    }

    fn devices(&self, _: NoPayload) -> Result<Value> {
        Ok(self.devices_value(self.engine.devices()))
    }

    fn configure(&self, payload: ConfigurePayload) -> Result<Value> {
        let live = self.engine.live();
        let changes_output = payload.output.is_some() || payload.output_device.is_some() || payload.output_system_default || payload.buffer_size.is_some() || payload.sample_rate.is_some();
        if changes_output && live.is_recording() {
            fail!("Finish recording before reconfiguring audio.");
        }
        let input = st_audio::inputs::InputRequest {
            uid: payload.input.clone(),
            id: whole(payload.input_device).map(|id| id.clamp(0, u32::MAX as i64) as u32),
            channels: payload.input_channels.as_ref().map(|channels| channels.iter().filter_map(|channel| whole(Some(*channel))).map(|channel| channel.clamp(0, 4096) as u32).collect()),
            monitor: payload.monitor,
            monitor_volume: payload.monitor_volume.map(|volume| volume as f32),
            meter: payload.input_meter,
        };
        let request = DeviceRequest {
            output_id: whole(payload.output_device).map(|id| id.clamp(0, u32::MAX as i64) as u32),
            output_uid: payload.output,
            system_default: payload.output_system_default,
            buffer_size: whole(payload.buffer_size).map(|size| size.clamp(0, 1 << 20) as u32),
            sample_rate: payload.sample_rate,
        };
        let status = self.engine.configure(&request).map_err(Error::new)?;
        let (choice, buffer_size) = self.engine.device_choice();
        let output = match choice {
            DeviceChoice::Named(uid) => Some(uid),
            DeviceChoice::Null => Some(st_audio::device::NULL_DEVICE.to_string()),
            DeviceChoice::SystemDefault | DeviceChoice::None => None,
        };
        self.settings.set(SETTINGS_SECTION, &DeviceSettings { output, buffer_size: Some(buffer_size), sample_rate: Some(status.sample_rate) })?;
        // The input after the output: it runs at the engine's (possibly new) sample rate.
        live.inputs().set_format(status.sample_rate, status.buffer_size, status.output_uid == st_audio::device::NULL_DEVICE);
        live.inputs().configure(&input).map_err(Error::new)?;
        Ok(self.devices_value(status))
    }
}

/// Creates the audio engine, attaches it to `session` and registers the audio commands.
pub fn install(commands: &mut Commands, session: &crate::Session, settings: &Arc<Settings>, events: EventSink, options: &AudioOptions) -> Result<Arc<Audio>> {
    let saved: DeviceSettings = settings.get(SETTINGS_SECTION);
    let device = match (&options.output, saved.output.as_deref()) {
        (AudioOutput::Null, _) | (_, Some(st_audio::device::NULL_DEVICE)) => DeviceChoice::Null,
        (AudioOutput::System, Some(uid)) => DeviceChoice::Named(uid.to_string()),
        (AudioOutput::System, None) => DeviceChoice::SystemDefault,
    };
    let samples = Arc::new(DecodedSamples::for_host());
    let engine = AudioEngine::new(EngineOptions {
        sample_rate: saved.sample_rate.filter(|rate| (8_000.0..=384_000.0).contains(rate)).unwrap_or(48_000.0),
        buffer_size: saved.buffer_size.unwrap_or(512),
        samples: samples.clone(),
        inserts: Arc::new(BuiltinFactory),
        device,
    })
    .map_err(Error::new)?;
    let audio = Arc::new(Audio { engine, samples, events: events.clone(), settings: settings.clone(), sustain: Mutex::default(), reported: Mutex::default(), stop_hooks: Mutex::default() });
    session.attach_audio(audio.clone());
    commands
        .with(&audio)
        .register("transport.play", Audio::play)
        .register("transport.stop", Audio::stop)
        .register("transport.seek", Audio::seek)
        .register("transport.loop", Audio::set_loop)
        .register("transport.queue", Audio::queue)
        .register("transport.recordArm", Audio::record_arm)
        .register("engine.panic", Audio::panic)
        .register_deferred("note.on", Audio::note_on)
        .register_deferred("note.off", Audio::note_off)
        .register("step.preview", Audio::preview)
        .register("engine.instrumentParameter", Audio::instrument_parameter)
        .register("midi.expression", Audio::expression)
        .register("engine.snapshot", Audio::snapshot)
        .register("routing.status", Audio::routing_status)
        .register("builtin.effects", Audio::effects)
        .register_blocking("audio.devices", Audio::devices)
        .register_blocking("audio.configure", Audio::configure);
    let status_events = events;
    let weak = Arc::downgrade(&audio);
    audio.engine.start_service(move |engine, device_changed| {
        status_events.emit_latest("status", engine.status());
        if device_changed {
            if let Some(audio) = weak.upgrade() {
                status_events.emit("audio.devices", audio.devices_value(engine.devices()));
            }
        }
    });
    Ok(audio)
}

impl Drop for Audio {
    fn drop(&mut self) {
        // A take in progress is closed as it is; the input device is released.
        let _ = self.engine.live().stop_record();
        self.engine.live().inputs().close();
        self.engine.shutdown();
    }
}

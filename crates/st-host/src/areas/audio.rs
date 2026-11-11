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
//! | `step.preview`, `engine.panic`, `engine.instrumentParameter` | |
//! | `midi.expression` | pitch bend, controllers, the pedals, channel and poly pressure for a track's instrument: the device of an external MIDI instrument, a plug-in instrument, the engine's own voices (see the handler, `Audio::expression`) |
//! | `engine.snapshot` | the project with built-in effect state captured; plug-in descriptors keep the state they carry |
//! | `audio.devices`, `audio.configure` | outputs and the input; the choice is remembered in the settings section `audio.devices`. `audio.devices` also reports the line inputs of the published project (`lineInputs`, `lineInputError`) |
//! | `builtin.effects`, `routing.status` | |
//!
//! Line inputs have no command: a track's `audioInput` in the project is the whole routing, and
//! every publication applies it (`st_audio`'s `engine/lines.rs`). What the host adds is the
//! state of each route in `audio.devices`, sent as an event when a publication or a device
//! changes it.
//!
//! Events: `status` (20 Hz), `audio.devices` (the output changed by itself: a device was lost
//! or came back; a line input went live or was paused), `sample.error` (a sample of the
//! published project could not be loaded).
//!
//! Samples come from `st-samples` through [`st_audio::DecodedSamples`], which decodes on its own
//! worker threads; the sample commands and the events of finished decodes are in
//! [`crate::areas::samples`].
//!
//! # The pedals
//!
//! The sustain pedal belongs to the instrument. [`Audio`] keeps the one record of the pedals
//! that are down ([`st_midi::SustainState`]), for the notes and pedals of the interface
//! (`note.on`, `note.off`, `midi.expression`; their source is [`st_midi::APP_SOURCE`]) and for
//! those of MIDI devices (the MIDI area calls [`Audio::key_down`], [`Audio::key_up`] and
//! [`Audio::pedal`] for each of them):
//!
//! - a plug-in instrument and an external MIDI instrument get the pedal, with its value
//!   (controller 64; likewise sostenuto, 66, and soft, 67), and their note-offs as played: the
//!   instrument sustains;
//! - the engine's own instruments have no pedal: while a sustain pedal is down (64 or more)
//!   the host holds back the note-offs of the keys its source releases, until it comes up.
//!   The same goes for a plug-in instrument that controller 64 cannot reach (a CLAP note
//!   port that takes no MIDI, a VST3 plug-in that maps no parameter to it).
//!
//! A pedal acts on what its own source and channel play. It follows them to another track:
//! the instrument played first keeps sustaining, the next gets the pedal with its first note,
//! and both are told when the pedal comes up. An instrument several pedals reach hears the
//! highest of them. `transport.stop` tells every instrument pedal-up and ends what the host
//! holds, and a pedal still down reaches an instrument again with the next note;
//! `engine.panic` silences everything and forgets the pedals. An instrument that is replaced
//! while a pedal is down (another plug-in, a plug-in restored after a crash, another device
//! or channel) gets the pedal again with the next note played to it; one whose track a
//! publication moves or removes is told pedal-up before the publication.
//!
//! Patterns carry notes with gates and no pedal data: all of this is live input only.

use std::sync::{Arc, Mutex, OnceLock};

use serde::Deserialize;
use serde_json::{json, Value};
use st_audio::graph::insert::{builtin_effects_metadata, BuiltinFactory};
use st_audio::{AudioEngine, DecodedSamples, DeviceChoice, DeviceRequest, EngineOptions, InputEdge, PlayOptions};
use st_midi::{PedalEffect, PedalTarget, SustainState, APP_SOURCE};
use st_project::model::{InstrumentKind, Step};
use st_project::Project;

use crate::commands::{Commands, NoPayload, Reply};
use crate::session::ProjectAudio;
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
    /// `uid` of the chosen input; absent to follow the system's default. The line inputs of a
    /// project name their device, and are live only while it is the input.
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<String>,
}

const SETTINGS_SECTION: &str = "audio.devices";

/// The audio service of the host.
pub struct Audio {
    engine: Arc<AudioEngine>,
    /// Where the engine's samples come from (decoded on worker threads).
    samples: Arc<DecodedSamples>,
    events: EventSink,
    settings: Arc<Settings>,
    /// The pedals that are down, of the interface and of MIDI devices, and what they hold
    /// (see the module documentation). Locked while what a message leads to is carried out,
    /// so that messages of several sources take effect in the order they arrived.
    pedals: Mutex<SustainState>,
    /// Told when a note that a sustain pedal kept sounding ends ([`Audio::on_sustained_note_end`]).
    sustained_note_end: OnceLock<SustainedNoteEnd>,
    /// Sample problems already reported, so a publication repeats none.
    reported: Mutex<Vec<st_audio::SampleProblem>>,
    /// Called on `transport.stop` and `engine.panic`: Stop also cancels a running export.
    stop_hooks: Mutex<Vec<Box<dyn Fn() + Send + Sync>>>,
    /// The state of the project's line inputs as the interface was told last.
    line_inputs: Mutex<Value>,
    /// The MIDI area's part of `midi.expression` and of the pedals.
    external_midi: OnceLock<Arc<dyn ExternalInstruments>>,
}

/// The devices of external MIDI instruments, as the MIDI area offers them to live playing.
pub trait ExternalInstruments: Send + Sync {
    /// Sends a channel message (`[status, data1, data2]`) to the device of an external MIDI
    /// instrument, on the instrument's channel. `None` when the instrument is not an external
    /// MIDI instrument; `Some(false)` when it is and its device is absent.
    fn send(&self, instrument: usize, message: [u8; 3]) -> Option<bool>;
    /// Counts the changes of the instrument's route: another device, another channel, a
    /// device that came back. A pedal the device was sent before a change is void after it.
    fn generation(&self, instrument: usize) -> u64;
}

/// Called with `(source, channel, note, where it sounded)` when a note whose key was released
/// under a sustain pedal ends.
pub type SustainedNoteEnd = Box<dyn Fn(i32, u8, u8, PedalTarget) + Send + Sync>;

/// What became of a released key ([`Audio::key_up`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyUp {
    /// What the note sounds on.
    pub target: PedalTarget,
    /// The sustain pedal of the key's source is down: the note goes on sounding until the pedal
    /// lets go ([`Audio::on_sustained_note_end`]).
    pub sustained: bool,
    /// The note-off is to be sent now: always for an instrument with a pedal of its own, and
    /// for the engine's instruments when no pedal holds the note.
    pub send_note_off: bool,
}

impl ProjectAudio for Audio {
    fn prepare(&self, project: &Project, replace: bool) -> Result<Value> {
        self.detach_pedals_of_moved_tracks(project);
        // An opened or replaced project loads every plug-in's saved state again.
        let report = self.engine.publish_with(project, replace).map_err(Error::new)?;
        self.report_samples(report);
        crate::areas::samples::retain_for(&self.samples, project);
        // A publication is what routes line inputs: the page hears what became of them.
        self.report_line_inputs();
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
    owner: Option<String>,
    controller: Option<f64>,
    note: Option<f64>,
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

    /// Where `midi.expression` and the pedals send what belongs to an external MIDI instrument
    /// (the MIDI area sets it; without it external instruments get nothing). Only the first
    /// call counts.
    pub fn set_external_midi(&self, devices: Arc<dyn ExternalInstruments>) {
        let _ = self.external_midi.set(devices);
    }

    /// Who hears when a note that a sustain pedal kept sounding ends (the MIDI area: a key it
    /// is recording gets its length there). Only the first call counts. The receiver is called
    /// with the pedals locked: it must not call back into the pedal methods.
    pub fn on_sustained_note_end(&self, receiver: SustainedNoteEnd) {
        let _ = self.sustained_note_end.set(receiver);
    }

    // ---- the pedals (see the module documentation) -----------------------------------------

    fn pedals(&self) -> std::sync::MutexGuard<'_, SustainState> {
        self.pedals.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What a note played on `track` with `instrument` sounds on, and that instrument's
    /// generation: a number that changes when what stands there may have lost its pedal.
    fn pedal_target(&self, track: i32, instrument: i32) -> (PedalTarget, u64) {
        let (track, instrument) = (track.max(0), instrument.max(0) as usize);
        match self.engine.instrument_kind(instrument) {
            // A plug-in the pedal cannot reach has none of its own: the host holds its notes.
            Some(InstrumentKind::Plugin) => match self.engine.plugin_pedal(track) {
                Some(generation) => (PedalTarget::Plugin { track: track as usize }, generation),
                None => (PedalTarget::Engine { track: track as usize }, 0),
            },
            Some(InstrumentKind::Midi) => (PedalTarget::External { instrument }, self.external_midi.get().map_or(0, |devices| devices.generation(instrument))),
            _ => (PedalTarget::Engine { track: track as usize }, 0),
        }
    }

    /// Sends a channel message to an instrument with a pedal of its own.
    fn send_to(&self, target: PedalTarget, message: [u8; 3]) -> bool {
        match target {
            PedalTarget::Plugin { track } => self.engine.expression(track as i32, message),
            PedalTarget::External { instrument } => self.external_midi.get().and_then(|devices| devices.send(instrument, message)).unwrap_or(false),
            PedalTarget::Engine { .. } => false,
        }
    }

    /// Carries out what the pedals answered, in order. `silenced`: the engine is being
    /// silenced as a whole, so nothing is sent and no note is stopped one by one. Returns the
    /// instruments a pedal could not be sent to.
    fn perform(&self, effects: Vec<PedalEffect>, silenced: bool) -> Vec<PedalTarget> {
        let mut unreached = Vec::new();
        for effect in effects {
            match effect {
                PedalEffect::Send { target, controller, value } => {
                    if !silenced && !self.send_to(target, [0xB0, controller, value]) {
                        unreached.push(target);
                    }
                }
                PedalEffect::NoteEnds { source, channel, note, target } => {
                    if let (PedalTarget::Engine { track }, false) = (target, silenced) {
                        self.engine.note_off(track as i32, i32::from(note));
                    }
                    if let Some(receiver) = self.sustained_note_end.get() {
                        receiver(source, channel, note, target);
                    }
                }
            }
        }
        unreached
    }

    /// A key of `source` on `channel` (1 to 16) is about to play `note` on `track` with
    /// `instrument`. Call before the note is started: a pedal of that source that is down
    /// reaches the instrument first. Returns what the note sounds on.
    pub fn key_down(&self, source: i32, channel: u8, track: i32, instrument: i32, note: u8) -> PedalTarget {
        let (target, generation) = self.pedal_target(track, instrument);
        let mut pedals = self.pedals();
        let effects = pedals.note_on(source, channel, note, track.max(0) as usize, target, generation);
        self.perform(effects, false);
        target
    }

    /// The key was released. The caller sends the note-off when the answer says so. What the
    /// note sounds on is what its key-down found; `instrument` only counts for a key that was
    /// not seen going down.
    pub fn key_up(&self, source: i32, channel: u8, track: i32, instrument: i32, note: u8) -> KeyUp {
        let (otherwise, _) = self.pedal_target(track, instrument);
        let (target, sustained) = self.pedals().note_off(source, channel, note, track.max(0) as usize, otherwise);
        KeyUp { target, sustained, send_note_off: target.has_pedal() || !sustained }
    }

    /// A pedal of `source` on `channel` moved: `controller` 64 (sustain), 66 (sostenuto) or 67
    /// (soft) to `value` (0 to 127). `destination` is the track and instrument a note of that
    /// source would play now, if its input is accepted there. Returns whether the pedal went
    /// somewhere: to the instrument it is played to, or into the host's own holding of notes
    /// for an instrument of the engine (the sustain pedal only).
    pub fn pedal(&self, source: i32, channel: u8, controller: u8, value: u8, destination: Option<(i32, i32)>) -> bool {
        let played = destination.map(|(track, instrument)| self.pedal_target(track, instrument));
        let mut pedals = self.pedals();
        let effects = pedals.pedal(source, channel, controller, value, played);
        let unreached = self.perform(effects, false);
        match played {
            Some((target, _)) if target.has_pedal() => !unreached.contains(&target),
            Some(_) => controller == st_midi::sustain::SUSTAIN,
            None => false,
        }
    }

    /// `project` is about to be published. The pedals know the engine's instruments and the
    /// plug-in instruments by their track's place in the list: a track the project moves or
    /// removes takes its instrument away from there. While it is still there, the instrument
    /// is told pedal-up and the notes the host holds on it are let go. (An instrument that is
    /// replaced where it stands gets the pedal again with its next note; an external
    /// instrument's device and channel are the MIDI area's to clear.)
    fn detach_pedals_of_moved_tracks(&self, project: &Project) {
        let mut pedals = self.pedals();
        if pedals.is_idle() {
            return;
        }
        let leaving = self.engine.tracks_leaving(project);
        let effects = pedals.detach(|target| match target {
            PedalTarget::Engine { track } | PedalTarget::Plugin { track } => leaving.contains(&track),
            PedalTarget::External { .. } => false,
        });
        self.perform(effects, false);
    }

    /// The pedals of the sources `lifted` names come up (their input route was taken down).
    pub fn lift_pedals(&self, lifted: impl Fn(i32) -> bool) {
        let mut pedals = self.pedals();
        let effects = pedals.lift(lifted);
        self.perform(effects, false);
    }

    /// The transport stops: every instrument is told pedal-up and what the host holds is let
    /// go. Call before the engine is stopped: the note-offs go ahead of the stop.
    pub fn silence_pedals(&self) {
        let mut pedals = self.pedals();
        let effects = pedals.silence();
        self.perform(effects, false);
    }

    /// A panic: the engine, the plug-ins and the devices are silenced as a whole (and told
    /// pedal-up there); the pedals are forgotten.
    pub fn forget_pedals(&self) {
        let mut pedals = self.pedals();
        let effects = pedals.silence();
        self.perform(effects, true);
        pedals.clear();
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
        self.silence_pedals();
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
        self.forget_pedals();
        self.run_stop_hooks();
        self.engine.panic();
        self.engine.live().preview().stop(true);
        Ok(self.engine.status())
    }

    fn note_on(&self, payload: NotePayload, reply: Reply<Value>) {
        let (track, instrument) = self.engine.note_destination(whole(payload.track), whole(payload.instrument));
        let note = whole(payload.note).unwrap_or(60).clamp(0, 127) as i32;
        let velocity = whole(payload.velocity).unwrap_or(100).clamp(0, 127) as i32;
        let apply = payload.apply_note.unwrap_or(true);
        if apply {
            // A pedal of the interface that is down reaches the instrument before the note.
            self.key_down(APP_SOURCE, 1, track, instrument, note as u8);
        }
        if payload.capture_input {
            let edge = InputEdge { owner: payload.input_owner.unwrap_or_default(), on: true, track, instrument, note, velocity, apply_note: apply };
            return self.engine.input_edge(edge, Box::new(move |acknowledgement| reply.ok(json!({ "ok": true, "inputAcknowledgement": acknowledgement }))));
        }
        self.engine.note_on(track, instrument, note, velocity);
        reply.ok(json!({ "ok": true }));
    }

    fn note_off(&self, payload: NotePayload, reply: Reply<Value>) {
        let (track, instrument) = self.engine.note_destination(whole(payload.track), whole(payload.instrument));
        let note = whole(payload.note).unwrap_or(60).clamp(0, 127) as i32;
        let apply = payload.apply_note.unwrap_or(true);
        // With the interface's sustain pedal down the note goes on sounding: an instrument with
        // a pedal of its own gets the note-off and sustains, the host holds the note-off of an
        // instrument of the engine. The key release is captured all the same.
        let up = apply.then(|| self.key_up(APP_SOURCE, 1, track, instrument, note as u8));
        let (sustained, send_note_off) = up.map_or((false, false), |up| (up.sustained, up.send_note_off));
        let done = move |mut result: Value| {
            if sustained {
                result["sustained"] = json!(true);
            }
            reply.ok(result)
        };
        if payload.capture_input {
            let velocity = whole(payload.velocity).unwrap_or(0).clamp(0, 127) as i32;
            let edge = InputEdge { owner: payload.input_owner.unwrap_or_default(), on: false, track, instrument, note, velocity, apply_note: send_note_off };
            return self.engine.input_edge(edge, Box::new(move |acknowledgement| done(json!({ "ok": true, "inputAcknowledgement": acknowledgement }))));
        }
        if send_note_off {
            self.engine.note_off(track, note);
        }
        done(json!({ "ok": true }));
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

    /// `midi.expression`: pitch bend, a controller, channel pressure (`aftertouch`) or poly
    /// pressure (`polyAftertouch`, with its `note`) played live for the instrument of the track
    /// whose channel is `owner` (`track:<routingID>`; the track of the last note without
    /// one). `value` is -1..1 for pitch bend and 0..1 otherwise.
    ///
    /// The message goes where a key played on a MIDI keyboard would send it:
    ///
    /// - an external MIDI instrument: to its device, on its channel;
    /// - a plug-in instrument: to the plug-in, with its next block (`AudioEngine::expression`);
    /// - the engine's own instruments: pitch bend (two semitones either way) and the
    ///   modulation wheel (controller 1); they have no use for the rest.
    ///
    /// `sent` tells whether the message went anywhere.
    ///
    /// The pedals (`sustain`, which is controller 64, and controllers 66 and 67) are the
    /// interface's own pedals: they act on the notes the interface plays (see the module
    /// documentation). A plug-in or external instrument gets the pedal with its value and
    /// sustains by itself; for an instrument of the engine the host holds the note-offs while
    /// the sustain pedal is at a half or more.
    fn expression(&self, payload: ExpressionPayload) -> Result<Value> {
        let Some(value) = payload.value.filter(|value| value.is_finite()) else { fail!("Expression value must be a finite number.") };
        let named = match payload.owner.as_deref() {
            Some(owner) => {
                let index = self.engine.with_project(|project| project.and_then(|project| project.track_list().iter().position(|track| track.owner().as_deref() == Some(owner))));
                let Some(index) = index else { fail!("{owner} is not the channel of a track of this project.") };
                Some(index as i64)
            }
            None => None,
        };
        let (track, last_instrument) = self.engine.note_destination(named, None);
        let (owner, instrument) = self
            .engine
            .with_project(|project| project.and_then(|project| project.track_list().get(track as usize).map(|track| (track.owner(), track.instrument() as i32))))
            .unwrap_or((None, last_instrument));
        let seven = (value.clamp(0.0, 1.0) * 127.0).round() as u8;
        let controller = whole(payload.controller).unwrap_or(1).clamp(0, 127) as u8;
        let message = match (payload.kind.as_str(), controller) {
            ("pitchBend", _) => {
                let bend = (((value.clamp(-1.0, 1.0) + 1.0) * 8192.0).round() as u16).min(16_383);
                [0xE0, (bend & 0x7F) as u8, (bend >> 7) as u8]
            }
            (kind @ ("sustain" | "cc"), controller) if kind == "sustain" || st_midi::sustain::PEDALS.contains(&controller) => {
                let controller = if kind == "sustain" { st_midi::sustain::SUSTAIN } else { controller };
                let sent = self.pedal(APP_SOURCE, 1, controller, seven, Some((track, instrument)));
                return Ok(json!({ "sent": sent, "owner": owner, "instrument": instrument }));
            }
            ("cc", 120..) => fail!("Use engine.panic for channel-mode messages."),
            ("cc", controller) => [0xB0, controller, seven],
            ("aftertouch", _) => [0xD0, seven, 0],
            ("polyAftertouch", _) => match whole(payload.note).filter(|note| (0..=127).contains(note)) {
                Some(note) => [0xA0, note as u8, seven],
                None => fail!("Poly aftertouch needs a note."),
            },
            (other, _) => fail!("Unsupported expression kind: {other}."),
        };
        let external = self.external_midi.get().and_then(|devices| devices.send(instrument.max(0) as usize, message));
        let sent = external.unwrap_or_else(|| self.engine.expression(track, message));
        Ok(json!({ "sent": sent, "owner": owner, "instrument": instrument }))
    }

    fn snapshot(&self, _: NoPayload) -> Result<Value> {
        Ok(self.engine.snapshot())
    }

    fn routing_status(&self, _: NoPayload) -> Result<Value> {
        Ok(self.engine.routing_status())
    }

    /// `routing.capabilities`: what the mixer can route and export. The limits are the project
    /// format's. `exportTargets` are the strips of the mixer in use (`{owner, name, kind}`):
    /// each can be written as a stem, a track, group or return after its fader and before its
    /// output bus, the master as the complete mix. `sidechains.inputs` are the inserts in use
    /// that read a sidechain (`{instanceID, owner, slot, pluginName}`); an insert reads one, in
    /// stereo.
    fn routing_capabilities(&self, _: NoPayload) -> Result<Value> {
        let targets = self.engine.routing_targets();
        Ok(json!({
            "buses": { "max": st_project::validate::MAX_BUSES },
            "sends": { "taps": ["preFader", "postFader"], "max": st_project::validate::MAX_ROUTES },
            "sidechains": { "taps": ["preInserts", "preFader", "postFader"], "max": st_project::validate::MAX_ROUTES, "inputs": targets["sidechainInputs"] },
            "exportTargets": targets["strips"],
            "audioExport": st_samples::export::format::capabilities(),
        }))
    }

    fn effects(&self, _: NoPayload) -> Result<Value> {
        Ok(builtin_effects_metadata())
    }

    fn devices_value(&self, status: st_audio::DevicesStatus) -> Value {
        // The input follows the engine's format; the null input is offered with the null output.
        let live = self.engine.live();
        live.inputs().set_format(status.sample_rate, status.buffer_size, &status.output_uid);
        let mut value = serde_json::to_value(status).unwrap_or(Value::Null);
        live.decorate_devices(&mut value);
        // The line inputs the published project routes into its tracks, and why the first one
        // that should run does not.
        let (lines, problem) = self.engine.line_inputs();
        *self.line_inputs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = line_states(&lines);
        value["lineInputs"] = Value::Array(lines);
        value["lineInputError"] = json!(problem);
        value
    }

    /// Sends `audio.devices` when the state of a line input is not what the interface was told
    /// last (a publication routed or paused one, a device came or went).
    fn report_line_inputs(&self) {
        let states = line_states(&self.engine.line_inputs().0);
        if *self.line_inputs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) != states {
            self.events.emit("audio.devices", self.devices_value(self.engine.devices()));
        }
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
        // The input after the output: it follows the engine's (possibly new) sample rate and device.
        live.inputs().set_format(status.sample_rate, status.buffer_size, &status.output_uid);
        let configured = live.inputs().configure(&input).map_err(Error::new);
        self.settings.set(SETTINGS_SECTION, &DeviceSettings { output, buffer_size: Some(buffer_size), sample_rate: Some(status.sample_rate), input: live.inputs().chosen() })?;
        // The project's line inputs follow the devices: another input or another block size
        // pauses, resumes or re-times them.
        self.engine.refresh_inputs().map_err(Error::new)?;
        configured?;
        Ok(self.devices_value(status))
    }
}

/// What the interface shows of each line input: the part of `lineInputs` that an `audio.devices`
/// event is worth sending for (not the counters).
fn line_states(lines: &[Value]) -> Value {
    Value::Array(lines.iter().map(|line| json!([line["track"], line["state"], line["status"], line["monitoring"], line["latencyFrames"]])).collect())
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
    if let Some(input) = saved.input.filter(|uid| !uid.is_empty()) {
        // Choosing the input opens nothing: it is opened when something uses it.
        let _ = engine.live().inputs().configure(&st_audio::inputs::InputRequest { uid: Some(input), ..Default::default() });
    }
    let audio = Arc::new(Audio { engine, samples, events: events.clone(), settings: settings.clone(), pedals: Mutex::default(), sustained_note_end: OnceLock::new(), reported: Mutex::default(), stop_hooks: Mutex::default(), line_inputs: Mutex::new(json!([])), external_midi: OnceLock::new() });
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
        .register("routing.capabilities", Audio::routing_capabilities)
        .register("builtin.effects", Audio::effects)
        .register_blocking("audio.devices", Audio::devices)
        .register_blocking("audio.configure", Audio::configure);
    let status_events = events;
    let weak = Arc::downgrade(&audio);
    audio.engine.start_service(move |engine, device_changed| {
        status_events.emit_latest("status", engine.status());
        // A device that came or went (which pauses and resumes line inputs too).
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

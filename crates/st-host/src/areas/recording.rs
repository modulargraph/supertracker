//! The recording area: takes of the master output or of the input device.
//!
//! | Command | Notes |
//! |---|---|
//! | `audio.record` `{source?: "master" or "input", path?, name?, instrument?, owner?, monitor?, monitorVolume?}` | starts a take into a new 24-bit WAV file: `path`, or `Recordings` in the support folder under `name`. `source: "master"` records what the device gets; anything else records the chosen input channels, with the input's and the output's latency left out of the start of the file, monitored when `monitor` is true. Answers `{recording: true, path, channels}` and sends `audio.recording` |
//! | `audio.stopRecord` | ends the take on its source's clock and completes the file: `{recording: false, path, duration, frames, channels, partial, source, sourceStartFrame, sourceEndFrame, latencyCompensation, instrument?, owner?}` (and the same as an `audio.recording` event); `{recording: false}` when no take runs |
//!
//! The input device, its channels, the meter and monitoring are part of `audio.configure` and
//! `audio.devices` ([`crate::areas::audio`]); the status events carry `recording`,
//! `recordingDuration`, `inputPeakLeft` / `inputPeakRight` and `inputMetering`. The same input
//! stream feeds the line inputs a project routes into its tracks; those are part of the project
//! and have no command ([`crate::areas::audio`]). A master take holds them, as it holds
//! everything the device gets.
//!
//! The audio of a take never touches these handlers: the render thread (master) or the input
//! device's thread copies it into a ring that a writer thread drains ([`st_audio::recording`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use st_audio::live::{RecordRequest, RecordSource};

use crate::areas::audio::Audio;
use crate::commands::{Commands, NoPayload};
use crate::{fail, Error, EventSink, Result};

/// What the page said about the take, to hand back when it ends.
#[derive(Default)]
struct About {
    instrument: Option<i64>,
    owner: Option<String>,
}

/// The recording service of the host.
pub struct Recorder {
    audio: Arc<Audio>,
    events: EventSink,
    support: PathBuf,
    about: Mutex<About>,
}

impl Recorder {
    fn record(&self, payload: Value) -> Result<Value> {
        let live = self.audio.engine().live();
        if live.is_recording() {
            fail!("A recording is already running.");
        }
        let source = if payload["source"] == "master" { RecordSource::Master } else { RecordSource::Input };
        let text = |key: &str| payload[key].as_str().filter(|value| !value.is_empty());
        let path = st_samples::paths::recording_destination(text("path"), text("name"), Some(&self.support.to_string_lossy()), Path::new("/"))?;
        // An input take is late by the input's latency and by what the output needs to be heard.
        let devices = self.audio.engine().devices();
        let output_latency = if devices.output_uid == st_audio::device::NULL_DEVICE { 0 } else { devices.buffer_size as u64 };
        let request = RecordRequest {
            source,
            path: path.clone(),
            monitor: payload["monitor"] == true,
            monitor_volume: payload["monitorVolume"].as_f64().unwrap_or(0.5) as f32,
            compensation_frames: if source == RecordSource::Input { live.inputs().latency_frames() as u64 + output_latency } else { 0 },
        };
        live.inputs().set_format(devices.sample_rate, devices.buffer_size, devices.output_uid == st_audio::device::NULL_DEVICE);
        let channels = live.record(&request).map_err(Error::new)?;
        *self.about.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = About { instrument: payload["instrument"].as_f64().map(|instrument| instrument as i64), owner: text("owner").map(str::to_string) };
        let path = path.to_string_lossy().into_owned();
        self.events.emit("audio.recording", json!({ "recording": true, "path": path, "source": if source == RecordSource::Master { "master" } else { "input" }, "channels": channels }));
        Ok(json!({ "recording": true, "path": path, "channels": channels }))
    }

    fn stop(&self, _: NoPayload) -> Result<Value> {
        let Some(take) = self.audio.engine().live().stop_record() else { return Ok(json!({ "recording": false })) };
        let about = std::mem::take(&mut *self.about.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        let file = &take.file;
        let path = file.path.to_string_lossy().into_owned();
        let frame = |frame: Option<i64>| frame.map_or(Value::Null, |frame| json!(frame.to_string()));
        let mut result = json!({
            "recording": false, "path": path, "duration": file.frames as f64 / file.sample_rate as f64, "frames": file.frames, "channels": file.channels,
            "sampleRate": file.sample_rate, "partial": file.problem.is_some(), "source": if take.source == RecordSource::Master { "master" } else { "input" },
            "sourceStartFrame": frame(file.start_frame), "sourceEndFrame": frame(file.end_frame),
            "latencyCompensation": { "frames": take.compensation_frames, "seconds": take.compensation_frames as f64 / file.sample_rate as f64 },
        });
        if let Some(instrument) = about.instrument {
            result["instrument"] = json!(instrument);
        }
        if let Some(owner) = about.owner {
            result["owner"] = json!(owner);
        }
        if let Some(problem) = &file.problem {
            result["error"] = json!(problem);
        }
        self.events.emit("audio.recording", &result);
        match &file.problem {
            Some(problem) => fail!("{problem} Partial audio was preserved at {path}."),
            None => Ok(result),
        }
    }
}

/// Registers the recording commands.
pub fn install(commands: &mut Commands, audio: &Arc<Audio>, support: &Path, events: EventSink) -> Arc<Recorder> {
    let service = Arc::new(Recorder { audio: audio.clone(), events, support: support.to_path_buf(), about: Mutex::default() });
    commands.with(&service).register_blocking("audio.record", Recorder::record).register_blocking("audio.stopRecord", Recorder::stop);
    service
}

//! What runs on the live output after the mixer: the preview voices, the monitored input and
//! the tap of a master recording.
//!
//! [`Live`] is the control side, one per [`crate::AudioEngine`]; it survives a restart of the
//! engine at another sample rate. Each renderer gets a fresh [`OutputStage`] from
//! [`Live::stage`], which the render thread calls on the mixed output once per render call:
//!
//! ```text
//!   plan (strips ... master fader, meters)  ->  + preview voices  ->  + monitored input  ->  master tap  ->  device
//! ```
//!
//! A master recording therefore holds what the device got, previews and monitoring included.
//! [`Live`] also owns the input ([`crate::inputs::Inputs`]: one stream for the meter,
//! monitoring, takes and the line inputs of the project's tracks) and the take in progress
//! ([`Live::record`], [`Live::stop_record`]).
//!
//! Previews bypass the project's master strip: the master fader, the master inserts and the
//! master meter do not see them, and neither do offline renders, which run on engines of their
//! own. The line inputs of tracks are not part of this stage: they enter the mixer at their
//! strips ([`crate::graph::plan`]).

use crate::graph::render::OutputStage;
use crate::inputs::{InputHub, Inputs};
use crate::preview::{PreviewPlayer, PreviewVoices};
use crate::recording::{Finished, Recording, Tap};
use st_samples::preview::PreviewCache;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a take records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordSource {
    /// The output, after the mixer, previews and monitoring.
    Master,
    /// The chosen channels of the input device.
    Input,
}

/// `audio.record`.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordRequest {
    pub source: RecordSource,
    /// The file to create (it must not exist).
    pub path: PathBuf,
    /// Input takes: whether the input is heard while it is recorded, and how loud.
    pub monitor: bool,
    pub monitor_volume: f32,
    /// Latency to compensate in an input take, in frames: the take's first frames are left out.
    pub compensation_frames: u64,
}

struct Take {
    recording: Recording,
    source: RecordSource,
    compensation: u64,
}

/// A take that has ended.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeFinished {
    pub source: RecordSource,
    pub file: Finished,
    /// Frames left out at the start of an input take (its latency compensation).
    pub compensation_frames: u64,
}

/// The control side of the live output stage.
pub struct Live {
    preview: Arc<PreviewPlayer>,
    /// Offline renders in flight (export, bounce, freeze), for the status' `exporting`.
    exporting: std::sync::atomic::AtomicUsize,
    inputs: Inputs,
    /// The tap of a master take, for the stage. The render thread only `try_lock`s it.
    master_tap: Arc<Mutex<Option<Tap>>>,
    take: Mutex<Option<Take>>,
    stopping: AtomicBool,
    sample_rate: Mutex<f64>,
}

impl Default for Live {
    fn default() -> Live {
        Live {
            preview: Arc::new(PreviewPlayer::new(Arc::new(PreviewCache::default()))),
            exporting: Default::default(),
            inputs: Inputs::default(),
            master_tap: Arc::default(),
            take: Mutex::default(),
            stopping: AtomicBool::new(false),
            sample_rate: Mutex::new(48_000.0),
        }
    }
}

struct Stage {
    preview: PreviewVoices,
    hub: Arc<InputHub>,
    master_tap: Arc<Mutex<Option<Tap>>>,
    scratch: (Vec<f32>, Vec<f32>),
}

impl OutputStage for Stage {
    fn process(&mut self, time: u64, left: &mut [f32], right: &mut [f32]) {
        self.preview.mix(left, right);
        let frames = left.len().min(right.len());
        let step = self.scratch.0.len();
        let mut at = 0;
        while at < frames {
            let end = (at + step).min(frames);
            self.hub.monitor(&mut left[at..end], &mut right[at..end], (&mut self.scratch.0, &mut self.scratch.1));
            at = end;
        }
        if let Ok(tap) = self.master_tap.try_lock() {
            if let Some(tap) = tap.as_ref() {
                tap.capture(time, &left[..frames], &right[..frames]);
            }
        }
    }
}

impl Live {
    /// The stage for a renderer at `sample_rate`. What the previous stage played is gone with it.
    pub fn stage(&self, sample_rate: f64) -> Box<dyn OutputStage> {
        *self.sample_rate.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = sample_rate;
        Box::new(Stage { preview: self.preview.voices(sample_rate), hub: self.inputs.hub().clone(), master_tap: self.master_tap.clone(), scratch: (vec![0.0; 8192], vec![0.0; 8192]) })
    }

    /// The audio input: device, channels, meter and monitoring.
    pub fn inputs(&self) -> &Inputs {
        &self.inputs
    }

    /// Starts a take. One at a time.
    pub fn record(&self, request: &RecordRequest) -> Result<u16, String> {
        let mut take = self.take.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if take.is_some() {
            return Err("A recording is already running.".to_string());
        }
        let rate = *self.sample_rate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (channels, compensation) = match request.source {
            RecordSource::Master => (2, 0),
            RecordSource::Input => (self.inputs.take_channels(), request.compensation_frames),
        };
        let (recording, tap) = Recording::start(&request.path, rate.round() as u32, channels, compensation)?;
        match request.source {
            RecordSource::Master => *self.master_tap.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(tap),
            RecordSource::Input => {
                if let Err(error) = self.inputs.begin_take(tap, Some((request.monitor, request.monitor_volume))) {
                    drop(recording);
                    let _ = std::fs::remove_file(&request.path);
                    return Err(error);
                }
            }
        }
        *take = Some(Take { recording, source: request.source, compensation });
        Ok(channels)
    }

    /// Ends the take, if one runs: its source stops on its own clock, the file is completed.
    pub fn stop_record(&self) -> Option<TakeFinished> {
        let take = self.take.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()?;
        self.stopping.store(true, Ordering::Release);
        let file = take.recording.finish(Duration::from_secs(2));
        match take.source {
            RecordSource::Master => *self.master_tap.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None,
            RecordSource::Input => self.inputs.end_take(),
        }
        self.stopping.store(false, Ordering::Release);
        Some(TakeFinished { source: take.source, file, compensation_frames: take.compensation })
    }

    /// Whether a take is running.
    pub fn is_recording(&self) -> bool {
        self.take.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_some()
    }

    /// Browser previews.
    pub fn preview(&self) -> &Arc<PreviewPlayer> {
        &self.preview
    }

    /// Marks an export as running (or ended); the status reports `exporting` while any runs.
    pub fn set_exporting(&self, running: bool) {
        use std::sync::atomic::Ordering;
        if running {
            self.exporting.fetch_add(1, Ordering::AcqRel);
        } else {
            let _ = self.exporting.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| count.checked_sub(1));
        }
    }

    /// Adds what this stage knows to the engine's status.
    pub fn decorate_status(&self, status: &mut serde_json::Value) {
        status["exporting"] = serde_json::json!(self.exporting.load(std::sync::atomic::Ordering::Acquire) > 0);
        let stopping = self.stopping.load(Ordering::Acquire);
        let take = self.take.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        status["recording"] = serde_json::json!(take.is_some() || stopping);
        status["recordingStopping"] = serde_json::json!(stopping);
        status["recordingDuration"] = serde_json::json!(take.as_ref().map_or(0.0, |take| take.recording.duration()));
        // The input meter: the device's when it is on, else what an input take captures.
        let metering = self.inputs.metering();
        let peaks = match (metering, take.as_ref()) {
            (true, _) => Some(self.inputs.take_peaks()),
            (false, Some(take)) if take.source == RecordSource::Input => Some(take.recording.take_peaks()),
            _ => None,
        };
        status["inputMetering"] = serde_json::json!(metering);
        if let Some((left, right)) = peaks {
            status["inputPeakLeft"] = serde_json::json!(left);
            status["inputPeakRight"] = serde_json::json!(right);
        }
    }

    /// Adds the input's state to the engine's device report (`audio.devices`).
    pub fn decorate_devices(&self, devices: &mut serde_json::Value) {
        if let (Some(devices), serde_json::Value::Object(input)) = (devices.as_object_mut(), self.inputs.status()) {
            devices.extend(input);
        }
        // The null input joins the system's inputs where it is offered, and is the default
        // input while there is no other.
        if let Some(null) = self.inputs.null_input() {
            if let (Some(inputs), Ok(null)) = (devices["inputs"].as_array_mut(), serde_json::to_value(&null)) {
                inputs.push(null);
            }
            if devices["defaultInput"].is_null() {
                devices["defaultInput"] = serde_json::json!(0);
            }
        }
        // The test tone input is listed while it is the chosen one, so a page can show it.
        if let Some(tone) = self.inputs.tone_input() {
            if let (Some(inputs), Ok(tone)) = (devices["inputs"].as_array_mut(), serde_json::to_value(&tone)) {
                inputs.push(tone);
            }
        }
        devices["inputLatencyFrames"] = serde_json::json!(self.inputs.latency_frames());
    }

    /// Housekeeping on the engine's tick: frees what the render thread has finished with and
    /// looks after the input device. Returns true when the input in use was lost.
    pub fn collect(&self) -> bool {
        self.preview.collect();
        self.inputs.watch()
    }
}

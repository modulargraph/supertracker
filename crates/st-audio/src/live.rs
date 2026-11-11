//! What runs on the live output after the mixer: the preview voices, the monitored input and
//! the taps of a recording.
//!
//! [`Live`] is the control side, one per [`crate::AudioEngine`]; it survives a restart of the
//! engine at another sample rate. Each renderer gets a fresh [`OutputStage`] from
//! [`Live::stage`], which the render thread calls on the mixed output once per render call:
//!
//! ```text
//!   plan (strips ... master fader, meters)  ->  + preview voices  ->  + monitored input  ->  master tap  ->  device
//!                                                                     the chosen input channels,
//!                                                                     on the output's clock  ->  input tap
//! ```
//!
//! A master recording therefore holds what the device got, previews and monitoring included.
//! [`Live`] also owns the input ([`crate::inputs::Inputs`]: one stream for the meter,
//! monitoring, takes and the line inputs of the project's tracks) and the take in progress
//! ([`Live::record`], [`Live::stop_record`]).
//!
//! # The clock of an input take
//!
//! A take of the input is recorded here, on the render thread, from the feed that brings the
//! chosen channels over from the input's thread ([`crate::inputs::LineFeed`]): the file is at
//! the engine's sample rate and on the output's clock, one frame per frame the output played,
//! and its start and end are graph frames like a master take's. That is what keeps a take in
//! place over its whole length when it is put into the song, whose time is the output's.
//! When the input is the output's own device the frames are the device's, bit for bit. When
//! it is another device they have been through the converter once (100 dB below the signal,
//! see [`crate::asrc`]). The alternative, the device's own frames at its own rate with the
//! measured ratio beside them, would leave the correction to everything that later plays the
//! file: without it a take from a clock 200 ppm off is 60 ms out after five minutes, and a
//! file at another nominal rate is resampled when it is loaded anyway. The price is that an
//! input take needs a running renderer, which the application always has (the null device
//! renders where no audio device is).
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
    /// Input takes: what the output adds to the latency of what is played to it, in frames.
    /// The take's first frames are left out for it, for the latency the input device reports
    /// and for the delay of the feed that brings the input to the render thread.
    pub output_latency_frames: u64,
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
    /// Frames left out at the start of an input take: the latency of the output, of the input
    /// device and of the input's feed.
    pub compensation_frames: u64,
}

/// The control side of the live output stage.
pub struct Live {
    preview: Arc<PreviewPlayer>,
    /// Offline renders in flight (export, bounce, freeze), for the status' `exporting`.
    exporting: std::sync::atomic::AtomicUsize,
    inputs: Inputs,
    /// The tap of a master take and the tap of an input take, for the stage. The render thread
    /// only `try_lock`s them.
    master_tap: Arc<Mutex<Option<Tap>>>,
    input_tap: Arc<Mutex<Option<Tap>>>,
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
            input_tap: Arc::default(),
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
    input_tap: Arc<Mutex<Option<Tap>>>,
    sample_rate: f64,
    scratch: (Vec<f32>, Vec<f32>),
}

impl OutputStage for Stage {
    fn process(&mut self, time: u64, clock_ns: u64, left: &mut [f32], right: &mut [f32]) {
        self.preview.mix(left, right);
        let frames = left.len().min(right.len());
        let step = self.scratch.0.len();
        let input_tap = self.input_tap.try_lock();
        let input_tap = input_tap.as_ref().ok().and_then(|tap| tap.as_ref());
        let mut at = 0;
        while at < frames {
            let end = (at + step).min(frames);
            // The chosen input channels, as the output's clock hears them.
            let (heard_left, heard_right) = (&mut self.scratch.0[..end - at], &mut self.scratch.1[..end - at]);
            let heard = self.hub.listen(clock_ns + (at as f64 / self.sample_rate * 1e9) as u64, heard_left, heard_right);
            if let (true, Some(volume)) = (heard, self.hub.monitor_gain()) {
                for index in 0..end - at {
                    left[at + index] += heard_left[index] * volume;
                    right[at + index] += heard_right[index] * volume;
                }
            }
            // The take holds every frame the output rendered: silence where the input has
            // nothing.
            if let Some(tap) = input_tap {
                if !heard {
                    heard_left.fill(0.0);
                    heard_right.fill(0.0);
                }
                tap.capture(time + at as u64, heard_left, heard_right);
            }
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
        Box::new(Stage { preview: self.preview.voices(sample_rate), hub: self.inputs.hub().clone(), master_tap: self.master_tap.clone(), input_tap: self.input_tap.clone(), sample_rate, scratch: (vec![0.0; 8192], vec![0.0; 8192]) })
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
        // The input is opened first: its latency is part of what the take leaves out.
        let (channels, compensation) = match request.source {
            RecordSource::Master => (2, 0),
            RecordSource::Input => (self.inputs.take_channels(), request.output_latency_frames + self.inputs.begin_take(Some((request.monitor, request.monitor_volume)))? as u64),
        };
        let (recording, tap) = match Recording::start(&request.path, rate.round() as u32, channels, compensation) {
            Ok(started) => started,
            Err(error) => {
                if request.source == RecordSource::Input {
                    self.inputs.end_take();
                }
                return Err(error);
            }
        };
        let slot = if request.source == RecordSource::Master { &self.master_tap } else { &self.input_tap };
        *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(tap);
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
            RecordSource::Input => {
                *self.input_tap.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                self.inputs.end_take();
            }
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

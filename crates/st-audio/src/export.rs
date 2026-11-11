//! Export: the song, one pattern, stems or one track rendered offline to WAV files.
//!
//! The render runs on a private engine ([`crate::offline`]), so the live engine keeps playing.
//! The files go through `st-samples`' export pipeline: each output is rendered as 32-bit float
//! at the render rate into the staging folder of an `ExportTransaction`, converted to the
//! requested encoding and sample rate by a `DeliveryJob`, and everything is published with one
//! rename at the end. A failed or cancelled export leaves the destination as it was.
//!
//! | Mode | What is rendered | Files |
//! |---|---|---|
//! | `song` (default) | the song from its start, without looping, plus the tail | one, at `path` |
//! | `pattern` | one pattern (the active one, or `pattern`) as a one-entry song, without timeline clips | one, at `path` |
//! | `stems` | the song once, with a capture of every chosen channel (`owners`; every track by default) after its fader and before its output bus; `master` is the complete mix | `NN-name.wav` in the folder `path` |
//! | `track` | as stems, for one track (`track`): a bounce, always 32-bit float at the render rate | one, at `path` |
//!
//! Stems are captured in the one pass that renders the mix, so sidechains, returns and master
//! processing behave as in playback, and all files are aligned to the same frame. Parent and
//! child stems overlap (a group's stem contains its tracks). The built-in delay and reverb
//! returns are part of the master only.
//!
//! The tail (`tail` seconds, 0..30, default 3) follows the musical end: the transport stops
//! there, held notes are released, and effects ring out.

use crate::engine::AudioEngine;
use crate::graph::spec::{StripKind, MASTER};
use crate::offline::{OfflineRender, RenderTarget, RunSummary};
use crate::samples_async::{BlockingSamples, DecodedSamples};
use serde_json::{json, Value};
use st_project::model::Project;
use st_samples::export::format::RENDER_SAMPLE_RATE;
use st_samples::export::{DeliveryJob, DeliverySettings, ExportEntry, ExportFormat, ExportFormatRequest, ExportKind, ExportTransaction, RenderFileWriter};
use st_samples::CancelToken;
use std::path::Path;
use std::sync::Arc;

/// What an export says when it was cancelled (the interface recognizes "cancelled").
pub const CANCELLED: &str = "Export cancelled. Existing destination files were preserved.";

/// `audio.export`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportRequest {
    /// The file to write, or the folder for stems.
    pub path: String,
    /// "song", "pattern", "stems" or "track".
    pub mode: String,
    /// Stems: the channels to capture (`track:<id>`, `bus:<id>`, `master`); every track when absent.
    pub owners: Option<Vec<String>>,
    /// Track bounce: the track's index.
    pub track: Option<i64>,
    /// Pattern mode: the pattern number; the active pattern when absent.
    pub pattern: Option<i64>,
    /// Seconds of tail after the musical end, 0..30; 3 when absent.
    pub tail: Option<f64>,
    pub format: ExportFormatRequest,
}

/// What a render needs from the host.
pub struct RenderContext<'a> {
    /// The engine whose project is rendered.
    pub live: &'a AudioEngine,
    /// The sample cache (shared with the live engine).
    pub samples: &'a Arc<DecodedSamples>,
    pub cancel: &'a CancelToken,
    /// Progress 0..1 and the phase ("rendering", "delivery").
    pub progress: &'a dyn Fn(f64, &str),
}

/// The project the live engine plays, as JSON with the state of every insert captured now, for
/// a render to change and publish on its private engine.
pub fn snapshot_for_render(live: &AudioEngine) -> Result<Value, String> {
    if live.with_project(|project| project.is_none()) {
        return Err("There is no project to render yet.".to_string());
    }
    let mut value = live.snapshot();
    value["loopSong"] = json!(false);
    value["metronome"] = json!(false);
    Ok(value)
}

/// Refuses projects that cannot be rendered offline: a track that plays a live line input. A
/// frozen track does not (its input is closed and its print holds what it played), so Freeze
/// by recording is what makes such a track exportable.
pub fn require_renderable(project: &Value) -> Result<(), String> {
    let live_input = project["tracks"].as_array().is_some_and(|tracks| tracks.iter().any(|track| track["audioInput"]["enabled"] == true && !track["frozen"].is_object()));
    if live_input {
        return Err("Freeze, record or disable live line inputs before offline rendering. Hardware inputs cannot be rendered offline.".to_string());
    }
    Ok(())
}

/// A private engine at the render rate with `project` published.
pub fn offline_engine(context: &RenderContext<'_>, project: &Value) -> Result<OfflineRender, String> {
    let samples = Arc::new(BlockingSamples::new(context.samples.clone(), context.cancel.clone()));
    let mut offline = OfflineRender::new(context.live, samples, RENDER_SAMPLE_RATE as f64)?;
    let project = Project::from_value(project.clone()).map_err(|error| error.to_string())?;
    offline.publish(&project)?;
    Ok(offline)
}

/// Writes captures to render files, each exactly `frames` long.
pub(crate) struct FileTarget<'a> {
    writers: Vec<RenderFileWriter>,
    frames: u64,
    cancel: &'a CancelToken,
    progress: &'a dyn Fn(f64),
    cancelled: &'static str,
    reported: u64,
}

impl<'a> FileTarget<'a> {
    pub(crate) fn new(entries: &[&Path], sample_rate: u32, frames: u64, cancel: &'a CancelToken, cancelled: &'static str, progress: &'a dyn Fn(f64)) -> Result<Self, String> {
        let writers = entries.iter().map(|path| RenderFileWriter::create(path, sample_rate, 2).map_err(|error| error.to_string())).collect::<Result<Vec<_>, _>>()?;
        Ok(FileTarget { writers, frames, cancel, progress, cancelled, reported: 0 })
    }

    /// Closes the files; fails when one is short.
    pub(crate) fn finish(self) -> Result<(), String> {
        for writer in self.writers {
            let written = writer.finalize().map_err(|error| error.to_string())?;
            if written != self.frames {
                return Err("The render ended before every channel reached the requested duration.".to_string());
            }
        }
        Ok(())
    }
}

impl RenderTarget for FileTarget<'_> {
    fn write(&mut self, capture: usize, start: u64, left: &[f32], right: &[f32]) -> Result<(), String> {
        let count = (self.frames.saturating_sub(start) as usize).min(left.len());
        if count > 0 {
            self.writers[capture].write_planar(&[&left[..count], &right[..count]]).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn wanted(&mut self, available: u64, _stopped: bool) -> Result<Option<u64>, String> {
        if self.cancel.is_cancelled() {
            return Err(self.cancelled.to_string());
        }
        if available >= self.reported + 65_536 || available >= self.frames {
            self.reported = available;
            (self.progress)(available as f64 / self.frames.max(1) as f64);
        }
        Ok(Some(self.frames))
    }
}

/// Converts the renders of `entries` (each `frames` long) to their delivery files; `progress`
/// gets 0..1.
pub(crate) fn deliver(entries: &[ExportEntry], frames: u64, settings: DeliverySettings, cancel: &CancelToken, cancelled: &str, progress: &dyn Fn(f64)) -> Result<(), String> {
    for (index, entry) in entries.iter().enumerate() {
        let mut job = DeliveryJob::new(&entry.source_path, frames, &entry.delivery_path, settings, 4096).map_err(|error| error.to_string())?;
        loop {
            if cancel.is_cancelled() {
                job.discard();
                return Err(cancelled.to_string());
            }
            let state = job.advance(8).map_err(|error| error.to_string())?;
            progress((index as f64 + state.output_frames as f64 / state.total_output_frames.max(1) as f64) / entries.len() as f64);
            if state.complete {
                break;
            }
        }
        job.publish().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if matches!(c, '/' | ':' | '\\' | '\0') || c.is_control() { '-' } else { c }).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "Channel".to_string() } else { cleaned }
}

/// `audio.export`: renders and writes the files. The result is what the interface gets:
/// `{path, paths, mode, frames, musicalFrames, tailFrames, sourceFrames, container, encoding,
/// sampleRate, bitDepth, isFloat, dither, renderSampleRate, latencyCompensation, stems: [{owner,
/// name, path, boundary, frames, sourceFrames, sampleRate, arrivalFrames}], renderPasses,
/// stemPolicy}`.
#[cfg(unix)]
pub fn export(context: &RenderContext<'_>, request: &ExportRequest) -> Result<Value, String> {
    if request.path.is_empty() {
        return Err("Choose an export destination.".to_string());
    }
    let mode = if request.mode.is_empty() { "song" } else { request.mode.as_str() };
    let format = ExportFormat::resolve(&request.format, mode, RENDER_SAMPLE_RATE).map_err(|error| error.to_string())?;
    let settings = DeliverySettings::from_format(format).map_err(|error| error.to_string())?;
    let tail = request.tail.filter(|tail| tail.is_finite()).unwrap_or(3.0).clamp(0.0, 30.0);

    let mut project = snapshot_for_render(context.live)?;
    require_renderable(&project)?;
    let active = project["activePattern"].as_i64().unwrap_or(0);
    let mut start_pattern = active;
    if mode == "pattern" {
        let pattern = request.pattern.unwrap_or(active);
        let exists = project["patterns"].as_array().is_some_and(|patterns| patterns.iter().enumerate().any(|(index, item)| item["id"].as_i64().unwrap_or(index as i64) == pattern));
        if !exists {
            return Err("The preview or export pattern does not exist.".to_string());
        }
        project["sequence"] = json!([pattern]);
        project["audioClips"] = json!([]);
        start_pattern = pattern;
    }
    let offline = offline_engine(context, &project)?;
    let rate = offline.sample_rate();

    // The channels to capture, in the mixer's order.
    let strips = &offline.spec().strips;
    let chosen: Vec<String> = match mode {
        "stems" => match &request.owners {
            Some(owners) if owners.is_empty() => return Err("Select one or more channels for stem export.".to_string()),
            Some(owners) => owners.clone(),
            None => strips.iter().filter(|strip| matches!(strip.kind, StripKind::Track(_))).map(|strip| strip.id.clone()).collect(),
        },
        "track" => {
            let index = request.track.unwrap_or(0);
            let strip = strips.iter().find(|strip| matches!(strip.kind, StripKind::Track(track) if track as i64 == index));
            vec![strip.ok_or("The selected bounce track does not exist.")?.id.clone()]
        }
        _ => Vec::new(),
    };
    let unique: std::collections::HashSet<&String> = chosen.iter().collect();
    if unique.len() != chosen.len() || !chosen.iter().all(|owner| strips.iter().any(|strip| strip.id == *owner)) {
        return Err("Stem selection contains a duplicate or unavailable channel.".to_string());
    }
    if mode == "stems" && chosen.is_empty() {
        return Err("Add a track or select a bus before exporting stems.".to_string());
    }
    let owners: Vec<&crate::graph::spec::StripSpec> = strips.iter().filter(|strip| chosen.contains(&strip.id)).collect();

    let duration = offline.engine().duration(true);
    if !(duration > 0.0 && duration < 86_400.0) {
        return Err("Export duration is invalid or exceeds 24 hours.".to_string());
    }
    let musical_frames = (duration * rate) as u64;
    let frames = ((duration + tail) * rate) as u64;
    let delivered_frames = st_samples::export::delivery::preflight(frames, &settings).map_err(|error| error.to_string())?;

    let destination = Path::new(&request.path);
    let mut transaction = ExportTransaction::new(destination, if mode == "stems" { ExportKind::Stems } else { ExportKind::File }).map_err(|error| error.to_string())?;
    let leaf = destination.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "export.wav".to_string());
    let mut entries = Vec::new();
    let mut captures: Vec<Option<String>> = Vec::new();
    for (index, owner) in owners.iter().enumerate() {
        let name = if mode == "stems" { format!("{:02}-{}.wav", index + 1, file_name(&owner.name)) } else { leaf.clone() };
        entries.push(transaction.register(&name).map_err(|error| error.to_string())?);
        // The master's stem is the output itself.
        captures.push((owner.id != MASTER).then(|| owner.id.clone()));
    }
    if owners.is_empty() {
        entries.push(transaction.register(&leaf).map_err(|error| error.to_string())?);
        captures.push(None);
    }

    let outcome = (|| -> Result<RunSummary, String> {
        offline.play_from(start_pattern as i32, 0, 0, true);
        let sources: Vec<&Path> = entries.iter().map(|entry| entry.source_path.as_path()).collect();
        let rendering = |fraction: f64| (context.progress)(0.8 * fraction, "rendering");
        let mut target = FileTarget::new(&sources, RENDER_SAMPLE_RATE, frames, context.cancel, CANCELLED, &rendering)?;
        let summary = offline.run(&captures, Some(musical_frames), &mut target)?;
        target.finish()?;
        deliver(&entries, frames, settings, context.cancel, CANCELLED, &|fraction| (context.progress)(0.8 + 0.2 * fraction, "delivery"))?;
        Ok(summary)
    })();
    let summary = match outcome {
        Ok(summary) => summary,
        Err(error) => {
            let _ = transaction.discard();
            return Err(error);
        }
    };
    if context.cancel.is_cancelled() {
        let _ = transaction.discard();
        return Err(CANCELLED.to_string());
    }
    let paths: Vec<String> = transaction.publish().map_err(|error| error.to_string())?.into_iter().map(|path| path.to_string_lossy().into_owned()).collect();

    let musical_delivered = st_samples::export::delivery::output_frames(musical_frames, RENDER_SAMPLE_RATE, settings.sample_rate()).map_err(|error| error.to_string())?;
    let stems: Vec<Value> = owners
        .iter()
        .enumerate()
        .map(|(index, owner)| {
            json!({
                "owner": owner.id, "name": owner.name, "path": paths.get(index),
                "boundary": if owner.id == MASTER { "masterOutput" } else { "postFaderPreParent" },
                "frames": delivered_frames, "sourceFrames": frames, "sampleRate": settings.sample_rate(),
                "arrivalFrames": summary.arrivals.get(index), "arrivalSampleRate": RENDER_SAMPLE_RATE,
            })
        })
        .collect();
    let mut result = json!({
        "path": request.path, "paths": paths, "mode": mode,
        "frames": delivered_frames, "musicalFrames": musical_delivered, "tailFrames": delivered_frames - musical_delivered.min(delivered_frames), "sourceFrames": frames,
        "duration": delivered_frames as f64 / settings.sample_rate() as f64,
        "latencyCompensation": { "frames": summary.latency_frames, "seconds": summary.latency_frames as f64 / rate },
        "stems": stems, "renderPasses": 1, "stemPolicy": "fullGraph",
    });
    if let (Value::Object(result), Ok(Value::Object(metadata))) = (&mut result, serde_json::to_value(settings.metadata())) {
        result.extend(metadata);
    }
    Ok(result)
}

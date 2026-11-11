//! Render selection to sample: the notes of some tracks in a row range of one pattern, rendered
//! on a private engine while the live one keeps playing.
//!
//! The render project keeps only the selected rows of the selected tracks (other tracks are
//! muted and lose their notes and inserts), the buses and returns they reach, and the master
//! when the render goes *through master*; *track only* renders with the master at unity and
//! without its inserts. The selection starts cleanly at its first row; at its end the transport
//! stops (held notes are released) and the tail is kept until the output has stayed below the
//! threshold (default -70 dBFS) for the hold time (default 200 ms), or until the maximum tail
//! (default 10 s, at most 60), where it is faded over 10 ms. The result is a 32-bit float WAV
//! (mono or stereo; 44.1, 48 or 96 kHz; optionally normalized to -0.3 dBFS).

use crate::export::{offline_engine, require_renderable, snapshot_for_render, RenderContext};
use crate::offline::RenderTarget;
use serde_json::{json, Value};
use st_samples::export::format::RENDER_SAMPLE_RATE;
use st_samples::resample::{resample, ResampleQuality};
use st_samples::wav::{write_wav, Dither, WavEncoding, WavSpec};
use std::path::Path;

pub const MAXIMUM_TAIL_SECONDS: f64 = 60.0;
pub const DEFAULT_TAIL_SECONDS: f64 = 10.0;
/// The longest selection, in seconds.
pub const MAXIMUM_MUSICAL_SECONDS: f64 = 600.0;
/// What a cancelled render answers.
pub const CANCELLED: &str = "The render was cancelled.";

/// `render.selection`.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub pattern: i64,
    pub tracks: Vec<usize>,
    pub first_row: usize,
    pub last_row: usize,
    pub through_master: bool,
    pub maximum_tail: f64,
    pub threshold_db: f64,
    pub hold_seconds: f64,
    pub channels: usize,
    pub normalize: bool,
    pub sample_rate: u32,
    pub name: String,
}

fn whole(value: &Value) -> Option<i64> {
    value.as_f64().filter(|number| number.is_finite() && number.fract() == 0.0).map(|number| number as i64)
}

impl Request {
    /// Reads and validates the payload `{pattern, tracks, firstRow, lastRow, mode?, tail?,
    /// thresholdDB?, holdSeconds?, channels?, sampleRate?, normalize?, name?}`.
    pub fn from_payload(payload: &Value) -> Result<Request, String> {
        let pattern = whole(&payload["pattern"]).ok_or("Choose a pattern to render.")?;
        let raw: Vec<i64> = payload["tracks"].as_array().map(|tracks| tracks.iter().filter_map(whole).collect()).unwrap_or_default();
        let mut tracks: Vec<usize> = raw.iter().filter(|track| **track >= 0).map(|track| *track as usize).collect();
        tracks.sort_unstable();
        tracks.dedup();
        if tracks.is_empty() || tracks.len() != raw.len() {
            return Err("Select one or more tracks to render.".to_string());
        }
        let (first, last) = match (whole(&payload["firstRow"]), whole(&payload["lastRow"])) {
            (Some(first), Some(last)) if first >= 0 && last >= first && last < 256 => (first as usize, last as usize),
            _ => return Err("Select a row range to render.".to_string()),
        };
        let mut request = Request {
            pattern,
            tracks,
            first_row: first,
            last_row: last,
            through_master: true,
            maximum_tail: DEFAULT_TAIL_SECONDS,
            threshold_db: -70.0,
            hold_seconds: 0.2,
            channels: 2,
            normalize: payload["normalize"].as_bool().unwrap_or(false),
            sample_rate: 48_000,
            name: "Render".to_string(),
        };
        if let Some(mode) = payload["mode"].as_str() {
            if mode != "master" && mode != "track" {
                return Err("Choose Through master or Track only.".to_string());
            }
            request.through_master = mode == "master";
        }
        let number = |key: &str, low: f64, high: f64, message: &str| -> Result<Option<f64>, String> {
            match &payload[key] {
                Value::Null => Ok(None),
                value => value.as_f64().filter(|number| (low..=high).contains(number)).map(Some).ok_or_else(|| message.to_string()),
            }
        };
        if let Some(tail) = number("tail", 0.0, MAXIMUM_TAIL_SECONDS, "Choose a maximum tail between 0 and 60 seconds.")? {
            request.maximum_tail = tail;
        }
        if let Some(threshold) = number("thresholdDB", -120.0, -20.0, "Choose a silence threshold between −120 and −20 dBFS.")? {
            request.threshold_db = threshold;
        }
        if let Some(hold) = number("holdSeconds", 0.01, 5.0, "Choose a silence hold between 10 ms and 5 s.")? {
            request.hold_seconds = hold;
        }
        if !payload["channels"].is_null() {
            request.channels = whole(&payload["channels"]).filter(|channels| [1, 2].contains(channels)).ok_or("Choose mono or stereo.")? as usize;
        }
        if !payload["sampleRate"].is_null() {
            request.sample_rate = whole(&payload["sampleRate"]).filter(|rate| [44_100, 48_000, 96_000].contains(rate)).ok_or("Choose 44.1, 48 or 96 kHz.")? as u32;
        }
        if let Some(name) = payload["name"].as_str().map(str::trim).filter(|name| !name.is_empty()) {
            request.name = name.chars().take(120).collect();
        }
        Ok(request)
    }
}

/// The project to render for a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub project: Value,
    /// Exact musical length in frames at `sample_rate`; `None` with a tempo map (the render
    /// then ends where the engine stops).
    pub musical_frames: Option<u64>,
    pub kept_plugins: usize,
    pub warnings: Vec<String>,
}

fn note_cells(step: &Value) -> Vec<&Value> {
    let mut cells = vec![step];
    cells.extend(step["columns"].as_array().into_iter().flatten().filter(|cell| cell.is_object()));
    cells
}

fn has_notes(rows: &[Value], first: usize, last: usize) -> bool {
    rows.iter().skip(first).take(last + 1 - first).any(|step| step.is_object() && note_cells(step).iter().any(|cell| cell["note"].as_f64().is_some_and(|note| note >= 0.0)))
}

fn track_owner(track: &Value) -> Option<String> {
    track["routingID"].as_str().map(|id| format!("track:{id}"))
}

/// The index of the track whose channel owns a plug-in descriptor, if a track does.
fn plugin_track(plugin: &Value, tracks: &[Value]) -> Option<usize> {
    let owner = plugin["owner"].as_str()?;
    tracks.iter().position(|track| track_owner(track).as_deref() == Some(owner))
}

/// The owners the `sources` reach through outputs and sends (themselves included).
fn reachable_owners(project: &Value, sources: &[String]) -> Vec<String> {
    let routing = &project["routing"];
    let mut reached: Vec<String> = sources.to_vec();
    let mut index = 0;
    while index < reached.len() {
        let owner = reached[index].clone();
        index += 1;
        let mut next: Vec<String> = Vec::new();
        match routing["outputs"].get(&owner) {
            Some(Value::String(destination)) => next.push(destination.clone()),
            Some(Value::Null) => {}
            _ => next.push("master".to_string()),
        }
        for send in routing["sends"].as_array().into_iter().flatten() {
            if send["source"].as_str() == Some(owner.as_str()) {
                next.extend(send["destination"].as_str().map(str::to_string));
            }
        }
        for owner in next {
            if !reached.contains(&owner) {
                reached.push(owner);
            }
        }
    }
    reached
}

/// Frames of rows `first..=last` at the project's tempo (odd rows are delayed by the swing).
pub fn musical_frames(project: &Value, first: usize, last: usize, sample_rate: f64) -> Option<u64> {
    if !project["timing"].is_null() {
        return None;
    }
    let swing = (project["swing"].as_f64().unwrap_or(0.0) as f32).clamp(0.0, 0.95) as f64;
    let lines = project["linesPerBeat"].as_f64().unwrap_or(4.0).clamp(1.0, 32.0).floor();
    let bpm = project["bpm"].as_f64().unwrap_or(120.0).clamp(20.0, 999.0);
    let row = |index: usize| index as f64 + if index % 2 == 1 { swing } else { 0.0 };
    Some(((row(last + 1) - row(first)) * sample_rate * 60.0 / bpm / lines).round() as u64)
}

/// The render project for `request` from the published project (with captured insert state).
pub fn plan(canonical: &Value, request: &Request, sample_rate: f64) -> Result<Plan, String> {
    let mut project = canonical.clone();
    let mut tracks: Vec<Value> = canonical["tracks"].as_array().cloned().unwrap_or_default();
    if !request.tracks.iter().all(|track| *track < tracks.len()) {
        return Err("A selected track no longer exists.".to_string());
    }
    let mut patterns: Vec<Value> = canonical["patterns"].as_array().cloned().unwrap_or_default();
    let position = patterns.iter().enumerate().position(|(index, pattern)| whole(&pattern["id"]).unwrap_or(index as i64) == request.pattern).ok_or("The selected pattern no longer exists.")?;
    let mut pattern = patterns[position].clone();
    let length = whole(&pattern["length"]).unwrap_or(64).clamp(1, 256) as usize;
    if request.last_row >= length {
        return Err("The selection extends past the end of the pattern.".to_string());
    }
    let instruments: Vec<Value> = canonical["instruments"].as_array().cloned().unwrap_or_default();
    let steps: Vec<Value> = pattern["steps"].as_array().cloned().unwrap_or_default();
    let rows_of = |track: usize| -> Vec<Value> { steps.get(track).and_then(Value::as_array).cloned().unwrap_or_default() };
    let name_of = |track: usize| -> String { tracks[track]["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("Track {}", track + 1)) };

    let mut any_notes = false;
    for &index in &request.tracks {
        let (track, name) = (&tracks[index], name_of(index));
        if !track["frozen"].is_null() {
            return Err(format!("{name} is frozen. Unfreeze it to render its notes."));
        }
        if track["audioInput"]["enabled"] == true {
            return Err(format!("{name} plays live line input, which cannot be rendered offline. Record it to a sample first."));
        }
        let rows = rows_of(index);
        let mut used = vec![whole(&track["instrument"]).unwrap_or(0)];
        for step in rows.iter().skip(request.first_row).take(request.last_row + 1 - request.first_row).filter(|step| step.is_object()) {
            used.extend(note_cells(step).iter().filter_map(|cell| whole(&cell["instrument"])).filter(|instrument| *instrument >= 0));
        }
        if used.iter().any(|instrument| instruments.get(*instrument as usize).is_some_and(|item| item["type"] == "midi")) {
            return Err(format!("{name} plays an external MIDI instrument. Record its audio before rendering it."));
        }
        any_notes |= has_notes(&rows, request.first_row, request.last_row);
    }
    if !any_notes {
        return Err("The selection has no notes to render.".to_string());
    }

    // The pattern ends after the last selected row; only the selected tracks keep their notes.
    let rendered: Vec<Value> = (0..tracks.len())
        .map(|index| {
            let mut rows = vec![Value::Null; request.last_row + 1];
            if request.tracks.contains(&index) {
                let source = rows_of(index);
                for row in request.first_row..=request.last_row {
                    if let Some(step) = source.get(row) {
                        rows[row] = step.clone();
                    }
                }
            }
            Value::Array(rows)
        })
        .collect();
    pattern["steps"] = Value::Array(rendered);
    pattern["length"] = json!(request.last_row + 1);
    let selected_owners: Vec<String> = request.tracks.iter().filter_map(|track| track_owner(&tracks[*track])).collect();
    let lanes: Vec<Value> = pattern["automation"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|lane| match lane["owner"].as_str() {
            Some(owner) if owner.starts_with("track:") => selected_owners.iter().any(|selected| selected == owner),
            Some(owner) => request.through_master || owner != "master",
            None => request.tracks.contains(&(whole(&lane["track"]).unwrap_or(0).max(0) as usize)),
        })
        .cloned()
        .collect();
    pattern["automation"] = Value::Array(lanes);
    patterns[position] = pattern;
    project["patterns"] = Value::Array(patterns);

    let descriptors: Vec<Value> = canonical["audioPlugins"].as_array().cloned().unwrap_or_default();
    let original_tracks = tracks.clone();
    for (index, track) in tracks.iter_mut().enumerate() {
        track["solo"] = json!(false);
        if let Some(track) = track.as_object_mut() {
            track.remove("audioInput");
            track.remove("frozen");
        }
        track["mute"] = json!(!request.tracks.contains(&index));
    }
    project["tracks"] = Value::Array(tracks);

    // Plug-ins: the selected tracks', and those of every bus and return they reach; the
    // master's only through master.
    let reachable = reachable_owners(canonical, &selected_owners);
    let kept: Vec<Value> = descriptors
        .into_iter()
        .filter(|plugin| {
            if let Some(track) = plugin_track(plugin, &original_tracks) {
                return request.tracks.contains(&track);
            }
            match plugin["owner"].as_str().unwrap_or("") {
                "master" => request.through_master,
                owner if owner.starts_with("bus:") => reachable.iter().any(|reached| reached == owner),
                _ => true,
            }
        })
        .collect();
    let kept_ids: Vec<&str> = kept.iter().filter_map(|plugin| plugin["instanceID"].as_str()).collect();
    if let Some(sidechains) = project["routing"]["sidechains"].as_array().cloned() {
        let wired: Vec<Value> = sidechains.into_iter().filter(|sidechain| sidechain["destination"]["instanceID"].as_str().is_none_or(|id| kept_ids.contains(&id))).collect();
        project["routing"]["sidechains"] = Value::Array(wired);
    }
    if let Some(buses) = project["routing"]["buses"].as_array_mut() {
        for bus in buses {
            bus["solo"] = json!(false);
        }
    }
    let kept_plugins = kept.len();
    project["audioPlugins"] = Value::Array(kept);
    if !request.through_master {
        project["masterVolume"] = json!(1.0);
    }
    project["sequence"] = json!([request.pattern]);
    project["activePattern"] = json!(request.pattern);
    project["loopSong"] = json!(false);
    project["metronome"] = json!(false);
    project["audioClips"] = json!([]);
    if let Some(project) = project.as_object_mut() {
        project.remove("songLoop");
    }

    let mut warnings = Vec::new();
    let clip_on_selection = canonical["audioClips"].as_array().is_some_and(|clips| clips.iter().any(|clip| request.tracks.contains(&(whole(&clip["track"]).unwrap_or(0).max(0) as usize))));
    if clip_on_selection {
        warnings.push("Audio clips on the selected tracks are not part of a pattern render.".to_string());
    }
    let frames = musical_frames(canonical, request.first_row, request.last_row, sample_rate);
    if frames.is_some_and(|frames| frames as f64 > MAXIMUM_MUSICAL_SECONDS * sample_rate) {
        return Err(format!("The selection is longer than {} minutes. Render a shorter range.", (MAXIMUM_MUSICAL_SECONDS / 60.0) as u32));
    }
    Ok(Plan { project, musical_frames: frames, kept_plugins, warnings })
}

/// When to stop rendering a tail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TailDecision {
    Render,
    /// The output stayed below the threshold for the hold time: keep this many frames.
    Silent(u64),
    /// The maximum tail was reached: keep this many frames and fade the end.
    Maximum(u64),
}

/// `rendered` frames exist, the last one at or above the threshold is `last_loud` (`None` for
/// none). Once the musical part has ended, rendering stops `hold` quiet frames after the later
/// of the musical end and the last loud frame, or at the musical end plus `maximum_tail`.
pub fn tail_decision(rendered: u64, last_loud: Option<u64>, musical_frames: u64, musical_ended: bool, hold: u64, maximum_tail: u64) -> TailDecision {
    if !musical_ended || rendered < musical_frames {
        return TailDecision::Render;
    }
    let end = musical_frames.max(last_loud.map_or(0, |frame| frame + 1));
    let cap = musical_frames + maximum_tail;
    if end > cap {
        return TailDecision::Maximum(cap);
    }
    if rendered - end >= hold { TailDecision::Silent(end) } else { TailDecision::Render }
}

/// Collects one capture in memory and decides where its tail ends.
pub(crate) struct TailTarget<'a> {
    pub(crate) left: Vec<f32>,
    pub(crate) right: Vec<f32>,
    threshold: f32,
    hold: u64,
    maximum_tail: u64,
    musical_frames: Option<u64>,
    /// Keep exactly this many frames, whatever sounds (a fixed tail).
    pub(crate) fixed: Option<u64>,
    last_loud: Option<u64>,
    pub(crate) decision: TailDecision,
    cancel: &'a st_samples::CancelToken,
    cancelled: &'static str,
    progress: &'a dyn Fn(f64),
    reported: u64,
}

impl<'a> TailTarget<'a> {
    pub(crate) fn new(threshold_db: f64, hold: u64, maximum_tail: u64, musical_frames: Option<u64>, cancel: &'a st_samples::CancelToken, cancelled: &'static str, progress: &'a dyn Fn(f64)) -> Self {
        let expected = (musical_frames.unwrap_or(4 * 48_000) + maximum_tail / 4) as usize;
        TailTarget {
            left: Vec::with_capacity(expected),
            right: Vec::with_capacity(expected),
            threshold: 10f64.powf(threshold_db / 20.0) as f32,
            hold,
            maximum_tail,
            musical_frames,
            fixed: None,
            last_loud: None,
            decision: TailDecision::Render,
            cancel,
            cancelled,
            progress,
            reported: 0,
        }
    }

    /// The musical length: as planned, or where the engine stopped.
    pub(crate) fn musical(&self) -> u64 {
        self.musical_frames.unwrap_or(self.left.len() as u64)
    }
}

impl RenderTarget for TailTarget<'_> {
    fn write(&mut self, _capture: usize, start: u64, left: &[f32], right: &[f32]) -> Result<(), String> {
        if let Some(loud) = left.iter().zip(right).rposition(|(l, r)| l.abs() >= self.threshold || r.abs() >= self.threshold) {
            self.last_loud = Some(start + loud as u64);
        }
        self.left.extend_from_slice(left);
        self.right.extend_from_slice(right);
        Ok(())
    }

    fn wanted(&mut self, available: u64, stopped: bool) -> Result<Option<u64>, String> {
        if self.cancel.is_cancelled() {
            return Err(self.cancelled.to_string());
        }
        if stopped && self.musical_frames.is_none() {
            self.musical_frames = Some(available);
        }
        if let Some(total) = self.fixed {
            self.decision = if available >= total { TailDecision::Silent(total) } else { TailDecision::Render };
            return Ok(Some(total));
        }
        let musical = self.musical_frames.unwrap_or(u64::MAX);
        self.decision = tail_decision(available, self.last_loud, musical, stopped || available >= musical, self.hold, self.maximum_tail);
        if available >= self.reported + 32_768 {
            self.reported = available;
            let total = if musical == u64::MAX { available * 2 } else { musical + self.maximum_tail };
            (self.progress)((available as f64 / total.max(1) as f64).min(1.0));
        }
        Ok(match self.decision {
            TailDecision::Render => None,
            TailDecision::Silent(frames) | TailDecision::Maximum(frames) => Some(frames.min(available)),
        })
    }
}

/// The finished audio of a render.
pub struct Output {
    pub channels: Vec<Vec<f32>>,
    pub peak: f32,
    pub gain: f32,
}

/// Trims to `frames`, fades the last `fade_frames` (a capped tail), mixes to mono when asked and
/// normalizes the peak to -0.3 dBFS when asked.
pub fn finish(left: &[f32], right: &[f32], frames: usize, fade_frames: usize, channels: usize, normalize: bool) -> Output {
    let count = frames.min(left.len()).min(right.len());
    let (mut left, mut right) = (left[..count].to_vec(), right[..count].to_vec());
    let fade = fade_frames.min(count);
    for index in 0..fade {
        let gain = (fade - index - 1) as f32 / fade as f32;
        left[count - fade + index] *= gain;
        right[count - fade + index] *= gain;
    }
    let mut result = if channels == 1 { vec![left.iter().zip(&right).map(|(l, r)| (l + r) * 0.5).collect()] } else { vec![left, right] };
    let mut peak = result.iter().flatten().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    let mut gain = 1.0f32;
    if normalize && peak > 0.0 {
        gain = 10f32.powf(-0.3 / 20.0) / peak;
        result.iter_mut().flatten().for_each(|sample| *sample *= gain);
        peak *= gain;
    }
    Output { channels: result, peak, gain }
}

/// `render.selection`: renders `request` from the live engine's project and writes the sample
/// to `destination`. `progress` gets the phase ("capturing", "rendering", "writing", "done")
/// and 0..1 (through `context.progress`). The result is `{path, name, frames, sampleRate,
/// channels, musicalFrames, renderedFrames, tailFrames, tailEnd: "silence" or "maximum", peak,
/// normalizeGain, mode, latencyFrames, plugins, rootNote: 60, warnings?}`.
pub fn render_selection(context: &RenderContext<'_>, request: &Request, destination: &Path) -> Result<Value, String> {
    (context.progress)(0.0, "capturing");
    let canonical = snapshot_for_render(context.live)?;
    require_renderable(&json!({ "tracks": request.tracks.iter().filter_map(|track| canonical["tracks"].get(*track)).collect::<Vec<_>>() }))?;
    let rate = RENDER_SAMPLE_RATE as f64;
    let plan = plan(&canonical, request, rate)?;
    let offline = offline_engine(context, &plan.project)?;
    if context.cancel.is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    (context.progress)(0.05, "rendering");
    // The pattern was cut to end at the last selected row; playback starts at the first one.
    offline.play_from(request.pattern as i32, request.first_row as i32, 0, true);
    let rendering = |fraction: f64| (context.progress)(0.05 + 0.85 * fraction, "rendering");
    let hold = (request.hold_seconds * rate).round() as u64;
    let maximum_tail = (request.maximum_tail * rate).round() as u64;
    let mut target = TailTarget::new(request.threshold_db, hold, maximum_tail, plan.musical_frames, context.cancel, CANCELLED, &rendering);
    let summary = offline.run(&[None], plan.musical_frames, &mut target)?;
    drop(offline);

    (context.progress)(0.92, "writing");
    let (keep, tail_end) = match target.decision {
        TailDecision::Maximum(frames) => (frames, "maximum"),
        TailDecision::Silent(frames) => (frames, "silence"),
        TailDecision::Render => (target.left.len() as u64, "silence"),
    };
    let musical = target.musical().min(keep);
    let fade = if tail_end == "maximum" { (rate * 0.01) as usize } else { 0 };
    let mut output = finish(&target.left, &target.right, keep as usize, fade, request.channels, request.normalize);
    if output.channels.first().is_none_or(|channel| channel.is_empty()) || output.peak <= 0.0 {
        return Err("The selection rendered silence. Check the tracks' mute, routing and instruments.".to_string());
    }
    if request.sample_rate != RENDER_SAMPLE_RATE {
        output.channels = resample(&output.channels, RENDER_SAMPLE_RATE, request.sample_rate, ResampleQuality::Mastering).map_err(|error| error.to_string())?;
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("The folder for the rendered sample could not be created: {error}"))?;
    }
    let spec = WavSpec { sample_rate: request.sample_rate, channels: output.channels.len() as u16, encoding: WavEncoding::Float32, dither: Dither::None };
    write_wav(destination, spec, &output.channels, &[]).map_err(|error| error.to_string())?;
    let mut result = json!({
        "path": destination.to_string_lossy(), "name": request.name, "frames": output.channels[0].len(), "sampleRate": request.sample_rate,
        "channels": output.channels.len(), "musicalFrames": musical, "renderedFrames": keep, "tailFrames": keep.saturating_sub(musical),
        "tailEnd": tail_end, "peak": output.peak, "normalizeGain": output.gain, "mode": if request.through_master { "master" } else { "track" },
        "latencyFrames": summary.latency_frames, "plugins": plan.kept_plugins, "rootNote": 60,
    });
    if !plan.warnings.is_empty() {
        result["warnings"] = json!(plan.warnings);
    }
    (context.progress)(1.0, "done");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_validated_with_messages_for_the_user() {
        let request = Request::from_payload(&json!({ "pattern": 2, "tracks": [3, 1], "firstRow": 4, "lastRow": 11, "mode": "track", "tail": 2.5, "channels": 1, "sampleRate": 44100, "normalize": true, "name": "  Riff  " })).unwrap();
        assert_eq!((request.pattern, request.tracks.clone(), request.first_row, request.last_row, request.through_master), (2, vec![1, 3], 4, 11, false));
        assert_eq!((request.maximum_tail, request.channels, request.sample_rate, request.normalize, request.name.as_str()), (2.5, 1, 44_100, true, "Riff"));
        let defaults = Request::from_payload(&json!({ "pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 0 })).unwrap();
        assert_eq!((defaults.through_master, defaults.maximum_tail, defaults.threshold_db, defaults.hold_seconds, defaults.channels, defaults.sample_rate), (true, 10.0, -70.0, 0.2, 2, 48_000));
        for (payload, message) in [
            (json!({ "tracks": [0], "firstRow": 0, "lastRow": 0 }), "Choose a pattern"),
            (json!({ "pattern": 0, "tracks": [], "firstRow": 0, "lastRow": 0 }), "one or more tracks"),
            (json!({ "pattern": 0, "tracks": [1, 1], "firstRow": 0, "lastRow": 0 }), "one or more tracks"),
            (json!({ "pattern": 0, "tracks": [0], "firstRow": 5, "lastRow": 4 }), "row range"),
            (json!({ "pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 0, "mode": "bus" }), "Through master"),
            (json!({ "pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 0, "tail": 61 }), "maximum tail"),
            (json!({ "pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 0, "channels": 3 }), "mono or stereo"),
            (json!({ "pattern": 0, "tracks": [0], "firstRow": 0, "lastRow": 0, "sampleRate": 22050 }), "44.1, 48 or 96"),
        ] {
            assert!(Request::from_payload(&payload).unwrap_err().contains(message), "{payload}");
        }
    }

    #[test]
    fn the_tail_ends_at_silence_or_at_its_maximum() {
        // Still playing, or the hold not over yet.
        assert_eq!(tail_decision(900, Some(100), 1_000, false, 50, 500), TailDecision::Render);
        assert_eq!(tail_decision(1_020, Some(999), 1_000, true, 50, 500), TailDecision::Render);
        // Quiet since the musical end for the hold time: nothing of the tail is kept.
        assert_eq!(tail_decision(1_050, Some(900), 1_000, true, 50, 500), TailDecision::Silent(1_000));
        // A tail that rang until frame 1 199: kept up to there.
        assert_eq!(tail_decision(1_250, Some(1_199), 1_000, true, 50, 500), TailDecision::Silent(1_200));
        // Still loud at the cap.
        assert_eq!(tail_decision(1_600, Some(1_580), 1_000, true, 50, 500), TailDecision::Maximum(1_500));
        assert_eq!(tail_decision(1_000, None, 1_000, true, 0, 0), TailDecision::Silent(1_000));
    }

    #[test]
    fn finishing_fades_mixes_down_and_normalizes() {
        let output = finish(&[0.5; 10], &[-0.25; 10], 8, 4, 2, false);
        assert_eq!((output.channels[0].len(), output.gain, output.peak), (8, 1.0, 0.5));
        assert_eq!(output.channels[0][4..], [0.375, 0.25, 0.125, 0.0]);
        let mono = finish(&[0.5; 4], &[0.1; 4], 4, 0, 1, true);
        assert_eq!(mono.channels.len(), 1);
        assert!((mono.peak - 10f32.powf(-0.3 / 20.0)).abs() < 1e-6 && (mono.channels[0][0] - mono.peak).abs() < 1e-6);
    }
}

//! Track Freeze: a track's instrument and inserts printed to audio, and that print played in
//! their place.
//!
//! The project keeps a frozen track complete: its notes, instruments, clips, line-input route
//! and plug-in descriptors (with the state captured when it was frozen) stay where they are, and
//! `tracks[i].frozen` (`st_project::model::FreezeRecord`) names the prints. There is no second,
//! staged freeze model: this record is all there is.
//!
//! # Playing a frozen track
//!
//! Every published project passes through [`playback_project`] on its way to the engine
//! (`AudioEngine::publish`). For a frozen track it
//!
//! * removes the notes and source step effects (the mix step effects volume, pan, delay and
//!   reverb stay as control-only rows), the source automation lanes (mix lanes stay) and the
//!   track's timeline clips: they are in the print;
//! * closes its line input (the input device is released when no other track uses it, and the
//!   hardware return's latency leaves the compensation plan);
//! * adds the song print as a timeline clip on the track from beat 0. In a looping song the
//!   clip ends a hair inside the musical end, so the loop length is unchanged; otherwise (and in
//!   every export) it plays to the end of the file, tail included.
//!
//! The mixer gives a frozen track's strip no inserts ([`crate::graph::spec`]): the plug-in
//! manager unloads the instances (their latency leaves the compensation plan), while fader,
//! pan, mute, solo and sends stay live and act after the print. [`crate::project_sync`] adds
//! what a timeline clip cannot express ([`engine_prints`]): the pattern prints (a head and a
//! loop print per pattern, played by the engine in Pattern mode, aligned to the rows) and, in a
//! looping song, the printed tail ringing over the loop point (a song tail clip starting at the
//! musical end's frame).
//!
//! # Printing
//!
//! [`freeze_track`] renders on a private engine ([`crate::offline`]): the song once with the
//! track at unity (fader 1, centred, unmuted, sends at zero, its mix automation and mix step
//! effects removed), capturing the track's strip, which with the fader at unity is the signal
//! after its inserts. Then every pattern the track uses, twice: one pass with its tail (the
//! head), and several passes of which the last one with its tail is kept (the loop print: the
//! tails of the earlier passes are in it). The tail is `tail` seconds, or in auto mode what
//! stays above -90 dBFS (20 ms hold) within `maxTail` seconds. Prints are 32-bit float WAV
//! files at the render rate, identified by their SHA-256.
//!
//! A track whose sound is made outside the computer (a live line input, an external MIDI
//! instrument) cannot be rendered: [`FreezeMethod::Record`] plays the song on the live engine
//! instead and records the same prints at the track's strip ([`crate::freeze_recording`]).
//!
//! # The fingerprint
//!
//! [`fingerprint`] hashes what a print depends on: the track's source settings, the instruments
//! it plays, its plug-in descriptors (without state), its clips, its rows and source lanes in
//! the patterns the song plays or the track uses, tempo, swing and the sequence. Media files
//! enter by the SHA-256 of their content, never by their path: saving the project elsewhere
//! (which copies samples into its media folder) or relinking a moved sample to the same
//! content leaves the fingerprint as it was, so the track is not reported as stale.

use crate::graph::spec::track_owner;
use st_project::model::pattern::NOTE_NONE;
use st_project::model::{AudioClip, Project, Step, StepEffect, StepEffectKind};
use std::borrow::Cow;

fn is_mix_effect(effect: Option<&StepEffect>) -> bool {
    matches!(effect.and_then(|effect| effect.kind), Some(StepEffectKind::Volume | StepEffectKind::Pan | StepEffectKind::Delay | StepEffectKind::Reverb))
}

/// Whether a lane target is the mixer's (it stays live on a frozen track).
pub fn is_mix_target(target: &str) -> bool {
    matches!(target, "volume" | "pan" | "delay" | "reverb") || target.starts_with("send:")
}

/// The project as the engine plays it: frozen tracks play their prints. See the module
/// documentation. Borrows the project when nothing is frozen.
pub fn playback_project(project: &Project) -> Cow<'_, Project> {
    let frozen: Vec<usize> = project.track_list().iter().enumerate().filter(|(_, track)| track.frozen.as_ref().is_some_and(|record| record.path.as_deref().is_some_and(|path| !path.is_empty()))).map(|(index, _)| index).collect();
    if frozen.is_empty() {
        return Cow::Borrowed(project);
    }
    let owners: Vec<String> = frozen.iter().map(|index| track_owner(project, *index)).collect();
    let looping = project.loop_song();
    let mut result = project.clone();
    for pattern in result.patterns.iter_mut().flatten() {
        for (track, rows) in pattern.steps.iter_mut().flatten().enumerate() {
            if !frozen.contains(&track) {
                continue;
            }
            for cell in rows.iter_mut() {
                let Some(step) = cell.take() else { continue };
                // A note or an empty row keeps its mix effects; a note-off or cut row controls
                // nothing live.
                let live = step.note() >= 0 || step.note() == NOTE_NONE;
                let fx1 = step.fx1.filter(|effect| live && is_mix_effect(Some(effect)));
                let fx2 = step.fx2.filter(|effect| live && is_mix_effect(Some(effect)));
                if fx1.is_some() || fx2.is_some() {
                    *cell = Some(Step { fx1, fx2, ..Step::default() });
                }
            }
        }
        if let Some(lanes) = pattern.automation.as_mut() {
            lanes.retain(|lane| {
                let belongs = match lane.owner.as_deref() {
                    Some(owner) => owners.iter().any(|frozen| frozen == owner),
                    None => frozen.contains(&(lane.track().max(0) as usize)),
                };
                !belongs || is_mix_target(lane.target_name())
            });
        }
    }
    let mut clips: Vec<AudioClip> = result.audio_clips.take().unwrap_or_default();
    clips.retain(|clip| !frozen.contains(&(clip.track().max(0) as usize)));
    for &index in &frozen {
        let Some(track) = result.tracks.as_mut().and_then(|tracks| tracks.get_mut(index)) else { continue };
        if let Some(input) = track.audio_input.as_mut() {
            input.enabled = Some(false);
        }
        let Some(record) = &track.frozen else { continue };
        let end = record.musical_end_beat.unwrap_or(0.0);
        clips.push(AudioClip {
            id: Some(format!("freeze-print-{}", record.id.as_deref().unwrap_or_default())),
            track: Some(index as i64),
            path: record.path.clone(),
            start_beat: Some(0.0),
            // 0 plays to the end of the file (the tail extends a song that does not loop).
            duration_beats: Some(if looping { (end - 1e-7).max(1e-6) } else { 0.0 }),
            source_offset: Some(0.0),
            gain: Some(1.0),
            fade_in: Some(0.0),
            fade_out: Some(0.0),
            muted: Some(false),
            ..AudioClip::default()
        });
    }
    result.audio_clips = Some(clips);
    Cow::Owned(result)
}

/// What the engine plays for frozen tracks beyond timeline clips.
#[derive(Debug, Clone, PartialEq)]
pub enum EnginePrint {
    /// A pattern's prints for Pattern mode: the first pass with its tail, and the looping pass.
    Pattern { track: usize, pattern: i64, head: String, loop_print: Option<String> },
    /// A looping song: the printed tail from frame `start_frame` of the print, played from the
    /// start of the next pass.
    SongTail { track: usize, path: String, start_frame: i64 },
}

impl EnginePrint {
    /// The files this print plays.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        let (first, second) = match self {
            EnginePrint::Pattern { head, loop_print, .. } => (head.as_str(), loop_print.as_deref()),
            EnginePrint::SongTail { path, .. } => (path.as_str(), None),
        };
        std::iter::once(first).chain(second)
    }
}

/// The pattern prints and song tails of the frozen tracks of `project`.
pub fn engine_prints(project: &Project) -> Vec<EnginePrint> {
    let mut prints = Vec::new();
    for (track, item) in project.track_list().iter().enumerate() {
        let Some(record) = &item.frozen else { continue };
        for print in record.pattern_prints.iter().flatten() {
            let (Some(pattern), Some(head)) = (print.pattern, print.path.as_deref().filter(|path| !path.is_empty())) else { continue };
            let loop_print = print.loop_print.as_ref().and_then(|print| print.path.clone()).filter(|path| !path.is_empty());
            prints.push(EnginePrint::Pattern { track, pattern, head: head.to_string(), loop_print });
        }
        let musical = record.musical_frames.unwrap_or(0);
        if let (true, true, Some(path)) = (project.loop_song(), musical > 0 && record.tail_frames.unwrap_or(0) > 0, record.path.as_deref().filter(|path| !path.is_empty())) {
            prints.push(EnginePrint::SongTail { track, path: path.to_string(), start_frame: musical });
        }
    }
    prints
}

// ---- printing -----------------------------------------------------------------------------

use crate::export::{offline_engine, snapshot_for_render, RenderContext};
use crate::offline::OfflineRender;
use crate::render_selection::{TailDecision, TailTarget};
use serde_json::{json, Value};
use st_samples::export::format::RENDER_SAMPLE_RATE;
use st_samples::wav::{write_wav, Dither, WavEncoding, WavSpec};
use std::path::Path;

/// What a cancelled freeze answers.
pub const CANCELLED: &str = "Freeze was cancelled. Nothing was changed.";
const AUTO_THRESHOLD_DB: f64 = -90.0;
const AUTO_HOLD_SECONDS: f64 = 0.02;

/// The SHA-256 (hex) of a media file's content, by the path a project names it with; `None`
/// when the file cannot be read. The host caches these by file identity.
pub type ContentHash<'a> = &'a dyn Fn(&str) -> Option<String>;

/// How a track is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeMethod {
    /// Rendered on a private engine, faster than real time.
    Offline,
    /// Played once on the live engine and recorded at the track's strip
    /// ([`crate::freeze_recording`]): for sound that is made outside the computer.
    Record,
}

/// `freeze.track`.
#[derive(Debug, Clone, PartialEq)]
pub struct FreezeRequest {
    pub track: usize,
    /// A fixed tail in seconds (0..30); `None` for an automatic one.
    pub tail: Option<f64>,
    /// Auto tail: the most that is kept, seconds (0..30, default 10).
    pub maximum_tail: f64,
    pub method: FreezeMethod,
    /// Freeze by recording: also record a head and a loop print of every pattern the track
    /// uses (each is a take of its own). Without them the track is silent in Pattern mode.
    pub patterns: bool,
    /// Freeze by recording: whether the mix is heard during the takes.
    pub monitor: bool,
}

impl FreezeRequest {
    /// Reads `{track, tail?, maxTail?, method?: "offline" or "record", patterns?, monitor?}`.
    pub fn from_payload(payload: &Value) -> Result<FreezeRequest, String> {
        let track = payload["track"].as_f64().filter(|track| track.is_finite() && *track >= 0.0).ok_or("Choose a track to freeze.")? as usize;
        let method = match payload["method"].as_str() {
            None | Some("offline") => FreezeMethod::Offline,
            Some("record") => FreezeMethod::Record,
            Some(other) => return Err(format!("Unknown freeze method {other}: choose offline or record.")),
        };
        let seconds = |key: &str| -> Result<Option<f64>, String> {
            match &payload[key] {
                Value::Null => Ok(None),
                value => value.as_f64().filter(|seconds| (0.0..=30.0).contains(seconds)).map(Some).ok_or_else(|| "Choose a tail between 0 and 30 seconds.".to_string()),
            }
        };
        Ok(FreezeRequest {
            track,
            tail: seconds("tail")?,
            maximum_tail: seconds("maxTail")?.unwrap_or(10.0),
            method,
            patterns: payload["patterns"].as_bool().unwrap_or(true),
            monitor: payload["monitor"].as_bool().unwrap_or(true),
        })
    }
}

fn whole(value: &Value, fallback: i64) -> i64 {
    value.as_f64().filter(|number| number.is_finite()).map_or(fallback, |number| number as i64)
}

fn owner_of(track: &Value) -> Option<String> {
    track["routingID"].as_str().filter(|id| !id.is_empty()).map(|id| format!("track:{id}"))
}

fn lane_belongs(lane: &Value, index: usize, owner: Option<&str>) -> bool {
    match lane["owner"].as_str() {
        Some(lane_owner) => Some(lane_owner) == owner,
        None => whole(&lane["track"], 0) == index as i64,
    }
}

fn is_mix_lane(lane: &Value) -> bool {
    is_mix_target(lane["target"].as_str().unwrap_or("volume"))
}

fn is_mix_fx(effect: &Value) -> bool {
    matches!(effect["type"].as_str(), Some("volume" | "pan" | "delay" | "reverb"))
}

/// A step without its mix step effects; `Null` when nothing else is in it.
fn source_step(step: &Value) -> Value {
    let Some(step) = step.as_object() else { return Value::Null };
    let kept: serde_json::Map<String, Value> = step.iter().filter(|(key, value)| !((key.as_str() == "fx1" || key.as_str() == "fx2") && is_mix_fx(value))).map(|(key, value)| (key.clone(), value.clone())).collect();
    if kept.is_empty() { Value::Null } else { Value::Object(kept) }
}

/// Whether `pattern` has anything of track `index` that a print contains: notes, source step
/// effects or source automation.
pub fn track_uses(pattern: &Value, index: usize, owner: Option<&str>) -> bool {
    let rows = pattern["steps"][index].as_array();
    rows.is_some_and(|rows| rows.iter().any(|step| !source_step(step).is_null())) || pattern["automation"].as_array().is_some_and(|lanes| lanes.iter().any(|lane| lane_belongs(lane, index, owner) && !is_mix_lane(lane)))
}

fn pattern_id(pattern: &Value, position: usize) -> i64 {
    whole(&pattern["id"], position as i64)
}

/// JSON with object keys in sorted order, whatever order the value holds them in.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        // 1 and 1.0 are the same number, whichever way a writer spelled it.
        Value::Number(number) => match number.as_f64() {
            Some(value) => out.push_str(&format!("{value}")),
            None => out.push_str(&number.to_string()),
        },
        other => out.push_str(&other.to_string()),
    }
}

/// A media reference as the fingerprint sees it: the content's hash for a file, the reference
/// itself for built-in samples and for files that cannot be read.
fn media_identity(path: &str, hash: ContentHash<'_>) -> Value {
    if path.is_empty() || st_samples::builtin::is_builtin(path) {
        return json!(path);
    }
    match hash(path) {
        Some(digest) => json!(format!("sha256:{digest}")),
        None => json!(format!("unreadable:{}", Path::new(path).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default())),
    }
}

/// Replaces the media paths of an instrument or clip object by content identities.
fn with_media_identities(item: &Value, hash: ContentHash<'_>) -> Value {
    let mut item = item.clone();
    for key in ["samplePath", "path"] {
        if let Some(path) = item[key].as_str().map(str::to_string) {
            item[key] = media_identity(&path, hash);
        }
    }
    if let Some(zones) = item["zones"].as_array_mut() {
        for zone in zones {
            *zone = with_media_identities(zone, hash);
        }
    }
    if item["takeSet"].is_object() {
        // Takes other than the active one are not heard.
        item.as_object_mut().map(|item| item.remove("takeSet"));
    }
    item
}

fn without(item: &Value, keys: &[&str]) -> Value {
    match item.as_object() {
        Some(map) => Value::Object(map.iter().filter(|(key, _)| !keys.contains(&key.as_str())).map(|(key, value)| (key.clone(), value.clone())).collect()),
        None => item.clone(),
    }
}

/// The fingerprint of what a print of track `index` depends on. See the module documentation.
pub fn fingerprint(project: &Value, index: usize, hash: ContentHash<'_>) -> Result<String, String> {
    let track = project["tracks"].get(index).ok_or("Choose an existing track.")?;
    let owner = owner_of(track);
    let empty = Vec::new();
    let instruments = project["instruments"].as_array().unwrap_or(&empty);
    let mut referenced = vec![whole(&track["instrument"], 0)];
    let sequence: Vec<i64> = project["sequence"].as_array().map_or_else(|| vec![0], |sequence| sequence.iter().map(|item| whole(item, 0)).collect());
    let mut patterns = Vec::new();
    for (position, pattern) in project["patterns"].as_array().unwrap_or(&empty).iter().enumerate() {
        if !sequence.contains(&pattern_id(pattern, position)) && !track_uses(pattern, index, owner.as_deref()) {
            continue;
        }
        let rows: Vec<Value> = pattern["steps"][index].as_array().unwrap_or(&empty).iter().map(source_step).collect();
        for step in &rows {
            referenced.extend(std::iter::once(step).chain(step["columns"].as_array().into_iter().flatten()).map(|cell| whole(&cell["instrument"], -1)).filter(|instrument| *instrument >= 0));
        }
        let lanes: Vec<Value> = pattern["automation"].as_array().unwrap_or(&empty).iter().filter(|lane| lane_belongs(lane, index, owner.as_deref()) && !is_mix_lane(lane)).map(|lane| without(lane, &["id", "name"])).collect();
        patterns.push(json!({ "id": pattern_id(pattern, position), "length": whole(&pattern["length"], 64), "steps": rows, "automation": lanes }));
    }
    referenced.sort_unstable();
    referenced.dedup();
    let cosmetic = ["name", "color", "selected", "collapsed", "editorOpen", "windowFrame", "viewState", "id", "plugins", "frozen", "volume", "pan", "mute", "solo", "delaySend", "reverbSend"];
    let volatile = ["parameters", "latency", "latencyFrames", "tailTime", "stateError", "name", "editorOpen", "windowFrame", "state"];
    let plugins: Vec<Value> = project["audioPlugins"].as_array().unwrap_or(&empty).iter().filter(|plugin| plugin["owner"].as_str().is_some() && plugin["owner"].as_str() == owner.as_deref()).map(|plugin| without(plugin, &volatile)).collect();
    let clips: Vec<Value> = project["audioClips"].as_array().unwrap_or(&empty).iter().filter(|clip| whole(&clip["track"], 0) == index as i64).map(|clip| with_media_identities(&without(clip, &["id", "name"]), hash)).collect();
    let payload = json!({
        "track": without(track, &cosmetic),
        "instruments": referenced.iter().map(|instrument| instruments.get(*instrument as usize).map_or(Value::Null, |item| with_media_identities(&without(item, &["name", "id", "plugin"]), hash))).collect::<Vec<_>>(),
        "plugins": plugins,
        "clips": clips,
        "patterns": patterns,
        "timing": { "bpm": project["bpm"].as_f64().unwrap_or(120.0), "linesPerBeat": whole(&project["linesPerBeat"], 4), "swing": project["swing"].as_f64().unwrap_or(0.0), "sequence": sequence },
    });
    let mut text = String::new();
    canonical(&payload, &mut text);
    Ok(st_project::media::sha256_hex(text.as_bytes()))
}

/// The project a print of track `index` is rendered from: the track at unity, without its mix
/// automation and mix step effects.
fn print_project(canonical: &Value, index: usize) -> Value {
    let mut project = canonical.clone();
    let owner = owner_of(&canonical["tracks"][index]);
    let track = &mut project["tracks"][index];
    for (key, value) in [("volume", json!(1.0)), ("pan", json!(0.0)), ("mute", json!(false)), ("solo", json!(false)), ("delaySend", json!(0.0)), ("reverbSend", json!(0.0))] {
        track[key] = value;
    }
    for pattern in project["patterns"].as_array_mut().into_iter().flatten() {
        if let Some(rows) = pattern["steps"][index].as_array_mut() {
            for step in rows {
                *step = source_step(step);
            }
        }
        if let Some(lanes) = pattern["automation"].as_array_mut() {
            lanes.retain(|lane| !(lane_belongs(lane, index, owner.as_deref()) && is_mix_lane(lane)));
        }
    }
    project["loopSong"] = json!(false);
    project["metronome"] = json!(false);
    project
}

/// Why track `index` cannot be printed by `method`, if it cannot.
fn refusal(project: &Value, index: usize, method: FreezeMethod) -> Option<String> {
    let track = &project["tracks"][index];
    let name = track["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("Track {}", index + 1));
    if !track["frozen"].is_null() {
        return Some(format!("{name} is already frozen."));
    }
    let line_input = track["audioInput"]["enabled"] == true;
    let instruments = project["instruments"].as_array();
    let mut used = vec![whole(&track["instrument"], 0)];
    for pattern in project["patterns"].as_array().into_iter().flatten() {
        used.extend(pattern["steps"][index].as_array().into_iter().flatten().map(|step| whole(&step["instrument"], -1)).filter(|instrument| *instrument >= 0));
    }
    let external = used.iter().any(|instrument| instruments.and_then(|items| items.get(*instrument as usize)).is_some_and(|item| item["type"] == "midi"));
    match method {
        FreezeMethod::Offline if line_input => return Some(format!("{name} plays a live line input, which cannot be rendered offline. Choose Freeze by recording.")),
        FreezeMethod::Offline if external => {
            return Some(format!("{name} plays an external MIDI instrument, which cannot be rendered offline. Choose Freeze by recording, with the instrument's return as this track's live input."));
        }
        // The instrument's sound has to come back somewhere for a recording to hold it.
        FreezeMethod::Record if external && !line_input => {
            return Some(format!("{name} plays an external MIDI instrument whose sound does not come back into it. Make the instrument's return this track's live input, then freeze by recording."));
        }
        _ => {}
    }
    let owner = owner_of(track)?;
    let routing = &project["routing"];
    let into = routing["outputs"].as_object().is_some_and(|outputs| outputs.values().any(|destination| destination.as_str() == Some(owner.as_str())))
        || routing["sends"].as_array().is_some_and(|sends| sends.iter().any(|send| send["destination"].as_str() == Some(owner.as_str())));
    if into {
        return Some(format!("{name} receives another channel's output or send. Remove that route before freezing it."));
    }
    let pre_insert = ["sends", "sidechains"].iter().any(|kind| routing[*kind].as_array().is_some_and(|routes| routes.iter().any(|route| route["source"].as_str() == Some(owner.as_str()) && route["tap"] == "preInserts")));
    if pre_insert {
        return Some(format!("{name} has a send or sidechain tapped before its inserts, which a print cannot feed. Move the tap after the inserts before freezing."));
    }
    None
}

/// One print file.
pub(crate) struct Print {
    pub path: String,
    pub sha256: String,
    pub frames: u64,
    /// Frames of the musical part (the rest is tail).
    pub musical: u64,
}

/// What a freeze prints, whichever way: the song, and the patterns as the record lists them.
pub(crate) struct Prints {
    /// Sample rate of the files.
    pub rate: f64,
    pub song: Print,
    /// `{pattern, path, sha256, frames, passFrames, passes, loop: {path, sha256, frames}}` each.
    pub patterns: Vec<Value>,
    /// Patterns the track uses that have no print.
    pub unprinted: Vec<i64>,
    /// What the user should know about the prints.
    pub notes: Vec<String>,
}

/// What a freeze of one track works from.
pub(crate) struct PrintPlan<'a> {
    /// The project with the state of every insert captured (no loop, no metronome).
    pub canonical: &'a Value,
    pub index: usize,
    /// The track's strip, `track:<routingID>`.
    pub owner: String,
    pub name: String,
    /// The numbers of the patterns the track uses, for Pattern mode.
    pub used: Vec<i64>,
    /// A fixed tail in seconds, or `None` for an automatic one of at most `maximum_tail`.
    pub tail: Option<f64>,
    pub maximum_tail: f64,
    directory: &'a Path,
    id: String,
}

impl PrintPlan<'_> {
    /// The file of the print called `suffix` ("song", "p3-head", ...).
    pub fn file(&self, suffix: &str) -> std::path::PathBuf {
        let safe: String = self.name.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(40).collect();
        self.directory.join(format!("{}-{}-{suffix}.wav", if safe.trim().is_empty() { "Track" } else { safe.trim() }, self.id))
    }

    /// Passes of a pattern's loop print: enough for the tails of the earlier passes to have
    /// reached their steady state in the last one. `tail` is the head print's tail in frames.
    pub fn loop_passes(&self, tail: u64, pass: u64, rate: f64) -> u64 {
        match self.tail {
            Some(fixed) => 1 + ((fixed * rate) as u64).div_ceil(pass.max(1)),
            None => 1 + (tail * 3 / 2).div_ceil(pass.max(1)),
        }
        .clamp(2, 64)
    }
}

/// What one print renders.
struct PrintJob<'a> {
    pattern: i32,
    /// Frames of the musical part (all passes).
    musical_frames: u64,
    /// A fixed tail in frames, or `None` for an automatic one.
    fixed_tail: Option<u64>,
    maximum_tail: u64,
    /// Frames dropped from the start (the earlier passes of a loop print).
    drop_frames: u64,
    destination: &'a Path,
}

fn render_print(context: &RenderContext<'_>, offline: &OfflineRender, owner: &str, job: &PrintJob<'_>, progress: &dyn Fn(f64)) -> Result<Print, String> {
    let rate = RENDER_SAMPLE_RATE as f64;
    offline.play_from(job.pattern, 0, 0, true);
    let mut target = TailTarget::new(AUTO_THRESHOLD_DB, (AUTO_HOLD_SECONDS * rate) as u64, job.maximum_tail, job.musical_frames, context.cancel, CANCELLED, progress);
    target.fixed = job.fixed_tail.map(|tail| job.musical_frames + tail);
    offline.run(&[Some(owner.to_string())], Some(job.musical_frames), &mut target)?;
    let keep = match target.decision {
        TailDecision::Silent(frames) | TailDecision::Maximum(frames) => frames,
        TailDecision::Render => target.left.len() as u64,
    }
    .min(target.left.len() as u64);
    let (from, to) = ((job.drop_frames.min(keep)) as usize, keep as usize);
    let spec = WavSpec { sample_rate: RENDER_SAMPLE_RATE, channels: 2, encoding: WavEncoding::Float32, dither: Dither::None };
    write_wav(job.destination, spec, &[&target.left[from..to], &target.right[from..to]], &[]).map_err(|error| error.to_string())?;
    let sha256 = st_project::media::hash_file(job.destination, u64::MAX, None).map_err(|error| error.to_string())?;
    // Leave the engine silent for the next print.
    offline.engine().stop();
    offline.engine().panic();
    offline.engine().render_offline(2 * crate::offline::BLOCK, crate::offline::BLOCK);
    Ok(Print { path: job.destination.to_string_lossy().into_owned(), sha256, frames: (to - from) as u64, musical: job.musical_frames })
}

/// Seconds since the Unix epoch as an ISO 8601 UTC time.
fn iso8601(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3_600, rest % 3_600 / 60, rest % 60)
}

/// Refuses a song print the project format cannot record: the musical part and the longest tail
/// it may get, in frames at `rate`, against `st_project::validate::MAX_FREEZE_FRAMES`. Both ways
/// of freezing ask before they render or record anything.
pub(crate) fn check_print_length(musical: u64, tail_limit: u64, rate: f64) -> Result<(), String> {
    let limit = st_project::validate::MAX_FREEZE_FRAMES as u64;
    if musical.saturating_add(tail_limit) > limit {
        return Err(format!("The song is too long to freeze: a print holds {} minutes at most.", (limit as f64 / rate / 60.0).floor()));
    }
    Ok(())
}

/// The song and the patterns of `plan`, rendered on a private engine.
fn offline_prints(context: &RenderContext<'_>, plan: &PrintPlan<'_>, written: &mut Vec<String>) -> Result<Prints, String> {
    let rate = RENDER_SAMPLE_RATE as f64;
    let print = print_project(plan.canonical, plan.index);
    let maximum_tail = (plan.maximum_tail * rate) as u64;
    let fixed_tail = plan.tail.map(|tail| (tail * rate) as u64);
    let steps = 1 + 2 * plan.used.len();
    let mut offline = offline_engine(context, &print)?;
    let duration = offline.engine().duration(true);
    let musical_frames = (duration * rate) as u64;
    if duration.is_nan() || musical_frames == 0 {
        return Err("The song is empty: there is nothing to freeze.".to_string());
    }
    check_print_length(musical_frames, fixed_tail.unwrap_or(maximum_tail), rate)?;
    let song_path = plan.file("song");
    written.push(song_path.to_string_lossy().into_owned());
    let start = whole(&plan.canonical["activePattern"], 0) as i32;
    let song = render_print(context, &offline, &plan.owner, &PrintJob { pattern: start, musical_frames, fixed_tail, maximum_tail, drop_frames: 0, destination: &song_path }, &|fraction| (context.progress)(fraction / steps as f64, "song"))?;

    // Each pattern: a head (one pass and its tail) and the looping pass.
    let mut patterns = Vec::new();
    for (position, number) in plan.used.iter().enumerate() {
        let step = |part: usize| move |fraction: f64| (context.progress)((1 + 2 * position + part) as f64 / steps as f64 + fraction / steps as f64, "patterns");
        let mut project = print.clone();
        project["audioClips"] = json!([]);
        project["sequence"] = json!([number]);
        offline.publish(&st_project::model::Project::from_value(project.clone()).map_err(|error| error.to_string())?)?;
        let pass = (offline.engine().duration(true) * rate) as u64;
        if pass == 0 {
            continue;
        }
        let head_path = plan.file(&format!("p{number}-head"));
        written.push(head_path.to_string_lossy().into_owned());
        let head = render_print(context, &offline, &plan.owner, &PrintJob { pattern: *number as i32, musical_frames: pass, fixed_tail, maximum_tail, drop_frames: 0, destination: &head_path }, &step(0))?;
        let tail = head.frames.saturating_sub(pass);
        let passes = plan.loop_passes(tail, pass, rate);
        project["sequence"] = json!(vec![*number; passes as usize]);
        offline.publish(&st_project::model::Project::from_value(project).map_err(|error| error.to_string())?)?;
        let loop_path = plan.file(&format!("p{number}-loop"));
        written.push(loop_path.to_string_lossy().into_owned());
        let total = (offline.engine().duration(true) * rate) as u64;
        let looped = render_print(context, &offline, &plan.owner, &PrintJob { pattern: *number as i32, musical_frames: total, fixed_tail: Some(tail), maximum_tail, drop_frames: total.saturating_sub(pass), destination: &loop_path }, &step(1))?;
        patterns.push(json!({
            "pattern": number, "path": head.path, "sha256": head.sha256, "frames": head.frames, "passFrames": pass, "passes": passes,
            "loop": { "path": looped.path, "sha256": looped.sha256, "frames": looped.frames },
        }));
    }
    Ok(Prints { rate, song, patterns, unprinted: Vec::new(), notes: Vec::new() })
}

/// `freeze.track`: prints the track's song and patterns into `directory` and answers with
/// `{record, audioPlugins, releasedPlugins, unprintedPatterns, notes, track}`: `record` is the
/// `frozen` object the interface stores in the track, `audioPlugins` the project's plug-in
/// descriptors with the state captured for the print. `context.progress` gets 0..1 and the
/// phase ("song", "patterns"). The prints are rendered offline or, with
/// [`FreezeMethod::Record`], recorded from the live engine ([`crate::freeze_recording`]).
pub fn freeze_track(context: &RenderContext<'_>, request: &FreezeRequest, directory: &Path, hash: ContentHash<'_>) -> Result<Value, String> {
    // The snapshot is for rendering (no loop, no metronome); the project's own values of those
    // are not part of a print.
    let canonical = snapshot_for_render(context.live)?;
    let index = request.track;
    if canonical["tracks"].get(index).is_none() {
        return Err("Choose an existing track.".to_string());
    }
    if let Some(reason) = refusal(&canonical, index, request.method) {
        return Err(reason);
    }
    let track = &canonical["tracks"][index];
    let owner = owner_of(track).ok_or_else(|| format!("Track {} has no routingID.", index + 1))?;
    let name = track["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("Track {}", index + 1));
    let id = format!("{}{}", st_samples::paths::unique_suffix(), st_samples::paths::unique_suffix()).to_lowercase();
    std::fs::create_dir_all(directory).map_err(|error| format!("The folder for frozen audio could not be created: {error}"))?;
    let empty = Vec::new();
    // Patterns the track uses, for Pattern mode.
    let used: Vec<i64> = canonical["patterns"].as_array().unwrap_or(&empty).iter().enumerate().filter(|(_, pattern)| track_uses(pattern, index, Some(&owner))).map(|(position, pattern)| pattern_id(pattern, position)).collect();
    let plan = PrintPlan { canonical: &canonical, index, owner: owner.clone(), name, used, tail: request.tail, maximum_tail: request.maximum_tail, directory, id: id.clone() };
    let mut written: Vec<String> = Vec::new();
    let outcome = (|| -> Result<Value, String> {
        let prints = match request.method {
            FreezeMethod::Offline => offline_prints(context, &plan, &mut written)?,
            FreezeMethod::Record => crate::freeze_recording::recorded_prints(context, &plan, request, &mut written)?,
        };
        let song = &prints.song;
        let lines = canonical["linesPerBeat"].as_f64().unwrap_or(4.0).clamp(1.0, 32.0);
        let length_of = |number: i64| canonical["patterns"].as_array().unwrap_or(&empty).iter().enumerate().find(|(position, pattern)| pattern_id(pattern, *position) == number).map_or(64, |(_, pattern)| whole(&pattern["length"], 64).clamp(1, 256));
        let sequence: Vec<i64> = canonical["sequence"].as_array().map_or_else(|| vec![0], |sequence| sequence.iter().map(|item| whole(item, 0)).collect());
        let musical_end_beat: f64 = sequence.iter().map(|number| length_of(*number) as f64 / lines).sum();
        let dormant: Vec<&str> = canonical["audioPlugins"].as_array().unwrap_or(&empty).iter().filter(|plugin| plugin["owner"].as_str() == Some(owner.as_str())).filter_map(|plugin| plugin["instanceID"].as_str()).collect();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
        let record = json!({
            "version": 1, "id": id, "path": song.path, "sha256": song.sha256, "sampleRate": prints.rate,
            "frames": song.frames, "musicalFrames": song.musical.min(song.frames), "tailFrames": song.frames.saturating_sub(song.musical),
            "musicalEndBeat": musical_end_beat, "tailSeconds": request.tail.unwrap_or(request.maximum_tail),
            "dormantPluginIDs": dormant, "fingerprint": fingerprint(&canonical, index, hash)?, "createdAt": iso8601(now),
            "patternPrints": prints.patterns, "unprintedPatterns": prints.unprinted,
            "tailMode": if request.tail.is_some() { "fixed" } else { "auto" }, "method": if request.method == FreezeMethod::Record { "recorded" } else { "offline" },
        });
        Ok(json!({ "track": index, "record": record, "audioPlugins": canonical["audioPlugins"], "releasedPlugins": dormant.len(), "unprintedPatterns": prints.unprinted, "notes": prints.notes }))
    })();
    if outcome.is_err() {
        for path in &written {
            let _ = std::fs::remove_file(path);
        }
    }
    (context.progress)(1.0, "done");
    outcome
}

/// `freeze.status`: one row per frozen track of `project`: `{track, id, available, stale,
/// path, dormantPlugins, patternPrints, patternPlayback, unprintedPatterns}`.
pub fn status(project: &Value, hash: ContentHash<'_>) -> Vec<Value> {
    let mut rows = Vec::new();
    let empty = Vec::new();
    for (index, track) in project["tracks"].as_array().unwrap_or(&empty).iter().enumerate() {
        let record = &track["frozen"];
        if !record.is_object() {
            continue;
        }
        let readable = |path: &Value| path.as_str().is_some_and(|path| std::fs::File::open(path).is_ok());
        let prints = record["patternPrints"].as_array().unwrap_or(&empty);
        let available = readable(&record["path"]) && prints.iter().all(|print| readable(&print["path"]) && readable(&print["loop"]["path"]));
        let owner = owner_of(track);
        let printed: Vec<i64> = prints.iter().map(|print| whole(&print["pattern"], -1)).collect();
        let missing: Vec<i64> = project["patterns"].as_array().unwrap_or(&empty).iter().enumerate().filter(|(position, pattern)| track_uses(pattern, index, owner.as_deref()) && !printed.contains(&pattern_id(pattern, *position))).map(|(position, pattern)| pattern_id(pattern, position)).collect();
        let recorded = record["fingerprint"].as_str().unwrap_or("");
        let stale = !recorded.is_empty() && fingerprint(project, index, hash).is_ok_and(|current| current != recorded);
        rows.push(json!({
            "track": index, "id": record["id"], "available": available, "stale": stale, "path": record["path"],
            "dormantPlugins": record["dormantPluginIDs"].as_array().map_or(0, Vec::len), "patternPrints": prints.len(),
            "patternPlayback": missing.is_empty(), "unprintedPatterns": missing,
        }));
    }
    rows
}

//! The export area: everything rendered offline from the open project.
//!
//! | Command | Notes |
//! |---|---|
//! | `audio.export` `{path, mode?: "song" / "pattern" / "stems" / "track", owners?, track?, pattern?, tail?, encoding?, sampleRate?, dither?}` | `st_audio::export::export` on a private engine; `audio.exportProgress` `{progress, path, phase}` events, `audio.exported` at the end. `transport.stop` (or `engine.panic`) cancels it: nothing is published and the destination is as it was |
//! | `audio.bounceTrack` `{track, path?, tail?}` | a track-mode export (32-bit float at the render rate) to `path` or to `Renders` in the support folder; answers with a clip for the timeline |
//! | `render.selection` `{pattern, tracks, firstRow, lastRow, mode?, tail?, sampleRate?, channels?, normalize?, name?, path?}` | `st_audio::render_selection`; the sample goes to `Recordings` in the support folder unless `path` is given; `render.selectionProgress` `{job, phase, progress}` events |
//! | `render.cancelSelection` | cancels the running selection render: `{cancelled: bool}` |
//! | `freeze.track` `{track, tail?, maxTail?, method?, patterns?, monitor?}` | `st_audio::freeze::freeze_track`: prints the track's song and patterns into `Freezes` of the support folder and answers `{record, audioPlugins, releasedPlugins, unprintedPatterns, notes}`; the interface stores `record` as `tracks[i].frozen` and publishes, from when the engine plays the prints and the track's plug-ins are unloaded (and its line input closed). `freeze.progress` `{track, progress, phase, recording}` events. `method: "record"` records the prints from the live engine instead (`st_audio::freeze_recording`): the song plays in real time, once per print, and `transport.stop` cancels |
//! | `freeze.unfreeze` `{track}` | nothing to do in the host but to agree: the interface removes `frozen` and publishes, and the engine plays the live chain again (the plug-ins load from their descriptors) |
//! | `freeze.status` | `{tracks: [{track, id, available, stale, patternPlayback, …}], busy}`: a print is stale when the fingerprint of what it was made from has changed; media files count by content (SHA-256), so Save As and relinking do not make a print stale |
//!
//! Each render runs on its command's own worker thread, on an engine of its own
//! ([`st_audio::offline`]): the live engine keeps playing and is not touched. The exception is
//! a freeze by recording, which is the live engine playing. One export (or bounce, or freeze)
//! and one selection render can run at a time; while an export runs the status reports
//! `exporting`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use st_audio::export::{ExportRequest, RenderContext};
use st_audio::render_selection;
use st_samples::CancelToken;

use crate::areas::audio::Audio;
use crate::commands::{Commands, NoPayload};
use crate::{fail, Error, EventSink, Result};

/// The export service of the host.
pub struct Export {
    audio: Arc<Audio>,
    events: EventSink,
    support: PathBuf,
    /// The running export or bounce.
    exporting: Mutex<Option<CancelToken>>,
    /// The running selection render.
    selection: Mutex<Option<CancelToken>>,
    jobs: AtomicU64,
    /// SHA-256 of media files by path, with the size and modification time they were hashed at.
    hashes: Mutex<std::collections::HashMap<String, (u64, Option<std::time::SystemTime>, String)>>,
}

/// Holds one of the service's slots for the length of a render.
struct Running<'a> {
    slot: &'a Mutex<Option<CancelToken>>,
    token: CancelToken,
    audio: Option<&'a Audio>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        *self.slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        if let Some(audio) = self.audio {
            audio.engine().live().set_exporting(false);
        }
    }
}

fn text(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_string)
}

fn whole(payload: &Value, key: &str) -> Option<i64> {
    payload.get(key).and_then(Value::as_f64).filter(|value| value.is_finite()).map(|value| value as i64)
}

impl Export {
    fn begin_export(&self) -> Result<Running<'_>> {
        let mut slot = self.exporting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            fail!("Finish the current export first, or press Stop to cancel it.");
        }
        let token = CancelToken::new();
        *slot = Some(token.clone());
        self.audio.engine().live().set_exporting(true);
        Ok(Running { slot: &self.exporting, token, audio: Some(&self.audio) })
    }

    /// Stop was pressed: a running export is cancelled.
    fn cancel_export(&self) {
        if let Some(token) = self.exporting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
            token.cancel();
        }
    }

    fn request(payload: &Value) -> Result<ExportRequest> {
        Ok(ExportRequest {
            path: text(payload, "path").unwrap_or_default(),
            mode: text(payload, "mode").unwrap_or_else(|| "song".to_string()),
            owners: match payload.get("owners") {
                None | Some(Value::Null) => None,
                Some(Value::Array(owners)) => Some(owners.iter().filter_map(Value::as_str).map(str::to_string).collect()),
                Some(_) => fail!("Select one or more channels for stem export."),
            },
            track: whole(payload, "track"),
            pattern: whole(payload, "pattern"),
            tail: payload.get("tail").and_then(Value::as_f64),
            format: serde_json::from_value(payload.clone())?,
        })
    }

    fn run_export(&self, request: &ExportRequest) -> Result<Value> {
        let running = self.begin_export()?;
        let path = request.path.clone();
        let progress = |fraction: f64, phase: &str| self.events.emit("audio.exportProgress", json!({ "progress": fraction, "path": path, "phase": phase }));
        let context = RenderContext { live: self.audio.engine(), samples: self.audio.samples(), cancel: &running.token, progress: &progress };
        let result = st_audio::export::export(&context, request).map_err(Error::new)?;
        drop(running);
        self.events.emit("audio.exported", &result);
        Ok(result)
    }

    fn export(&self, payload: Value) -> Result<Value> {
        self.run_export(&Self::request(&payload)?)
    }

    fn bounce_track(&self, payload: Value) -> Result<Value> {
        let Some(index) = whole(&payload, "track").filter(|track| *track >= 0) else { fail!("Choose a track to bounce.") };
        let (track, bpm) = self.audio.engine().with_project(|project| {
            let project = project?;
            let track = project.track_list().get(index as usize)?;
            Some(((project.to_value()["tracks"][index as usize].clone(), track.display_name(index as usize)), project.bpm()))
        }).ok_or_else(|| Error::new("Choose a track to bounce."))?;
        let (track, name) = track;
        let mut request = Self::request(&payload)?;
        request.mode = "track".to_string();
        request.track = Some(index);
        if request.path.is_empty() {
            let folder = self.support.join("Renders");
            std::fs::create_dir_all(&folder).map_err(|error| Error::context("The Renders folder could not be created", error))?;
            request.path = folder.join(format!("Track-{}-{}.wav", index + 1, st_samples::paths::unique_suffix())).to_string_lossy().into_owned();
        }
        let result = self.run_export(&request)?;
        let duration = result["frames"].as_f64().unwrap_or(0.0) / result["sampleRate"].as_f64().unwrap_or(48_000.0);
        let clip = json!({
            "id": format!("clip-{}{}", st_samples::paths::unique_suffix(), st_samples::paths::unique_suffix()), "name": format!("{name} bounce"), "path": request.path,
            "track": index, "startBeat": 0, "durationBeats": duration * bpm / 60.0, "sourceOffset": 0, "gain": 1, "fadeIn": 0, "fadeOut": 0, "muted": false,
        });
        Ok(json!({ "path": request.path, "clip": clip, "sourceTrack": track, "sourceTrackIndex": index, "duration": duration }))
    }

    fn render_selection(&self, payload: Value) -> Result<Value> {
        let request = render_selection::Request::from_payload(&payload).map_err(Error::new)?;
        let running = {
            let mut slot = self.selection.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_some() {
                fail!("Another selection is being rendered. Wait for it to finish or cancel it.");
            }
            let token = CancelToken::new();
            *slot = Some(token.clone());
            Running { slot: &self.selection, token, audio: None }
        };
        let destination = match text(&payload, "path") {
            Some(path) => PathBuf::from(path),
            None => st_samples::paths::recording_destination(None, Some(&request.name), Some(&self.support.to_string_lossy()), Path::new("/"))?,
        };
        let job = self.jobs.fetch_add(1, Ordering::Relaxed) + 1;
        let progress = |fraction: f64, phase: &str| self.events.emit("render.selectionProgress", json!({ "job": job, "phase": phase, "progress": fraction.clamp(0.0, 1.0) }));
        let context = RenderContext { live: self.audio.engine(), samples: self.audio.samples(), cancel: &running.token, progress: &progress };
        render_selection::render_selection(&context, &request, &destination).map_err(Error::new)
    }

    /// The SHA-256 of a media file, hashed once per version of the file.
    fn content_hash(&self, path: &str) -> Option<String> {
        let metadata = std::fs::metadata(path).ok()?;
        let identity = (metadata.len(), metadata.modified().ok());
        if let Some((size, modified, hash)) = self.hashes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(path) {
            if (*size, *modified) == identity {
                return Some(hash.clone());
            }
        }
        let hash = st_project::media::hash_file(Path::new(path), u64::MAX, None).ok()?;
        self.hashes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(path.to_string(), (identity.0, identity.1, hash.clone()));
        Some(hash)
    }

    fn freeze_track(&self, payload: Value) -> Result<Value> {
        let request = st_audio::freeze::FreezeRequest::from_payload(&payload).map_err(Error::new)?;
        // One render at a time, and Stop cancels it: for a freeze by recording that is the
        // transport's own Stop, which ends the take.
        let running = self.begin_export()?;
        let (track, recording) = (request.track, request.method == st_audio::freeze::FreezeMethod::Record);
        let progress = |fraction: f64, phase: &str| self.events.emit("freeze.progress", json!({ "track": track, "progress": fraction.clamp(0.0, 1.0), "phase": phase, "recording": recording }));
        let context = RenderContext { live: self.audio.engine(), samples: self.audio.samples(), cancel: &running.token, progress: &progress };
        st_audio::freeze::freeze_track(&context, &request, &self.support.join("Freezes"), &|path| self.content_hash(path)).map_err(Error::new)
    }

    fn unfreeze_track(&self, payload: Value) -> Result<Value> {
        let Some(index) = whole(&payload, "track").filter(|track| *track >= 0) else { fail!("Choose a track to unfreeze.") };
        let frozen = self.audio.engine().with_project(|project| project.and_then(|project| project.track_list().get(index as usize).map(|track| track.frozen.is_some())));
        match frozen {
            Some(true) => Ok(json!({ "track": index, "audioPlugins": self.audio.engine().snapshot()["audioPlugins"] })),
            Some(false) => fail!("That track is not frozen."),
            None => fail!("Choose an existing track."),
        }
    }

    fn freeze_status(&self, _: NoPayload) -> Result<Value> {
        let busy = self.exporting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_some();
        if self.audio.engine().with_project(|project| project.is_none_or(|project| !project.track_list().iter().any(|track| track.frozen.is_some()))) {
            return Ok(json!({ "tracks": [], "busy": busy }));
        }
        let project = self.audio.engine().snapshot();
        Ok(json!({ "tracks": st_audio::freeze::status(&project, &|path| self.content_hash(path)), "busy": busy }))
    }

    fn cancel_selection(&self, _: NoPayload) -> Result<Value> {
        let running = self.selection.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(token) = &running {
            token.cancel();
        }
        Ok(json!({ "cancelled": running.is_some() }))
    }
}

/// Registers the export commands.
pub fn install(commands: &mut Commands, audio: &Arc<Audio>, support: &Path, events: EventSink) -> Arc<Export> {
    let service = Arc::new(Export { audio: audio.clone(), events, support: support.to_path_buf(), exporting: Mutex::default(), selection: Mutex::default(), jobs: AtomicU64::new(0), hashes: Mutex::default() });
    let stop = Arc::downgrade(&service);
    audio.on_stop(move || {
        if let Some(service) = stop.upgrade() {
            service.cancel_export();
        }
    });
    commands
        .with(&service)
        .register_blocking("audio.export", Export::export)
        .register_blocking("audio.bounceTrack", Export::bounce_track)
        .register_blocking("render.selection", Export::render_selection)
        .register("render.cancelSelection", Export::cancel_selection)
        .register_blocking("freeze.track", Export::freeze_track)
        .register_blocking("freeze.unfreeze", Export::unfreeze_track)
        .register_blocking("freeze.status", Export::freeze_status);
    service
}

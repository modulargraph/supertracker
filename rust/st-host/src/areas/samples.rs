//! The sample area: loading, editing and analysing instrument samples, the events of the
//! engine's background decodes, and the sampler's live playheads.
//!
//! | Command | Notes |
//! |---|---|
//! | `sample.load` `{path, instrument}` | decodes on the command's own worker thread and answers with the sample's summary (`st_samples::SampleInfo`, with 512 peak columns); the sample is then in the cache, so the publication that names it installs it at once. A newer load for the same instrument supersedes one still decoding |
//! | `sample.info` `{instrument, path}` | the summary of a sample the cache holds, `{available: false}` otherwise; never decodes |
//! | `sample.peaks` `{instrument, path?, start?, end?, bins?, snap?, direction?, window?}` | zoomed waveform columns and zero-crossing snapping (`DecodedSample::inspect`) |
//! | `sample.edit` `{instrument, path?, operation, start, end, …}` | `st_samples::edit_to_file`: a new file in `Edits` of the support folder (or `outputDirectory`); the source file is never modified |
//! | `sample.analyzeSlices` `{instrument, path?, sensitivity?, …}` | `st_samples::analyze_slices` |
//! | `sample.wavetables`, `builtin.drumModels` | the built-in tables and drum models |
//! | `audio.clipInfo` `{path}` | the summary of a timeline clip's file |
//! | `sampler.playheads` `{enabled, instrument?}` | while enabled, a 30 Hz poll of the engine's voice positions sends `sampler.playheads` events (`emit_latest`), only when the snapshot changed |
//!
//! Events: `sample.loaded` `{instrument, sample}` when a sample the published project waited
//! for is decoded (also when its file changed on disk and was decoded again),
//! `sample.zonesLoaded` `{instrument, count}`, `audio.clipLoaded` (the clip file's summary);
//! failures arrive as `sample.error` / `audio.clipError` through [`Audio::samples_changed`].
//!
//! Threads: the commands that touch files are blocking commands (a worker thread each);
//! project samples are decoded by the workers of [`st_audio::DecodedSamples`], whose listener
//! (on such a worker) applies the result to the engine and sends the events. Nothing here
//! decodes on the bridge thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_audio::{DecodedSamples, LoadEvent};
use st_project::model::InstrumentKind;
use st_project::Project;
use st_samples::{CancelToken, Operation};

use crate::areas::audio::Audio;
use crate::commands::{Commands, NoPayload};
use crate::{fail, Error, EventSink, Result};

/// How long a sample nothing names any more stays decoded (undo within this time is instant).
const GRACE: Duration = Duration::from_secs(120);

/// Drops decoded samples that `project` no longer names and nothing asked for lately.
pub(crate) fn retain_for(samples: &DecodedSamples, project: &Project) {
    if samples.len() <= 8 {
        return;
    }
    let mut named: HashSet<&str> = HashSet::new();
    for instrument in project.instrument_list() {
        named.extend(instrument.sample_path.as_deref());
        named.extend(instrument.zones.as_deref().unwrap_or(&[]).iter().filter_map(|zone| zone.file()));
    }
    named.extend(project.clip_list().iter().filter_map(|clip| clip.file()));
    samples.retain(&|path| named.contains(path), GRACE);
}

/// The sample service of the host.
pub struct Samples {
    audio: Arc<Audio>,
    samples: Arc<DecodedSamples>,
    events: EventSink,
    support: PathBuf,
    /// The newest load or edit per instrument: starting another cancels it.
    newest: Mutex<HashMap<i64, CancelToken>>,
    /// The stop flag of the running playhead poll.
    playheads: Mutex<Option<Arc<AtomicBool>>>,
    last_apply: Mutex<Instant>,
}

fn instrument_of(payload: &Value, fallback: i64) -> Result<i64> {
    let index = payload.get("instrument").and_then(Value::as_f64).filter(|value| value.is_finite()).map_or(fallback, |value| value as i64);
    if index != -1 && !(0..4096).contains(&index) {
        fail!("Choose an instrument.");
    }
    Ok(index)
}

fn text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

impl Samples {
    /// The path of the sample instrument `index` plays in the published project.
    fn instrument_path(&self, index: i64) -> Option<String> {
        self.audio.engine().with_project(|project| {
            let instrument = project?.instrument_list().get(usize::try_from(index).ok()?)?;
            instrument.sample_path.clone().filter(|path| !path.is_empty() && instrument.kind() == InstrumentKind::Sample)
        })
    }

    /// The token of a new load or edit for `instrument`; the previous one is cancelled.
    fn supersede(&self, instrument: i64) -> CancelToken {
        let token = CancelToken::new();
        if let Some(previous) = self.newest.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(instrument, token.clone()) {
            previous.cancel();
        }
        token
    }

    fn superseded(token: &CancelToken, what: &str) -> Result<()> {
        if token.is_cancelled() {
            fail!("{what} was superseded by a newer project or sample change.");
        }
        Ok(())
    }

    fn load(&self, payload: Value) -> Result<Value> {
        let Some(path) = text(&payload, "path") else { fail!("No sample path was provided.") };
        let token = self.supersede(instrument_of(&payload, 0)?);
        let loaded = self.samples.load(path, &token);
        Self::superseded(&token, "Sample load")?;
        Ok(serde_json::to_value(&*loaded.map_err(Error::new)?.info)?)
    }

    fn info(&self, payload: Value) -> Result<Value> {
        let index = instrument_of(&payload, -1)?;
        let known = text(&payload, "path").filter(|path| self.instrument_path(index).as_deref() == Some(*path)).and_then(|path| self.samples.info(path));
        Ok(match known {
            Some(info) => serde_json::to_value(&*info)?,
            None => json!({ "available": false }),
        })
    }

    /// The sample a request works on: the given path, else the instrument's.
    fn source(&self, payload: &Value, missing: &str) -> Result<String> {
        match text(payload, "path") {
            Some(path) => Ok(path.to_string()),
            None => self.instrument_path(instrument_of(payload, 0)?).ok_or_else(|| Error::new(missing)),
        }
    }

    fn peaks(&self, payload: Value) -> Result<Value> {
        let path = self.source(&payload, "Load a sample before editing it.")?;
        // The editor asks while it draws: answer from decoded audio only.
        if self.samples.info(&path).is_none() {
            fail!("The sample is still loading. Try again in a moment.");
        }
        let request: st_samples::InspectRequest = serde_json::from_value(payload)?;
        let sample = self.samples.decoded(&path, &CancelToken::new()).map_err(Error::new)?;
        Ok(serde_json::to_value(sample.inspect(&request))?)
    }

    fn edit(&self, payload: Value) -> Result<Value> {
        let path = self.source(&payload, "Load a sample before editing it.")?;
        let token = self.supersede(instrument_of(&payload, 0)?);
        let request: st_samples::EditRequest = serde_json::from_value(payload)?;
        let directory = request.output_directory.as_deref().filter(|directory| !directory.is_empty()).map_or_else(|| self.support.join("Edits"), PathBuf::from);
        let source = self.samples.decoded(&path, &token);
        Self::superseded(&token, "Sample edit")?;
        let edited = st_samples::edit_to_file(&*source.map_err(Error::new)?, &request, &directory, &mut Operation::new().with_cancel(&token));
        Self::superseded(&token, "Sample edit")?;
        Ok(serde_json::to_value(&*self.samples.insert(edited?))?)
    }

    fn analyze_slices(&self, payload: Value) -> Result<Value> {
        let path = self.source(&payload, "Load a sample before detecting transients.")?;
        let request: st_samples::SliceRequest = serde_json::from_value(payload)?;
        let sample = self.samples.decoded(&path, &CancelToken::new()).map_err(Error::new)?;
        Ok(serde_json::to_value(st_samples::analyze_slices(&sample, &request, &mut Operation::new())?)?)
    }

    fn clip_info(&self, payload: Value) -> Result<Value> {
        let Some(path) = text(&payload, "path") else { fail!("Choose an audio clip file.") };
        Ok(serde_json::to_value(&*self.samples.load(path, &CancelToken::new()).map_err(Error::new)?.info)?)
    }

    fn wavetables(&self, _: NoPayload) -> Result<Value> {
        Ok(serde_json::to_value(st_samples::builtin::wavetable_list())?)
    }

    fn drum_models(&self, _: NoPayload) -> Result<Value> {
        Ok(st_audio::samples_async::drum_models_metadata())
    }

    // ---- decodes the project waited for ----------------------------------------------------

    /// On a decode worker: one sample of the published project is ready, or failed.
    fn decoded(&self, event: &LoadEvent) {
        // A kit publishes many samples at once: apply them together, with a look in between.
        let apply = {
            let mut last = self.last_apply.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let due = event.remaining == 0 || last.elapsed() >= Duration::from_millis(250);
            if due {
                *last = Instant::now();
            }
            due
        };
        if apply {
            if let Err(error) = self.audio.samples_changed() {
                self.events.emit("error", json!({ "message": error.to_string() }));
            }
        }
        let Ok(info) = &event.result else { return };
        // Who waited for it; the events go out after the engine's lock is released.
        let (mut instruments, mut zoned, mut clip) = (Vec::new(), Vec::new(), false);
        self.audio.engine().with_project(|project| {
            let Some(project) = project else { return };
            for (index, instrument) in project.instrument_list().iter().enumerate() {
                if instrument.kind() == InstrumentKind::Sample && instrument.sample_path.as_deref() == Some(event.path.as_str()) {
                    instruments.push(index);
                }
                let zones: Vec<&str> = instrument.zones.as_deref().unwrap_or(&[]).iter().filter_map(|zone| zone.file()).collect();
                if zones.contains(&event.path.as_str()) && zones.iter().all(|path| self.samples.info(path).is_some()) {
                    zoned.push((index, zones.len()));
                }
            }
            clip = project.clip_list().iter().any(|clip| clip.file() == Some(event.path.as_str()));
        });
        for index in instruments {
            self.events.emit("sample.loaded", json!({ "instrument": index, "sample": &**info }));
        }
        for (index, count) in zoned {
            self.events.emit("sample.zonesLoaded", json!({ "instrument": index, "count": count }));
        }
        if clip {
            self.events.emit("audio.clipLoaded", &**info);
        }
    }

    // ---- playheads -------------------------------------------------------------------------

    fn playheads(self: &Arc<Self>, payload: Value) -> Result<Value> {
        let enabled = payload.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        let mut playheads = self.playheads.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(running) = playheads.take() {
            running.store(true, Ordering::Release);
        }
        if !enabled {
            self.audio.engine().watch_voice_positions(-2);
            return Ok(json!({ "ok": true, "watching": -2 }));
        }
        let watching = payload.get("instrument").and_then(Value::as_f64).filter(|value| value.is_finite()).map_or(-1, |value| value.clamp(-1.0, 4096.0) as i32);
        let stop = Arc::new(AtomicBool::new(false));
        *playheads = Some(stop.clone());
        self.audio.engine().watch_voice_positions(watching);
        let service = Arc::downgrade(self);
        std::thread::Builder::new().name("sampler-playheads".to_string()).spawn(move || poll_playheads(service, watching, stop)).map_err(|error| Error::new(format!("The playhead poll could not start: {error}")))?;
        Ok(json!({ "ok": true, "watching": watching }))
    }
}

/// Thirty times a second: the engine's voice snapshot as a `sampler.playheads` event, when it
/// changed. An empty snapshot is sent once, so the last playheads disappear.
fn poll_playheads(service: Weak<Samples>, watching: i32, stop: Arc<AtomicBool>) {
    let (mut last_serial, mut shown) = (0u64, false);
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(33));
        let Some(service) = service.upgrade() else { return };
        if stop.load(Ordering::Acquire) {
            return;
        }
        let engine = service.audio.engine();
        // The engine may have been replaced (another sample rate): the watch is repeated.
        engine.watch_voice_positions(watching);
        let Some((voices, serial)) = engine.playheads() else { continue };
        if (voices.is_empty() && !shown) || (serial == last_serial && !voices.is_empty()) {
            continue;
        }
        last_serial = serial;
        shown = !voices.is_empty();
        service.events.emit_latest("sampler.playheads", json!({ "instrument": watching, "voices": voices }));
    }
}

impl Drop for Samples {
    fn drop(&mut self) {
        if let Some(running) = self.playheads.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            running.store(true, Ordering::Release);
        }
    }
}

/// Registers the sample commands and connects the engine's sample cache to the interface.
pub fn install(commands: &mut Commands, audio: &Arc<Audio>, support: &std::path::Path, events: EventSink) -> Arc<Samples> {
    let service = Arc::new(Samples {
        audio: audio.clone(),
        samples: audio.samples().clone(),
        events,
        support: support.to_path_buf(),
        newest: Mutex::default(),
        playheads: Mutex::default(),
        last_apply: Mutex::new(Instant::now()),
    });
    let listener = Arc::downgrade(&service);
    service.samples.set_listener(move |event| {
        if let Some(service) = listener.upgrade() {
            service.decoded(event);
        }
    });
    let playheads = service.clone();
    commands
        .with(&service)
        .register_blocking("sample.load", Samples::load)
        .register("sample.info", Samples::info)
        .register_blocking("sample.peaks", Samples::peaks)
        .register_blocking("sample.edit", Samples::edit)
        .register_blocking("sample.analyzeSlices", Samples::analyze_slices)
        .register_blocking("audio.clipInfo", Samples::clip_info)
        .register("sample.wavetables", Samples::wavetables)
        .register("builtin.drumModels", Samples::drum_models);
    commands.register("sampler.playheads", move |payload: Value| playheads.playheads(payload));
    service
}

//! Importing Maschine groups (`.mxgrp`) as kits.
//!
//! [`import_group`] reads a group's archive ([`archive`]) and converts what has a proven
//! layout into editable kit objects:
//!
//! * **Sampler** Sounds become sampler instruments with their zones (exact key, velocity and
//!   frame ranges; samples resolved inside the expansion's library folder) and the Sampler
//!   controls that have a verified conversion ([`mapping::map_sampler`]).
//! * **Audio** modules become a plain sample of their one file.
//! * **Drum synths** become a built-in rendered drum sample (`builtin:drums/<model>/<controls>`).
//! * **Effects** become built-in effects with mapped controls ([`mapping::map_effect`]),
//!   numbered contiguously per channel; a channel holds sixteen inserts.
//! * **Hosted plug-ins** are VST2 plug-ins, which this host does not load: their identity and
//!   state are kept with the descriptor (marked missing) and the pad is reported.
//! * Mixer level, pan, mute and choke group of every Sound.
//!
//! Everything that is approximate, limited or not converted is said in `warnings`, and the
//! import report lists every module with its original parameter values. Pattern notes,
//! modulation, MIDI routing and auxiliary sends are not converted. Nothing here runs vendor
//! code or opens a file outside the group and its expansion's library folder.

pub mod archive;
pub mod catalogue;
mod curves;
pub mod mapping;
pub mod plugin_state;

use crate::api::KitReadResponse;
use crate::error::{Error, Result};
use crate::kit;
use crate::util::{base64_encode, extension, unique_id};
use archive::{invalid, Archive, Module, Zone};
use plugin_state::PluginState;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Insert slots per channel.
pub const INSERT_SLOT_LIMIT: usize = 16;

const INTERNAL_EFFECT_TYPES: [&str; 25] = [
    "BeatDelay", "CabinetEmulation", "Chorus", "Compressor", "EQ", "FM", "Filter", "Flanger", "FreqShifter", "Gate", "GrainDelay", "GrainStretch", "Iceverb", "Limiter", "LoFi", "Maximizer", "Metaverb",
    "MullhollandDrive", "Phaser", "PlateReverb", "Reflex", "Resochord", "Reverb", "Saturator", "TransientDesigner",
];
const DRUM_TYPES: [&str; 6] = ["Kick", "Snare", "Hihat", "Tom", "Perc", "Cymbal"];

/// One pad of an imported group.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedSound {
    /// Pad position, 0..16.
    pub pad: usize,
    /// Display name; an unnamed Sound is shown by its pad number, as Maschine does.
    pub name: String,
    /// The instrument: a sampler (`type: "sample"`), a hosted plug-in (`type: "au"`), or an
    /// empty sampler. `unavailable: true` marks a source that could not be converted.
    pub instrument: Map<String, Value>,
    /// Mixer volume, 0..=2.
    pub volume: f64,
    /// Pan, -1..=1.
    pub pan: f64,
    /// Muted.
    pub mute: bool,
    /// Choke group, 0 = none.
    pub choke_group: u32,
    /// The Sound's original mixer members.
    pub maschine_channel: Value,
    /// The plug-in descriptors of this pad (instrument plug-in, inserts), without an owner.
    pub plugins: Vec<Value>,
}

impl ImportedSound {
    /// Whether the pad plays or processes something: it has a sample, zones, a plug-in or inserts.
    pub fn has_source(&self) -> bool {
        let text = |key: &str| self.instrument.get(key).and_then(Value::as_str).unwrap_or("");
        text("type") == "au" || !text("samplePath").is_empty() || self.instrument.get("zones").and_then(Value::as_array).is_some_and(|zones| !zones.is_empty()) || !self.plugins.is_empty()
    }
}

/// A decoded and converted group.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedGroup {
    /// The group file.
    pub path: String,
    /// The group's name.
    pub name: String,
    /// The sixteen pads in order.
    pub sounds: Vec<ImportedSound>,
    /// The Group's own inserts, without an owner.
    pub group_plugins: Vec<Value>,
    /// What is approximate, limited, missing or not converted.
    pub warnings: Vec<String>,
    /// Versions, the source path and one entry per module with its original parameters and how
    /// it was converted (`status`).
    pub report: Value,
}

/// Reads the group at `path`. `root` is the library folder it was indexed under, used to find
/// the expansion's samples when the folder layout does not show it.
pub fn import_group(path: &str, root: Option<&str>) -> Result<ImportedGroup> {
    let file = Path::new(path);
    let size = fs::metadata(file).map_err(|error| Error::io("read", file, &error))?.len();
    if size == 0 || size > 32 * 1024 * 1024 {
        return Err(invalid("file exceeds the supported 32 MB limit."));
    }
    let source = fs::read(file).map_err(|error| Error::io("read", file, &error))?;
    let archive = Archive::parse(&source)?;
    let library = expansion_root(file, root.map(Path::new));
    let mut importer = Importer { archive: &archive, warnings: Vec::new(), reports: Vec::new(), frames: HashMap::new(), library };
    if archive.modules.is_empty() {
        let name = if archive.group_name.is_empty() { "This Group" } else { &archive.group_name };
        importer.warn(format!("{name} has no audio modules: its Sounds only address external MIDI or audio inputs, so they import as empty pads."));
    }
    let sounds: Vec<ImportedSound> = archive.sounds.iter().map(|sound| importer.sound(sound)).collect();
    let group_modules: Vec<&Module> = archive.group_modules();
    let group_plugins = importer.effects(&group_modules, "bus", &archive.group_name);
    importer.warn("Pattern notes, modulation, MIDI routing and auxiliary sends are retained in the original archive but are not converted yet.");
    importer.warn("Group mixer level and unconverted sampler control modes remain in the original archive; sample zones retain their exact key, velocity and frame ranges.");
    let report = json!({
        "format": "MaschineGroupImport", "version": 1, "archiveVersion": archive.archive_version, "groupVersion": archive.group_version, "soundVersion": archive.sound_version,
        "sourcePath": path, "modules": importer.reports, "warnings": importer.warnings,
    });
    Ok(ImportedGroup { path: path.to_string(), name: archive.group_name.clone(), sounds, group_plugins, warnings: importer.warnings, report })
}

struct Importer<'a> {
    archive: &'a Archive,
    warnings: Vec<String>,
    reports: Vec<Value>,
    frames: HashMap<PathBuf, u64>,
    library: PathBuf,
}

/// Display name of a Sound; Maschine shows an unnamed Sound by its pad number.
pub fn display_name(sound: &archive::Sound) -> String {
    if sound.name.trim_matches(' ').is_empty() {
        format!("Sound {}", sound.index + 1)
    } else {
        sound.name.clone()
    }
}

impl Importer<'_> {
    fn warn(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.warnings.contains(&text) {
            self.warnings.push(text);
        }
    }

    fn report(&self, module: &Module, owner: &str, slot: usize) -> Value {
        let mut report = json!({ "owner": owner, "slot": slot, "type": module.kind, "version": module.version, "objectID": module.object_id, "enabled": module.enabled });
        if let Ok(parameters) = self.archive.parameters(module) {
            report["parameters"] = json!(parameters);
        }
        report
    }

    /// A descriptor for a hosted plug-in. The host does not load VST2: the descriptor keeps the
    /// identity and the state bytes and is marked missing.
    fn hosted(&mut self, state: &PluginState, instrument: bool, enabled: bool, label: &str) -> Result<Value> {
        let component = state.component().filter(|_| state.chunk_version == 1 && !state.payload.is_empty()).ok_or_else(|| invalid("only an identified VST2 plug-in chunk can be kept with the kit."))?;
        self.warn(format!("{label}: {} is a VST2 plug-in, which SuperTracker does not load. Its state is kept with the kit; load the plug-in's current version on this pad.", state.plugin_name));
        let mut summary = state.summary();
        summary["warnings"] = json!(["Vendor content paths are unchanged; missing ensembles or samples may require repair inside the plug-in."]);
        Ok(json!({
            "instanceID": unique_id(), "kind": if instrument { "instrument" } else { "effect" }, "slot": 0, "component": component, "id": component, "format": "VST2",
            "name": state.plugin_name, "state": base64_encode(&state.payload), "bypass": !enabled, "missing": true, "maschinePluginState": summary,
        }))
    }

    /// Converts an insert chain. A group can hold more inserts than a channel has slots:
    /// loadable inserts are numbered contiguously; when a chain is longer, bypassed inserts are
    /// left out first, then the trailing ones. Every omitted module keeps its original state in
    /// the report.
    fn effects(&mut self, modules: &[&Module], owner: &str, label: &str) -> Vec<Value> {
        struct Candidate {
            report: usize,
            descriptor: Value,
            enabled: bool,
            kind: String,
            original: usize,
        }
        let mut candidates: Vec<Candidate> = Vec::new();
        for (slot, module) in modules.iter().enumerate() {
            let mut report = self.report(module, owner, slot);
            report["kind"] = json!("effect");
            if module.kind == "PluginHost" {
                let converted = PluginState::read(self.archive, module).and_then(|state| {
                    report["pluginState"] = state.summary();
                    if state.is_instrument != Some(false) {
                        return Err(invalid("an insert slot contains an instrument plug-in."));
                    }
                    self.hosted(&state, false, module.enabled, label)
                });
                match converted {
                    Ok(descriptor) => {
                        report["status"] = json!("external-state-retained");
                        candidates.push(Candidate { report: self.reports.len(), descriptor, enabled: module.enabled, kind: module.kind.clone(), original: slot });
                    }
                    Err(error) => {
                        report["status"] = json!("unavailable");
                        self.warn(format!("{label}: {error}"));
                    }
                }
                self.reports.push(report);
                continue;
            }
            let raw: Vec<f64> = report.get("parameters").and_then(Value::as_array).map(|values| values.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let Some(mapped) = mapping::map_effect(&module.kind, module.version, &raw) else {
                report["status"] = json!("unavailable");
                self.reports.push(report);
                self.warn(format!("{} on {label} is unavailable; its original state is retained in the import report.", module.kind));
                continue;
            };
            for warning in &mapped.warnings {
                self.warn(warning.clone());
            }
            let effect_id = mapped.effect_id().to_string();
            let parameters = mapped.by_id();
            let name = catalogue::effect(&effect_id).map(|effect| effect.name).unwrap_or(&effect_id);
            // The state of a built-in effect: its id and normalized values by parameter id.
            let state = json!({ "format": "SuperTracker.BuiltinFX", "version": 1, "id": effect_id, "parameters": parameters });
            report["status"] = json!("approximation-mapped-controls");
            let descriptor = json!({
                "instanceID": unique_id(), "kind": "effect", "slot": 0, "component": mapped.id, "id": mapped.id, "name": name, "format": "Builtin",
                "state": base64_encode(state.to_string().as_bytes()), "bypass": !module.enabled,
            });
            candidates.push(Candidate { report: self.reports.len(), descriptor, enabled: module.enabled, kind: module.kind.clone(), original: slot });
            self.reports.push(report);
        }
        let mut omitted: Vec<usize> = Vec::new();
        if candidates.len() > INSERT_SLOT_LIMIT {
            let excess = candidates.len() - INSERT_SLOT_LIMIT;
            omitted.extend((0..candidates.len()).rev().filter(|index| !candidates[*index].enabled).take(excess));
            for index in (0..candidates.len()).rev() {
                if omitted.len() >= excess {
                    break;
                }
                if !omitted.contains(&index) {
                    omitted.push(index);
                }
            }
        }
        let mut plugins = Vec::new();
        for (index, mut candidate) in candidates.into_iter().enumerate() {
            if omitted.contains(&index) {
                self.reports[candidate.report]["status"] = json!("omitted-insert-limit");
                self.warn(format!(
                    "{label}: {} (original insert {}) was not loaded because a SuperTracker channel holds {INSERT_SLOT_LIMIT} inserts; its original state is retained in the import report.",
                    candidate.kind,
                    candidate.original + 1
                ));
                continue;
            }
            candidate.descriptor["slot"] = json!(plugins.len());
            self.reports[candidate.report]["insertSlot"] = json!(plugins.len());
            plugins.push(candidate.descriptor);
        }
        plugins
    }

    fn sound(&mut self, sound: &archive::Sound) -> ImportedSound {
        let archive = self.archive;
        let owner = format!("pad:{}", sound.index + 1);
        let modules = archive.modules_of(sound);
        let name = display_name(sound);
        let mut instrument: Map<String, Value> =
            json!({ "type": "sample", "name": name, "volume": 1.0, "mode": "oneshot", "rootNote": 60, "attack": 0.0, "decay": 0.0, "sustain": 1.0, "release": 0.01 }).as_object().cloned().unwrap_or_default();
        let mut plugins: Vec<Value> = Vec::new();
        // A Sound may hold only inserts for an external or internal audio input: the position
        // of a module alone does not make it a generator.
        let first_is_effect =
            modules.first().is_some_and(|module| INTERNAL_EFFECT_TYPES.contains(&module.kind.as_str()) || (module.kind == "PluginHost" && PluginState::read(archive, module).is_ok_and(|state| state.is_instrument == Some(false))));
        if first_is_effect {
            plugins = self.effects(&modules, &owner, &name);
            self.warn(format!("{name}: its effects-only Sound retains the insert chain, but original audio input routing is not converted."));
        } else if let Some(source) = modules.first() {
            let mut report = self.report(source, &owner, 0);
            report["kind"] = json!("instrument");
            let converted = self.source(source, &name, &mut instrument, &mut report, &mut plugins);
            if let Err(error) = converted {
                instrument.insert("unavailable".to_string(), json!(true));
                report["status"] = json!("unavailable");
                self.warn(format!("{name}: {error}"));
            }
            if !source.enabled {
                instrument.insert("volume".to_string(), json!(0.0));
            }
            self.reports.push(report);
            plugins.extend(self.effects(&modules[1..], &owner, &name));
            // Inserts follow the instrument plug-in's own slot numbering.
        }
        let (mut volume, mut pan, mut mute, mut choke_group, mut maschine_channel) = (1.0, 0.0, false, 0u32, json!({}));
        match archive.channel(sound) {
            Ok(channel) => {
                let gain = mapping::level_amplitude(channel["level"]).unwrap_or(1.0);
                volume = gain.min(2.0);
                pan = channel["pan"];
                mute = channel["mute"] != 0.0 || channel["muteAudio"] != 0.0;
                choke_group = channel["chokeGroup"] as u32;
                if gain > 2.0 {
                    self.warn(format!("Sound level exceeded the mixer maximum and was limited: {name}"));
                }
                if channel.get("chokeMode").copied().unwrap_or(0.0) != 0.0 {
                    self.warn(format!("Nonstandard choke behavior on {name} uses SuperTracker's exclusive choke mode."));
                }
                maschine_channel = json!(channel);
            }
            Err(error) => self.warn(format!("{name}: mixer controls were not decoded: {error}")),
        }
        ImportedSound { pad: sound.index, name, instrument, volume, pan, mute, choke_group, maschine_channel, plugins }
    }

    /// Converts the generator module of a Sound into `instrument`.
    fn source(&mut self, module: &Module, name: &str, instrument: &mut Map<String, Value>, report: &mut Value, plugins: &mut Vec<Value>) -> Result<()> {
        let archive = self.archive;
        match module.kind.as_str() {
            "Sampler" => {
                let zones = archive.zones(module)?;
                match archive.sampler_parameters(module, &zones) {
                    Ok(controls) => {
                        report["samplerParameters"] = json!(controls);
                        let mapped = mapping::map_sampler(&controls);
                        // The voice mode is a property of the pad's track.
                        instrument.extend(mapped.instrument);
                        for warning in mapped.warnings {
                            self.warn(warning);
                        }
                        report["samplerPhysical"] = json!(mapped.physical);
                    }
                    Err(_) => self.warn(format!("{name}: sampler controls were retained without conversion.")),
                }
                let mapped = self.map_zones(&zones);
                if mapped.is_empty() {
                    return Err(invalid("no sample zone could be imported."));
                }
                instrument.insert("samplePath".to_string(), mapped[0]["path"].clone());
                instrument.insert("zones".to_string(), Value::Array(mapped));
                report["status"] = json!("sample-zones-decoded");
                report["zones"] = zones.iter().map(|zone| json!({ "objectID": zone.object_id, "path": zone.path, "origin": zone.origin, "library": zone.library, "values": zone.values })).collect();
            }
            "AudioModule" => {
                let reference = archive.audio_module_sample(module)?;
                report["audioSample"] = json!({ "objectID": reference.object_id, "path": reference.path, "origin": reference.origin, "library": reference.library });
                let mapped = self.map_zones(std::slice::from_ref(&reference));
                if mapped.is_empty() {
                    return Err(invalid("the Audio module's sample could not be imported."));
                }
                instrument.insert("samplePath".to_string(), mapped[0]["path"].clone());
                instrument.insert("zones".to_string(), Value::Array(mapped));
                self.warn(format!("{name}: the Audio module's loop is imported as a plain sample; tempo sync, time-stretching and its play modes are not reproduced."));
                report["status"] = json!("audio-sample-decoded");
            }
            "PluginHost" => {
                let state = PluginState::read(archive, module)?;
                report["pluginState"] = state.summary();
                if state.is_instrument != Some(true) {
                    return Err(invalid("a Sound source contains an effect plug-in."));
                }
                let descriptor = self.hosted(&state, true, module.enabled, name)?;
                instrument.insert("type".to_string(), json!("au"));
                plugins.push(descriptor);
                report["status"] = json!("external-state-retained");
            }
            kind if DRUM_TYPES.contains(&kind) => {
                let raw: Vec<f64> = report.get("parameters").and_then(Value::as_array).map(|values| values.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                match mapping::map_drum(kind, module.version, &raw) {
                    Some(mapped) => {
                        // The drum model becomes a one-shot sample of its rendering: a built-in
                        // sample reference that carries the model and its eight controls.
                        let controls: String = mapped.drum_parameters.iter().map(|value| format!("{:08x}", (value.clamp(0.0, 1.0) as f32).to_bits())).collect();
                        instrument.insert("samplePath".to_string(), json!(format!("builtin:drums/{}/{controls}", mapped.drum_model)));
                        for (key, value) in [("start", 0.0), ("end", 1.0), ("loopStart", 0.0), ("loopEnd", 1.0)] {
                            instrument.insert(key.to_string(), json!(value));
                        }
                        for warning in mapped.warnings {
                            self.warn(warning);
                        }
                        report["status"] = json!("approximation-mapped-controls");
                    }
                    None => {
                        instrument.insert("unavailable".to_string(), json!(true));
                        report["status"] = json!("unavailable");
                        self.warn(format!("{name}: unrecognized DrumSynth control layout."));
                    }
                }
            }
            other => {
                instrument.insert("unavailable".to_string(), json!(true));
                report["status"] = json!("unavailable");
                self.warn(format!("{name}: {other} instrument state is retained but cannot be restored yet."));
            }
        }
        Ok(())
    }

    fn frame_count(&mut self, path: &Path) -> u64 {
        if let Some(frames) = self.frames.get(path) {
            return *frames;
        }
        let frames = crate::probe::audio_info(path).and_then(|info| info.frames).unwrap_or(0);
        self.frames.insert(path.to_path_buf(), frames);
        frames
    }

    /// Sampler zones in SuperTracker form. A zone whose path leaves the expansion library is
    /// skipped (and reported), so one bad zone never makes the whole Sound unavailable; a
    /// missing sample keeps its zone, marked unavailable.
    fn map_zones(&mut self, zones: &[Zone]) -> Vec<Value> {
        let mut mapped = Vec::new();
        for zone in zones {
            let mut path = match resolve(&zone.path, &self.library) {
                Ok(path) => path,
                Err(error) => {
                    self.warn(format!("Sample zone skipped: {}: {error}", zone.path));
                    continue;
                }
            };
            let mut frames = self.frame_count(&path);
            // REX2 loops cannot be decoded here. Expansions ship the same loop rendered as a WAV
            // beside it. That rendering is used only when this Sound's slices tile it exactly
            // (the last slice ends on its final frame); otherwise the timelines differ.
            if frames == 0 && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("rx2")) {
                let wav = path.with_extension("wav");
                let wav_frames = if wav.exists() { self.frame_count(&wav) } else { 0 };
                let last = zones.iter().filter(|other| other.path == zone.path).map(|other| other.values.get("endFrame").copied().unwrap_or(0.0).max(other.values.get("loopEndFrame").copied().unwrap_or(0.0))).fold(f64::NEG_INFINITY, f64::max);
                if wav_frames > 0 && (last + 1.0 - wav_frames as f64).abs() <= 1.0 {
                    path = wav;
                    frames = wav_frames;
                    self.warn(format!("REX2 loop {} plays from its bundled WAV rendering with the original slice ranges.", zone.path));
                } else {
                    self.warn(format!("REX2 loop {} cannot be decoded and its slices do not match a bundled WAV rendering.", zone.path));
                }
            }
            let mut values = zone.values.clone();
            // An Audio module reference covers the whole file (end markers of -1).
            if frames > 0 {
                for key in ["endFrame", "loopEndFrame"] {
                    if values.get(key).copied().unwrap_or(0.0) < 0.0 {
                        values.insert(key, (frames - 1) as f64);
                    }
                }
            }
            let gain = mapping::level_amplitude(values["gain"]);
            let mut entry = json!({
                "path": path.to_string_lossy(), "keyLow": values["keyLow"] as i64, "keyHigh": values["keyHigh"] as i64, "velocityLow": values["velocityLow"] as i64, "velocityHigh": values["velocityHigh"] as i64,
                "rootNote": values["root"] as i64, "pan": values["pan"], "tune": values["tune"], "volume": gain.unwrap_or(1.0).min(2.0), "loopEnabled": values["loopEnabled"] != 0.0, "maschineSource": zone.values,
            });
            if gain.unwrap_or(0.0) > 2.0 {
                self.warn(format!("A sample-zone level exceeded the supported gain and was limited: {}", zone.path));
            }
            if frames > 0 {
                // Native ranges include their final frame; SuperTracker's end points are exclusive.
                for (key, source, increment) in [("start", "startFrame", 0.0), ("end", "endFrame", 1.0), ("loopStart", "loopStartFrame", 0.0), ("loopEnd", "loopEndFrame", 1.0)] {
                    entry[key] = json!(((values[source] + increment) / frames as f64).clamp(0.0, 1.0));
                }
            } else {
                self.warn(format!("Missing or unreadable sample: {}", zone.path));
                entry["unavailable"] = json!(true);
            }
            mapped.push(entry);
        }
        mapped
    }
}

/// Converts zones as [`import_group`] does, for a library folder: the mapped zones and the
/// warnings. See [`ImportedGroup`] for the zone fields.
pub fn map_zones(zones: &[Zone], library: &Path) -> (Vec<Value>, Vec<String>) {
    // The archive is not consulted while mapping zones.
    let empty = Archive { bytes: Vec::new(), archive_version: 0, group_version: 0, sound_version: 0, sounds: Vec::new(), modules: Vec::new(), has_slots: false, group_name_offset: 0, group_name: String::new(), sound_layout: archive::SoundLayout { first_name_delta: 0, name_delta: 0, channel_shift: 0 } };
    let mut importer = Importer { archive: &empty, warnings: Vec::new(), reports: Vec::new(), frames: HashMap::new(), library: library.to_path_buf() };
    let mapped = importer.map_zones(zones);
    (mapped, importer.warnings)
}

/// The installed expansion folder of a group: the nearest folder named "… Library", or (for
/// expansions installed under another name) the folder holding the product's `.nicnt` file;
/// else `root`, else the folder two levels above the group.
pub fn expansion_root(group: &Path, root: Option<&Path>) -> PathBuf {
    let mut candidate = group.parent().map(Path::to_path_buf).unwrap_or_default();
    for _ in 0..8 {
        if candidate.file_name().is_some_and(|name| name.to_string_lossy().ends_with(" Library")) {
            return candidate;
        }
        let has_marker = fs::read_dir(&candidate).is_ok_and(|entries| entries.flatten().any(|entry| entry.file_name().to_string_lossy().to_lowercase().ends_with(".nicnt")));
        if has_marker {
            return candidate;
        }
        match candidate.parent() {
            Some(parent) if parent != candidate => candidate = parent.to_path_buf(),
            _ => break,
        }
    }
    root.map(Path::to_path_buf).unwrap_or_else(|| group.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default())
}

fn lexical(path: &Path) -> PathBuf {
    st_project::media::normalize(path)
}

/// The file a sample reference names inside the expansion folder `library`.
///
/// References are relative to the library; some expansions write them with one or two leading
/// slashes ("//Samples/Drums/Kick.wav"), which is accepted for the library's own Samples folder
/// only. Any other absolute path, a drive letter, `..`, or a symbolic link that leads out of
/// the library is refused. The file need not exist.
pub fn resolve(relative: &str, library: &Path) -> Result<PathBuf> {
    let leaves = || invalid("sample path leaves its expansion library.");
    let mut normalized = relative.replace('\\', "/");
    for prefix in ["//Samples/", "/Samples/"] {
        if normalized.starts_with(prefix) {
            normalized = normalized[prefix.len() - "Samples/".len()..].to_string();
            break;
        }
    }
    if normalized.starts_with('/') || normalized.contains(':') || normalized.split('/').any(|part| part == "..") {
        return Err(leaves());
    }
    let root = fs::canonicalize(library).unwrap_or_else(|_| lexical(library));
    let mut pending: std::collections::VecDeque<String> = normalized.split('/').filter(|part| !part.is_empty()).map(String::from).collect();
    let mut current = root.clone();
    let mut hops = 0;
    while let Some(part) = pending.pop_front() {
        current = lexical(&current.join(&part));
        if !current.starts_with(&root) {
            return Err(leaves());
        }
        if let Ok(target) = fs::read_link(&current) {
            hops += 1;
            if hops > 32 {
                return Err(invalid("sample path contains a symbolic-link cycle."));
            }
            let destination = lexical(&if target.is_absolute() { target } else { current.parent().unwrap_or(&root).join(target) });
            let Ok(inside) = destination.strip_prefix(&root) else { return Err(leaves()) };
            for component in inside.components().rev() {
                if let Component::Normal(name) = component {
                    pending.push_front(name.to_string_lossy().into_owned());
                }
            }
            current = root.clone();
        }
    }
    if current == root || !current.starts_with(&root) {
        return Err(leaves());
    }
    Ok(current)
}

impl ImportedGroup {
    /// The group as a group project (see [`crate::kit::group`]): one member track per pad that
    /// has a source (a group without any keeps its first pad, so its bus and inserts survive),
    /// the group bus with the Group's inserts, and the routes between them. Identities are new
    /// on every call.
    pub fn group_project(&self) -> Value {
        let (group_id, bus_id) = (unique_id(), unique_id());
        let mut members: Vec<&ImportedSound> = self.sounds.iter().filter(|sound| sound.has_source()).collect();
        if members.is_empty() {
            members.extend(self.sounds.first());
        }
        let (mut tracks, mut instruments, mut plugins): (Vec<Value>, Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new(), Vec::new());
        let mut outputs = Map::new();
        for (index, sound) in members.iter().enumerate() {
            let routing_id = format!("{group_id}-pad-{}", sound.pad + 1);
            let mut instrument = sound.instrument.clone();
            instrument.insert("id".to_string(), json!(index));
            instrument.insert("name".to_string(), json!(sound.name));
            // Mono and legato are the member track's voice mode.
            let voice_mode = instrument.remove("voiceMode").and_then(|mode| mode.as_str().map(String::from));
            let mut track = json!({
                "id": index, "name": sound.name, "routingID": routing_id, "groupID": group_id, "pad": sound.pad, "padNote": 60, "instrument": index,
                "volume": sound.volume, "pan": sound.pan, "mute": sound.mute, "solo": false, "delaySend": 0.0, "reverbSend": 0.0,
            });
            if sound.choke_group > 0 {
                track["chokeGroup"] = json!(sound.choke_group.min(16));
            }
            if let Some(mode) = voice_mode.filter(|mode| mode == "mono" || mode == "legato") {
                track["voiceMode"] = json!(mode);
            }
            for plugin in &sound.plugins {
                let mut descriptor = plugin.clone();
                descriptor["owner"] = json!(format!("track:{routing_id}"));
                plugins.push(descriptor);
            }
            outputs.insert(format!("track:{routing_id}"), json!(format!("bus:{bus_id}")));
            tracks.push(track);
            instruments.push(Value::Object(instrument));
        }
        for plugin in &self.group_plugins {
            let mut descriptor = plugin.clone();
            descriptor["owner"] = json!(format!("bus:{bus_id}"));
            plugins.push(descriptor);
        }
        outputs.insert(format!("bus:{bus_id}"), json!("master"));
        json!({
            "version": st_project::model::FORMAT_VERSION, "name": self.name, "tracks": tracks, "instruments": instruments,
            "groups": [{ "id": group_id, "name": self.name, "collapsed": false, "mixerCollapsed": false, "busID": bus_id, "importNotes": self.warnings }],
            "audioPlugins": plugins,
            "routing": { "buses": [{ "id": bus_id, "name": self.name, "type": "group", "groupID": group_id, "volume": 1.0, "pan": 0.0, "mute": false, "solo": false }],
                         "outputs": outputs, "sends": [], "sidechains": [] },
        })
    }
}

/// Imports the group at `path` as `kit.read` returns it.
pub(crate) fn read_group(path: &str, root: Option<&str>) -> Result<KitReadResponse> {
    let path = crate::util::normalized(path);
    if extension(&path) != "mxgrp" {
        return Err(Error::invalid("Choose a Maschine .mxgrp group."));
    }
    let imported = import_group(&path, root)?;
    let project = imported.group_project();
    let mut metadata = serde_json::to_value(kit::metadata(&path, root)?).unwrap_or(Value::Null);
    metadata["name"] = json!(imported.name);
    metadata["soundCount"] = json!(kit::group::loaded_sound_count(&project));
    metadata["metadataSource"] = json!("native Maschine object archive");
    Ok(KitReadResponse { path, metadata, sounds: kit::group::sound_summaries(&project), group_project: project, warnings: imported.warnings, report: Some(imported.report) })
}

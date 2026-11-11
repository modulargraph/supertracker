//! The plug-in area: the inserts and instruments of the mixer as the interface edits them
//! (`plugin.*`), the catalogue of installed plug-ins (`plugins.*`) and crash recovery.
//!
//! [`install`] creates the plug-in manager of `st-audio` ([`st_audio::plugins`]), attaches it
//! to the audio engine and registers the commands below. Plug-ins run in
//! `supertracker-plugin-container` processes; built-in effects are processors of the mixer.
//! Both are inserts to the interface, so every `plugin.*` command serves both.
//!
//! # Who owns the project
//!
//! The interface does. A command that changes the insert chain (`plugin.load`, `.remove`,
//! `.move`, a bypass) applies the change to the engine's copy of the project at once, so it is
//! heard and can be asked about (`plugin.state`, `plugin.parameters`), and answers with the
//! descriptors; the interface then stores them in its project and publishes it, which finds
//! the same instances by their `instanceID` and changes nothing.
//!
//! | Command | Notes |
//! |---|---|
//! | `plugins.list` | built-in effects and the scanned plug-ins; `{details: true}` adds the blocklist, the folders and the scan's progress |
//! | `plugins.scan` | `{rescan?, retryFailed?, folders?, wait?}`; progress in `plugins.catalogue` events |
//! | `plugin.load`, `plugin.loadNKS` | creates the instance; the reply is its descriptor with `parameters` and `presets` |
//! | `plugin.remove`, `plugin.move` | |
//! | `plugin.parameters`, `plugin.parameter`, `plugin.state`, `plugin.preset`, `plugin.editor` | |
//! | `plugins.recovery`, `plugins.recovery.retry` | the report the recovery dialog reads; retry restores lost instances and loads held-back plug-ins again |
//!
//! Events: `plugins.catalogue` (scan progress), `plugin.parameter` and `plugin.gesture` (a
//! value changed, a control grabbed or released, in a plug-in's editor), `plugin.changed`
//! (parameter list, latency, state, editor closed, restored), `plugin.error` (a plug-in could
//! not be loaded or stopped working) and `plugins.recovery` (the report, when a plug-in was
//! lost while running).
//!
//! Settings section `plugins`: `{folders: [path], standardFolders: bool, grouping: "plugin" |
//! "manufacturer" | "shared" | "instance", container?: path}`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use st_audio::graph::insert::builtin_effects_metadata;
use st_audio::graph::spec::track_owner;
use st_audio::plugins::catalog::is_instrument;
use st_audio::plugins::client::{ParameterInfo, PARAM_AUTOMATABLE, PARAM_BOOLEAN, PARAM_HIDDEN, PARAM_READ_ONLY, PARAM_STEPPED};
use st_audio::plugins::manager::{default_socket_dir, grouping_policy, yield_activation};

/// The window layer installs the macOS call that lets a container's editor take the keyboard.
pub use st_audio::plugins::manager::set_activation_yielder;
use st_audio::plugins::{Catalog, CatalogOptions, Hosted, Lookup, PluginEvent, PluginManager, PluginOptions};
use st_audio::AudioEngine;
use st_project::Project;

use crate::areas::audio::Audio;
use crate::commands::{Commands, NoPayload};
use crate::{fail, Error, EventSink, Result, Settings};

const SETTINGS_SECTION: &str = "plugins";
/// Insert slots per channel.
const INSERT_SLOTS: i64 = 16;
/// Parameters whose text the plug-in is asked for in one `plugin.parameters`.
const DISPLAY_LIMIT: usize = 512;

/// The settings section `plugins`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct PluginSettings {
    /// The user's own plug-in folders, searched before the standard ones.
    folders: Vec<PathBuf>,
    standard_folders: bool,
    /// How instances share container processes.
    grouping: String,
    /// The container executable; absent: next to the host's own executable.
    container: Option<PathBuf>,
}

impl Default for PluginSettings {
    fn default() -> Self {
        PluginSettings { folders: Vec::new(), standard_folders: true, grouping: "plugin".to_string(), container: None }
    }
}

/// How [`install`] sets plug-in hosting up.
#[derive(Debug, Clone, Default)]
pub struct PluginAreaOptions {
    /// The container executable (tests); else the settings, else the client's own search
    /// (`$STPC_CONTAINER_PATH`, then next to the host's executable).
    pub container: Option<PathBuf>,
    /// Folders searched besides the settings' (tests; `$SUPERTRACKER_PLUGIN_PATH`).
    pub folders: Vec<PathBuf>,
    /// Do not search the standard folders, whatever the settings say (tests).
    pub only_these_folders: bool,
}

impl PluginAreaOptions {
    /// `$SUPERTRACKER_PLUGIN_PATH`: folders (separated like `$PATH`) searched for plug-ins
    /// besides the settings'; `$SUPERTRACKER_PLUGIN_STANDARD_FOLDERS=0` leaves the standard
    /// folders out.
    pub fn from_environment() -> PluginAreaOptions {
        let folders = std::env::var_os("SUPERTRACKER_PLUGIN_PATH").map(|paths| std::env::split_paths(&paths).filter(|path| !path.as_os_str().is_empty()).collect()).unwrap_or_default();
        PluginAreaOptions { container: None, folders, only_these_folders: std::env::var("SUPERTRACKER_PLUGIN_STANDARD_FOLDERS").is_ok_and(|value| value == "0") }
    }
}

/// The plug-in service of the host.
pub struct Plugins {
    engine: Arc<AudioEngine>,
    manager: Arc<PluginManager>,
    catalog: Option<Arc<Catalog>>,
    catalog_error: Option<String>,
    events: EventSink,
    settings: Arc<Settings>,
    extra_folders: Vec<PathBuf>,
    /// One change of the insert chain at a time.
    editing: Mutex<()>,
}

/// Which insert or instrument a request means.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    owner: String,
    instrument: bool,
    slot: i64,
}

impl Target {
    fn kind(&self) -> &'static str {
        if self.instrument { "instrument" } else { "effect" }
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty())
}

fn whole(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_f64).filter(|number| number.is_finite()).map(|number| number as i64)
}

fn is_builtin(component: &str) -> bool {
    st_project::model::plugin::builtin_effect_id(component).is_some()
}

/// A new `instanceID`.
fn new_instance_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_nanos() as u64).unwrap_or(0);
    let mut hash = time ^ (std::process::id() as u64) << 32 ^ NEXT.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^= hash >> 33;
    format!("plug-{hash:016x}")
}

/// The channel a descriptor names.
fn owner_of(descriptor: &Value) -> &str {
    text(descriptor, "owner").unwrap_or_default()
}

fn track_index(project: &Project, owner: &str) -> Option<usize> {
    (0..project.track_list().len()).find(|index| track_owner(project, *index) == owner)
}

fn descriptor_kind(descriptor: &Value) -> &str {
    if descriptor.get("kind").and_then(Value::as_str) == Some("instrument") { "instrument" } else { "effect" }
}

fn matches(descriptor: &Value, target: &Target) -> bool {
    owner_of(descriptor) == target.owner && descriptor_kind(descriptor) == target.kind() && (target.instrument || whole(descriptor, "slot").unwrap_or(0) == target.slot)
}

fn parameter_value(info: &ParameterInfo, plain: f64, display: Option<String>) -> Value {
    let width = info.max - info.min;
    let normalized = if width > 0.0 { ((plain - info.min) / width).clamp(0.0, 1.0) } else { 0.0 };
    let mut item = json!({
        "address": info.id.to_string(), "id": if info.identifier.is_empty() { info.id.to_string() } else { info.identifier.clone() },
        "name": info.name, "shortName": info.short_name, "group": info.group, "min": info.min, "max": info.max, "value": plain, "normalized": normalized,
        "unit": 0, "unitName": info.unit_label, "display": display.unwrap_or_else(|| if info.unit_label.is_empty() { format!("{plain:.3}") } else { format!("{plain:.3} {}", info.unit_label) }),
        "flags": info.flags, "writable": info.flags & PARAM_READ_ONLY == 0, "automatable": info.flags & PARAM_AUTOMATABLE != 0 && info.flags & PARAM_READ_ONLY == 0,
        "hidden": info.flags & PARAM_HIDDEN != 0, "defaultValue": if width > 0.0 { ((info.default - info.min) / width).clamp(0.0, 1.0) } else { 0.0 },
    });
    let steps = if info.flags & PARAM_BOOLEAN != 0 { 1 } else if info.flags & PARAM_STEPPED != 0 { info.step_count.max(if width > 0.0 { width.round() as u32 } else { 0 }) } else { 0 };
    if steps > 0 {
        item["stepCount"] = json!(steps);
        item["normalizedStep"] = json!(1.0 / steps as f64);
        if info.value_strings.len() as u32 == steps + 1 {
            item["valueStrings"] = json!(info.value_strings);
        }
    }
    item
}

impl Plugins {
    /// The plug-in manager, for other areas (export engines take `manager().offline()`).
    pub fn manager(&self) -> &Arc<PluginManager> {
        &self.manager
    }

    pub fn catalog(&self) -> Option<&Arc<Catalog>> {
        self.catalog.as_ref()
    }

    fn project(&self) -> Result<Project> {
        self.engine.project().ok_or_else(|| Error::new("No project is open in the audio engine yet."))
    }

    /// The insert or instrument a request names: `{owner, kind?, slot?}`.
    fn target(&self, payload: &Value) -> Result<Target> {
        let instrument = payload.get("kind").and_then(Value::as_str).unwrap_or("effect") == "instrument";
        let Some(owner) = text(payload, "owner").map(String::from) else {
            fail!("Choose the channel of the plug-in: its owner (track:<routingID>, bus:<id>, master or instrument:<n>).");
        };
        let slot = if instrument { 0 } else { whole(payload, "slot").unwrap_or(0) };
        if !(0..INSERT_SLOTS).contains(&slot) {
            fail!("Insert slots are 0 to {}.", INSERT_SLOTS - 1);
        }
        if instrument && !owner.starts_with("track:") {
            fail!("An instrument plug-in belongs to a track.");
        }
        Ok(Target { owner, instrument, slot })
    }

    /// The descriptor of `target` in the engine's project.
    fn descriptor(&self, project: &Project, target: &Target) -> Option<Value> {
        project.to_value()["audioPlugins"].as_array().into_iter().flatten().find(|descriptor| matches(descriptor, target)).cloned()
    }

    fn require(&self, project: &Project, target: &Target) -> Result<Value> {
        self.descriptor(project, target).ok_or_else(|| Error::new(if target.instrument { "This track has no instrument plug-in.".to_string() } else { format!("Insert slot {} of this channel is empty.", target.slot + 1) }))
    }

    /// The hosted instances of a plug-in descriptor (one per playing track for an effect of an
    /// instrument's chain), or why there is none.
    fn hosted(&self, target: &Target, descriptor: &Value) -> Result<Vec<Arc<Hosted>>> {
        let keys = self.engine.insert_keys(&target.owner, target.instrument, target.slot as usize, text(descriptor, "instanceID"));
        let mut hosted = Vec::new();
        let mut reason = None;
        for key in &keys {
            match self.manager.lookup(key) {
                Lookup::Hosted(instance) => hosted.push(instance),
                Lookup::Unavailable(why) => reason = Some(why),
                Lookup::Unknown => {}
            }
        }
        if hosted.is_empty() {
            let name = text(descriptor, "name").or_else(|| text(descriptor, "component")).unwrap_or("This plug-in");
            return Err(Error::new(reason.unwrap_or_else(|| {
                if keys.is_empty() && target.owner.starts_with("instrument:") {
                    format!("{name} is not loaded: no track plays this instrument.")
                } else {
                    format!("{name} is not loaded in the audio engine, so its parameters are unavailable. Replace the plug-in or reopen the project to load it again.")
                }
            })));
        }
        Ok(hosted)
    }

    /// Changes the engine's copy of the project and publishes it to the engine.
    fn edit(&self, project: &Project, change: impl FnOnce(&mut Value) -> Result<()>) -> Result<Project> {
        let mut value = project.to_value();
        if !value["audioPlugins"].is_array() {
            value["audioPlugins"] = json!([]);
        }
        change(&mut value)?;
        let next = Project::from_value(value)?;
        self.engine.publish_with(&next, false).map_err(Error::new)?;
        Ok(next)
    }

    // ---- descriptors ---------------------------------------------------------------------

    /// What a reply says about a descriptor beyond what the project stores: live latency,
    /// presets, status, and the parameter list when asked for.
    fn summary(&self, target: &Target, mut descriptor: Value, parameters: bool) -> Value {
        let component = text(&descriptor, "component").unwrap_or_default().to_string();
        descriptor["owner"] = json!(target.owner);
        descriptor["kind"] = json!(target.kind());
        descriptor["slot"] = json!(target.slot);
        if is_builtin(&component) {
            descriptor["presets"] = json!([]);
            descriptor["hostStatus"] = json!({ "available": true });
            if parameters {
                descriptor["parameters"] = json!(self.builtin_parameters(target, &descriptor).unwrap_or_default());
            }
            return descriptor;
        }
        match self.hosted(target, &descriptor) {
            Ok(hosted) => {
                let first = &hosted[0];
                let rate = self.engine.sample_rate().max(1.0);
                descriptor["latency"] = json!(first.latency() as f64 / rate);
                descriptor["tailTime"] = json!(first.tail_seconds(rate));
                descriptor["presets"] = json!(first.presets().into_iter().map(|(number, name)| json!({ "number": number, "name": name })).collect::<Vec<_>>());
                descriptor["hostStatus"] = match first.lost() {
                    Some(reason) => json!({ "available": false, "lost": true, "reason": reason }),
                    None => json!({ "available": true, "hasEditor": first.has_editor, "sidechain": first.sidechain, "editorOpen": first.editor_open() }),
                };
                if parameters {
                    descriptor["parameters"] = json!(self.plugin_parameters(first, None).unwrap_or_default());
                }
            }
            Err(error) => descriptor["hostStatus"] = json!({ "available": false, "reason": error.to_string() }),
        }
        descriptor
    }

    fn builtin_parameters(&self, target: &Target, descriptor: &Value) -> Option<Vec<Value>> {
        let key = self.engine.insert_keys(&target.owner, false, target.slot as usize, text(descriptor, "instanceID")).into_iter().next()?;
        self.engine.builtin_parameters(&key)
    }

    fn plugin_parameters(&self, hosted: &Hosted, addresses: Option<&HashSet<String>>) -> Result<Vec<Value>> {
        let parameters = hosted.parameters().map_err(Error::new)?;
        let values = hosted.values().map_err(Error::new)?;
        let lost = hosted.lost().is_some();
        let mut list = Vec::with_capacity(parameters.len());
        for info in parameters.iter().filter(|info| addresses.is_none_or(|wanted| wanted.contains(&info.id.to_string()))) {
            let plain = values.iter().find(|(id, _)| *id == info.id).map_or(info.default, |(_, value)| *value);
            // The plug-in's own text, for as many parameters as is reasonable to ask one by one.
            let display = if !lost && list.len() < DISPLAY_LIMIT { hosted.display(info.id, &[plain]).and_then(|texts| texts.into_iter().next()).filter(|text| !text.is_empty()) } else { None };
            list.push(parameter_value(info, plain, display));
        }
        Ok(list)
    }

    // ---- catalogue -----------------------------------------------------------------------

    fn catalog_entries(&self) -> Vec<Value> {
        let mut entries: Vec<Value> = builtin_effects_metadata()
            .as_array()
            .into_iter()
            .flatten()
            .map(|effect| json!({ "id": effect["id"], "name": effect["name"], "manufacturer": "SuperTracker", "vendor": "SuperTracker", "format": "Builtin", "kind": "effect", "type": "effect", "category": effect["category"], "isInstrument": false, "sidechain": effect["sidechain"] }))
            .collect();
        for plugin in self.catalog.iter().flat_map(|catalog| catalog.plugins()) {
            let kind = if is_instrument(&plugin) { "instrument" } else { "effect" };
            let mut entry = json!({
                "id": format!("{}:{}", plugin.format, plugin.id), "name": plugin.name, "manufacturer": plugin.vendor, "vendor": plugin.vendor, "version": plugin.version,
                "format": plugin.format.to_ascii_uppercase(), "kind": kind, "type": kind, "category": plugin.category, "isInstrument": kind == "instrument",
            });
            if !plugin.path.is_empty() {
                entry["bundlePath"] = json!(plugin.path);
            }
            entries.push(entry);
        }
        entries
    }

    /// `plugins.list`.
    fn list(&self, payload: Value) -> Result<Value> {
        let plugins = self.catalog_entries();
        if payload.get("details") != Some(&json!(true)) {
            return Ok(Value::Array(plugins));
        }
        let catalog = self.catalog.as_ref();
        Ok(json!({
            "plugins": plugins, "scanning": catalog.is_some_and(|catalog| catalog.scanning()), "progress": catalog.map(|catalog| catalog.progress()),
            "blocklist": catalog.map(|catalog| catalog.blocked()).unwrap_or_default(), "folders": catalog.map(|catalog| catalog.folders()).unwrap_or_default(),
            "error": self.catalog_error,
        }))
    }

    fn catalogue_event(&self, catalog: &Catalog, progress: &st_audio::plugins::ScanProgress) {
        let mut payload = serde_json::to_value(progress).unwrap_or(Value::Null);
        if !progress.scanning {
            payload["blocklist"] = json!(catalog.blocked());
        }
        self.events.emit("plugins.catalogue", payload);
    }

    fn run_scan(self: &Arc<Self>, rescan: bool, retry_failed: bool) -> Option<st_audio::plugins::ScanProgress> {
        let catalog = self.catalog.clone()?;
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
        catalog.scan(rescan, retry_failed, &mut |progress| {
            // Every file would be too many events for a large collection.
            if !progress.scanning || progress.completed == 0 || last.elapsed() >= std::time::Duration::from_millis(100) {
                last = std::time::Instant::now();
                self.catalogue_event(&catalog, progress);
            }
        })
    }

    /// `plugins.scan`.
    fn scan(self: &Arc<Self>, payload: Value) -> Result<Value> {
        let Some(catalog) = self.catalog.clone() else { fail!("{}", self.catalog_error.clone().unwrap_or_else(|| "Plug-in scanning is unavailable.".to_string())) };
        if let Some(folders) = payload.get("folders").and_then(Value::as_array) {
            let folders: Vec<PathBuf> = folders.iter().filter_map(Value::as_str).filter(|folder| !folder.is_empty()).map(PathBuf::from).collect();
            if folders.iter().any(|folder| !folder.is_absolute()) {
                fail!("Plug-in folders must be absolute paths.");
            }
            let saved = self.settings.update(SETTINGS_SECTION, |settings: &mut PluginSettings| settings.folders = folders)?;
            let mut all = self.extra_folders.clone();
            all.extend(saved.folders);
            catalog.set_folders(all).map_err(Error::new)?;
        }
        let (rescan, retry) = (payload.get("rescan") == Some(&json!(true)), payload.get("retryFailed") == Some(&json!(true)));
        if payload.get("wait") == Some(&json!(true)) {
            // One scan at a time: this one follows the one that is running.
            let done = loop {
                match self.run_scan(rescan, retry) {
                    Some(done) => break done,
                    None => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            };
            return Ok(json!({ "scanning": false, "result": done, "plugins": self.catalog_entries(), "blocklist": catalog.blocked() }));
        }
        if !catalog.scanning() {
            let service = self.clone();
            let _ = std::thread::Builder::new().name("plugin-scan".to_string()).spawn(move || {
                service.run_scan(rescan, retry);
            });
        }
        Ok(json!({ "scanning": true }))
    }

    // ---- loading -------------------------------------------------------------------------

    /// The descriptor `plugin.load` puts into the project.
    fn new_descriptor(&self, target: &Target, payload: &Value) -> Result<Value> {
        let Some(component) = text(payload, "component").or_else(|| text(payload, "id")) else { fail!("Choose a plug-in to load.") };
        let mut descriptor = Map::new();
        // What the interface stored with the descriptor travels on (pages, NKS binding).
        for key in ["nks", "state", "name", "manufacturer", "format", "bundlePath", "bypass"] {
            if let Some(value) = payload.get(key).filter(|value| !value.is_null()) {
                descriptor.insert(key.to_string(), value.clone());
            }
        }
        descriptor.insert("owner".into(), json!(target.owner));
        descriptor.insert("kind".into(), json!(target.kind()));
        descriptor.insert("slot".into(), json!(target.slot));
        descriptor.insert("instanceID".into(), json!(text(payload, "instanceID").map(String::from).unwrap_or_else(new_instance_id)));
        descriptor.insert("component".into(), json!(component));
        descriptor.entry("bypass").or_insert(json!(false));
        if is_builtin(component) {
            if target.instrument {
                fail!("Built-in effects are insert effects, not instruments.");
            }
            let id = format!("stfx:{}", component.split_once(':').map_or(component, |(_, id)| id));
            let effects = builtin_effects_metadata();
            let Some(effect) = effects.as_array().into_iter().flatten().find(|effect| effect["id"] == json!(id)) else { fail!("This version has no built-in effect named {component}.") };
            descriptor.insert("component".into(), json!(id));
            descriptor.insert("format".into(), json!("Builtin"));
            descriptor.insert("manufacturer".into(), json!("SuperTracker"));
            descriptor.entry("name").or_insert(effect["name"].clone());
            return Ok(Value::Object(descriptor));
        }
        let Some((format, id)) = st_audio::plugins::ident::parse_component(component, text(payload, "format")) else {
            fail!("{component} is not a plug-in this version can host (CLAP, VST3 or Audio Unit).");
        };
        descriptor.insert("component".into(), json!(format!("{format}:{id}")));
        descriptor.insert("format".into(), json!(format.to_ascii_uppercase()));
        // The catalogue knows the plug-in's file, name and maker, once its scan has listed the
        // plug-in. A descriptor it leaves unnamed is named by the loaded instance ([`Plugins::place`]).
        if let Some(found) = self.catalog.as_ref().and_then(|catalog| catalog.find(&format, &id)) {
            let stale = text(payload, "bundlePath").is_none_or(|path| !std::path::Path::new(path).exists());
            if stale && !found.path.is_empty() {
                descriptor.insert("bundlePath".into(), json!(found.path));
            }
            descriptor.entry("name").or_insert(json!(found.name));
            descriptor.entry("manufacturer").or_insert(json!(found.vendor));
            if is_instrument(&found) != target.instrument && text(payload, "state").is_none() {
                fail!("{} is {}, not {}.", found.name, if target.instrument { "an effect" } else { "an instrument" }, if target.instrument { "an instrument" } else { "an effect" });
            }
        }
        Ok(Value::Object(descriptor))
    }

    /// Puts `descriptor` into the engine's project in place of whatever `target` held, and
    /// answers with its summary. On a failed load the engine gets its previous project back.
    fn place(&self, project: &Project, target: &Target, descriptor: Value) -> Result<Value> {
        let replaced: Vec<String> = project.to_value()["audioPlugins"].as_array().into_iter().flatten().filter(|other| matches(other, target)).filter_map(|other| text(other, "instanceID").map(String::from)).collect();
        let instance = text(&descriptor, "instanceID").unwrap_or_default().to_string();
        self.manager.forget_failures();
        self.edit(project, |value| {
            let plugins = value["audioPlugins"].as_array_mut().expect("an array");
            plugins.retain(|other| !matches(other, target));
            plugins.push(descriptor.clone());
            remove_sidechains(value, &replaced.iter().filter(|id| **id != instance).cloned().collect::<Vec<_>>());
            Ok(())
        })?;
        let component = text(&descriptor, "component").unwrap_or_default();
        let failure = if is_builtin(component) {
            self.engine.inserts().into_iter().find(|insert| insert.instance_id == instance && !insert.available).map(|insert| insert.reason.unwrap_or_default())
        } else {
            self.hosted(target, &descriptor).err().map(|error| error.to_string()).filter(|_| !(target.owner.starts_with("instrument:") && self.engine.insert_keys(&target.owner, false, target.slot as usize, Some(&instance)).is_empty()))
        };
        if let Some(reason) = failure {
            // Nothing was committed by the interface yet: the engine returns to what it had.
            let _ = self.engine.publish_with(project, false);
            return Err(Error::new(reason));
        }
        let descriptor = self.named(target, descriptor)?;
        Ok(self.summary(target, descriptor, true))
    }

    /// `descriptor` with the name and maker its loaded instance reports, where it has none
    /// (neither the request nor the catalogue knew them), stored in the engine's project too.
    fn named(&self, target: &Target, mut descriptor: Value) -> Result<Value> {
        let Ok(hosted) = self.hosted(target, &descriptor) else { return Ok(descriptor) };
        let identity = &hosted[0].identity;
        let missing: Vec<(&str, &String)> = [("name", &identity.name), ("manufacturer", &identity.manufacturer)].into_iter().filter(|(key, value)| text(&descriptor, key).is_none() && !value.is_empty()).collect();
        if missing.is_empty() {
            return Ok(descriptor);
        }
        for (key, value) in &missing {
            descriptor[*key] = json!(value);
        }
        let current = self.project()?;
        self.edit(&current, |value| {
            for other in value["audioPlugins"].as_array_mut().into_iter().flatten().filter(|other| other["instanceID"] == descriptor["instanceID"]) {
                for (key, value) in &missing {
                    other[*key] = json!(value);
                }
            }
            Ok(())
        })?;
        Ok(descriptor)
    }

    /// `plugin.load`.
    fn load(&self, payload: Value) -> Result<Value> {
        let _editing = self.editing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = self.project()?;
        let target = self.target(&payload)?;
        let descriptor = self.new_descriptor(&target, &payload)?;
        self.place(&project, &target, descriptor)
    }

    /// `plugin.loadNKS`: the preset's plug-in, found among the installed ones, loaded with the
    /// preset's exact state; the reply carries the preset's controller pages bound to the
    /// parameters the plug-in really has.
    fn load_nks(&self, payload: Value) -> Result<Value> {
        let Some(path) = text(&payload, "path") else { fail!("Choose an NKS preset to load.") };
        let preset = st_library::nks::preset::NksHostPreset::read(path).map_err(|error| Error::new(error.to_string()))?;
        let Some(catalog) = self.catalog.as_ref() else { fail!("{}", self.catalog_error.clone().unwrap_or_else(|| "The plug-in catalogue is unavailable.".to_string())) };
        let wanted = preset.identity.name.clone().unwrap_or_else(|| "the preset's plug-in".to_string());
        let find = || catalog.plugins().into_iter().find(|plugin| plugin.format == "vst3" && preset.matches_vst3(&plugin.id));
        // A plug-in installed since the last scan is found by scanning (what is known is cached).
        let Some(plugin) = find().or_else(|| catalog.scan(false, false, &mut |_| {}).and_then(|_| find())) else {
            fail!("{wanted} is not installed as a VST3 plug-in, so this NKS preset cannot be loaded.");
        };
        let _editing = self.editing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = self.project()?;
        let mut request = payload.clone();
        if request.get("kind").is_none() {
            request["kind"] = json!("instrument");
        }
        request["component"] = json!(format!("vst3:{}", plugin.id));
        request["bundlePath"] = json!(plugin.path);
        request["name"] = json!(plugin.name);
        request["manufacturer"] = json!(plugin.vendor);
        // The preset holds the component's own state: framed as the container stores VST3 state.
        request["state"] = json!(base64(&vst3_component_state(&plugin.id, &preset.state)?));
        let target = self.target(&request)?;
        let mut descriptor = self.new_descriptor(&target, &request)?;
        descriptor["nks"] = json!({ "name": preset.name, "path": preset.path, "identity": preset.identity, "pages": [], "warnings": preset.warnings });
        let mut loaded = self.place(&project, &target, descriptor)?;
        // Bind the pages to the parameters the loaded plug-in has, and keep the binding in the
        // engine's descriptor too.
        let ids: HashSet<u32> = loaded["parameters"].as_array().into_iter().flatten().filter_map(|parameter| parameter["address"].as_str().and_then(|address| address.parse().ok())).collect();
        let pages = serde_json::to_value(preset.validated_mappings(&plugin.id, &ids)).unwrap_or_else(|_| json!([]));
        loaded["nks"]["pages"] = pages.clone();
        let current = self.project()?;
        let _ = self.edit(&current, |value| {
            for other in value["audioPlugins"].as_array_mut().into_iter().flatten().filter(|other| other["instanceID"] == loaded["instanceID"]) {
                other["nks"]["pages"] = pages.clone();
            }
            Ok(())
        });
        Ok(loaded)
    }

    /// `plugin.remove`.
    fn remove(&self, payload: Value) -> Result<Value> {
        let _editing = self.editing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = self.project()?;
        let target = self.target(&payload)?;
        let removed: Vec<String> = project.to_value()["audioPlugins"].as_array().into_iter().flatten().filter(|other| matches(other, &target)).filter_map(|other| text(other, "instanceID").map(String::from)).collect();
        let found = self.descriptor(&project, &target).is_some();
        if found {
            self.edit(&project, |value| {
                value["audioPlugins"].as_array_mut().expect("an array").retain(|other| !matches(other, &target));
                remove_sidechains(value, &removed);
                Ok(())
            })?;
        }
        Ok(json!({ "removed": found, "owner": target.owner, "kind": target.kind(), "slot": target.slot, "instanceIDs": removed }))
    }

    /// `plugin.move`: the effect in `fromSlot` takes the chain position of `toSlot`; the
    /// effects between shift by one. Slot numbers in use stay the same set; automation lanes
    /// follow their effect.
    fn move_effect(&self, payload: Value) -> Result<Value> {
        let _editing = self.editing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = self.project()?;
        let mut request = payload.clone();
        request["kind"] = json!("effect");
        request["slot"] = payload.get("fromSlot").cloned().unwrap_or(Value::Null);
        let target = self.target(&request)?;
        let Some(to_slot) = whole(&payload, "toSlot").filter(|slot| (0..INSERT_SLOTS).contains(slot)) else { fail!("Choose the slot to move the effect to.") };
        let chain_of = |project: &Project| -> Vec<Value> {
            let mut chain: Vec<Value> = project.to_value()["audioPlugins"].as_array().into_iter().flatten().filter(|other| owner_of(other) == target.owner && descriptor_kind(other) == "effect").cloned().collect();
            chain.sort_by_key(|descriptor| whole(descriptor, "slot").unwrap_or(0));
            chain
        };
        let chain = chain_of(&project);
        let slots: Vec<i64> = chain.iter().map(|descriptor| whole(descriptor, "slot").unwrap_or(0)).collect();
        let (Some(from), Some(to)) = (slots.iter().position(|slot| *slot == target.slot), slots.iter().position(|slot| *slot == to_slot)) else {
            fail!("Both slots must hold an effect of this channel.");
        };
        if from == to {
            fail!("The effect is already in that position.");
        }
        // The states before the move, for the interface's Undo.
        let previous: Vec<Value> = chain.iter().map(|descriptor| self.snapshot_of(&project, &Target { owner: target.owner.clone(), instrument: false, slot: whole(descriptor, "slot").unwrap_or(0) }).unwrap_or_else(|| descriptor.clone())).collect();
        if let Some(failed) = previous.iter().find(|descriptor| descriptor.get("stateError").is_some()) {
            fail!("{}: {}", text(failed, "name").unwrap_or("A plug-in"), text(failed, "stateError").unwrap_or("its state could not be captured"));
        }
        let mut order = slots.clone();
        let moved = order.remove(from);
        order.insert(to, moved);
        // The effect that was in order[i] now sits in slots[i].
        let map: Vec<(i64, i64)> = order.iter().zip(&slots).map(|(old, new)| (*old, *new)).collect();
        let lookup = |slot: i64| map.iter().find(|(old, _)| *old == slot).map(|(_, new)| *new);
        let owner = target.owner.clone();
        let next = self.edit(&project, |value| {
            for other in value["audioPlugins"].as_array_mut().expect("an array") {
                if owner_of(other) == owner && descriptor_kind(other) == "effect" {
                    if let Some(slot) = lookup(whole(other, "slot").unwrap_or(0)) {
                        other["slot"] = json!(slot);
                    }
                }
            }
            // The lanes of the channel's effects (`plugin:effect:<slot>:<address>`) follow them.
            for lane in value["patterns"].as_array_mut().into_iter().flatten().flat_map(|pattern| pattern["automation"].as_array_mut().into_iter().flatten()) {
                let Some(target_text) = lane.get("target").and_then(Value::as_str).filter(|_| text(lane, "owner") == Some(owner.as_str())) else { continue };
                let parts: Vec<&str> = target_text.split(':').collect();
                let ["plugin", "effect", slot, address] = parts.as_slice() else { continue };
                if let Some(slot) = slot.parse().ok().and_then(lookup) {
                    lane["target"] = json!(format!("plugin:effect:{slot}:{address}"));
                }
            }
            Ok(())
        })?;
        let after: Vec<Value> = chain_of(&next).into_iter().map(|descriptor| {
            let slot = whole(&descriptor, "slot").unwrap_or(0);
            let mut item = self.summary(&Target { owner: owner.clone(), instrument: false, slot }, descriptor.clone(), false);
            // The state is the one captured before the move: moving changes none.
            if let Some(state) = previous.iter().find(|prior| prior["instanceID"] == descriptor["instanceID"]).and_then(|prior| prior.get("state")) {
                item["state"] = state.clone();
            }
            item
        }).collect();
        let slot_map: Vec<Value> = previous.iter().map(|prior| { let slot = whole(prior, "slot").unwrap_or(0); json!({ "fromSlot": slot, "toSlot": lookup(slot).unwrap_or(slot), "instanceID": prior["instanceID"] }) }).collect();
        let value = next.to_value();
        Ok(json!({
            "moved": true, "owner": owner, "fromSlot": target.slot, "toSlot": to_slot, "previousPlugins": previous, "plugins": after, "slotMap": slot_map,
            "audioPlugins": value["audioPlugins"], "routing": value["routing"],
        }))
    }

    // ---- parameters, state, presets, editor ----------------------------------------------

    /// `plugin.parameters`.
    fn parameters(&self, payload: Value) -> Result<Value> {
        let project = self.project()?;
        let target = self.target(&payload)?;
        let descriptor = self.require(&project, &target)?;
        let addresses: Option<HashSet<String>> = payload.get("addresses").and_then(Value::as_array).map(|list| list.iter().filter_map(|address| address.as_str().map(String::from).or_else(|| address.as_u64().map(|number| number.to_string()))).collect());
        let list = if is_builtin(text(&descriptor, "component").unwrap_or_default()) {
            let Some(mut list) = self.builtin_parameters(&target, &descriptor) else { fail!("This effect is not in the mixer: no track plays its instrument, or the project is still loading.") };
            if let Some(wanted) = &addresses {
                list.retain(|parameter| parameter["address"].as_str().is_some_and(|address| wanted.contains(address)));
            }
            list
        } else {
            self.plugin_parameters(&self.hosted(&target, &descriptor)?[0], addresses.as_ref())?
        };
        Ok(json!({ "parameters": list }))
    }

    /// `plugin.parameter`: `{address, value, normalized?}` sets a value; `{bypass}` bypasses.
    fn parameter(&self, payload: Value) -> Result<Value> {
        let project = self.project()?;
        let target = self.target(&payload)?;
        let descriptor = self.require(&project, &target)?;
        if let Some(bypass) = payload.get("bypass").and_then(Value::as_bool) {
            let _editing = self.editing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            self.edit(&project, |value| {
                for other in value["audioPlugins"].as_array_mut().expect("an array").iter_mut().filter(|other| matches(other, &target)) {
                    other["bypass"] = json!(bypass);
                }
                Ok(())
            })?;
            let mut descriptor = descriptor;
            descriptor["bypass"] = json!(bypass);
            return Ok(self.summary(&target, descriptor, false));
        }
        let address = payload.get("address").and_then(|address| address.as_str().and_then(|text| text.parse::<u64>().ok()).or_else(|| address.as_u64()));
        let (Some(address), Some(value)) = (address, payload.get("value").and_then(Value::as_f64).filter(|value| value.is_finite())) else {
            fail!("Setting a parameter needs its address and a finite value.");
        };
        let normalized = payload.get("normalized").and_then(Value::as_bool).unwrap_or(true);
        if is_builtin(text(&descriptor, "component").unwrap_or_default()) {
            let keys = self.engine.insert_keys(&target.owner, false, target.slot as usize, text(&descriptor, "instanceID"));
            let Some(list) = keys.first().and_then(|key| self.engine.builtin_parameters(key)) else { fail!("This effect is not in the mixer.") };
            let Some(info) = list.get(address as usize) else { fail!("The effect has no such parameter.") };
            let (min, max) = (info["min"].as_f64().unwrap_or(0.0), info["max"].as_f64().unwrap_or(1.0));
            // Built-in effects map their ranges themselves (some are logarithmic): only
            // normalized values go in.
            let position = if normalized { value.clamp(0.0, 1.0) } else if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
            for key in &keys {
                self.engine.set_insert_parameter(key, address, position);
            }
            let mut result = keys.first().and_then(|key| self.engine.builtin_parameters(key)).and_then(|list| list.get(address as usize).cloned()).unwrap_or_else(|| info.clone());
            result["written"] = json!(position);
            return Ok(result);
        }
        let hosted = self.hosted(&target, &descriptor)?;
        let parameters = hosted[0].parameters().map_err(Error::new)?;
        let Some(info) = parameters.iter().find(|info| info.id == address) else { fail!("The plug-in has no such parameter.") };
        let plain = if normalized { info.min + value.clamp(0.0, 1.0) * (info.max - info.min) } else { value };
        let mut written = plain;
        for instance in &hosted {
            written = instance.set_parameter(address, plain).map_err(Error::new)?;
        }
        let display = hosted[0].display(address, &[written]).and_then(|texts| texts.into_iter().next()).filter(|text| !text.is_empty());
        let mut result = parameter_value(info, written, display);
        result["written"] = result["normalized"].clone();
        Ok(result)
    }

    /// The descriptor of `target` with its state captured now.
    fn snapshot_of(&self, project: &Project, target: &Target) -> Option<Value> {
        let value = project.to_value();
        let index = value["audioPlugins"].as_array()?.iter().position(|descriptor| matches(descriptor, target))?;
        self.engine.snapshot_one(|candidate, _| candidate == index)
    }

    /// `plugin.state`: `{}` for every descriptor with its state; a target for one; a target
    /// and `state` to load a state first.
    fn state(&self, payload: Value) -> Result<Value> {
        if payload.get("owner").is_none() {
            if payload.get("state").is_some() {
                fail!("Restoring plug-in state requires a serialized state string and the owner of the plug-in.");
            }
            return Ok(self.engine.snapshot()["audioPlugins"].clone());
        }
        let project = self.project()?;
        let target = self.target(&payload)?;
        let descriptor = self.require(&project, &target)?;
        let mut project = project;
        if let Some(state) = payload.get("state") {
            let Some(state) = state.as_str() else { fail!("Restoring plug-in state requires a serialized state string and the owner of the plug-in.") };
            let _editing = self.editing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if is_builtin(text(&descriptor, "component").unwrap_or_default()) {
                for key in self.engine.insert_keys(&target.owner, false, target.slot as usize, text(&descriptor, "instanceID")) {
                    self.engine.forget_insert_values(&key);
                }
            } else {
                for instance in self.hosted(&target, &descriptor)? {
                    instance.load_state(state).map_err(Error::new)?;
                }
            }
            project = self.edit(&project, |value| {
                for other in value["audioPlugins"].as_array_mut().expect("an array").iter_mut().filter(|other| matches(other, &target)) {
                    other["state"] = json!(state);
                }
                Ok(())
            })?;
        }
        let mut snapshot = self.snapshot_of(&project, &target).unwrap_or(descriptor);
        // A plug-in that is not hosted keeps the state it carries; say why it is not loaded
        // only when there is nothing to keep.
        if !is_builtin(text(&snapshot, "component").unwrap_or_default()) && snapshot.get("state").is_none() && snapshot.get("stateError").is_none() {
            if let Err(error) = self.hosted(&target, &snapshot) {
                snapshot["stateError"] = json!(error.to_string());
            }
        }
        Ok(self.summary(&target, snapshot, false))
    }

    /// `plugin.preset`: `{number}` loads a factory preset; without it the presets are listed.
    fn preset(&self, payload: Value) -> Result<Value> {
        let project = self.project()?;
        let target = self.target(&payload)?;
        let descriptor = self.require(&project, &target)?;
        if is_builtin(text(&descriptor, "component").unwrap_or_default()) {
            if payload.get("number").is_some() {
                fail!("Built-in effects have no factory presets.");
            }
            return Ok(json!([]));
        }
        let hosted = self.hosted(&target, &descriptor)?;
        let Some(number) = whole(&payload, "number").or_else(|| whole(&payload, "index")) else {
            return Ok(json!(hosted[0].presets().into_iter().map(|(number, name)| json!({ "number": number, "name": name })).collect::<Vec<_>>()));
        };
        let presets = hosted[0].presets();
        let Some((id, name)) = presets.iter().find(|(id, _)| *id as i64 == number) else { fail!("The factory preset does not exist.") };
        for instance in &hosted {
            instance.load_preset(*id).map_err(Error::new)?;
        }
        let mut summary = self.summary(&target, descriptor, true);
        summary["currentPreset"] = json!({ "number": id, "name": name });
        Ok(summary)
    }

    /// `plugin.editor`: opens the plug-in's window in its container, or brings it to the
    /// front; `{open: false}` closes it.
    fn editor(&self, payload: Value) -> Result<Value> {
        let project = self.project()?;
        let target = self.target(&payload)?;
        let descriptor = self.require(&project, &target)?;
        let name = text(&descriptor, "name").unwrap_or("Plug-in").to_string();
        if is_builtin(text(&descriptor, "component").unwrap_or_default()) {
            fail!("{name} is a built-in effect: its controls are on its pages, it has no window of its own.");
        }
        let hosted = self.hosted(&target, &descriptor)?;
        let instance = &hosted[0];
        if payload.get("open") == Some(&json!(false)) {
            instance.close_editor().map_err(Error::new)?;
            return Ok(json!({ "shown": false, "open": false, "name": name }));
        }
        let place = match track_index(&project, &target.owner) {
            Some(track) => format!("Track {}", track + 1),
            None => target.owner.clone(),
        };
        // The container's process must be allowed to take the keyboard (macOS).
        yield_activation(self.manager.container_pid(instance));
        let (width, height, window) = instance.open_editor(&format!("{name} · {place}")).map_err(Error::new)?;
        Ok(json!({ "shown": true, "open": true, "custom": true, "name": name, "width": width, "height": height, "windowID": window }))
    }

    // ---- recovery ------------------------------------------------------------------------

    fn recovery_report(&self) -> Value {
        self.manager.recovery().report()
    }

    fn recovery(&self, _: NoPayload) -> Result<Value> {
        Ok(self.recovery_report())
    }

    /// `plugins.recovery.retry`: `{keys}` (all plug-ins waiting for review without it).
    fn retry(&self, payload: Value) -> Result<Value> {
        let keys: Vec<String> = match payload.get("keys").or_else(|| payload.get("ids")).and_then(Value::as_array) {
            Some(keys) => keys.iter().filter_map(|key| key.as_str().map(String::from)).collect(),
            None => self.manager.recovery().pending_keys(),
        };
        let _editing = self.editing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let authorized = self.manager.recovery().authorize(&keys);
        // Instances lost while running come back with their state...
        let mut errors: Vec<Value> = self.manager.restore(Some(&keys)).into_iter().map(|(name, message)| json!({ "name": name, "message": message })).collect();
        // ...and plug-ins that were held back are loaded again by the project.
        self.manager.forget_failures();
        if let Some(project) = self.engine.project() {
            self.engine.publish_with(&project, false).map_err(Error::new)?;
        }
        for (_, name, reason) in self.manager.failures() {
            if authorized.iter().any(|identity| identity.display_name() == name) {
                errors.push(json!({ "name": name, "message": reason }));
            }
        }
        let mut report = self.recovery_report();
        report["retryErrors"] = json!(errors);
        self.events.emit("plugin.changed", json!({ "what": "restored" }));
        Ok(report)
    }

    // ---- events from the manager ---------------------------------------------------------

    fn placed(&self, key: &str) -> Option<Value> {
        let placement = self.manager.hosted(key)?.placement();
        Some(json!({ "owner": placement.owner, "kind": if placement.instrument { "instrument" } else { "effect" }, "slot": placement.slot, "instanceID": placement.instance_id }))
    }

    fn forward(&self, event: &PluginEvent) {
        match event {
            PluginEvent::Parameter { key, address, normalized, value } => {
                let Some(mut payload) = self.placed(key) else { return };
                payload["address"] = json!(address.to_string());
                payload["value"] = json!(normalized);
                payload["normalized"] = json!(normalized);
                payload["plainValue"] = json!(value);
                payload["source"] = json!("editor");
                self.events.emit("plugin.parameter", payload);
            }
            PluginEvent::Gesture { key, address, begin } => {
                let Some(mut payload) = self.placed(key) else { return };
                payload["address"] = json!(address.to_string());
                payload["begin"] = json!(begin);
                self.events.emit("plugin.gesture", payload);
            }
            PluginEvent::Changed { key, what } => {
                let Some(mut payload) = self.placed(key) else { return };
                payload["what"] = json!(what);
                self.events.emit("plugin.changed", payload);
            }
            PluginEvent::Lost { key, name, reason } => {
                let mut payload = self.placed(key).unwrap_or_else(|| json!({}));
                payload["error"] = json!(reason);
                payload["name"] = json!(name);
                payload["lost"] = json!(true);
                self.events.emit("plugin.error", payload);
                self.events.emit("plugins.recovery", self.recovery_report());
            }
            PluginEvent::LoadFailed { key, name, error } => {
                self.events.emit("plugin.error", json!({ "error": error, "name": name, "key": key }));
            }
            PluginEvent::Graph { .. } => {}
        }
    }
}

/// Drops the sidechains that end in the given instances.
fn remove_sidechains(project: &mut Value, instances: &[String]) {
    if instances.is_empty() {
        return;
    }
    if let Some(sidechains) = project["routing"]["sidechains"].as_array_mut() {
        sidechains.retain(|sidechain| !sidechain["destination"].as_str().is_some_and(|id| instances.iter().any(|instance| instance == id)));
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for position in 0..4 {
            out.push(if position <= chunk.len() { BASE64[(word >> (18 - 6 * position) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// A VST3 component state in the container's state framing (`docs/PLUGIN_CONTAINER.md`,
/// "State container"): the bytes an NKS preset holds, without a controller state.
fn vst3_component_state(class_id: &str, component: &[u8]) -> Result<Vec<u8>> {
    if class_id.len() != 32 || !class_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        fail!("The plug-in's VST3 class ID is not 32 hexadecimal digits.");
    }
    let mut out = Vec::with_capacity(40 + component.len());
    out.extend_from_slice(b"STV3");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    for index in 0..16 {
        out.push(u8::from_str_radix(&class_id[index * 2..index * 2 + 2], 16).unwrap_or(0));
    }
    out.extend_from_slice(&(component.len() as u64).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(component);
    Ok(out)
}

/// Creates the plug-in manager and the catalogue, attaches them to the audio engine and
/// registers the plug-in commands.
pub fn install(commands: &mut Commands, audio: &Arc<Audio>, settings: &Arc<Settings>, support: &std::path::Path, events: EventSink, options: &PluginAreaOptions) -> Result<Arc<Plugins>> {
    let saved: PluginSettings = settings.get(SETTINGS_SECTION);
    let container = options.container.clone().or(saved.container.clone());
    let mut plugin_options = PluginOptions::default();
    plugin_options.supervisor.container_path = container.clone();
    plugin_options.supervisor.policy = grouping_policy(&saved.grouping);
    plugin_options.recovery_path = Some(support.join("plugin-recovery.json"));
    let manager = PluginManager::new(plugin_options).map_err(Error::new)?;
    let mut folders = options.folders.clone();
    folders.extend(saved.folders.iter().cloned());
    let catalog = Catalog::new(CatalogOptions {
        container_path: container,
        socket_dir: default_socket_dir(),
        cache_path: Some(support.join("plugins.stpcscan")),
        folders,
        standard_folders: saved.standard_folders && !options.only_these_folders,
        timeout: None,
    });
    let (catalog, catalog_error) = match catalog {
        Ok(catalog) => (Some(Arc::new(catalog)), None),
        Err(error) => (None, Some(error)),
    };
    if let Some(catalog) = &catalog {
        manager.set_catalog(catalog.clone());
    }
    let engine = audio.engine().clone();
    engine.set_plugins(manager.clone());
    let service = Arc::new(Plugins { engine, manager: manager.clone(), catalog, catalog_error, events, settings: settings.clone(), extra_folders: options.folders.clone(), editing: Mutex::default() });
    let listener = Arc::downgrade(&service);
    manager.listen(Arc::new(move |event| {
        if let Some(service) = listener.upgrade() {
            service.forward(event);
        }
    }));
    let scanner = service.clone();
    commands
        .with(&service)
        .register_blocking("plugins.list", Plugins::list)
        .register_blocking("plugins.recovery", Plugins::recovery)
        .register_blocking("plugins.recovery.retry", Plugins::retry)
        .register_blocking("plugin.load", Plugins::load)
        .register_blocking("plugin.loadNKS", Plugins::load_nks)
        .register_blocking("plugin.remove", Plugins::remove)
        .register_blocking("plugin.move", Plugins::move_effect)
        .register_blocking("plugin.parameters", Plugins::parameters)
        .register_blocking("plugin.parameter", Plugins::parameter)
        .register_blocking("plugin.state", Plugins::state)
        .register_blocking("plugin.preset", Plugins::preset)
        .register_blocking("plugin.editor", Plugins::editor);
    commands.register_blocking("plugins.scan", move |payload: Value| scanner.scan(payload));
    // What changed in the plug-in folders since the last run is scanned in the background;
    // unchanged files are answered from the cache without starting a process.
    let background = service.clone();
    let _ = std::thread::Builder::new().name("plugin-scan".to_string()).spawn(move || {
        background.run_scan(false, false);
    });
    Ok(service)
}

impl Drop for Plugins {
    fn drop(&mut self) {
        if let Some(catalog) = &self.catalog {
            catalog.cancel();
        }
        self.manager.shutdown();
    }
}

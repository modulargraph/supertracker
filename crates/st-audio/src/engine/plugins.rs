//! Plug-in hosting on [`AudioEngine`]: attaching a [`PluginManager`], reconciling its
//! instances with each published project, and what the engine reports about them. A child
//! module of [`crate::engine`], so that these additions stay out of that file.

use super::AudioEngine;
use crate::graph::insert::InsertFactory;
use crate::graph::spec::StripKind;
use crate::plugins::{PluginEvent, PluginFactory, PluginManager};
use serde_json::{json, Value};
use st_project::model::{PluginDescriptor, PluginKind, Project};
use std::sync::Arc;
use tracker_engine::engine::tables::MAX_BLOCK;

impl AudioEngine {
    /// Attaches the plug-in manager: from now on plug-in inserts and instruments of published
    /// projects are instances of `manager`, and a latency change or a restore compiles the
    /// mixer again. Once per engine. A project published earlier gets its plug-ins now.
    pub fn set_plugins(self: &Arc<Self>, manager: Arc<PluginManager>) {
        if self.plugins.set(manager.clone()).is_err() {
            return;
        }
        let engine = Arc::downgrade(self);
        manager.listen(Arc::new(move |event| {
            if let (PluginEvent::Graph { keys }, Some(engine)) = (event, engine.upgrade()) {
                engine.recompile_inserts(keys);
            }
        }));
        let project = {
            let mut state = self.lock();
            state.core.compiler.set_factory(self.insert_factory());
            state.project.clone()
        };
        if let Some(project) = project {
            let _ = self.publish_with(&project, false);
        }
    }

    /// The attached plug-in manager.
    pub fn plugins(&self) -> Option<&Arc<PluginManager>> {
        self.plugins.get()
    }

    /// The factory the compiler uses: the engine's own, behind the plug-in factory once a
    /// manager is attached.
    pub(super) fn insert_factory(&self) -> Arc<dyn InsertFactory> {
        match self.plugins.get() {
            Some(manager) => Arc::new(PluginFactory::new(manager.clone(), self.inserts.clone())),
            None => self.inserts.clone(),
        }
    }

    /// Before a project is compiled: the manager's instances follow it. Runs without the
    /// engine's lock (loading a plug-in can take long). Returns the inserts whose processor
    /// must be made anew.
    pub(super) fn reconcile_plugins(&self, project: &Project, force_states: bool) -> Vec<String> {
        let Some(manager) = self.plugins.get() else { return Vec::new() };
        let sample_rate = self.sample_rate();
        manager.reconcile(project, sample_rate, MAX_BLOCK, force_states)
    }

    /// Compiles the current mixer again with fresh processors for `keys` (their latency,
    /// sidechain input or availability changed) and publishes the plan.
    pub fn recompile_inserts(&self, keys: &[String]) {
        let mut state = self.lock();
        let core = &mut state.core;
        for key in keys {
            core.compiler.invalidate_insert(key);
        }
        if let Ok(plan) = core.compiler.compile(&core.spec) {
            core.meters = plan.meters().clone();
            core.shared.publish(plan);
            core.shared.automation_changed();
        }
    }

    /// The engine restarted at another sample rate: the instances follow (on the manager's thread).
    pub(super) fn plugins_format_changed(&self, sample_rate: f64) {
        if let Some(manager) = self.plugins.get() {
            manager.request_format(sample_rate, MAX_BLOCK);
        }
    }

    /// A channel message for the plug-in instrument of `track`, for the first frame of its
    /// next block ([`crate::plugins::manager::Hosted::send_midi`]). False when the track has
    /// no plug-in instrument, or it is not processing.
    pub(super) fn plugin_midi(&self, track: i32, bytes: [u8; 3]) -> bool {
        let Some(manager) = self.plugins.get() else { return false };
        let key = {
            let state = self.lock();
            state.core.spec.strips.iter().find(|strip| strip.kind == StripKind::Track(track.max(0) as usize)).and_then(|strip| strip.inserts.iter().find(|insert| insert.instrument.is_some())).map(|insert| insert.key.clone())
        };
        key.and_then(|key| manager.hosted(&key)).is_some_and(|hosted| hosted.send_midi(bytes))
    }

    /// Live state of the plug-ins over the compiled mixer's insert report: an insert whose
    /// instance was lost is unavailable (with the reason), latencies are the current ones.
    pub(super) fn overlay_plugins(&self, routing: &mut Value) {
        let Some(manager) = self.plugins.get() else { return };
        let keys: Vec<String> = self.lock().core.spec.strips.iter().flat_map(|strip| strip.inserts.iter().map(|insert| insert.key.clone())).collect();
        let Some(inserts) = routing.get_mut("inserts").and_then(Value::as_array_mut) else { return };
        for (insert, key) in inserts.iter_mut().zip(&keys) {
            let Some(hosted) = manager.hosted(key) else { continue };
            insert["format"] = json!(hosted.identity.format_label());
            if let Some(reason) = hosted.lost() {
                insert["available"] = json!(false);
                insert["lost"] = json!(true);
                insert["reason"] = json!(reason);
            }
        }
    }

    /// `engine.snapshot` for one plug-in descriptor: its state read from its instance now
    /// (`stateError` when that fails), at its base values when the song does not play. A descriptor without a hosted instance (not installed,
    /// held back) keeps exactly what it carries; a lost instance gives the last state known.
    pub(super) fn capture_plugin_state(&self, descriptor: &PluginDescriptor, item: &mut Value) {
        let Some(manager) = self.plugins.get() else { return };
        let owner = descriptor.owner.as_deref().unwrap_or_default();
        let instrument = descriptor.kind() == PluginKind::Instrument;
        let Some(hosted) = manager.find(owner, instrument, descriptor.slot().max(0) as usize, descriptor.instance_id.as_deref()) else { return };
        // While the song does not play, the state is the patch as the user made it: what the
        // last lanes and step locks left in the plug-in goes back to its base values first.
        // (While it plays, the state has the values of that moment.) A Stop the audio thread
        // has not applied yet counts as stopped: a save that follows a Stop at once waits
        // for it, briefly, and gives up when nobody renders.
        let status = || self.lock().core.status.status();
        if !status().transport_playing() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
            while status().playing != 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if status().playing == 0 {
                hosted.return_to_base();
            }
        }
        match hosted.snapshot_state() {
            Ok(Some(state)) => {
                item["state"] = json!(state);
                if let Some(object) = item.as_object_mut() {
                    object.remove("stateError");
                }
            }
            Ok(None) => {}
            Err(error) => item["stateError"] = json!(error),
        }
        let sample_rate = self.sample_rate().max(1.0);
        item["latency"] = json!(hosted.latency() as f64 / sample_rate);
        item["tailTime"] = json!(hosted.tail_seconds(sample_rate));
    }

    /// The keys of the processors that render one descriptor: one for a channel's insert or
    /// instrument, one per playing track for an effect of an instrument's chain.
    pub fn insert_keys(&self, owner: &str, instrument: bool, slot: usize, instance_id: Option<&str>) -> Vec<String> {
        let state = self.lock();
        let Some(project) = &state.project else { return Vec::new() };
        let chain = owner.starts_with("instrument:");
        let tracks = project.track_list();
        let mut keys = Vec::new();
        for strip in &state.core.spec.strips {
            let plays = match strip.kind {
                StripKind::Track(index) => tracks.get(index).is_some_and(|track| format!("instrument:{}", track.instrument().max(0)) == owner),
                _ => false,
            };
            for insert in &strip.inserts {
                let matches = match instance_id.filter(|id| !id.is_empty()) {
                    Some(id) => insert.instance_id == id,
                    None if chain => insert.instrument_chain && plays && insert.slot == slot && !instrument,
                    None => !insert.instrument_chain && strip.id == owner && insert.instrument.is_some() == instrument && (instrument || insert.slot == slot),
                };
                if matches {
                    keys.push(insert.key.clone());
                }
            }
        }
        keys
    }

    /// The parameters of a built-in effect insert with their current values, as
    /// `plugin.parameters` lists them; `None` when `key` is not a built-in effect of the mixer.
    pub fn builtin_parameters(&self, key: &str) -> Option<Vec<Value>> {
        use tracker_engine::effects::{self, capi, common};
        let state = self.lock();
        let insert = state.core.spec.strips.iter().flat_map(|strip| &strip.inserts).find(|insert| insert.key == key)?;
        let descriptor = std::ffi::CString::new(insert.builtin_id()?).ok().and_then(|id| effects::find(&id))?;
        let values = state.core.compiler.insert_values(insert);
        let text = |value: &std::ffi::CStr| value.to_string_lossy().into_owned();
        Some(
            descriptor
                .params
                .iter()
                .enumerate()
                .map(|(address, info)| {
                    let normalized = values.iter().find(|(known, _)| *known == address as u64).map_or_else(|| capi::normalized(info, info.default_value), |(_, value)| *value);
                    let labels: Vec<String> = (0..info.label_count()).filter_map(|index| info.label(index).map(text)).collect();
                    let steps = if info.flags & common::PARAMETER_TOGGLE != 0 { 1 } else if !labels.is_empty() { labels.len() as i64 - 1 } else if info.flags & common::PARAMETER_INTEGER != 0 { (info.maximum - info.minimum).round() as i64 } else { 0 };
                    let mut item = json!({
                        "address": address.to_string(), "id": text(info.id()), "name": text(info.name()), "min": info.minimum, "max": info.maximum,
                        "value": capi::physical(info, normalized), "normalized": normalized, "unit": 0, "unitName": text(info.unit()), "display": "",
                        "flags": info.flags, "writable": true, "automatable": true, "defaultValue": capi::normalized(info, info.default_value),
                    });
                    if steps > 0 {
                        item["stepCount"] = json!(steps);
                        item["normalizedStep"] = json!(1.0 / steps as f64);
                        if labels.len() as i64 == steps + 1 {
                            item["valueStrings"] = json!(labels);
                        }
                    }
                    item
                })
                .collect(),
        )
    }

    /// Forgets the values set live on an insert: its descriptor's state counts again.
    pub fn forget_insert_values(&self, key: &str) {
        self.lock().core.compiler.clear_live_values_of(key);
    }

    /// `engine.snapshot` for the one descriptor `chosen` picks (by its index and itself): only
    /// that plug-in is asked for its state.
    pub fn snapshot_one(&self, chosen: impl Fn(usize, &PluginDescriptor) -> bool) -> Option<Value> {
        let (mut value, project) = self.snapshot_builtin();
        let project = project?;
        let (index, descriptor) = project.plugin_list().iter().enumerate().find(|(index, descriptor)| chosen(*index, descriptor))?;
        let mut item = value.get_mut("audioPlugins")?.get_mut(index)?.take();
        if !descriptor.component.as_deref().is_some_and(|component| component.starts_with("stfx:") || component.starts_with("builtin:")) {
            self.capture_plugin_state(descriptor, &mut item);
        }
        Some(item)
    }
}

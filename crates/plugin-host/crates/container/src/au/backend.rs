//! The `"au"` backend (macOS only): [`AuBackend`] creates [`AuPlugin`]s, which implement
//! the runtime's `Plugin` trait on top of `AUAudioUnit`. Everything here runs on the main
//! thread; the audio half is in [`super::render`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, Message};
use objc2_audio_toolbox::{AUAudioUnit, AUAudioUnitPreset, AUParameter, AUParameterGroup};
use objc2_foundation::{NSDictionary, NSString};
use stpc_runtime::{
    BusInfo, CreateRequest, EditorInfo, HostHandle, InstanceInfo, Notification, ParameterInfo, Plugin, PluginBackend,
    PluginError, PresetInfo, Processor, INFO_HAS_BYPASS, INFO_HAS_EDITOR, INFO_INSTRUMENT, INFO_PRODUCES_MIDI,
    INFO_SUSTAIN_PEDAL, INFO_WANTS_MIDI,
};

use super::editor::Editor;
use super::ident::{self, ComponentId, ComponentKind};
use super::layout::ChannelMap;
use super::objc::{describe_error, guard, guard_quiet, ns};
use super::observe::{Observer, TreeObservation, CHANGED_ALL_VALUES, CHANGED_EDITOR_SIZE};
use super::params::{self, RawParameter};
use super::plist;
use super::render::{
    AuProcessor, MidiOutFn, MidiOutShared, MusicalContextFn, ProcessorSetup, RenderDiagnostics, TransportShared,
    TransportStateFn,
};
use super::state::{self, Incoming, PayloadKind};
use super::timing::Reported;
use super::unit::{self, BusLayout, Options, V2};

/// Deepest parameter group nesting that is followed.
const MAX_GROUP_DEPTH: usize = 64;

/// The plug-in format backend for Audio Units. `plugin_id` is
/// `type:subtype:manufacturer` (see [`super::ident`]); `options` see [`Options`].
#[derive(Default)]
pub struct AuBackend;

impl AuBackend {
    pub fn new() -> AuBackend {
        AuBackend
    }
}

impl PluginBackend for AuBackend {
    fn format(&self) -> &str {
        "au"
    }

    fn create(&self, request: &CreateRequest<'_>, host: HostHandle) -> Result<Box<dyn Plugin>, PluginError> {
        if MainThreadMarker::new().is_none() {
            return Err(PluginError::failed("Audio Units can only be opened on the main thread."));
        }
        let id = ComponentId::parse(request.plugin_id).map_err(|error| PluginError::failed(error.to_string()))?;
        let kind = id.component_kind().ok_or_else(|| {
            PluginError::failed(format!("{id} is not an instrument, effect or MIDI effect Audio Unit."))
        })?;
        if !unit::is_installed(id) {
            return Err(PluginError::failed(format!("The Audio Unit is not installed: {id}")));
        }
        let au = unit::instantiate(id, Options::parse(request.options))?;
        Ok(Box::new(AuPlugin::new(au, id, kind, request, host)?))
    }
}

/// Parameter metadata of the current tree, built on demand and dropped when the tree is
/// replaced.
struct ParameterCache {
    infos: Vec<ParameterInfo>,
    by_address: HashMap<u64, Retained<AUParameter>>,
    ranges: HashMap<u64, (f32, f32)>,
    /// Sorted addresses of rampable parameters.
    rampable: Vec<u64>,
    /// Global-scope parameters of a version 2 unit (their address is the parameter id).
    v2_global: HashSet<u64>,
}

struct AuPlugin {
    unit: Retained<AUAudioUnit>,
    id: ComponentId,
    kind: ComponentKind,
    host: HostHandle,
    v2: Option<V2>,
    name: String,
    manufacturer: String,
    version: String,
    takes_midi: bool,
    bypass_supported: bool,

    host_in: Vec<u32>,
    host_out: Vec<u32>,
    sample_rate: f64,
    max_frames: u32,
    layout: BusLayout,
    /// False while the unit's bus formats may not match `layout` (a reconfiguration for
    /// a new sample rate failed half-way).
    configured: bool,
    allocated: bool,
    offline: bool,

    transport: Arc<TransportShared>,
    midi_out: Option<Arc<MidiOutShared>>,
    diagnostics: Arc<RenderDiagnostics>,
    reported_failures: u32,
    // The unit copies these when they are set; kept so that their lifetime is plain.
    _musical_context: RcBlock<MusicalContextFn>,
    _transport_state: RcBlock<TransportStateFn>,
    _midi_out_block: Option<RcBlock<MidiOutFn>>,

    observer: Retained<Observer>,
    tree: Option<TreeObservation>,
    cache: Option<ParameterCache>,
    /// Default per address: stated by the unit, else the value first seen.
    defaults: HashMap<u64, f32>,
    reported: Reported,
    reuse: state::Reuse,
    editor: Option<Editor>,
}

fn collect_parameters(
    group: &AUParameterGroup,
    path: &mut Vec<String>,
    depth: usize,
    out: &mut Vec<(Retained<AUParameter>, Vec<String>)>,
) {
    if depth > MAX_GROUP_DEPTH {
        return;
    }
    // SAFETY: property reads on the main thread.
    let children = unsafe { group.children() };
    for index in 0..children.count() {
        let node = children.objectAtIndex(index);
        let object: &AnyObject = &node;
        if let Some(parameter) = object.downcast_ref::<AUParameter>() {
            out.push((parameter.retain(), path.clone()));
        } else if let Some(subgroup) = object.downcast_ref::<AUParameterGroup>() {
            // SAFETY: property read.
            path.push(unsafe { subgroup.displayName() }.to_string());
            collect_parameters(subgroup, path, depth + 1, out);
            path.pop();
        }
    }
}

/// Display text of a value. A real pointer is always passed: the bridge forwards it to
/// version 2 plug-ins, some of which (JUCE's among them) dereference it although the API
/// allows nil for "the current value".
fn display_value(parameter: &AUParameter, value: f32) -> String {
    // SAFETY: `value` outlives the call.
    unsafe { parameter.stringFromValue(&value) }.to_string()
}

impl AuPlugin {
    fn new(
        unit: Retained<AUAudioUnit>,
        id: ComponentId,
        kind: ComponentKind,
        request: &CreateRequest<'_>,
        host: HostHandle,
    ) -> Result<AuPlugin, PluginError> {
        let v2 = V2::of(&unit);
        let transport = Arc::new(TransportShared::default());
        transport.set_sample_rate(request.sample_rate);
        let musical_context = transport.musical_context_block();
        let transport_state = transport.transport_state_block();
        // The host blocks go in before render resources are ever allocated: a unit may
        // capture them at that point.
        // SAFETY: main thread; the blocks are heap blocks the unit copies.
        let produces_midi = guard("receiving the host callbacks", || unsafe {
            unit.setMusicalContextBlock(RcBlock::as_ptr(&musical_context));
            unit.setTransportStateBlock(RcBlock::as_ptr(&transport_state));
            unit.MIDIOutputNames().count() > 0
        })?;
        let midi_out = produces_midi.then(|| MidiOutShared::new(request.out_event_capacity as usize));
        let midi_out_block = midi_out.as_ref().map(|shared| shared.block());
        if let Some(block) = &midi_out_block {
            // SAFETY: as above.
            guard("receiving the MIDI output callback", || unsafe {
                unit.setMIDIOutputEventBlock(RcBlock::as_ptr(block));
            })?;
        }

        let layout = unit::configure(
            &unit,
            kind,
            request.sample_rate,
            request.max_frames,
            request.input_buses,
            request.output_buses,
        )?;

        // SAFETY: property reads.
        let (name, manufacturer, version, takes_midi) = guard("describing itself", || unsafe {
            let name = unit
                .audioUnitName()
                .or_else(|| unit.componentName())
                .map(|name| name.to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| request.name.to_string());
            let manufacturer = unit.manufacturerName().map(|name| name.to_string()).unwrap_or_default();
            let version = ident::version_string(unit.componentVersion());
            (name, manufacturer, version, !unit.scheduleMIDIEventBlock().is_null())
        })?;
        let is_effect = matches!(kind, ComponentKind::Effect | ComponentKind::MusicEffect);
        let bypass_supported = is_effect && v2.is_none_or(|v2| v2.supports_bypass());

        let observer = Observer::new(host.clone());
        observer.watch_unit(&unit);
        let mut plugin = AuPlugin {
            unit,
            id,
            kind,
            host,
            v2,
            name,
            manufacturer,
            version,
            takes_midi,
            bypass_supported,
            host_in: request.input_buses.to_vec(),
            host_out: request.output_buses.to_vec(),
            sample_rate: request.sample_rate,
            max_frames: request.max_frames,
            layout,
            configured: true,
            allocated: false,
            offline: false,
            transport,
            midi_out,
            diagnostics: Arc::new(RenderDiagnostics::default()),
            reported_failures: 0,
            _musical_context: musical_context,
            _transport_state: transport_state,
            _midi_out_block: midi_out_block,
            observer,
            tree: None,
            cache: None,
            defaults: HashMap::new(),
            reported: Reported { latency: 0, tail: 0 },
            reuse: state::Reuse::default(),
            editor: None,
        };
        plugin.sync_tree(false);
        // Records the values of a fresh instance as defaults where the unit states none.
        plugin.cache();
        let (latency, tail) = plugin.read_timing();
        plugin.reported = Reported::new(latency, tail, plugin.sample_rate);
        Ok(plugin)
    }

    // ---- parameters -----------------------------------------------------------------------

    /// Follows a replaced parameter tree: moves the observers over and forgets the cached
    /// metadata. Returns whether the tree changed.
    fn sync_tree(&mut self, notify: bool) -> bool {
        // SAFETY: property read on the main thread.
        let current = guard_quiet("providing its parameters", || unsafe { self.unit.parameterTree() }).flatten();
        let same = match (&self.tree, &current) {
            (Some(observation), Some(tree)) => observation.is_for(Some(tree)),
            (None, None) => true,
            _ => false,
        };
        if same {
            return false;
        }
        // Dropping the observation removes its observers from the old tree.
        self.tree = None;
        self.cache = None;
        if let Some(tree) = current {
            self.tree = Some(TreeObservation::install(tree, &self.host));
        }
        if notify {
            self.host.notify(Notification::ParamsRescan);
        }
        true
    }

    fn build_cache(&mut self) -> ParameterCache {
        let mut cache = ParameterCache {
            infos: Vec::new(),
            by_address: HashMap::new(),
            ranges: HashMap::new(),
            rampable: Vec::new(),
            v2_global: HashSet::new(),
        };
        let Some(observation) = &self.tree else { return cache };
        let tree = observation.tree();
        if let Some(v2) = &self.v2 {
            cache.v2_global = v2.global_parameters().into_iter().map(u64::from).collect();
        }
        let mut found = Vec::new();
        guard_quiet("listing its parameters", || {
            collect_parameters(tree, &mut Vec::new(), 0, &mut found);
            if found.is_empty() {
                // A tree whose nodes are neither groups nor parameters: take the flat list.
                // SAFETY: property read.
                let all = unsafe { tree.allParameters() };
                found.extend((0..all.count()).map(|index| (all.objectAtIndex(index), Vec::new())));
            }
        });
        let mut raws = Vec::with_capacity(found.len());
        for (parameter, groups) in &found {
            // One misbehaving parameter must not cost the whole list.
            // SAFETY: property reads on the main thread.
            let raw = guard_quiet("describing a parameter", || unsafe {
                let address = parameter.address();
                let known = self.defaults.get(&address).copied();
                let default = known.or_else(|| {
                    let stated = match &self.v2 {
                        Some(v2) if cache.v2_global.contains(&address) => v2.parameter_default(address as u32),
                        _ => None,
                    };
                    stated.or_else(|| Some(parameter.value()).filter(|value| value.is_finite()))
                });
                let mut raw = RawParameter {
                    address,
                    name: parameter.displayName().to_string(),
                    short_name: parameter.displayNameWithLength(8).to_string(),
                    key_path: parameter.keyPath().to_string(),
                    groups: groups.clone(),
                    unit: parameter.unit().0,
                    unit_name: parameter.unitName().map(|name| name.to_string()),
                    options: parameter.flags().0,
                    min: parameter.minValue(),
                    max: parameter.maxValue(),
                    default,
                    value_strings: parameter
                        .valueStrings()
                        .map(|labels| labels.iter().map(|label| label.to_string()).collect())
                        .unwrap_or_default(),
                };
                if let Some(steps) = params::should_enumerate_labels(&raw) {
                    raw.value_strings =
                        (0..=steps).map(|step| display_value(parameter, raw.min + step as f32)).collect();
                }
                raw
            });
            if let Some(raw) = raw {
                raws.push(raw);
            }
        }
        for (parameter, _) in found {
            // SAFETY: property read.
            if let Some(address) = guard_quiet("describing a parameter", || unsafe { parameter.address() }) {
                cache.by_address.entry(address).or_insert(parameter);
            }
        }
        for raw in params::dedup_by_address(raws) {
            if let Some(default) = raw.default {
                self.defaults.entry(raw.address).or_insert(default);
            }
            cache.ranges.insert(raw.address, (raw.min, raw.max));
            if params::can_ramp(raw.options) {
                cache.rampable.push(raw.address);
            }
            cache.infos.push(params::parameter_info(&raw));
        }
        cache.rampable.sort_unstable();
        cache
    }

    fn cache(&mut self) -> &ParameterCache {
        if self.cache.is_none() {
            let cache = self.build_cache();
            self.cache = Some(cache);
        }
        self.cache.as_ref().unwrap()
    }

    fn parameter(&mut self, id: u64) -> Option<Retained<AUParameter>> {
        self.cache().by_address.get(&id).cloned()
    }

    // ---- latency, tail ----------------------------------------------------------------------

    fn read_timing(&self) -> (f64, f64) {
        // SAFETY: property reads on the main thread.
        guard_quiet("reporting its latency", || unsafe { (self.unit.latency(), self.unit.tailTime()) })
            .unwrap_or((0.0, 0.0))
    }

    /// Tells the host if latency or tail differ from what it was last told.
    fn check_timing(&mut self) {
        let (latency, tail) = self.read_timing();
        let changes = self.reported.update(latency, tail, self.sample_rate);
        if let Some(frames) = changes.latency {
            self.host.notify(Notification::LatencyChanged { frames });
        }
        if let Some(frames) = changes.tail {
            self.host.notify(Notification::TailChanged { frames });
        }
    }

    /// After a state or preset load: values, the tree and the latency may all be new.
    fn after_state_change(&mut self) {
        if let Some(v2) = &self.v2 {
            // The unit's own Cocoa view listens for this to refresh itself.
            guard_quiet("refreshing its editor", || v2.notify_all_parameters_changed());
        }
        self.sync_tree(true);
        self.check_timing();
    }

    // ---- buses ----------------------------------------------------------------------------

    fn configure(&mut self, sample_rate: f64, max_frames: u32) -> Result<(), PluginError> {
        self.configured = false;
        self.layout = unit::configure(&self.unit, self.kind, sample_rate, max_frames, &self.host_in, &self.host_out)?;
        self.configured = true;
        self.sample_rate = sample_rate;
        self.max_frames = max_frames;
        self.transport.set_sample_rate(sample_rate);
        Ok(())
    }

    fn bus_infos(&self, input: bool) -> Vec<BusInfo> {
        let (host, unit) =
            if input { (&self.host_in, &self.layout.inputs) } else { (&self.host_out, &self.layout.outputs) };
        host.iter()
            .enumerate()
            .map(|(index, &channels)| match unit.get(index) {
                Some(bus) => BusInfo {
                    name: if !bus.name.is_empty() {
                        bus.name.clone()
                    } else if index == 0 {
                        if input {
                            "Input".into()
                        } else {
                            "Output".into()
                        }
                    } else if input {
                        format!("Sidechain {index}")
                    } else {
                        format!("Output {}", index + 1)
                    },
                    channels: ChannelMap::new(channels, bus.channels).used_host_channels(),
                    auxiliary: index > 0,
                },
                None => BusInfo {
                    name: if input { "Unused input".into() } else { "Unused output".into() },
                    channels: 0,
                    auxiliary: index > 0,
                },
            })
            .collect()
    }

    fn find_preset(&self, number: i64) -> Option<Retained<AUAudioUnitPreset>> {
        // SAFETY: property reads on the main thread.
        guard_quiet("listing its presets", || unsafe {
            let search = |presets: Retained<objc2_foundation::NSArray<AUAudioUnitPreset>>| {
                (0..presets.count()).map(|index| presets.objectAtIndex(index)).find(|p| p.number() as i64 == number)
            };
            if number >= 0 {
                self.unit.factoryPresets().and_then(search)
            } else if self.unit.supportsUserPresets() {
                search(self.unit.userPresets())
            } else {
                None
            }
        })
        .flatten()
    }
}

impl Plugin for AuPlugin {
    fn info(&mut self) -> InstanceInfo {
        let mut flags = INFO_HAS_EDITOR; // the unit's own view, or the generic editor
        if self.kind.is_source() {
            flags |= INFO_INSTRUMENT;
        }
        if self.takes_midi {
            // An Audio Unit gets MIDI bytes, the pedal among them.
            flags |= INFO_WANTS_MIDI | INFO_SUSTAIN_PEDAL;
        }
        if self.midi_out.is_some() {
            flags |= INFO_PRODUCES_MIDI;
        }
        if self.bypass_supported {
            flags |= INFO_HAS_BYPASS;
        }
        // The reply carries the current values, so they count as told.
        let (latency, tail) = self.read_timing();
        self.reported = Reported::new(latency, tail, self.sample_rate);
        InstanceInfo {
            name: self.name.clone(),
            manufacturer: self.manufacturer.clone(),
            version: self.version.clone(),
            flags,
            latency: self.reported.latency,
            tail: self.reported.tail,
            inputs: self.bus_infos(true),
            outputs: self.bus_infos(false),
            parameter_count: self.cache().infos.len() as u32,
        }
    }

    fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<Box<dyn Processor>, PluginError> {
        if self.allocated {
            // SAFETY: processing is excluded during `activate`.
            guard("releasing its render resources", || unsafe { self.unit.deallocateRenderResources() })?;
            self.allocated = false;
        }
        if sample_rate != self.sample_rate || !self.configured {
            self.configure(sample_rate, max_frames)?;
        }
        self.max_frames = max_frames;
        // SAFETY: main thread, processing excluded. The order is the documented one: set
        // the frame limit and the offline flag, fetch the render and schedule blocks,
        // then allocate.
        let (render, schedule_parameter, schedule_midi, cables, max_render) =
            guard("preparing to process audio", || unsafe {
                self.unit.setMaximumFramesToRender(max_frames.max(1));
                self.unit.setRenderingOffline(self.offline);
                let render = RcBlock::copy(self.unit.renderBlock());
                let schedule_parameter = RcBlock::copy(self.unit.scheduleParameterBlock());
                let schedule_midi = RcBlock::copy(self.unit.scheduleMIDIEventBlock());
                let cables = self.unit.virtualMIDICableCount().clamp(1, 255) as u8;
                let allocation =
                    self.unit.allocateRenderResourcesAndReturnError().map_err(|error| describe_error(&error));
                let max_render = self.unit.maximumFramesToRender();
                allocation.map(|()| (render, schedule_parameter, schedule_midi, cables, max_render))
            })?
            .map_err(|reason| {
                PluginError::failed(format!("{} could not start audio processing: {reason}", self.name))
            })?;
        self.allocated = true;
        let Some(render) = render else {
            // SAFETY: as above.
            guard_quiet("releasing its render resources", || unsafe { self.unit.deallocateRenderResources() });
            self.allocated = false;
            return Err(PluginError::failed(format!("{} has no render block.", self.name)));
        };

        let host = |buses: &[u32], index: usize| buses.get(index).copied().unwrap_or(0);
        let inputs: Vec<ChannelMap> = self
            .layout
            .inputs
            .iter()
            .enumerate()
            .map(|(index, bus)| ChannelMap::new(host(&self.host_in, index), bus.channels))
            .collect();
        // Bus 0 is always rendered (it drives the unit); further output buses only where
        // the host has a bus for them.
        let outputs: Vec<(usize, ChannelMap)> = self
            .layout
            .outputs
            .iter()
            .enumerate()
            .filter(|(index, bus)| *index == 0 || (bus.channels > 0 && host(&self.host_out, *index) > 0))
            .map(|(index, bus)| (index, ChannelMap::new(host(&self.host_out, index), bus.channels)))
            .collect();
        let rampable = self.cache().rampable.clone();
        let processor = AuProcessor::new(ProcessorSetup {
            unit: Retained::as_ptr(&self.unit),
            render,
            schedule_parameter,
            schedule_midi,
            cables,
            immediate_events: std::env::var("STPC_AU_EVENT_TIME").is_ok_and(|mode| mode == "immediate"),
            rampable,
            inputs,
            outputs,
            max_frames: max_frames as usize,
            max_render: max_render.max(1),
            transport: self.transport.clone(),
            midi_out: self.midi_out.clone(),
            diagnostics: self.diagnostics.clone(),
        });
        // Many units know their latency only once they are initialised.
        self.check_timing();
        Ok(Box::new(processor))
    }

    fn deactivate(&mut self, processor: Box<dyn Processor>) {
        // Main thread: the processor's blocks are released here, not on the audio thread.
        drop(processor);
        if self.allocated {
            // SAFETY: processing is excluded during `deactivate`.
            guard_quiet("releasing its render resources", || unsafe { self.unit.deallocateRenderResources() });
            self.allocated = false;
        }
    }

    fn set_offline(&mut self, offline: bool) {
        if self.offline == offline {
            return;
        }
        self.offline = offline;
        // A version 3 unit reads the flag when it allocates its render resources, so those
        // are cycled. A version 2 unit takes it as a live property (cycling would only
        // reload it).
        let cycle = self.allocated && self.v2.is_none();
        // SAFETY: main thread, processing excluded during `set_offline`. The processor's
        // cached blocks stay valid across the cycle.
        let result = guard("switching its offline rendering mode", || unsafe {
            if cycle {
                self.unit.deallocateRenderResources();
            }
            self.unit.setRenderingOffline(offline);
            if cycle {
                self.unit.allocateRenderResourcesAndReturnError().map_err(|error| describe_error(&error))
            } else {
                Ok(())
            }
        });
        let failure = match result {
            Ok(Ok(())) => return,
            Ok(Err(reason)) => reason,
            Err(error) => error.to_string(),
        };
        eprintln!("[au] {}: {failure}", self.name);
        if cycle {
            // The unit may be left without render resources (blocks then fail and the
            // host plays its fallback): ask for a deactivate / activate cycle, which
            // reports the error properly.
            // SAFETY: property read.
            self.allocated =
                guard_quiet("reporting its state", || unsafe { self.unit.renderResourcesAllocated() }).unwrap_or(false);
            self.host.notify(Notification::RestartRequested);
        }
    }

    fn set_bypass(&mut self, bypass: bool) -> Result<(), PluginError> {
        if !self.bypass_supported {
            return Err(PluginError::Unsupported);
        }
        // SAFETY: property accessors on the main thread.
        let now = guard("being bypassed", || unsafe {
            self.unit.setShouldBypassEffect(bypass);
            self.unit.shouldBypassEffect()
        })?;
        if now != bypass {
            // The unit did not take it: let the host bypass by dry pass-through.
            return Err(PluginError::Unsupported);
        }
        self.check_timing();
        Ok(())
    }

    fn parameters(&mut self) -> Vec<ParameterInfo> {
        self.sync_tree(false);
        self.cache().infos.clone()
    }

    fn parameter_value(&mut self, id: u64) -> Option<f64> {
        let parameter = self.parameter(id)?;
        // Version 2: the unit's own value where it reads back unambiguously (the bridge's
        // cached value can be stale indefinitely, see `V2::parameter_value`).
        if let Some(v2) = self.v2 {
            if self.cache().v2_global.contains(&id) {
                if let Some(value) = v2.parameter_value(id as u32) {
                    return Some(value as f64);
                }
            }
        }
        // SAFETY: property read.
        let value = guard_quiet("reporting a parameter value", || unsafe { parameter.value() })?;
        value.is_finite().then_some(value as f64)
    }

    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), PluginError> {
        let parameter =
            self.parameter(id).ok_or_else(|| PluginError::failed("The Audio Unit parameter does not exist."))?;
        let (min, max) = self.cache().ranges.get(&id).copied().unwrap_or((f32::NEG_INFINITY, f32::INFINITY));
        let value = params::sanitize_value(value, min, max)
            .ok_or_else(|| PluginError::failed("Parameter value must be finite."))?;
        // The observer token as originator keeps this write from coming back as an edit
        // "made in the plug-in". A unit may apply the write only on its next render; a
        // value read back before that can still be the old one.
        let originator = self.tree.as_ref().map_or(std::ptr::null_mut(), |tree| tree.originator());
        // SAFETY: `setValue:originator:` is the host's way to set a parameter, on any thread.
        guard("changing a parameter", || unsafe { parameter.setValue_originator(value, originator) })
    }

    fn format_parameter(&mut self, id: u64, value: f64) -> Option<String> {
        let parameter = self.parameter(id)?;
        if !value.is_finite() {
            return None;
        }
        guard_quiet("formatting a parameter value", || display_value(&parameter, value as f32))
    }

    fn parse_parameter(&mut self, id: u64, text: &str) -> Option<f64> {
        let parameter = self.parameter(id)?;
        // SAFETY: plain method call with a valid string.
        let value = guard_quiet("reading a parameter value", || unsafe { parameter.valueFromString(&ns(text)) })?;
        value.is_finite().then_some(value as f64)
    }

    fn state(&mut self) -> Result<Vec<u8>, PluginError> {
        // The document state where the unit has one, else the plain full state, else an
        // empty dictionary (a unit without state restores to itself).
        // SAFETY: property reads on the main thread.
        let (dictionary, from_full_state) = guard("saving its state", || unsafe {
            match self.unit.fullStateForDocument() {
                Some(dictionary) => (Some(dictionary), false),
                None => (self.unit.fullState(), true),
            }
        })?;
        let dictionary = dictionary.unwrap_or_else(NSDictionary::<NSString, AnyObject>::new);
        let captured = plist::capture(&dictionary).map_err(PluginError::failed)?;
        let mut flags = 0;
        if from_full_state {
            flags |= state::FLAG_FROM_FULL_STATE;
        }
        if captured.pruned {
            flags |= state::FLAG_PRUNED;
        }
        let fresh = state::encode_frame(captured.kind, flags, self.id, &captured.payload)
            .map_err(|error| PluginError::failed(error.to_string()))?;
        // Byte-identical to the previous capture when the state is the same.
        Ok(self.reuse.accept(fresh, captured.semantic))
    }

    fn set_state(&mut self, data: &[u8]) -> Result<(), PluginError> {
        let incoming = state::classify(data, self.id).map_err(|error| PluginError::failed(error.to_string()))?;
        let (decoded, framed) = match incoming {
            Incoming::Framed(frame) => (
                match frame.kind {
                    PayloadKind::BinaryPlist => plist::decode_plist(frame.payload),
                    PayloadKind::KeyedArchive => plist::decode_archive(frame.payload),
                },
                true,
            ),
            Incoming::BarePlist(bytes) => (plist::decode_plist(bytes), false),
        };
        let decoded = decoded.map_err(PluginError::failed)?;
        self.reuse.clear();
        // The setter has no acknowledgement; a unit that ignores or normalises the state
        // shows in the next capture, which always reads the unit's real state.
        // SAFETY: main thread; the dictionary was validated to have string keys.
        guard("loading a saved state", || unsafe { self.unit.setFullStateForDocument(Some(&decoded.dictionary)) })?;
        self.after_state_change();
        if framed {
            self.reuse.seed(data, decoded.semantic);
        }
        Ok(())
    }

    fn presets(&mut self) -> Vec<PresetInfo> {
        // SAFETY: property reads on the main thread.
        guard_quiet("listing its presets", || unsafe {
            let mut list = Vec::new();
            let mut add = |presets: Retained<objc2_foundation::NSArray<AUAudioUnitPreset>>| {
                for index in 0..presets.count() {
                    let preset = presets.objectAtIndex(index);
                    if let Some(id) = ident::preset_id(preset.number() as i64) {
                        list.push(PresetInfo { id, name: preset.name().to_string() });
                    }
                }
            };
            if let Some(factory) = self.unit.factoryPresets() {
                add(factory);
            }
            if self.unit.supportsUserPresets() {
                add(self.unit.userPresets());
            }
            list
        })
        .unwrap_or_default()
    }

    fn load_preset(&mut self, id: u32) -> Result<(), PluginError> {
        let preset = self
            .find_preset(ident::preset_number(id))
            .ok_or_else(|| PluginError::failed("The preset does not exist."))?;
        self.reuse.clear();
        // SAFETY: main thread.
        guard("loading a preset", || unsafe { self.unit.setCurrentPreset(Some(&preset)) })?;
        self.after_state_change();
        Ok(())
    }

    fn open_editor(&mut self, title: &str) -> Result<EditorInfo, PluginError> {
        if let Some(editor) = &self.editor {
            if !editor.user_closed() {
                editor.set_title(title);
                return Ok(editor.info());
            }
        }
        // A window the user closed is dismantled before a new one is made.
        self.editor = None;
        let editor = Editor::open(&self.unit, title, &self.host, &self.observer)?;
        let info = editor.info();
        self.editor = Some(editor);
        Ok(info)
    }

    fn close_editor(&mut self) {
        self.editor = None;
    }

    fn focus_editor(&mut self) {
        if let Some(editor) = self.editor.as_ref().filter(|editor| !editor.user_closed()) {
            editor.focus();
        }
    }

    fn set_editor_title(&mut self, title: &str) {
        if let Some(editor) = &self.editor {
            editor.set_title(title);
        }
    }

    fn idle(&mut self) {
        let changes = self.observer.take_changes();
        // The tree is also compared directly: not every unit announces a replacement.
        let tree_changed = self.sync_tree(true);
        if changes & CHANGED_ALL_VALUES != 0 && !tree_changed {
            self.host.notify(Notification::ParamsRescan);
        }
        // Polled as well as observed: few units post changes of latency and tail.
        self.check_timing();
        if self.editor.as_ref().is_some_and(|editor| editor.user_closed()) {
            self.editor = None;
        } else if changes & CHANGED_EDITOR_SIZE != 0 {
            if let Some(editor) = &self.editor {
                guard_quiet("resizing its editor", || editor.follow_preferred_size());
            }
        }
        let failures = self.diagnostics.failures.load(Ordering::Relaxed);
        if failures != self.reported_failures {
            eprintln!(
                "[au] {}: {} block(s) failed to render, last status {}",
                self.name,
                failures.wrapping_sub(self.reported_failures),
                self.diagnostics.last_status.load(Ordering::Relaxed)
            );
            self.reported_failures = failures;
        }
    }
}

impl Drop for AuPlugin {
    fn drop(&mut self) {
        // Main thread. Observers first, so that nothing calls back into a half-dismantled
        // plug-in; then the render resources; the unit itself is released last.
        self.editor = None;
        self.tree = None;
        self.cache = None;
        self.observer.unwatch_unit(&self.unit);
        // SAFETY: the runtime has stopped processing before it drops a plug-in.
        guard_quiet("being closed", || unsafe {
            if self.unit.renderResourcesAllocated() {
                self.unit.deallocateRenderResources();
            }
            self.unit.setMusicalContextBlock(std::ptr::null_mut());
            self.unit.setTransportStateBlock(std::ptr::null_mut());
            if self.midi_out.is_some() {
                self.unit.setMIDIOutputEventBlock(std::ptr::null_mut());
            }
        });
    }
}

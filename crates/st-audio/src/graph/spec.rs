//! The mixer a project describes, as plain values: strips, inserts, routes, the send effects and
//! the numbering of host automation destinations.
//!
//! [`GraphSpec::from_project`] is the only reader of the project for the graph. Two specs that
//! compare equal render the same, so a publication that changes no mixer value (a note edit)
//! compiles nothing. Every track, group bus, return bus and the master is a strip; a routing
//! table without buses is tracks into the master.

use st_project::model::{AutomationTarget, BusKind, InstrumentKind, Owner, PluginKind, Project, Routing, StepEffectKind, Tap};
use std::collections::HashMap;

/// Insert slots per channel (`st_project::model::plugin::INSERT_SLOTS`).
pub const INSERT_SLOTS: usize = 16;

/// The owner name of the master strip.
pub const MASTER: &str = "master";

#[derive(Debug, Clone, PartialEq)]
pub enum StripKind {
    /// A track's strip, fed by engine channel `index`.
    Track(usize),
    Group,
    Return,
    Master,
}

/// One insert slot of a strip, in signal order.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertSpec {
    /// Identity of the processor instance, unique in the graph. A track's own insert uses its
    /// descriptor's `instanceID`; an instrument's insert runs once per track that plays the
    /// instrument, as `<instanceID>-on-<track routing id>`.
    pub key: String,
    /// The project descriptor's `instanceID` (what step locks and sidechains name).
    pub instance_id: String,
    /// The descriptor's slot, 0..16.
    pub slot: usize,
    /// Part of the assigned instrument's chain (rendered before the track's own inserts).
    pub instrument_chain: bool,
    /// `stfx:<id>` (or `builtin:<id>`) for a built-in effect, else the plug-in's identifier.
    pub component: String,
    pub name: String,
    pub bypass: bool,
    /// The descriptor's saved state.
    pub state: Option<String>,
    /// Set for a track's instrument plug-in: the first processor of the strip, which adds its
    /// output (times `volume`, balanced by `pan`: the instrument's own) to the strip's input
    /// instead of processing it. `slot` is 0 and names no insert slot.
    pub instrument: Option<InstrumentMix>,
}

/// How a plug-in instrument enters its track strip: the instrument's volume (0..2) and pan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstrumentMix {
    pub volume: f32,
    pub pan: f32,
}

impl InsertSpec {
    /// The id of the built-in effect this insert names, if it names one.
    pub fn builtin_id(&self) -> Option<&str> {
        self.component.strip_prefix("stfx:").or_else(|| self.component.strip_prefix("builtin:"))
    }
}

/// A live input mixed into a track strip's input, ahead of its inserts ([`crate::inputs`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LineInputSpec {
    /// Where the render thread reads the input.
    pub feed: FeedHandle,
    /// Gain of the input in the strip, 0..2 (0 while the route is not monitored).
    pub gain: f32,
    /// Frames the input lags the engine: what lies between the engine rendering a frame and a
    /// hardware instrument's answer to it arriving here (the output's latency, the input's,
    /// and the feed's cushion). The mixer compensates for it like for an insert's latency.
    pub latency_frames: u32,
}

/// A [`crate::inputs::LineFeed`] as a spec holds it: two handles are equal when they are the
/// same feed.
#[derive(Clone)]
pub struct FeedHandle(pub std::sync::Arc<crate::inputs::LineFeed>);

impl PartialEq for FeedHandle {
    fn eq(&self, other: &FeedHandle) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for FeedHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "LineFeed@{:p}", std::sync::Arc::as_ptr(&self.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StripSpec {
    /// Owner name: `track:<routingID>`, `bus:<id>` or `master`.
    pub id: String,
    pub name: String,
    pub kind: StripKind,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    /// Owner the strip outputs to; `None` for the master and for a disconnected strip.
    pub output: Option<String>,
    pub inserts: Vec<InsertSpec>,
    /// Tracks: sends to the built-in delay and reverb returns (post-fader).
    pub delay_send: f32,
    pub reverb_send: f32,
    /// Tracks: the live input mixed into the strip. Never set by [`GraphSpec::from_project`]:
    /// which of a project's routes are live is the engine's to say (it knows the input device
    /// in use), so it adds them to the spec it compiles ([`GraphSpec::set_line_input`]).
    pub line_input: Option<LineInputSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RouteDestination {
    /// A send: the input of a strip.
    Strip(String),
    /// A sidechain: the auxiliary input of the insert whose descriptor has this `instanceID`.
    Insert(String),
}

/// A send or a sidechain.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteSpec {
    pub id: String,
    pub source: String,
    pub tap: Tap,
    pub destination: RouteDestination,
    pub gain: f32,
    pub enabled: bool,
}

/// The built-in send effects (`project.delay`, `project.reverb`), in the engine's ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendEffectsSpec {
    pub delay_time: f32,
    pub delay_feedback: f32,
    pub delay_mix: f32,
    /// Tempo-synced delay time in quarter notes; 0 = `delay_time` seconds.
    pub delay_beats: f64,
    pub delay_tone: f32,
    pub reverb_size: f32,
    pub reverb_damping: f32,
    pub reverb_mix: f32,
    pub reverb_predelay: f32,
}

/// What a host automation lane or a plug-in step lock controls. The index of a destination in
/// [`GraphSpec::destinations`] is the number the engine sends back with each value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Destination {
    /// A bus's or the master's fader: address 0 is volume, 1 is pan.
    Mix(String),
    /// The gain of a send or sidechain, by route id (address 0).
    Route(String),
    /// A parameter of an insert, by [`InsertSpec::key`]; the address is the parameter's.
    Insert(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphSpec {
    /// Tracks in project order, then buses in project order, then the master.
    pub strips: Vec<StripSpec>,
    pub sends: Vec<RouteSpec>,
    pub sidechains: Vec<RouteSpec>,
    pub effects: SendEffectsSpec,
    pub bpm: f64,
    pub destinations: Vec<Destination>,
    /// Identity of `destinations`: the engine stamps it on every host automation event.
    pub host_map: u64,
    /// `instrument:<n>` of the chain each track strip carries, by strip id.
    chain_owners: HashMap<String, String>,
}

/// The owner name of track `index`, `track:<routingID>`; empty when the project has no such track.
pub fn track_owner(project: &Project, index: usize) -> String {
    project.track_list().get(index).and_then(|track| track.owner()).unwrap_or_default()
}

fn clamp(value: f64, low: f64, high: f64, fallback: f64) -> f32 {
    (if value.is_finite() { value.clamp(low, high) } else { fallback }) as f32
}

impl GraphSpec {
    /// An empty mixer: just the master.
    pub fn empty() -> GraphSpec {
        GraphSpec::build(&Project::default(), &Routing::empty())
    }

    /// The mixer `project` describes. A project without its routing table, or with a track
    /// that has no `routingID`, describes none (`st_project::validate` refuses both).
    pub fn from_project(project: &Project) -> Result<GraphSpec, String> {
        let Some(routing) = project.routing.as_ref() else { return Err("The project has no routing table.".to_string()) };
        if let Some(index) = project.track_list().iter().position(|track| track.owner().is_none()) {
            return Err(format!("Track {} has no routingID.", index + 1));
        }
        Ok(GraphSpec::build(project, routing))
    }

    fn build(project: &Project, routing: &Routing) -> GraphSpec {
        let tracks = project.track_list();
        let outputs = routing.outputs.as_ref();
        let output_of = |owner: &str| -> Option<String> {
            match outputs.and_then(|outputs| outputs.get(owner)) {
                Some(Some(destination)) => Some(destination.clone()),
                Some(None) => None,
                None => Some(MASTER.to_string()),
            }
        };
        // Inserts by owner text, sorted by slot.
        let mut by_owner: HashMap<String, Vec<&st_project::model::PluginDescriptor>> = HashMap::new();
        for descriptor in project.plugin_list() {
            let (Some(owner), true) = (descriptor.owner.as_deref(), descriptor.kind() == PluginKind::Effect) else { continue };
            if (0..INSERT_SLOTS as i64).contains(&descriptor.slot()) {
                by_owner.entry(owner.to_string()).or_default().push(descriptor);
            }
        }
        for list in by_owner.values_mut() {
            list.sort_by_key(|descriptor| descriptor.slot());
            list.dedup_by_key(|descriptor| descriptor.slot());
        }
        let insert = |descriptor: &st_project::model::PluginDescriptor, owner: &str, suffix: Option<&str>| -> InsertSpec {
            let slot = descriptor.slot() as usize;
            let instance_id = descriptor.instance_id.clone().unwrap_or_default();
            let base = if instance_id.is_empty() { format!("{owner}/effect/{slot}") } else { instance_id.clone() };
            InsertSpec {
                key: match suffix {
                    Some(suffix) => format!("{base}-on-{suffix}"),
                    None => base,
                },
                instance_id,
                slot,
                instrument_chain: suffix.is_some(),
                component: descriptor.component.clone().unwrap_or_default(),
                name: descriptor.name.clone().unwrap_or_default(),
                bypass: descriptor.bypass(),
                state: descriptor.state.clone(),
                instrument: None,
            }
        };
        // A track's instrument plug-in (`kind: "instrument"`, owned by the track).
        let instruments = project.instrument_list();
        let instrument_plugin = |owner: &str, instrument: i64| -> Option<InsertSpec> {
            let descriptor = project.plugin_list().iter().find(|descriptor| {
                descriptor.kind() == PluginKind::Instrument && descriptor.owner.as_deref() == Some(owner) && descriptor.component.as_deref().is_some_and(|component| !component.is_empty())
            })?;
            let item = instruments.get(instrument.max(0) as usize);
            let instance_id = descriptor.instance_id.clone().unwrap_or_default();
            Some(InsertSpec {
                key: if instance_id.is_empty() { format!("{owner}/instrument") } else { instance_id.clone() },
                instance_id,
                slot: 0,
                instrument_chain: false,
                component: descriptor.component.clone().unwrap_or_default(),
                name: descriptor.name.clone().unwrap_or_default(),
                bypass: descriptor.bypass(),
                state: descriptor.state.clone(),
                instrument: Some(InstrumentMix { volume: clamp(item.map_or(0.7, |item| item.volume()), 0.0, 2.0, 0.7), pan: clamp(item.map_or(0.0, |item| item.pan()), -1.0, 1.0, 0.0) }),
            })
        };
        let own_inserts =
            |owner: &str| -> Vec<InsertSpec> { by_owner.get(owner).into_iter().flatten().map(|descriptor| insert(descriptor, owner, None)).collect() };

        let mut strips = Vec::with_capacity(tracks.len() + 4);
        for (index, track) in tracks.iter().enumerate() {
            let id = track_owner(project, index);
            // The assigned instrument's chain runs first, one instance per track.
            let instrument_owner = Owner::Instrument(track.instrument().max(0) as usize).text();
            let suffix = id.strip_prefix("track:").unwrap_or(&id).to_string();
            let mut inserts: Vec<InsertSpec> = instrument_plugin(&id, track.instrument()).into_iter().collect();
            inserts.extend(by_owner.get(&instrument_owner).into_iter().flatten().map(|descriptor| insert(descriptor, &instrument_owner, Some(&suffix))));
            inserts.extend(own_inserts(&id));
            // A frozen track plays its print: its instrument and inserts are in it (`crate::freeze`).
            if track.frozen.as_ref().is_some_and(|record| record.path.as_deref().is_some_and(|path| !path.is_empty())) {
                inserts.clear();
            }
            strips.push(StripSpec {
                name: track.display_name(index),
                kind: StripKind::Track(index),
                volume: clamp(track.volume(), 0.0, 2.0, 0.8),
                pan: clamp(track.pan(), -1.0, 1.0, 0.0),
                mute: track.mute(),
                solo: track.solo(),
                output: output_of(&id),
                inserts,
                delay_send: clamp(track.delay_send(), 0.0, 1.0, 0.0),
                reverb_send: clamp(track.reverb_send(), 0.0, 1.0, 0.0),
                line_input: None,
                id,
            });
        }
        for bus in routing.buses.as_deref().unwrap_or(&[]) {
            let Some(bus_id) = bus.id.as_deref().filter(|id| !id.is_empty()) else { continue };
            let id = format!("bus:{bus_id}");
            strips.push(StripSpec {
                name: bus.name.clone().unwrap_or_else(|| bus_id.to_string()),
                kind: if bus.kind == Some(BusKind::Return) { StripKind::Return } else { StripKind::Group },
                volume: clamp(bus.volume(), 0.0, 2.0, 1.0),
                pan: clamp(bus.pan(), -1.0, 1.0, 0.0),
                mute: bus.mute(),
                solo: bus.solo(),
                output: output_of(&id),
                inserts: own_inserts(&id),
                delay_send: 0.0,
                reverb_send: 0.0,
                line_input: None,
                id,
            });
        }
        strips.push(StripSpec {
            id: MASTER.to_string(),
            name: "Master".to_string(),
            kind: StripKind::Master,
            volume: clamp(project.master_volume(), 0.0, 2.0, 0.7),
            pan: 0.0,
            mute: false,
            solo: false,
            output: None,
            inserts: own_inserts(MASTER),
            delay_send: 0.0,
            reverb_send: 0.0,
            line_input: None,
        });

        let mut sends = Vec::new();
        for send in routing.sends.as_deref().unwrap_or(&[]) {
            let (Some(id), Some(source), Some(destination)) = (send.id.clone(), send.source.clone(), send.destination.clone()) else { continue };
            sends.push(RouteSpec {
                id,
                source,
                tap: send.tap.unwrap_or(Tap::PostFader),
                destination: RouteDestination::Strip(destination),
                gain: clamp(send.gain(), 0.0, 2.0, 1.0),
                enabled: send.enabled(),
            });
        }
        let mut sidechains = Vec::new();
        for sidechain in routing.sidechains.as_deref().unwrap_or(&[]) {
            let (Some(id), Some(source)) = (sidechain.id.clone(), sidechain.source.clone()) else { continue };
            let Some(instance) = sidechain.destination.as_ref().and_then(|destination| destination.instance_id.clone()) else { continue };
            sidechains.push(RouteSpec {
                id,
                source,
                tap: sidechain.tap.unwrap_or(Tap::PostFader),
                destination: RouteDestination::Insert(instance),
                gain: clamp(sidechain.gain(), 0.0, 2.0, 1.0),
                enabled: sidechain.enabled(),
            });
        }
        let delay = project.delay.clone().unwrap_or_default();
        let reverb = project.reverb.clone().unwrap_or_default();
        let beats = delay.beats();
        let effects = SendEffectsSpec {
            delay_time: clamp(delay.time(), 0.001, 3.99, 0.375),
            delay_feedback: clamp(delay.feedback(), 0.0, 0.98, 0.35),
            delay_mix: clamp(delay.mix(), 0.0, 1.0, 0.3),
            delay_beats: if beats.is_finite() && beats > 0.0 { beats.clamp(1.0 / 64.0, 16.0) } else { 0.0 },
            delay_tone: clamp(delay.tone(), 0.0, 1.0, 1.0),
            reverb_size: clamp(reverb.size(), 0.0, 1.0, 0.7),
            reverb_damping: clamp(reverb.damping(), 0.0, 0.99, 0.4),
            reverb_mix: clamp(reverb.mix(), 0.0, 1.0, 0.25),
            reverb_predelay: clamp(reverb.predelay(), 0.0, 0.5, 0.0),
        };
        let mut spec = GraphSpec {
            strips,
            sends,
            sidechains,
            effects,
            bpm: if project.bpm().is_finite() { project.bpm().clamp(20.0, 999.0) } else { 120.0 },
            destinations: Vec::new(),
            host_map: 0,
            chain_owners: HashMap::new(),
        };
        spec.number_destinations(project);
        spec
    }

    /// Gives the strip of track `track` a live input (or takes it away).
    pub fn set_line_input(&mut self, track: usize, input: Option<LineInputSpec>) {
        if let Some(strip) = self.strips.iter_mut().find(|strip| strip.kind == StripKind::Track(track)) {
            strip.line_input = input;
        }
    }

    pub fn strip(&self, owner: &str) -> Option<&StripSpec> {
        self.strips.iter().find(|strip| strip.id == owner)
    }

    /// The number of a destination, if any lane or lock of the project uses it.
    pub fn destination_index(&self, destination: &Destination) -> Option<usize> {
        self.destinations.iter().position(|known| known == destination)
    }

    /// The destinations of one owner lane: `(destination, parameter address)`. A lane of an
    /// instrument's insert has one destination per track that plays the instrument. Empty for
    /// lanes the engine plays itself (track mix and instrument parameters) and for targets that
    /// do not exist (they stay silent rather than land somewhere else).
    pub fn lane_destinations(&self, owner: &str, target: &str) -> Vec<(Destination, u64)> {
        match AutomationTarget::parse(target) {
            Some(AutomationTarget::Send(route)) => {
                let known = self.sends.iter().chain(&self.sidechains).any(|candidate| candidate.id == route);
                if known { vec![(Destination::Route(route), 0)] } else { Vec::new() }
            }
            Some(AutomationTarget::Native(name)) => {
                let address = match name.as_str() {
                    "volume" => 0,
                    "pan" => 1,
                    _ => return Vec::new(),
                };
                match self.strip(owner) {
                    Some(strip) if !matches!(strip.kind, StripKind::Track(_)) => vec![(Destination::Mix(owner.to_string()), address)],
                    _ => Vec::new(),
                }
            }
            // The instrument plug-in of the owner track, or of every track that plays the
            // owner instrument.
            Some(AutomationTarget::Plugin { instrument: true, address, .. }) => {
                let instrument_lane = matches!(Owner::parse(owner), Some(Owner::Instrument(_)));
                self.strips
                    .iter()
                    .filter(|strip| if instrument_lane { self.chain_owner(strip) == owner } else { strip.id == owner })
                    .flat_map(|strip| strip.inserts.iter().filter(|insert| insert.instrument.is_some()))
                    .map(|insert| (Destination::Insert(insert.key.clone()), address))
                    .collect()
            }
            Some(AutomationTarget::Plugin { instrument: false, slot, address }) => {
                let instrument_lane = matches!(Owner::parse(owner), Some(Owner::Instrument(_)));
                let mut found = Vec::new();
                for strip in &self.strips {
                    for insert in strip.inserts.iter().filter(|insert| insert.instrument.is_none()) {
                        let matches = insert.slot as i64 == slot
                            && if instrument_lane { insert.instrument_chain && self.chain_owner(strip) == owner } else { !insert.instrument_chain && strip.id == owner };
                        if matches {
                            found.push((Destination::Insert(insert.key.clone()), address));
                        }
                    }
                }
                found
            }
            _ => Vec::new(),
        }
    }

    /// `instrument:<n>` of the instrument chain a track strip carries (recorded when the spec
    /// was read).
    fn chain_owner<'a>(&'a self, strip: &'a StripSpec) -> &'a str {
        self.chain_owners.get(&strip.id).map(String::as_str).unwrap_or("")
    }

    /// The destination of a plug-in step lock on `track`: the insert of that track (its own or
    /// its instrument's) whose descriptor has `instance_id`. `(destination, slot)`.
    pub fn lock_destination(&self, track: usize, instance_id: &str) -> Option<(Destination, usize)> {
        let strip = self.strips.iter().find(|strip| strip.kind == StripKind::Track(track))?;
        let insert = strip.inserts.iter().find(|insert| !insert.instance_id.is_empty() && insert.instance_id == instance_id)?;
        Some((Destination::Insert(insert.key.clone()), insert.slot))
    }

    fn number_destinations(&mut self, project: &Project) {
        for (index, track) in project.track_list().iter().enumerate() {
            self.chain_owners.insert(track_owner(project, index), Owner::Instrument(track.instrument().max(0) as usize).text());
        }
        let mut destinations: Vec<Destination> = Vec::new();
        let mut add = |destination: Destination| {
            if !destinations.contains(&destination) {
                destinations.push(destination);
            }
        };
        for pattern in project.pattern_list() {
            for lane in pattern.automation.as_deref().unwrap_or(&[]) {
                let Some(owner) = lane.owner.as_deref() else { continue };
                for (destination, _) in self.lane_destinations(owner, lane.target_name()) {
                    add(destination);
                }
            }
            for (track, rows) in pattern.steps.as_deref().unwrap_or(&[]).iter().enumerate() {
                for step in rows.iter().flatten() {
                    for effect in step.effects().filter(|effect| effect.kind == Some(StepEffectKind::Plugin)) {
                        if let Some((destination, _)) = effect.instance_id.as_deref().and_then(|id| self.lock_destination(track, id)) {
                            add(destination);
                        }
                    }
                }
            }
        }
        // FNV-1a over the destination names; never 0 (0 means "no map" to the engine).
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for destination in &destinations {
            for byte in format!("{destination:?}|").bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        self.host_map = hash | 1;
        self.destinations = destinations;
    }
}

/// Whether an instrument is one the engine cannot sound itself (a plug-in or external MIDI).
pub fn is_external(kind: InstrumentKind) -> bool {
    kind != InstrumentKind::Sample
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn spec(project: Value) -> GraphSpec {
        GraphSpec::from_project(&Project::from_value(project).expect("a project")).expect("a mixer")
    }

    fn effect(owner: &str, slot: i64, instance: &str) -> Value {
        json!({ "owner": owner, "kind": "effect", "slot": slot, "instanceID": instance, "component": "stfx:eq" })
    }

    // Ported from tests/MixerChannelTests.swift: a track channel runs its instrument's chain
    // ahead of its own inserts, each in slot order, one instance of the chain per track.
    #[test]
    fn a_track_runs_its_instruments_chain_before_its_own_inserts() {
        let spec = spec(json!({
            "tracks": [{ "routingID": "a", "instrument": 1 }, { "routingID": "b", "instrument": 1 }, { "routingID": "c", "instrument": 0 }],
            "instruments": [{}, {}],
            "routing": {},
            "audioPlugins": [
                effect("track:a", 3, "own3"), effect("track:a", 0, "own0"), effect("instrument:1", 5, "chain5"), effect("instrument:1", 1, "chain1"),
                effect("master", 0, "m"), effect("track:a", 16, "beyond"), { "owner": "track:a", "kind": "instrument", "slot": 0, "instanceID": "synth" },
            ],
        }));
        let keys = |owner: &str| spec.strip(owner).unwrap().inserts.iter().map(|insert| insert.key.as_str()).collect::<Vec<_>>();
        assert_eq!(keys("track:a"), ["chain1-on-a", "chain5-on-a", "own0", "own3"]);
        assert_eq!(keys("track:b"), ["chain1-on-b", "chain5-on-b"]);
        assert!(keys("track:c").is_empty(), "a track that plays another instrument does not get the chain");
        assert_eq!(keys("master"), ["m"]);
        let chain = &spec.strip("track:a").unwrap().inserts[0];
        assert_eq!((chain.instance_id.as_str(), chain.slot, chain.instrument_chain), ("chain1", 1, true));
        assert_eq!(chain.builtin_id(), Some("eq"));
        // An instrument lane reaches every track's instance; a track lane only its own insert.
        assert_eq!(spec.lane_destinations("instrument:1", "plugin:effect:5:7"), [(Destination::Insert("chain5-on-a".into()), 7), (Destination::Insert("chain5-on-b".into()), 7)]);
        assert_eq!(spec.lane_destinations("track:a", "plugin:effect:3:2"), [(Destination::Insert("own3".into()), 2)]);
        assert!(spec.lane_destinations("track:a", "plugin:effect:5:2").is_empty(), "slot 5 of the track itself is empty");
        assert_eq!(spec.lock_destination(0, "chain1"), Some((Destination::Insert("chain1-on-a".into()), 1)));
        assert_eq!(spec.lock_destination(2, "chain1"), None);
    }

    #[test]
    fn strips_are_tracks_then_buses_then_the_master_with_their_outputs() {
        let spec = spec(json!({
            "masterVolume": 0.75,
            "tracks": [{ "routingID": "a", "name": "Drums", "volume": 0.8, "pan": -0.25, "solo": true, "delaySend": 0.3 }, { "routingID": "b", "volume": 1.2 }],
            "routing": {
                "buses": [{ "id": "drums", "name": "Drums", "type": "group", "volume": 1.1, "pan": 0.2 }, { "id": "space", "name": "Space", "type": "return" }],
                "outputs": { "track:a": "bus:drums", "track:b": null },
                "sends": [{ "id": "s", "source": "track:a", "tap": "preFader", "destination": "bus:space", "gain": 0.5 }, { "id": "off", "source": "track:b", "tap": "postFader", "destination": "bus:space", "enabled": false }],
                "sidechains": [{ "id": "k", "source": "track:b", "tap": "postFader", "destination": { "instanceID": "comp", "inputBus": 1 } }],
            },
        }));
        let ids: Vec<&str> = spec.strips.iter().map(|strip| strip.id.as_str()).collect();
        assert_eq!(ids, ["track:a", "track:b", "bus:drums", "bus:space", "master"]);
        let a = spec.strip("track:a").unwrap();
        assert_eq!((a.kind.clone(), a.volume, a.pan, a.solo, a.delay_send, a.output.as_deref()), (StripKind::Track(0), 0.8, -0.25, true, 0.3, Some("bus:drums")));
        assert_eq!(spec.strip("track:b").unwrap().output, None, "an explicit null is a disconnected output");
        assert_eq!(spec.strip("bus:space").unwrap().output.as_deref(), Some("master"), "an owner without an entry outputs to the master");
        assert_eq!((spec.strip("bus:drums").unwrap().kind.clone(), spec.strip("bus:space").unwrap().kind.clone()), (StripKind::Group, StripKind::Return));
        let master = spec.strip("master").unwrap();
        assert_eq!((master.volume, master.output.clone()), (0.75, None));
        assert_eq!((spec.sends.len(), spec.sends[0].tap, spec.sends[0].gain, spec.sends[1].enabled), (2, Tap::PreFader, 0.5, false));
        assert_eq!(spec.sidechains[0].destination, RouteDestination::Insert("comp".into()));
        assert_eq!(spec.lane_destinations("track:a", "send:s"), [(Destination::Route("s".into()), 0)]);
        assert_eq!(spec.lane_destinations("track:b", "send:k"), [(Destination::Route("k".into()), 0)]);
        assert!(spec.lane_destinations("track:a", "send:nothing").is_empty());
        assert_eq!(spec.lane_destinations("bus:drums", "pan"), [(Destination::Mix("bus:drums".into()), 1)]);
        assert!(spec.lane_destinations("track:a", "volume").is_empty(), "track mix is the engine's");
        assert!(spec.lane_destinations("bus:drums", "cutoff").is_empty());
    }

    #[test]
    fn a_routing_table_without_buses_is_tracks_into_the_master() {
        let plain = spec(json!({ "tracks": [{ "routingID": "one", "name": "One" }, { "routingID": "two", "name": "Two", "mute": true }], "routing": {} }));
        let ids: Vec<&str> = plain.strips.iter().map(|strip| strip.id.as_str()).collect();
        assert_eq!(ids, ["track:one", "track:two", "master"]);
        assert!(plain.strips[..2].iter().all(|strip| strip.output.as_deref() == Some("master")));
        assert_eq!((plain.strips[0].volume, plain.strips[1].mute, plain.strip("master").unwrap().volume), (0.8, true, 0.7));
        assert_eq!(GraphSpec::empty().strips.len(), 1);
    }

    #[test]
    fn a_project_without_its_routing_table_or_a_track_identity_describes_no_mixer() {
        let project = |value: Value| GraphSpec::from_project(&Project::from_value(value).expect("a project"));
        assert_eq!(project(json!({ "tracks": [{ "routingID": "a" }] })).unwrap_err(), "The project has no routing table.");
        assert_eq!(project(json!({ "tracks": [{ "routingID": "a" }, { "name": "Two" }], "routing": {} })).unwrap_err(), "Track 2 has no routingID.");
    }

    #[test]
    fn destinations_are_numbered_by_first_use_and_the_map_identity_follows_them() {
        let project = |lanes: Value| {
            json!({
                "tracks": [{ "routingID": "a" }],
                "routing": { "buses": [{ "id": "g", "name": "G", "type": "group" }], "sends": [{ "id": "s", "source": "track:a", "tap": "postFader", "destination": "bus:g" }] },
                "audioPlugins": [effect("track:a", 0, "fx")],
                "patterns": [{ "length": 16, "steps": [[{ "fx1": { "type": "plugin", "instanceID": "fx", "address": "3", "value": 0.5 } }]], "automation": lanes }],
            })
        };
        let lanes = json!([
            { "owner": "bus:g", "target": "volume", "points": [] }, { "owner": "bus:g", "target": "pan", "points": [] },
            { "owner": "master", "target": "volume", "points": [] }, { "owner": "track:a", "target": "send:s", "points": [] },
            { "track": 0, "target": "volume", "points": [] }, { "owner": "track:a", "target": "volume", "points": [] },
        ]);
        let first = spec(project(lanes.clone()));
        // Volume and pan of a strip share one destination; native track lanes have none.
        assert_eq!(first.destinations, [Destination::Mix("bus:g".into()), Destination::Mix("master".into()), Destination::Route("s".into()), Destination::Insert("fx".into())]);
        assert_eq!(first.destination_index(&Destination::Route("s".into())), Some(2));
        assert_ne!(first.host_map, 0);
        assert_eq!(first.host_map, spec(project(lanes)).host_map, "the same lanes give the same map");
        let fewer = spec(project(json!([{ "owner": "master", "target": "volume", "points": [] }])));
        assert_eq!(fewer.destinations, [Destination::Mix("master".into()), Destination::Insert("fx".into())]);
        assert_ne!(first.host_map, fewer.host_map, "another numbering is another map");
    }
}

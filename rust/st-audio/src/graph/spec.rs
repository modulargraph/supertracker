//! The mixer a project describes, as plain values: strips, inserts, routes, the send effects and
//! the numbering of host automation destinations.
//!
//! [`GraphSpec::from_project`] is the only reader of the project for the graph. Two specs that
//! compare equal render the same, so a publication that changes no mixer value (a note edit)
//! compiles nothing. One mix model exists, the standard one: every track, group bus, return bus
//! and the master is a strip; a project without `routing` is the same thing with no buses.

use st_project::model::{AutomationTarget, BusKind, InstrumentKind, Owner, PluginKind, Project, StepEffectKind, Tap};
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
}

impl InsertSpec {
    /// The id of the built-in effect this insert names, if it names one.
    pub fn builtin_id(&self) -> Option<&str> {
        self.component.strip_prefix("stfx:").or_else(|| self.component.strip_prefix("builtin:"))
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

/// The owner name of track `index`.
pub fn track_owner(project: &Project, index: usize) -> String {
    project.track_list().get(index).and_then(|track| track.owner()).unwrap_or_else(|| format!("track:#{index}"))
}

fn clamp(value: f64, low: f64, high: f64, fallback: f64) -> f32 {
    (if value.is_finite() { value.clamp(low, high) } else { fallback }) as f32
}

impl GraphSpec {
    /// An empty mixer: just the master.
    pub fn empty() -> GraphSpec {
        GraphSpec::from_project(&Project::default())
    }

    pub fn from_project(project: &Project) -> GraphSpec {
        let tracks = project.track_list();
        let routing = project.routing.as_ref();
        let outputs = routing.and_then(|routing| routing.outputs.as_ref());
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
            }
        };
        let own_inserts =
            |owner: &str| -> Vec<InsertSpec> { by_owner.get(owner).into_iter().flatten().map(|descriptor| insert(descriptor, owner, None)).collect() };

        let mut strips = Vec::with_capacity(tracks.len() + 4);
        for (index, track) in tracks.iter().enumerate() {
            let id = track_owner(project, index);
            // The assigned instrument's chain runs first, one instance per track.
            let instrument_owner = Owner::Instrument(track.instrument().max(0) as usize).text();
            let suffix = id.strip_prefix("track:").unwrap_or(&id).to_string();
            let mut inserts: Vec<InsertSpec> =
                by_owner.get(&instrument_owner).into_iter().flatten().map(|descriptor| insert(descriptor, &instrument_owner, Some(&suffix))).collect();
            inserts.extend(own_inserts(&id));
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
                id,
            });
        }
        for bus in routing.and_then(|routing| routing.buses.as_deref()).unwrap_or(&[]) {
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
        });

        let mut sends = Vec::new();
        for send in routing.and_then(|routing| routing.sends.as_deref()).unwrap_or(&[]) {
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
        for sidechain in routing.and_then(|routing| routing.sidechains.as_deref()).unwrap_or(&[]) {
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
            Some(AutomationTarget::Plugin { instrument: false, slot, address }) => {
                let instrument_lane = matches!(Owner::parse(owner), Some(Owner::Instrument(_)));
                let mut found = Vec::new();
                for strip in &self.strips {
                    for insert in &strip.inserts {
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

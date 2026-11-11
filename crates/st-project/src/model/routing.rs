//! Mixer routing: buses, channel outputs, sends and sidechains (`project.routing`).
//!
//! Mixer channels ("owners") are named `track:<routingID>`, `bus:<id>` and `master`.

use crate::json::{model, string_enum};
use std::collections::BTreeMap;

string_enum! {
    /// What a bus is for.
    pub enum BusKind {
        /// The bus of a track group.
        Group = "group",
        /// A return bus fed by sends.
        Return = "return",
    }
}

string_enum! {
    /// Where on a channel strip a send or sidechain takes its signal.
    pub enum Tap {
        /// Before the inserts.
        PreInserts = "preInserts",
        /// After the inserts, before the fader.
        PreFader = "preFader",
        /// After the fader.
        PostFader = "postFader",
    }
}

model! {
    /// A group or return bus.
    pub struct Bus {
        /// Identity; the bus's owner name is `bus:<id>`.
        "id" => id: String,
        /// Display name.
        "name" => name: String,
        /// Group or return.
        "type" => kind: BusKind,
        /// The track group a group bus belongs to (`project.groups[].id`).
        "groupID" => group_id: String,
        /// Fader, 0..2.
        "volume" => volume: f64 = 1.0,
        /// Pan, -1..1.
        "pan" => pan: f64 = 0.0,
        /// Muted.
        "mute" => mute: bool = false,
        /// Soloed.
        "solo" => solo: bool = false,
    }
}

model! {
    /// A send from a track or bus to another channel.
    pub struct Send {
        /// Identity (unique among sends and sidechains); automation target `send:<id>`.
        "id" => id: String,
        /// Source owner (a track or bus).
        "source" => source: String,
        /// Where the signal is taken.
        "tap" => tap: Tap,
        /// Destination owner.
        "destination" => destination: String,
        /// Gain, 0..2.
        "gain" => gain: f64 = 1.0,
        /// Whether the send passes signal.
        "enabled" => enabled: bool = true,
    }
}

model! {
    /// The input bus layout a sidechain was connected to, so a changed plug-in is noticed.
    pub struct LayoutSignature {
        /// The input bus's name.
        "name" => name: String,
        /// Its channel count, 1..64.
        "channels" => channels: i64,
    }
}

model! {
    /// Where a sidechain arrives: an auxiliary input bus of a plug-in instance.
    pub struct SidechainDestination {
        /// The plug-in instance (`audioPlugins[].instanceID`).
        "instanceID" => instance_id: String,
        /// Input bus index, 0..31.
        "inputBus" => input_bus: i64,
        /// The bus layout when the sidechain was made.
        "layoutSignature" => layout_signature: LayoutSignature,
    }
}

model! {
    /// A sidechain from a track or bus to a plug-in's auxiliary input.
    pub struct Sidechain {
        /// Identity (unique among sends and sidechains).
        "id" => id: String,
        /// Source owner, `track:<id>` or `bus:<id>`. It may name a channel that no longer exists;
        /// the sidechain is then unavailable, not invalid.
        "source" => source: String,
        /// Where the signal is taken.
        "tap" => tap: Tap,
        /// The plug-in input.
        "destination" => destination: SidechainDestination,
        /// Gain, 0..2.
        "gain" => gain: f64 = 1.0,
        /// Whether the sidechain passes signal.
        "enabled" => enabled: bool = true,
    }
}

model! {
    /// `project.routing`. Every project has one; its format is the project's (`project.version`).
    pub struct Routing {
        /// Group and return buses.
        "buses" => buses: Vec<Bus>,
        /// Output of each track and bus by owner name: another owner, or `null` for disconnected.
        /// An owner without an entry outputs to `master`.
        "outputs" => outputs: BTreeMap<String, Option<String>>,
        /// Sends.
        "sends" => sends: Vec<Send>,
        /// Sidechains.
        "sidechains" => sidechains: Vec<Sidechain>,
    }
}

impl Routing {
    /// The routing of a new project: no buses, sends or sidechains, every track to the master.
    pub fn empty() -> Routing {
        Routing {
            buses: Some(Vec::new()),
            outputs: Some(BTreeMap::new()),
            sends: Some(Vec::new()),
            sidechains: Some(Vec::new()),
            ..Routing::default()
        }
    }
}

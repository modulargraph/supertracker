//! The document inside a kit: a *group project*.
//!
//! A kit is a track group: its member tracks, their instruments, the group bus and the buses
//! it feeds, the plug-ins and built-in effects on those channels and the routes between them.
//! A group project is a small project document holding exactly that:
//!
//! ```json
//! { "version": 2, "name": "My kit", "bpm": 120, "linesPerBeat": 4,
//!   "tracks": [ { "id": 0, "name": "Kick", "routingID": "…", "groupID": "g", "pad": 0, "instrument": 0, … } ],
//!   "instruments": [ { "id": 0, "type": "sample", "samplePath": "…", … } ],
//!   "groups": [ { "id": "g", "name": "My kit", "busID": "b", … } ],
//!   "audioPlugins": [ { "owner": "track:…" | "bus:…" | "instrument:0", … } ],
//!   "routing": { "buses": [ … ], "outputs": { … }, "sends": [ … ], "sidechains": [ … ] },
//!   "pluginPages": { "<instanceID>": { … } } }
//! ```
//!
//! This is what `kit.read` returns as `groupProject` and what the page inserts into a session.

use crate::error::{Error, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// The most member tracks a kit holds.
pub const MAXIMUM_TRACKS: usize = 128;
/// The most instruments or plug-in descriptors a kit document holds.
pub const MAXIMUM_ENTRIES: usize = 1024;
/// The most sample zones of one instrument.
pub const MAXIMUM_ZONES: usize = 128;

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn index_of(value: Option<&Value>) -> Option<usize> {
    let number = value?.as_f64()?;
    (number.is_finite() && number >= 0.0 && number.trunc() == number && number < 1.0e9).then_some(number as usize)
}

/// Checks a group project: the one place a kit document is validated, whether it comes from a
/// `.stkit` package, from a Maschine group or from the session that is being saved. The page
/// inserts what passes without checking it again.
///
/// A group project is complete: `tracks` (1..=128), `instruments`, `audioPlugins`, one group in
/// `groups` with its bus, and the routing table. Every track belongs to the group and plays one
/// of the instruments. Everything else is what [`st_project::validate::validate`] asks of a
/// project (format version, identities, owners, routes, ranges), checked on the document with
/// what a kit does not have put in its place: one empty pattern, and a tempo when the kit names
/// none (a Maschine group has none).
pub fn validate(project: &Value) -> Result<()> {
    let Some(object) = project.as_object() else { return Err(Error::invalid("The kit document must be a JSON object.")) };
    let array = |key: &str, limit: usize, message: &str| -> Result<&[Value]> {
        match object.get(key) {
            Some(Value::Array(items)) if items.len() <= limit && items.iter().all(Value::is_object) => Ok(items),
            Some(Value::Array(_)) => Err(Error::invalid(message)),
            _ => Err(Error::invalid(format!("The kit document has no {key} list."))),
        }
    };
    let tracks = array("tracks", MAXIMUM_TRACKS, "A kit holds up to 128 tracks.")?;
    let instruments = array("instruments", MAXIMUM_ENTRIES, "Too many instruments in this kit document.")?;
    array("audioPlugins", MAXIMUM_ENTRIES, "Too many plug-in states in this kit document.")?;
    let groups = array("groups", 1, "A kit document holds one group.")?;
    let (Some(group), false) = (groups.first(), tracks.is_empty()) else { return Err(Error::invalid("A kit document needs its group and at least one track.")) };
    let id = text(group, "id").filter(|id| !id.is_empty()).ok_or_else(|| Error::invalid("The kit's group needs an ID."))?;
    let bus = text(group, "busID").filter(|bus| !bus.is_empty()).ok_or_else(|| Error::invalid("The kit's group needs its bus."))?;
    for track in tracks {
        if text(track, "groupID") != Some(id) {
            return Err(Error::invalid("Every track of a kit belongs to its group."));
        }
        if index_of(track.get("instrument")).is_none_or(|index| index >= instruments.len()) {
            return Err(Error::invalid("A kit track refers to an instrument that is missing."));
        }
    }
    let mut typed = st_project::Project::from_value(project.clone()).map_err(|error| Error::invalid(error.to_string()))?;
    if !typed.routing.as_ref().and_then(|routing| routing.buses.as_ref()).is_some_and(|buses| buses.iter().any(|item| item.id.as_deref() == Some(bus))) {
        return Err(Error::invalid("The kit's group bus is missing from its routing."));
    }
    if typed.patterns.is_some() || object.contains_key("patterns") {
        return Err(Error::invalid("A kit document holds no patterns."));
    }
    typed.patterns = Some(vec![st_project::model::Pattern { length: Some(1), steps: Some(vec![Vec::new(); tracks.len()]), ..Default::default() }]);
    if !object.contains_key("bpm") {
        typed.bpm = Some(120.0);
    }
    st_project::validate::validate(&typed).map_err(|error| Error::invalid(format!("The kit cannot be used: {error}")))
}

/// One pad of a kit, for the browser's detail view.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoundSummary {
    /// Pad position (the track's `pad`, or its position among the members).
    pub slot: usize,
    /// The member track's routing identity.
    pub id: String,
    /// Track name.
    pub name: String,
    /// Instrument type (`sample`, `au`, `midi`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Name of the instrument plug-in, or empty.
    pub plugin: String,
    /// A sampler without a sample or zones.
    pub empty: bool,
    /// The instrument's sample file does not exist.
    pub missing: bool,
}

fn member_instrument<'a>(project: &'a Value, track: &Value) -> Option<&'a Value> {
    list(project, "instruments").get(index_of(track.get("instrument")).unwrap_or(0))
}

fn is_empty_sampler(instrument: Option<&Value>) -> bool {
    let Some(instrument) = instrument else { return true };
    text(instrument, "type").unwrap_or("sample") == "sample" && text(instrument, "samplePath").unwrap_or("").is_empty() && list(instrument, "zones").is_empty()
}

/// The pads of a group project.
pub fn sound_summaries(project: &Value) -> Vec<SoundSummary> {
    let plugins = list(project, "audioPlugins");
    list(project, "tracks")
        .iter()
        .enumerate()
        .map(|(index, track)| {
            let instrument = member_instrument(project, track);
            let routing_id = text(track, "routingID").unwrap_or("");
            let owner = format!("track:{routing_id}");
            let plugin = plugins.iter().find(|plugin| text(plugin, "owner") == Some(&owner) && text(plugin, "kind") == Some("instrument"));
            let sample = instrument.and_then(|instrument| text(instrument, "samplePath")).unwrap_or("");
            SoundSummary {
                slot: index_of(track.get("pad")).unwrap_or(index),
                id: routing_id.to_string(),
                name: text(track, "name").map(String::from).unwrap_or_else(|| format!("Sound {}", index + 1)),
                kind: instrument.and_then(|instrument| text(instrument, "type")).unwrap_or("sample").to_string(),
                plugin: plugin.and_then(|plugin| text(plugin, "name")).unwrap_or("").to_string(),
                empty: is_empty_sampler(instrument),
                missing: !sample.is_empty() && !sample.starts_with("builtin:") && !std::path::Path::new(sample).exists(),
            }
        })
        .collect()
}

/// How many pads of a group project make a sound: an empty sampler (no sample and no zones) is
/// not counted, so the browser shows how many pads are actually loaded.
pub fn loaded_sound_count(project: &Value) -> usize {
    list(project, "tracks").iter().filter(|track| !is_empty_sampler(member_instrument(project, track))).count()
}

/// A group taken out of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedGroup {
    /// The group project.
    pub project: Value,
    /// The kit header fields the group implies: `name`, `type`, `tags`, `source`.
    pub metadata: Map<String, Value>,
    /// What had to change to make the group reusable (routes to tracks outside it).
    pub warnings: Vec<String>,
}

/// Takes the group `group_id` out of `project`: its member tracks with their instruments, its
/// bus and the buses it feeds, the plug-ins on those channels, and the routes between them.
///
/// Routes that leave the group cannot travel with a reusable kit: an output to another track
/// becomes Master, sends and sidechains from other tracks are left out; each is reported once.
/// `project` is not modified.
pub fn extract_group(project: &Value, group_id: &str) -> Result<ExtractedGroup> {
    let group = list(project, "groups")
        .iter()
        .find(|group| text(group, "id") == Some(group_id))
        .ok_or_else(|| Error::invalid("Select a group to save as a kit."))?;
    let bus_id = text(group, "busID").filter(|id| !id.is_empty()).ok_or_else(|| Error::invalid("Select a group to save as a kit."))?;
    let tracks = list(project, "tracks");
    let members: Vec<(usize, &Value)> = tracks.iter().enumerate().filter(|(_, track)| text(track, "groupID") == Some(group_id)).collect();
    if members.is_empty() {
        return Err(Error::invalid("The group has no tracks to save."));
    }
    if members.len() > MAXIMUM_TRACKS {
        return Err(Error::invalid("A kit holds up to 128 tracks."));
    }
    let routing = project.get("routing").cloned().unwrap_or_else(|| json!({}));
    let buses = list(&routing, "buses");
    let empty = Map::new();
    let outputs = routing.get("outputs").and_then(Value::as_object).unwrap_or(&empty);
    let sends = list(&routing, "sends");
    let bus_owner = format!("bus:{bus_id}");

    // Owners: the members, the group bus and every bus they reach through outputs and sends.
    let mut owners: BTreeSet<String> = members.iter().filter_map(|(_, track)| text(track, "routingID").map(|id| format!("track:{id}"))).collect();
    owners.insert(bus_owner.clone());
    let bus_owners: HashSet<String> = buses.iter().filter_map(|bus| text(bus, "id").map(|id| format!("bus:{id}"))).collect();
    loop {
        let mut grown = owners.clone();
        for owner in &owners {
            if let Some(destination) = outputs.get(owner).and_then(Value::as_str).filter(|destination| bus_owners.contains(*destination)) {
                grown.insert(destination.to_string());
            }
            for send in sends.iter().filter(|send| text(send, "source") == Some(owner)) {
                if let Some(destination) = text(send, "destination").filter(|destination| bus_owners.contains(*destination)) {
                    grown.insert(destination.to_string());
                }
            }
        }
        if grown == owners {
            break;
        }
        owners = grown;
    }

    // Instruments, renumbered in member order. Members that share an instrument keep sharing it.
    let source_instruments = list(project, "instruments");
    let mut instrument_map: BTreeMap<usize, usize> = BTreeMap::new();
    let mut instruments: Vec<Value> = Vec::new();
    let mut member_tracks: Vec<Value> = Vec::new();
    for (position, (_, track)) in members.iter().enumerate() {
        let old_instrument = index_of(track.get("instrument")).unwrap_or(0);
        let new_instrument = *instrument_map.entry(old_instrument).or_insert_with(|| {
            let mut instrument = source_instruments.get(old_instrument).cloned().filter(Value::is_object).unwrap_or_else(|| json!({ "type": "sample" }));
            instrument["id"] = json!(instruments.len());
            instruments.push(instrument);
            instruments.len() - 1
        });
        let mut member = (*track).clone();
        member["id"] = json!(position);
        member["instrument"] = json!(new_instrument);
        if let Some(object) = member.as_object_mut() {
            // A frozen print belongs to the session it was printed in.
            object.remove("frozen");
        }
        member_tracks.push(member);
    }

    // Plug-ins and built-in effects of the owners and of the members' instruments.
    let mut plugins: Vec<Value> = Vec::new();
    let mut plugin_ids: HashSet<String> = HashSet::new();
    for plugin in list(project, "audioPlugins") {
        let Some(owner) = text(plugin, "owner").map(String::from) else { continue };
        let mapped = if let Some(old) = owner.strip_prefix("instrument:").and_then(|digits| digits.parse::<usize>().ok()) {
            match instrument_map.get(&old) {
                Some(new) => format!("instrument:{new}"),
                None => continue,
            }
        } else if owners.contains(&owner) {
            owner
        } else {
            continue;
        };
        let mut copy = plugin.clone();
        copy["owner"] = json!(mapped);
        if let Some(id) = text(plugin, "instanceID") {
            plugin_ids.insert(id.to_string());
        }
        plugins.push(copy);
    }
    if plugins.len() > MAXIMUM_ENTRIES {
        return Err(Error::invalid("Too many plug-in states in this kit document."));
    }

    let mut warnings: Vec<String> = Vec::new();
    let mut warn = |message: &str| {
        if !warnings.iter().any(|known| known == message) {
            warnings.push(message.to_string());
        }
    };
    let mut mapped_outputs = Map::new();
    for owner in &owners {
        match outputs.get(owner) {
            Some(Value::String(target)) if target != "master" && !owners.contains(target) => {
                mapped_outputs.insert(owner.clone(), json!("master"));
                warn("An output to another track was changed to Master in the saved kit.");
            }
            Some(target) => {
                mapped_outputs.insert(owner.clone(), target.clone());
            }
            None => {
                mapped_outputs.insert(owner.clone(), json!(if owner.starts_with("track:") { bus_owner.as_str() } else { "master" }));
            }
        }
    }
    let selected_sends: Vec<&Value> = sends.iter().filter(|send| text(send, "source").is_some_and(|source| owners.contains(source))).collect();
    let kept_sends: Vec<Value> = selected_sends
        .iter()
        .filter(|send| text(send, "destination").is_some_and(|destination| destination == "master" || owners.contains(destination)))
        .map(|send| (*send).clone())
        .collect();
    if kept_sends.len() != selected_sends.len() {
        warn("Sends to other tracks are not included in this reusable kit.");
    }
    let relevant: Vec<&Value> = list(&routing, "sidechains")
        .iter()
        .filter(|route| text(route, "destination").is_some_and(|id| plugin_ids.contains(id)))
        .collect();
    let kept_sidechains: Vec<Value> = relevant.iter().filter(|route| text(route, "source").is_some_and(|source| owners.contains(source))).map(|route| (*route).clone()).collect();
    if kept_sidechains.len() != relevant.len() {
        warn("Sidechains driven by other tracks are not included in this reusable kit.");
    }
    let kept_buses: Vec<Value> = buses.iter().filter(|bus| text(bus, "id").is_some_and(|id| owners.contains(&format!("bus:{id}")))).cloned().collect();

    let name = text(group, "name").filter(|name| !name.trim().is_empty()).unwrap_or("Kit").to_string();
    let mut record = group.clone();
    if let Some(object) = record.as_object_mut() {
        // Where the session loaded or saved the kit is a session record, not part of the kit.
        object.remove("libraryPath");
        object.insert("collapsed".to_string(), json!(false));
        object.insert("mixerCollapsed".to_string(), json!(false));
    }
    let mut result = json!({
        "version": st_project::model::FORMAT_VERSION,
        "name": name,
        "bpm": project.get("bpm").cloned().filter(Value::is_number).unwrap_or(json!(120)),
        "linesPerBeat": project.get("linesPerBeat").cloned().filter(Value::is_number).unwrap_or(json!(4)),
        "tracks": member_tracks,
        "instruments": instruments,
        "groups": [record],
        "audioPlugins": plugins,
        "routing": {
            "buses": kept_buses,
            "outputs": mapped_outputs,
            "sends": kept_sends,
            "sidechains": kept_sidechains,
        },
    });
    if let Some(pages) = project.get("pluginPages").and_then(Value::as_object) {
        let kept: Map<String, Value> = pages.iter().filter(|(id, _)| plugin_ids.contains(*id)).map(|(id, page)| (id.clone(), page.clone())).collect();
        result["pluginPages"] = Value::Object(kept);
    }
    validate(&result)?;
    let mut metadata = Map::new();
    metadata.insert("name".to_string(), json!(name));
    metadata.insert("type".to_string(), json!("Kit"));
    metadata.insert("tags".to_string(), group.get("tags").cloned().filter(Value::is_array).unwrap_or(json!([])));
    metadata.insert("source".to_string(), json!("User"));
    Ok(ExtractedGroup { project: result, metadata, warnings })
}

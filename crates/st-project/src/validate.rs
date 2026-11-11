//! What the host checks before it accepts a project: counts, ranges, finite numbers and
//! references that must resolve. Reading ([`Project::from_json`]) is lenient; this is the gate.
//!
//! The limits come from the Swift host this crate replaces: `ProjectStore.validate` (sizes,
//! tempo, patterns, sequence, zones, clips), `AudioRouting.parse` (routing identities, buses,
//! outputs, sends, sidechains, feedback), `AudioRoutingAutomation` (plug-in descriptors and
//! owner automation lanes), `AudioPluginStepFX` (plug-in parameter locks) and
//! `AudioFreezeTrack.Record.parse` (Track Freeze records). Each constant names its source.

use crate::error::{Error, Result};
use crate::json::Json;
use crate::model::plugin::INSERT_SLOTS;
use crate::model::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Tracks in a project. A sanity bound against corrupt files (`ProjectLayout.trackSanityLimit`).
pub const MAX_TRACKS: usize = 1024;
/// Instruments in a project (`ProjectLayout.instrumentSanityLimit`).
pub const MAX_INSTRUMENTS: usize = 1024;
/// Patterns in a project; pattern numbers are 0..=255 (`ProjectStore.validate`).
pub const MAX_PATTERNS: usize = 256;
/// Rows in a pattern (`ProjectStore.validate`).
pub const MAX_PATTERN_ROWS: i64 = 256;
/// Entries in the song sequence (`ProjectStore.validate`, `MAX_ENTRIES` in `song-model.js`).
pub const MAX_SEQUENCE: usize = 1024;
/// Sample zones of an instrument (`ProjectStore.validate`).
pub const MAX_ZONES: usize = 128;
/// Audio clips in a song (`ProjectStore.validate`).
pub const MAX_AUDIO_CLIPS: usize = 256;
/// Buses in the routing (`AudioRouting.parse`).
pub const MAX_BUSES: usize = 1024;
/// Sends and sidechains together (`AudioRouting.parse`).
pub const MAX_ROUTES: usize = 128;
/// Points in an automation lane (`AudioRoutingAutomation`).
pub const MAX_AUTOMATION_POINTS: usize = 4096;
/// Tempo range in BPM (`ProjectStore.validate`).
pub const BPM_RANGE: (f64, f64) = (20.0, 400.0);
/// Frames in a Track Freeze print (`AudioFreezeTrack.Record.parse`).
pub const MAX_FREEZE_FRAMES: i64 = 100_000_000;
/// Input channels of an audio device that a track's live input can name (0..=255).
pub const MAX_INPUT_CHANNELS: i64 = 256;

/// The value range of a track or instrument automation target (`ProjectLayout.nativeAutomationRanges`).
pub fn native_automation_range(target: &str) -> Option<(f64, f64)> {
    Some(match target {
        "volume" | "instrumentVolume" => (0.0, 2.0),
        "pan" | "instrumentPan" | "filterEnvAmount" => (-1.0, 1.0),
        "cutoff" => (20.0, 20000.0),
        "resonance" | "delay" | "reverb" | "sustain" | "drive" | "lfoDepth" | "start" | "end" | "loopStart" | "loopEnd" | "grainPosition"
        | "filterSustain" | "lfo2Depth" | "wavetablePosition" | "grainSpray" | "grainReverse" | "grainStereo" => (0.0, 1.0),
        "pitch" | "tune" => (-48.0, 48.0),
        "attack" | "decay" | "filterAttack" | "filterDecay" => (0.0, 10.0),
        "release" | "filterRelease" => (0.0, 20.0),
        "bitDepth" => (1.0, 24.0),
        "lfoRate" | "lfo2Rate" => (0.0, 100.0),
        "grainSize" => (0.005, 2.0),
        "sliceCount" => (1.0, 128.0),
        "lfoBeats" | "lfo2Beats" => (0.03125, 64.0),
        "grainDensity" => (1.0, 8.0),
        "grainPitchSpread" => (0.0, 24.0),
        "grainScan" => (-4.0, 4.0),
        _ => return None,
    })
}

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::invalid(message))
}

/// Letters, digits, hyphens, underscores and periods, 1..=128 bytes (`AudioRouting.identifier`).
pub fn is_identifier(text: &str) -> bool {
    !text.is_empty() && text.len() <= 128 && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn identifier(value: Option<&str>, extra: &Map<String, Value>, key: &str, label: &str) -> Result<String> {
    match value {
        Some(text) if is_identifier(text) => Ok(text.to_string()),
        Some(_) => fail(format!("{label} must contain 1–128 letters, digits, hyphens, underscores, or periods.")),
        None if extra.contains_key(key) => fail(format!("{label} must be text.")),
        None => fail(format!("{label} is missing.")),
    }
}

/// Display text: at most 512 bytes, no control characters, not blank unless `allow_empty`
/// (`AudioRouting.text`).
fn text(value: &str, label: &str, allow_empty: bool) -> Result<()> {
    if value.len() > 512 || value.chars().any(char::is_control) || (!allow_empty && value.trim().is_empty()) {
        return fail(format!("{label} must be valid non-control text."));
    }
    Ok(())
}

/// A number field that must be a number when present and inside `range`.
fn ranged(value: Option<f64>, extra: &Map<String, Value>, key: &str, label: &str, range: (f64, f64)) -> Result<()> {
    if extra.contains_key(key) {
        return fail(format!("{label} must be a number."));
    }
    match value {
        Some(number) if !number.is_finite() || number < range.0 || number > range.1 => {
            fail(format!("{label} must be between {} and {}.", range.0, range.1))
        }
        _ => Ok(()),
    }
}

/// A flag that must be `true` or `false` when present.
fn flag(extra: &Map<String, Value>, key: &str, label: &str) -> Result<()> {
    if extra.contains_key(key) {
        return fail(format!("{label} must be true or false."));
    }
    Ok(())
}

/// Checks a project against everything the host enforces. `Ok` means the host accepts it;
/// `Err` carries one message for the user.
pub fn validate(project: &Project) -> Result<()> {
    reject_old_layouts(project)?;
    if project.version() != 1 || project.extra.contains_key("version") {
        return fail("This project uses an unsupported format version.");
    }
    let sizes = || format!("The project must contain 1–{MAX_TRACKS} tracks, up to {MAX_INSTRUMENTS} instruments, and 1–{MAX_PATTERNS} patterns.");
    let (Some(tracks), Some(instruments), Some(patterns)) = (&project.tracks, &project.instruments, &project.patterns) else {
        return fail(sizes());
    };
    if tracks.is_empty() || tracks.len() > MAX_TRACKS || instruments.len() > MAX_INSTRUMENTS || patterns.is_empty() || patterns.len() > MAX_PATTERNS {
        return fail(sizes());
    }
    match project.bpm {
        Some(bpm) if bpm.is_finite() && bpm >= BPM_RANGE.0 && bpm <= BPM_RANGE.1 => {}
        _ => return fail("Project tempo must be between 20 and 400 BPM."),
    }
    if !project.all_finite() {
        return fail("Project contains values that cannot be saved.");
    }

    let mut pattern_numbers = BTreeSet::new();
    for (position, pattern) in patterns.iter().enumerate() {
        let dimensions = || format!("A pattern has invalid dimensions (maximum {MAX_PATTERN_ROWS} rows and {MAX_TRACKS} tracks).");
        match (pattern.length, &pattern.steps) {
            (Some(length), Some(steps)) if (1..=MAX_PATTERN_ROWS).contains(&length) && steps.len() <= MAX_TRACKS => {}
            _ => return fail(dimensions()),
        }
        let number = pattern.number(position);
        if pattern.extra.contains_key("id") || !(0..MAX_PATTERNS as i64).contains(&number) || !pattern_numbers.insert(number) {
            return fail("Pattern identifiers must be unique numbers from 0 to 255.");
        }
    }
    let sequence_message = || format!("The song sequence must reference existing patterns (up to {MAX_SEQUENCE} entries).");
    if project.extra.contains_key("sequence") {
        return fail(sequence_message());
    }
    if let Some(sequence) = &project.sequence {
        if sequence.is_empty() || sequence.len() > MAX_SEQUENCE || !sequence.iter().all(|number| pattern_numbers.contains(number)) {
            return fail(sequence_message());
        }
    }
    for instrument in instruments {
        if instrument.zones.as_ref().is_some_and(|zones| zones.len() > MAX_ZONES) {
            return fail(format!("An instrument can contain up to {MAX_ZONES} sample zones."));
        }
    }
    if project.extra.contains_key("audioClips") {
        return fail("Audio clips must be a list of clips.");
    }
    let clips = project.clip_list();
    if clips.len() > MAX_AUDIO_CLIPS {
        return fail(format!("A song can contain up to {MAX_AUDIO_CLIPS} audio clips."));
    }
    for clip in clips {
        let track = clip.track();
        let (start, duration) = (clip.start_beat(), clip.duration_beats());
        if track < 0 || track as usize >= tracks.len() || !start.is_finite() || start < 0.0 || !duration.is_finite() || duration <= 0.0 {
            return fail("An audio clip has an invalid track, position, or duration.");
        }
    }

    let owners = validate_routing(project, tracks)?;
    validate_plugins(project, tracks, &owners)?;
    validate_patterns(project, patterns, &owners)?;
    for (index, track) in tracks.iter().enumerate() {
        if track.extra.contains_key("frozen") {
            return fail(format!("{}: the freeze record must be an object.", track.display_name(index)));
        }
        if let Some(record) = &track.frozen {
            validate_freeze(record).map_err(|error| Error::invalid(format!("{}: {}", track.display_name(index), error.message())))?;
        }
        if track.extra.contains_key("audioInput") {
            return fail(format!("{}: the live input must be an object.", track.display_name(index)));
        }
        if let Some(input) = &track.audio_input {
            validate_audio_input(input).map_err(|error| Error::invalid(format!("{}: {}", track.display_name(index), error.message())))?;
        }
    }
    Ok(())
}

/// A track's live input (`tracks[i].audioInput`): one channel of the device for mono or two
/// different ones for stereo, a gain of 0..2 and, while it is enabled, the device it was chosen
/// on.
fn validate_audio_input(input: &AudioInput) -> Result<()> {
    if ["enabled", "deviceUID", "channels", "gain", "monitor"].iter().any(|key| input.extra.contains_key(*key)) {
        return fail("The live input has an unreadable setting.");
    }
    if let Some(channels) = &input.channels {
        let distinct = channels.len() != 2 || channels[0] != channels[1];
        if channels.is_empty() || channels.len() > 2 || !distinct || channels.iter().any(|channel| !(0..MAX_INPUT_CHANNELS).contains(channel)) {
            return fail("The live input needs one input channel (mono) or two different ones (stereo).");
        }
    }
    let gain = input.gain();
    if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
        return fail("The live input's gain must be between 0 and 2.");
    }
    if input.enabled() && (input.channels.is_none() || input.device_uid.as_deref().unwrap_or("").is_empty()) {
        return fail("An enabled live input needs its audio device and input channels.");
    }
    Ok(())
}

/// Layouts older versions wrote and the Swift host converted on open (`ProjectMigration`).
/// The Rust host reads the current layout only.
fn reject_old_layouts(project: &Project) -> Result<()> {
    let old = |what: &str| -> Result<()> {
        fail(format!(
            "This project was saved in an older layout ({what}) that this version no longer reads. Open and save it with a version of SuperTracker that converts it, then open it again."
        ))
    };
    if project.extra.contains_key("timing") {
        return old("a tempo map");
    }
    if project.extra.contains_key("kits") {
        return old("kit lanes");
    }
    for track in project.track_list() {
        if track.extra.get("kind").and_then(Value::as_str) == Some("kit") {
            return old("kit lanes");
        }
    }
    for instrument in project.instrument_list() {
        let retired_type = matches!(instrument.extra.get("type").and_then(Value::as_str), Some("synth") | Some("drum"));
        // Untyped instruments were synths unless they name a sample source.
        let untyped_synth = instrument.kind.is_none()
            && !instrument.extra.contains_key("type")
            && instrument.extra.contains_key("waveform")
            && instrument.sample_path.as_deref().unwrap_or("").is_empty()
            && instrument.zones.as_ref().is_none_or(Vec::is_empty)
            && instrument.plugin.is_none();
        if retired_type || untyped_synth {
            return old("the built-in synth instrument");
        }
    }
    Ok(())
}

/// The mixer channels of a project, by owner name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Track(usize),
    Bus,
    Master,
}

struct Owners {
    channels: BTreeMap<String, Channel>,
    /// Route id -> source owner, for sends and sidechains.
    routes: BTreeMap<String, String>,
}

/// The mixer channels and the routes between them: every track has its identity, the routing
/// table is there, and what it names exists.
fn validate_routing(project: &Project, tracks: &[Track]) -> Result<Owners> {
    let mut channels = BTreeMap::new();
    for (index, track) in tracks.iter().enumerate() {
        let name = format!("Track {}", index + 1);
        let id = identifier(track.routing_id.as_deref(), &track.extra, "routingID", &format!("{name} routingID"))?;
        if channels.contains_key(&format!("track:{id}")) {
            return fail(format!("Duplicate track routingID: {id}."));
        }
        if track.extra.contains_key("name") {
            return fail(format!("{name}: the name must be text."));
        }
        if let Some(text_value) = &track.name {
            text(text_value, &format!("{name}: the name"), false)?;
        }
        ranged(track.volume, &track.extra, "volume", &format!("{name} volume"), (0.0, 2.0))?;
        ranged(track.pan, &track.extra, "pan", &format!("{name} pan"), (-1.0, 1.0))?;
        flag(&track.extra, "mute", &format!("{name} mute"))?;
        flag(&track.extra, "solo", &format!("{name} solo"))?;
        channels.insert(format!("track:{id}"), Channel::Track(index));
    }
    ranged(project.master_volume, &project.extra, "masterVolume", "Master volume", (0.0, 2.0))?;
    channels.insert("master".to_string(), Channel::Master);
    let Some(routing) = &project.routing else {
        return fail(if project.extra.contains_key("routing") { "The project's routing must be an object." } else { "The project has no routing table." });
    };
    if routing.extra.contains_key("buses") {
        return fail("Routing buses must be an array of objects.");
    }
    let buses = routing.buses.as_deref().unwrap_or(&[]);
    if buses.len() > MAX_BUSES {
        return fail(format!("Routing supports at most {MAX_BUSES} additional buses."));
    }
    for bus in buses {
        let id = identifier(bus.id.as_deref(), &bus.extra, "id", "Bus ID")?;
        if bus.kind.is_none() || channels.insert(format!("bus:{id}"), Channel::Bus).is_some() {
            return fail("Bus IDs must be unique and bus type must be group or return.");
        }
        match &bus.name {
            Some(name) => text(name, "Bus name", false)?,
            None => return fail("Bus name must be text."),
        }
        ranged(bus.volume, &bus.extra, "volume", "Bus volume", (0.0, 2.0))?;
        ranged(bus.pan, &bus.extra, "pan", "Bus pan", (-1.0, 1.0))?;
        flag(&bus.extra, "mute", "Bus mute")?;
        flag(&bus.extra, "solo", "Bus solo")?;
    }
    let known = |owner: &str, allow_master: bool| channels.contains_key(owner) && (allow_master || owner != "master");

    // Every track and bus outputs to master unless `outputs` says otherwise.
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut outputs: BTreeMap<&str, Option<&str>> = channels.keys().filter(|owner| *owner != "master").map(|owner| (owner.as_str(), Some("master"))).collect();
    if routing.extra.contains_key("outputs") {
        return fail("Routing outputs must be an object whose values are owners or null.");
    }
    for (source, destination) in routing.outputs.iter().flatten() {
        if !known(source, false) {
            return fail("Output source must reference an existing track or bus.");
        }
        match destination {
            Some(destination) if !known(destination, true) => return fail("Output destination must reference an existing owner."),
            Some(destination) => outputs.insert(source.as_str(), Some(destination.as_str())),
            None => outputs.insert(source.as_str(), None),
        };
    }
    for (source, destination) in &outputs {
        if let Some(destination) = destination {
            edges.push((source.to_string(), destination.to_string()));
        }
    }

    if routing.extra.contains_key("sends") {
        return fail("Sends must be an array of objects.");
    }
    if routing.extra.contains_key("sidechains") {
        return fail("Sidechains must be an array of objects.");
    }
    let sends = routing.sends.as_deref().unwrap_or(&[]);
    let sidechains = routing.sidechains.as_deref().unwrap_or(&[]);
    if sends.len() + sidechains.len() > MAX_ROUTES {
        return fail(format!("Routing supports at most {MAX_ROUTES} sends and sidechains combined."));
    }
    let mut routes: BTreeMap<String, String> = BTreeMap::new();
    for send in sends {
        let id = identifier(send.id.as_deref(), &send.extra, "id", "Route ID")?;
        let Some(source) = send.source.as_deref().filter(|source| known(source, false)) else {
            return fail("Route source must reference an existing track or bus.");
        };
        if routes.insert(id.clone(), source.to_string()).is_some() {
            return fail(format!("Duplicate route ID: {id}."));
        }
        if send.tap.is_none() {
            return fail("Unknown routing tap.");
        }
        ranged(send.gain, &send.extra, "gain", "Route gain", (0.0, 2.0))?;
        flag(&send.extra, "enabled", "Route enabled")?;
        let Some(destination) = send.destination.as_deref().filter(|destination| known(destination, true)) else {
            return fail("Send destination must reference an existing owner.");
        };
        if send.enabled() {
            edges.push((source.to_string(), destination.to_string()));
        }
    }
    for sidechain in sidechains {
        let id = identifier(sidechain.id.as_deref(), &sidechain.extra, "id", "Route ID")?;
        // The source may name a channel that is gone: the sidechain is then unavailable, not invalid.
        let Some(source) = sidechain.source.as_deref() else { return fail("Sidechain source must be an owner ID.") };
        match source.split_once(':') {
            Some(("track" | "bus", rest)) if is_identifier(rest) => {}
            _ => return fail("Sidechain source must reference a track or bus ID."),
        }
        if routes.insert(id.clone(), source.to_string()).is_some() {
            return fail(format!("Duplicate route ID: {id}."));
        }
        if sidechain.tap.is_none() {
            return fail("Unknown routing tap.");
        }
        ranged(sidechain.gain, &sidechain.extra, "gain", "Route gain", (0.0, 2.0))?;
        flag(&sidechain.extra, "enabled", "Route enabled")?;
        let layout = sidechain.destination.as_ref().and_then(|destination| destination.layout_signature.as_ref().map(|signature| (destination, signature)));
        let Some((destination, signature)) = layout else { return fail("Sidechains require a destination and saved input layout.") };
        identifier(destination.instance_id.as_deref(), &destination.extra, "instanceID", "Sidechain instance ID")?;
        match &signature.name {
            Some(name) => text(name, "Sidechain input name", true)?,
            None => return fail("Sidechain input name must be text."),
        }
        if !destination.input_bus.is_some_and(|bus| (0..=31).contains(&bus)) {
            return fail("Sidechain input bus must be a whole number from 0 to 31.");
        }
        if !signature.channels.is_some_and(|count| (1..=64).contains(&count)) {
            return fail("Sidechain input channels must be a whole number from 1 to 64.");
        }
    }
    if let Some(cycle) = find_cycle(&edges) {
        return fail(format!("Routing feedback is unsupported. Cycle includes: {}.", cycle.join(", ")));
    }
    Ok(Owners { channels, routes })
}

/// The owners on a loop of outputs and sends, or `None` when there is none: what is left after
/// repeatedly removing owners nothing feeds, then owners that feed nothing that is left.
fn find_cycle(edges: &[(String, String)]) -> Option<Vec<String>> {
    let mut left: BTreeSet<&str> = edges.iter().flat_map(|(source, destination)| [source.as_str(), destination.as_str()]).collect();
    loop {
        let fed: BTreeSet<&str> = edges.iter().filter(|(source, _)| left.contains(source.as_str())).map(|(_, destination)| destination.as_str()).collect();
        let feeding: BTreeSet<&str> = edges.iter().filter(|(_, destination)| left.contains(destination.as_str())).map(|(source, _)| source.as_str()).collect();
        let before = left.len();
        left.retain(|owner| fed.contains(owner) && feeding.contains(owner));
        if left.len() == before {
            break;
        }
    }
    (!left.is_empty()).then(|| left.into_iter().map(str::to_string).collect())
}

/// Plug-in descriptors (`AudioRoutingAutomation.desiredPluginLayouts`, `AudioRouting.Definition.graph`).
fn validate_plugins(project: &Project, tracks: &[Track], owners: &Owners) -> Result<()> {
    if project.extra.contains_key("audioPlugins") {
        return fail("Project plug-ins must be an array of objects.");
    }
    let mut instance_ids = BTreeSet::new();
    let mut slots = BTreeSet::new();
    for plugin in project.plugin_list() {
        if plugin.extra.contains_key("kind") {
            return fail("Unknown plug-in slot kind.");
        }
        let kind = plugin.kind();
        if plugin.extra.contains_key("slot") || !(0..INSERT_SLOTS).contains(&plugin.slot()) {
            return fail(format!("Plug-in slot must be a whole number from 0 to {}.", INSERT_SLOTS - 1));
        }
        if plugin.extra.contains_key("instanceID") {
            return fail("Plug-in instance ID must be text.");
        }
        let owner: String = if plugin.owner.is_some() || plugin.extra.contains_key("owner") {
            let owner = plugin.owner.as_deref().unwrap_or("?");
            match Owner::parse(owner) {
                // An instrument's insert chain runs on the tracks that play it; with none it has no instance.
                Some(Owner::Instrument(_)) => {
                    if kind != PluginKind::Effect {
                        continue;
                    }
                    owner.to_string()
                }
                _ if owners.channels.contains_key(owner) => owner.to_string(),
                _ => return fail(format!("Plug-in owner {owner} does not exist.")),
            }
        } else {
            let index = plugin.track.unwrap_or(0);
            if plugin.extra.contains_key("track") || index < 0 || index as usize >= tracks.len() {
                return fail("Plug-in track does not exist.");
            }
            tracks[index as usize].owner().unwrap_or_default()
        };
        if let Some(id) = &plugin.instance_id {
            if !is_identifier(id) {
                return fail("Plug-in instance ID must contain 1–128 letters, digits, hyphens, underscores, or periods.");
            }
            if !instance_ids.insert(id.clone()) {
                return fail(format!("Invalid or duplicate plug-in layout: {id}."));
            }
        }
        if kind == PluginKind::Instrument && (!matches!(owners.channels.get(&owner), Some(Channel::Track(_))) || plugin.slot() != 0) {
            return fail("Instruments require a track's instrument slot.");
        }
        if !slots.insert((owner, kind, plugin.slot())) {
            return fail("Two plug-ins occupy the same routing slot.");
        }
    }
    Ok(())
}

fn validate_points(lane: &AutomationLane, length: i64, range: (f64, f64)) -> Result<()> {
    if lane.extra.contains_key("points") {
        return fail("Automation points must be an array of objects.");
    }
    let points = lane.points.as_deref().unwrap_or(&[]);
    if points.len() > MAX_AUTOMATION_POINTS {
        return fail(format!("Automation supports at most {MAX_AUTOMATION_POINTS} points per lane."));
    }
    let mut rows: Vec<f64> = Vec::with_capacity(points.len());
    for point in points {
        let (Some(row), Some(value)) = (point.row, point.value) else {
            return fail("Automation points need a numeric row and value.");
        };
        if !(0.0..=length as f64).contains(&row) {
            return fail(format!("Automation row must be a finite number in 0...{length}."));
        }
        if !(range.0..=range.1).contains(&value) {
            return fail(format!("Automation value must be a finite number in {}...{}.", range.0, range.1));
        }
        rows.push(row);
    }
    rows.sort_by(f64::total_cmp);
    if rows.windows(2).any(|pair| pair[0] == pair[1]) {
        return fail("Duplicate automation point row.");
    }
    Ok(())
}

/// Owner automation lanes (`AudioRoutingAutomation.init`) and plug-in parameter locks
/// (`AudioPluginStepFX.init`).
fn validate_patterns(project: &Project, patterns: &[Pattern], owners: &Owners) -> Result<()> {
    let _ = project;
    for pattern in patterns {
        for row in pattern.steps.iter().flatten().flatten().flatten() {
            for effect in [&row.fx1, &row.fx2].into_iter().flatten() {
                if effect.kind != Some(StepEffectKind::Plugin) {
                    continue;
                }
                if effect.plugin_address().is_none() {
                    return fail("Plug-in step FX address must be an exact UInt64 decimal string.");
                }
                if !effect.value.is_some_and(|value| (0.0..=1.0).contains(&value)) {
                    return fail("Plug-in step FX value must be a finite normalized number from 0 to 1.");
                }
            }
        }
        if pattern.extra.contains_key("automation") {
            return fail("Automation lanes must be an array of objects.");
        }
        let length = pattern.length();
        let mut destinations = BTreeSet::new();
        for lane in pattern.automation.iter().flatten() {
            // Track-index lanes belong to the engine, which skips what it does not know.
            if lane.owner.is_none() && !lane.extra.contains_key("owner") {
                continue;
            }
            let owner = lane.owner.as_deref().unwrap_or("");
            if lane.extra.contains_key("interpolation") {
                return fail("Unknown automation interpolation.");
            }
            if let Some(Owner::Instrument(_)) = Owner::parse(owner) {
                // One lane per track that plays the instrument; only its effect parameters are automated.
                if let Some(AutomationTarget::Plugin { instrument: false, slot, .. }) = lane.target.as_deref().and_then(AutomationTarget::parse) {
                    if slot < INSERT_SLOTS {
                        validate_points(lane, length, (0.0, 1.0))?;
                    }
                }
                continue;
            }
            let (Some(channel), Some(target)) = (owners.channels.get(owner), lane.target.as_deref()) else {
                return fail("Host automation requires an existing owner and a target.");
            };
            let is_track = matches!(channel, Channel::Track(_));
            let range = match AutomationTarget::parse(target) {
                Some(AutomationTarget::Native(name)) if name == "volume" => (0.0, 2.0),
                Some(AutomationTarget::Native(name)) if name == "pan" => {
                    if owner == "master" {
                        return fail("Master pan automation is unsupported.");
                    }
                    (-1.0, 1.0)
                }
                Some(AutomationTarget::Native(name)) if is_track && native_automation_range(&name).is_some() => {
                    native_automation_range(&name).unwrap_or((0.0, 1.0))
                }
                Some(AutomationTarget::Send(route)) => {
                    // A lane for a send that is gone is unavailable, not invalid.
                    if owners.routes.get(&route).is_some_and(|source| source != owner) {
                        return fail("Send automation owner does not match its source.");
                    }
                    (0.0, 2.0)
                }
                Some(AutomationTarget::Plugin { instrument, slot, .. }) if slot < INSERT_SLOTS || (is_track && slot < INSERT_SLOTS * 2) => {
                    if instrument && slot != 0 {
                        return fail("Invalid instrument automation slot.");
                    }
                    (0.0, 1.0)
                }
                _ => return fail(format!("Unknown host automation target: {target}.")),
            };
            validate_points(lane, length, range)?;
            if !destinations.insert((owner.to_string(), target.to_string())) {
                return fail("Two automation lanes of a pattern control the same destination.");
            }
        }
    }
    Ok(())
}

/// A Track Freeze record (`AudioFreezeTrack.Record.parse`).
fn validate_freeze(record: &FreezeRecord) -> Result<()> {
    if record.version != Some(1) {
        return fail("Unsupported freeze record version.");
    }
    for (value, key) in [(&record.id, "id"), (&record.path, "path")] {
        if value.as_deref().unwrap_or("").is_empty() {
            return fail(format!("Freeze record is missing {key}."));
        }
    }
    let frame_count = |value: Option<i64>, key: &str| -> Result<i64> {
        match value {
            Some(frames) if (0..=MAX_FREEZE_FRAMES).contains(&frames) => Ok(frames),
            Some(_) => fail(format!("Freeze record has an invalid {key} frame count.")),
            None => fail(format!("Freeze record is missing {key}.")),
        }
    };
    let Some(rate) = record.sample_rate else { return fail("Freeze record is missing sampleRate.") };
    let frames = frame_count(record.frames, "frames")?;
    let musical = frame_count(record.musical_frames, "musicalFrames")?;
    let tail = frame_count(record.tail_frames, "tailFrames")?;
    let Some(end) = record.musical_end_beat else { return fail("Freeze record is missing musicalEndBeat.") };
    if !(8_000.0..=384_000.0).contains(&rate) || musical <= 0 || frames != musical + tail || end <= 0.0 {
        return fail("Freeze record has inconsistent frame counts.");
    }
    Ok(())
}

impl Project {
    /// Shorthand for [`validate`].
    pub fn validate(&self) -> Result<()> {
        validate(self)
    }

    /// Whether every number in the typed part of the document is finite (JSON cannot carry
    /// others). Values read from JSON always are; this guards values set in code.
    pub fn all_finite(&self) -> bool {
        let mut finite = true;
        visit_numbers(self, &mut |number| finite &= number.is_finite());
        finite
    }
}

/// Calls `visit` for the numbers validation must see as finite: the ones the engine and mixer read.
fn visit_numbers(project: &Project, visit: &mut dyn FnMut(f64)) {
    for value in [project.bpm, project.swing, project.master_volume, project.metronome_volume].into_iter().flatten() {
        visit(value);
    }
    if let Some(delay) = &project.delay {
        [delay.time, delay.feedback, delay.mix, delay.beats, delay.tone].into_iter().flatten().for_each(&mut *visit);
    }
    if let Some(reverb) = &project.reverb {
        [reverb.size, reverb.damping, reverb.mix, reverb.predelay].into_iter().flatten().for_each(&mut *visit);
    }
    for track in project.track_list() {
        [track.volume, track.pan, track.delay_send, track.reverb_send].into_iter().flatten().for_each(&mut *visit);
    }
    for instrument in project.instrument_list() {
        // Every number of an instrument, through its JSON form's typed fields.
        if let Value::Object(map) = Json::to_json(instrument) {
            for (key, value) in &map {
                if instrument.extra.contains_key(key) {
                    continue;
                }
                match value {
                    Value::Null => visit(f64::NAN),
                    Value::Array(items) if items.iter().any(Value::is_null) && key != "zones" => visit(f64::NAN),
                    _ => {}
                }
            }
        }
    }
    for pattern in project.pattern_list() {
        for step in pattern.steps.iter().flatten().flatten().flatten() {
            step.gate.into_iter().for_each(&mut *visit);
            for effect in [&step.fx1, &step.fx2].into_iter().flatten() {
                effect.value.into_iter().for_each(&mut *visit);
            }
            for cell in step.columns.iter().flatten().flatten() {
                cell.gate.into_iter().for_each(&mut *visit);
            }
        }
        for lane in pattern.automation.iter().flatten() {
            for point in lane.points.iter().flatten() {
                [point.row, point.value].into_iter().flatten().for_each(&mut *visit);
            }
        }
    }
    for clip in project.clip_list() {
        [clip.start_beat, clip.duration_beats, clip.source_offset, clip.source_duration, clip.gain, clip.fade_in, clip.fade_out]
            .into_iter()
            .flatten()
            .for_each(&mut *visit);
    }
}

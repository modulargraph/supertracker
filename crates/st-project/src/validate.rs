//! What the host checks before it accepts a project: the format version, counts, ranges, finite
//! numbers and references that must resolve. Reading ([`Project::from_json`]) is lenient; this is
//! the gate.

use crate::error::{Error, Result};
use crate::json::Json;
use crate::model::plugin::{parse_component, INSERT_SLOTS};
use crate::model::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The message for a project whose `version` is not [`FORMAT_VERSION`]. It is the only thing the
/// host knows about other formats: it reads this one and converts none.
pub const OTHER_VERSION: &str = "This project was made by a different version of SuperTracker and cannot be opened by this one.";

/// Tracks in a project. A sanity bound against corrupt files.
pub const MAX_TRACKS: usize = 1024;
/// Instruments in a project.
pub const MAX_INSTRUMENTS: usize = 1024;
/// Patterns in a project; pattern numbers are 0..=255.
pub const MAX_PATTERNS: usize = 256;
/// Rows in a pattern.
pub const MAX_PATTERN_ROWS: i64 = 256;
/// Entries in the song sequence (`MAX_ENTRIES` in `song-model.js`).
pub const MAX_SEQUENCE: usize = 1024;
/// Sample zones of an instrument.
pub const MAX_ZONES: usize = 128;
/// Audio clips in a song.
pub const MAX_AUDIO_CLIPS: usize = 256;
/// Buses in the routing.
pub const MAX_BUSES: usize = 1024;
/// Sends and sidechains together.
pub const MAX_ROUTES: usize = 128;
/// Points in an automation lane.
pub const MAX_AUTOMATION_POINTS: usize = 4096;
/// Tempo range in BPM.
pub const BPM_RANGE: (f64, f64) = (20.0, 400.0);
/// Frames in a Track Freeze print.
pub const MAX_FREEZE_FRAMES: i64 = 100_000_000;
/// Input channels of an audio device that a track's live input can name (0..=255).
pub const MAX_INPUT_CHANNELS: i64 = 256;

/// The value range of a track or instrument automation target.
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

/// Letters, digits, hyphens, underscores and periods, 1..=128 bytes.
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

/// Display text: at most 512 bytes, no control characters, not blank unless `allow_empty`.
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
    if project.version != Some(FORMAT_VERSION) {
        return fail(OTHER_VERSION);
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
    for (index, instrument) in instruments.iter().enumerate() {
        if instrument.extra.contains_key("type") {
            return fail(format!("Instrument {} has an unknown type.", index + 1));
        }
        validate_kept_plugin(project, tracks, index, instrument)?;
        // The input a hardware instrument is sampled from (`recordingInput`, the recorder's) names its device.
        if instrument.extra.get("recordingInput").is_some_and(|input| input.get("deviceUID").and_then(Value::as_str).unwrap_or("").is_empty()) {
            return fail(format!("Instrument {}: the saved recording input must name its audio device.", index + 1));
        }
        if instrument.extra.contains_key("zones") {
            return fail("Sample zones must be a list of zones.");
        }
        if instrument.zones.as_ref().is_some_and(|zones| zones.len() > MAX_ZONES) {
            return fail(format!("An instrument can contain up to {MAX_ZONES} sample zones."));
        }
        // Each of these has one name. Under another name the host would not see it and the
        // instrument would play something else, so the project is refused instead.
        if let Some(zone) = instrument.zones.iter().flatten().position(|zone| zone.extra.contains_key("path")) {
            return fail(format!("Instrument {}, zone {}: the sample must be saved as samplePath, not path.", index + 1, zone + 1));
        }
        if instrument.extra.contains_key("slices") {
            return fail(format!("Instrument {}: slice markers must be saved as slicePoints, not slices.", index + 1));
        }
        if instrument.extra.contains_key("voiceMode") {
            return fail(format!("Instrument {}: the voice mode belongs to the track that plays it, not to the instrument.", index + 1));
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
        if clip.extra.contains_key("samplePath") {
            return fail("An audio clip's file must be saved as path, not samplePath.");
        }
        let track = clip.track();
        let (start, duration) = (clip.start_beat(), clip.duration_beats());
        if track < 0 || track as usize >= tracks.len() || !start.is_finite() || start < 0.0 || !duration.is_finite() || duration <= 0.0 {
            return fail("An audio clip has an invalid track, position, or duration.");
        }
    }

    validate_performance(project)?;
    let owners = validate_routing(project, tracks, instruments.len())?;
    validate_groups(project, tracks)?;
    validate_plugins(project, &owners)?;
    validate_patterns(patterns, &owners)?;
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

/// The keyboard performance settings (`performance`): an object whose values have their types, a
/// root that is a pitch class, and names for the scale, the chord and the chord set (none is the
/// empty chord set). Which names exist is the interface's business.
fn validate_performance(project: &Project) -> Result<()> {
    if project.extra.contains_key("performance") {
        return fail("The keyboard performance settings must be an object.");
    }
    let Some(performance) = &project.performance else { return Ok(()) };
    if ["root", "scale", "scaleMode", "chord", "chordSet", "applyToMIDI"].iter().any(|key| performance.extra.contains_key(*key)) {
        return fail("The keyboard performance settings have an unreadable value.");
    }
    if !(0..=11).contains(&performance.root()) {
        return fail("The keyboard performance key must be a note from 0 to 11.");
    }
    let named = |name: &str, allow_empty: bool| (allow_empty && name.is_empty()) || is_identifier(name);
    if !named(&performance.scale(), false) || !named(&performance.chord(), false) || !named(&performance.chord_set(), true) {
        return fail("The keyboard performance settings name a scale, a chord and a chord set with letters, digits, hyphens, underscores or periods.");
    }
    Ok(())
}

/// The kind and slot of a plug-in descriptor, wherever it is kept.
fn descriptor_kind(plugin: &PluginDescriptor) -> Result<PluginKind> {
    if plugin.extra.contains_key("kind") {
        return fail("Unknown plug-in slot kind.");
    }
    if plugin.extra.contains_key("slot") || !(0..INSERT_SLOTS).contains(&plugin.slot()) {
        return fail(format!("Plug-in slot must be a whole number from 0 to {}.", INSERT_SLOTS - 1));
    }
    Ok(plugin.kind())
}

/// What a plug-in descriptor loads, wherever it is kept: `component` is `<format>:<id>` in its
/// one spelling ([`parse_component`]), and `format` is the format the component names.
fn descriptor_component(plugin: &PluginDescriptor, kind: PluginKind) -> Result<()> {
    let Some(component) = plugin.component.as_deref() else {
        return fail(if plugin.extra.contains_key("component") { "Plug-in component must be text." } else { "A plug-in has no component." });
    };
    let Some(parsed) = parse_component(component) else {
        return fail(format!(
            "Plug-in component {component} is not <format>:<id> with the format stfx, clap, vst3, au or vst2 (a VST3 id is 32 upper-case hex digits, a VST2 id 8)."
        ));
    };
    match plugin.format.as_deref() {
        None | Some("") => return fail(if plugin.extra.contains_key("format") { "Plug-in format must be text." } else { "A plug-in has no format." }),
        Some(format) if format != parsed.format => {
            return fail(format!("Plug-in format {format} does not match its component {component}, whose format is {}.", parsed.format));
        }
        Some(_) => {}
    }
    if parsed.prefix == "stfx" && kind == PluginKind::Instrument {
        return fail("Built-in effects are insert effects, not instruments.");
    }
    Ok(())
}

/// The plug-in of a plug-in instrument is in one place: on the tracks that play it
/// (`audioPlugins`, kind `instrument`). Only an instrument that no track plays with its plug-in
/// keeps the descriptor itself (`instruments[i].plugin`), without an owner or an instance; in
/// everything else it is the descriptor a track would play the instrument with.
fn validate_kept_plugin(project: &Project, tracks: &[Track], index: usize, instrument: &Instrument) -> Result<()> {
    let name = format!("Instrument {}", index + 1);
    if instrument.extra.contains_key("plugin") {
        return fail(format!("{name}: the plug-in it keeps must be an object."));
    }
    let Some(kept) = &instrument.plugin else { return Ok(()) };
    if instrument.kind() != InstrumentKind::Plugin {
        return fail(format!("{name} keeps a plug-in but is not a plug-in instrument."));
    }
    if kept.owner.is_some() || kept.instance_id.is_some() || kept.extra.contains_key("owner") || kept.extra.contains_key("instanceID") {
        return fail(format!("{name}: a plug-in kept by its instrument has no owner and no instance."));
    }
    let described = descriptor_kind(kept).and_then(|kind| {
        if kind != PluginKind::Instrument || kept.slot() != 0 {
            return fail("It must be an instrument plug-in (kind instrument, slot 0).");
        }
        descriptor_component(kept, kind)
    });
    if let Err(error) = described {
        return fail(format!("{name}, the plug-in it keeps: {}", error.message()));
    }
    let played = tracks.iter().any(|track| {
        track.instrument() == index as i64
            && track.owner().is_some_and(|owner| project.plugin_list().iter().any(|plugin| plugin.kind() == PluginKind::Instrument && plugin.owner.as_deref() == Some(owner.as_str())))
    });
    if played {
        return fail(format!("{name} keeps a second copy of the plug-in a track plays it with."));
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

/// The mixer channels of a project, by owner name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Track(usize),
    Bus,
    Master,
}

struct Owners {
    channels: BTreeMap<String, Channel>,
    /// How many instruments there are: `instrument:<index>` owns an insert chain for each.
    instruments: usize,
    /// Route id -> source owner, for sends and sidechains.
    routes: BTreeMap<String, String>,
}

/// The mixer channels and the routes between them: every track has its identity, the routing
/// table is there, and what it names exists.
fn validate_routing(project: &Project, tracks: &[Track], instruments: usize) -> Result<Owners> {
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
        if track.extra.contains_key("voiceMode") {
            return fail(format!("{name}: the voice mode must be poly, mono or legato."));
        }
        channels.insert(format!("track:{id}"), Channel::Track(index));
    }
    ranged(project.master_volume, &project.extra, "masterVolume", "Master volume", (0.0, 2.0))?;
    channels.insert("master".to_string(), Channel::Master);
    let Some(routing) = &project.routing else {
        return fail(if project.extra.contains_key("routing") { "The project's routing must be an object." } else { "The project has no routing table." });
    };
    // The table is complete: the page writes its four collections, empty or not.
    for (present, key) in [(routing.buses.is_some(), "buses"), (routing.outputs.is_some(), "outputs"), (routing.sends.is_some(), "sends"), (routing.sidechains.is_some(), "sidechains")] {
        if !present && !routing.extra.contains_key(key) {
            return fail(format!("The project's routing table has no {key}."));
        }
    }
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
        // The insert may be gone as well: the destination only has to be an instance identity.
        identifier(sidechain.destination.as_deref(), &sidechain.extra, "destination", "Sidechain destination")?;
    }
    if let Some(cycle) = find_cycle(&edges) {
        return fail(format!("Routing feedback is unsupported. Cycle includes: {}.", cycle.join(", ")));
    }
    Ok(Owners { channels, routes, instruments })
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

/// Plug-in descriptors: each names an existing owner, has its instance identity, its component and
/// the component's format, and no two share a slot. The parameter pages the user arranged
/// (`pluginPages`) belong to the instances there are.
fn validate_plugins(project: &Project, owners: &Owners) -> Result<()> {
    if project.extra.contains_key("audioPlugins") {
        return fail("Project plug-ins must be an array of objects.");
    }
    let mut instance_ids = BTreeSet::new();
    let mut slots = BTreeSet::new();
    for plugin in project.plugin_list() {
        let kind = descriptor_kind(plugin)?;
        if plugin.extra.contains_key("instanceID") {
            return fail("Plug-in instance ID must be text.");
        }
        let Some(owner) = plugin.owner.as_deref() else {
            return fail(if plugin.extra.contains_key("owner") { "Plug-in owner must be text." } else { "A plug-in has no owner." });
        };
        let owner: String = match Owner::parse(owner) {
            // An instrument's insert chain runs on the tracks that play it; with none it has no instance.
            Some(Owner::Instrument(index)) if index < owners.instruments => {
                if kind != PluginKind::Effect {
                    return fail("Instruments require a track's instrument slot.");
                }
                owner.to_string()
            }
            Some(Owner::Instrument(_)) => return fail(format!("Plug-in owner {owner} does not exist.")),
            _ if owners.channels.contains_key(owner) => owner.to_string(),
            _ => return fail(format!("Plug-in owner {owner} does not exist.")),
        };
        let Some(id) = &plugin.instance_id else { return fail("A plug-in has no instance ID.") };
        if !is_identifier(id) {
            return fail("Plug-in instance ID must contain 1–128 letters, digits, hyphens, underscores, or periods.");
        }
        if !instance_ids.insert(id.clone()) {
            return fail(format!("Invalid or duplicate plug-in layout: {id}."));
        }
        descriptor_component(plugin, kind)?;
        if kind == PluginKind::Instrument && (!matches!(owners.channels.get(&owner), Some(Channel::Track(_))) || plugin.slot() != 0) {
            return fail("Instruments require a track's instrument slot.");
        }
        if !slots.insert((owner, kind, plugin.slot())) {
            return fail("Two plug-ins occupy the same routing slot.");
        }
    }
    if project.extra.contains_key("pluginPages") {
        return fail("Plug-in parameter pages must be an object of pages by plug-in instance.");
    }
    if let Some(instance) = project.plugin_pages.iter().flatten().map(|(instance, _)| instance).find(|instance| !instance_ids.contains(*instance)) {
        return fail(format!("Plug-in parameter pages are kept for {instance}, which is no plug-in of the project."));
    }
    Ok(())
}

/// Track groups: every group has its identity, a name and at least one member, its members are
/// next to each other, and its bus is a group bus that names it. A track's `groupID` names a
/// group there is, and a bus's `groupID` likewise.
fn validate_groups(project: &Project, tracks: &[Track]) -> Result<()> {
    if project.extra.contains_key("groups") {
        return fail("Track groups must be an array of objects.");
    }
    let buses = project.routing.as_ref().and_then(|routing| routing.buses.as_deref()).unwrap_or(&[]);
    let mut groups: BTreeMap<&str, &str> = BTreeMap::new();
    for group in project.groups.iter().flatten() {
        let id = identifier(group.id.as_deref(), &group.extra, "id", "Group ID")?;
        let name = match &group.name {
            Some(name) => {
                text(name, "Group name", false)?;
                name.as_str()
            }
            None => return fail("Group name must be text."),
        };
        if ["color", "collapsed", "mixerCollapsed"].iter().any(|key| group.extra.contains_key(*key)) {
            return fail(format!("Group {name} has an unreadable setting."));
        }
        let Some(group_id) = group.id.as_deref() else { return fail("Group ID is missing.") };
        if groups.insert(group_id, name).is_some() {
            return fail(format!("Duplicate group ID: {id}."));
        }
        let bus = group.bus_id.as_deref().and_then(|bus_id| buses.iter().find(|bus| bus.id.as_deref() == Some(bus_id)));
        if !bus.is_some_and(|bus| bus.kind == Some(BusKind::Group) && bus.group_id.as_deref() == Some(group_id)) {
            return fail(format!("Group {name} has no group bus of its own."));
        }
    }
    if let Some(bus) = buses.iter().find(|bus| bus.extra.contains_key("groupID") || bus.group_id.as_deref().is_some_and(|id| !groups.contains_key(id))) {
        return fail(format!("Bus {} belongs to a track group that does not exist.", bus.name.as_deref().unwrap_or("?")));
    }
    // Members are next to each other: a group's run of tracks begins once.
    let mut begun: BTreeSet<&str> = BTreeSet::new();
    let mut previous: Option<&str> = None;
    for (index, track) in tracks.iter().enumerate() {
        if track.extra.contains_key("groupID") {
            return fail(format!("{}: the group it belongs to must be named by its ID.", track.display_name(index)));
        }
        let group = track.group_id.as_deref();
        if let Some(id) = group {
            let Some(name) = groups.get(id) else {
                return fail(format!("{} belongs to a track group that does not exist ({id}).", track.display_name(index)));
            };
            if previous != group && !begun.insert(id) {
                return fail(format!("The tracks of group {name} must be next to each other."));
            }
        }
        previous = group;
    }
    if let Some((_, name)) = groups.iter().find(|(id, _)| !begun.contains(*id)) {
        return fail(format!("Group {name} has no tracks."));
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

/// Owner automation lanes and plug-in parameter locks.
fn validate_patterns(patterns: &[Pattern], owners: &Owners) -> Result<()> {
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
            if lane.owner.is_none() && !lane.extra.contains_key("owner") {
                // A lane of the engine's own track parameters, which skips names it does not
                // know. What the mixer plays is addressed by owner: here it would play nothing.
                if matches!(lane.target.as_deref().and_then(AutomationTarget::parse), Some(AutomationTarget::Send(_) | AutomationTarget::Plugin { .. })) {
                    return fail("Send and plug-in automation lanes name their channel by owner, not by track index.");
                }
                continue;
            }
            if lane.track.is_some() || lane.extra.contains_key("track") {
                return fail("An automation lane names its channel by owner or its track by index, not both.");
            }
            let owner = lane.owner.as_deref().unwrap_or("");
            if lane.extra.contains_key("interpolation") {
                return fail("Unknown automation interpolation.");
            }
            if let Some(Owner::Instrument(index)) = Owner::parse(owner) {
                if index >= owners.instruments {
                    return fail(format!("Automation lane owner {owner} does not exist."));
                }
                // The lane plays on every track that plays the instrument; it controls a parameter
                // of an effect of the instrument's chain, and nothing else.
                let target = lane.target.as_deref().unwrap_or("");
                match AutomationTarget::parse(target) {
                    Some(AutomationTarget::Plugin { instrument: false, slot, .. }) if slot < INSERT_SLOTS => validate_points(lane, length, (0.0, 1.0))?,
                    _ => return fail(format!("An instrument's automation lanes control parameters of its effects, not {target}.")),
                }
                if !destinations.insert((owner.to_string(), target.to_string())) {
                    return fail("Two automation lanes of a pattern control the same destination.");
                }
                continue;
            }
            let (Some(channel), Some(target)) = (owners.channels.get(owner), lane.target.as_deref()) else {
                return fail("Host automation requires an existing owner and a target.");
            };
            let is_track = matches!(channel, Channel::Track(_));
            let range = match AutomationTarget::parse(target) {
                // A track's own parameters are the engine's: their lanes name the track by index.
                Some(AutomationTarget::Native(_)) if is_track => {
                    return fail("Automation of a track's own parameters names the track by index, not by owner.");
                }
                Some(AutomationTarget::Native(name)) if name == "volume" => (0.0, 2.0),
                Some(AutomationTarget::Native(name)) if name == "pan" => {
                    if owner == "master" {
                        return fail("Master pan automation is unsupported.");
                    }
                    (-1.0, 1.0)
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

/// A Track Freeze record.
fn validate_freeze(record: &FreezeRecord) -> Result<()> {
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

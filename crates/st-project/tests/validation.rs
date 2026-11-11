//! The limits the host enforces. Each case changes one thing in a valid project and expects the
//! message a user would read.

mod common;

use serde_json::{json, Value};
use st_project::model::FORMAT_VERSION;
use st_project::validate::*;
use st_project::Project;

/// A valid project: two tracks, a group bus and a return bus.
fn base() -> Value {
    json!({
        "version": 2, "name": "Valid", "bpm": 120, "masterVolume": 0.7,
        "tracks": [{ "id": 0, "routingID": "t0", "volume": 0.8, "pan": 0 }, { "id": 1, "routingID": "t1" }],
        "instruments": [{ "id": 0, "type": "sample" }],
        "patterns": [{ "id": 0, "length": 16, "steps": [[], []], "automation": [] }],
        "sequence": [0],
        "routing": { "buses": [{ "id": "g", "name": "Group", "type": "group" }, { "id": "r", "name": "Return", "type": "return" }],
            "outputs": { "track:t0": "bus:g", "track:t1": null }, "sends": [], "sidechains": [] },
        "audioPlugins": []
    })
}

fn check(value: Value) -> Result<(), String> {
    Project::from_value(value).unwrap().validate().map_err(|error| error.to_string())
}

/// Applies `change` to the valid project and returns the validation message.
fn rejected(change: impl FnOnce(&mut Value)) -> String {
    let mut value = base();
    change(&mut value);
    check(value).expect_err("the change must be rejected")
}

fn accepted(change: impl FnOnce(&mut Value)) {
    let mut value = base();
    change(&mut value);
    if let Err(message) = check(value) {
        panic!("rejected: {message}");
    }
}

fn insert(owner: &str, slot: i64) -> Value {
    json!({ "instanceID": format!("{}-{slot}", owner.replace(':', "-")), "owner": owner, "kind": "effect", "slot": slot, "component": "stfx:eq", "format": "Builtin", "state": "AA==" })
}

#[test]
fn the_base_project_is_valid() {
    accepted(|_| {});
    common::minimal(None).validate().unwrap();
}

#[test]
fn version_tempo_and_sizes() {
    // One number is read; every other one, and a project without one, gets the same message.
    assert_eq!(base()["version"], json!(FORMAT_VERSION));
    for version in [json!(FORMAT_VERSION - 1), json!(FORMAT_VERSION + 1), json!(0), json!(FORMAT_VERSION.to_string()), json!(FORMAT_VERSION as f64 + 0.5), Value::Null] {
        assert_eq!(rejected(|p| p["version"] = version), OTHER_VERSION);
    }
    assert_eq!(rejected(|p| { p.as_object_mut().unwrap().remove("version"); }), OTHER_VERSION);
    assert_eq!(OTHER_VERSION, "This project was made by a different version of SuperTracker and cannot be opened by this one.");
    // The version is checked first: whatever else another format holds is not looked at.
    assert_eq!(rejected(|p| { p["version"] = json!(1); p.as_object_mut().unwrap().remove("routing"); p["bpm"] = json!(0); }), OTHER_VERSION);
    for bpm in [json!(-10.0), json!(19.9), json!(400.5), json!("120"), Value::Null] {
        assert_eq!(rejected(|p| p["bpm"] = bpm), "Project tempo must be between 20 and 400 BPM.");
    }
    assert_eq!(rejected(|p| { p.as_object_mut().unwrap().remove("bpm"); }), "Project tempo must be between 20 and 400 BPM.");
    accepted(|p| p["bpm"] = json!(20));
    accepted(|p| p["bpm"] = json!(400));
    let sizes = "The project must contain 1–1024 tracks, up to 1024 instruments, and 1–256 patterns.";
    assert_eq!(rejected(|p| p["tracks"] = json!([])), sizes);
    assert_eq!(rejected(|p| p["tracks"] = json!("none")), sizes);
    assert_eq!(rejected(|p| p["patterns"] = json!([])), sizes);
    assert_eq!(rejected(|p| { p.as_object_mut().unwrap().remove("instruments"); }), sizes);
    assert_eq!(rejected(|p| p["instruments"] = json!(vec![json!({}); MAX_INSTRUMENTS + 1])), sizes);
    assert_eq!(rejected(|p| p["tracks"] = json!((0..=MAX_TRACKS).map(|index| json!({ "routingID": format!("t{index}") })).collect::<Vec<_>>())), sizes);
    assert_eq!(rejected(|p| p["patterns"] = json!((0..=MAX_PATTERNS).map(|index| json!({ "id": index, "length": 1, "steps": [] })).collect::<Vec<_>>())), sizes);
    accepted(|p| p["instruments"] = json!(vec![json!({}); MAX_INSTRUMENTS]));
    accepted(|p| {
        p["tracks"] = json!((0..MAX_TRACKS).map(|index| json!({ "routingID": format!("t{index}") })).collect::<Vec<_>>());
        p["routing"]["outputs"] = json!({});
    });
}

#[test]
fn patterns_and_the_song_sequence() {
    let dimensions = "A pattern has invalid dimensions (maximum 256 rows and 1024 tracks).";
    for length in [json!(0), json!(257), json!(16.5), Value::Null] {
        assert_eq!(rejected(|p| p["patterns"][0]["length"] = length), dimensions);
    }
    assert_eq!(rejected(|p| { p["patterns"][0].as_object_mut().unwrap().remove("steps"); }), dimensions);
    assert_eq!(rejected(|p| p["patterns"][0]["steps"] = json!(vec![json!([]); MAX_TRACKS + 1])), dimensions);
    assert_eq!(rejected(|p| p["patterns"][0]["steps"] = json!([[5]])), dimensions, "a row is an object or null");
    accepted(|p| p["patterns"][0]["length"] = json!(256));
    let ids = "Pattern identifiers must be unique numbers from 0 to 255.";
    assert_eq!(rejected(|p| p["patterns"][0]["id"] = json!(256)), ids);
    assert_eq!(rejected(|p| p["patterns"][0]["id"] = json!(-1)), ids);
    assert_eq!(rejected(|p| p["patterns"] = json!([{ "id": 3, "length": 4, "steps": [] }, { "id": 3, "length": 4, "steps": [] }])), ids);
    // Without an id a pattern's number is its position.
    assert_eq!(rejected(|p| p["patterns"] = json!([{ "id": 1, "length": 4, "steps": [] }, { "length": 4, "steps": [] }])), ids);
    let sequence = "The song sequence must reference existing patterns (up to 1024 entries).";
    assert_eq!(rejected(|p| p["sequence"] = json!([])), sequence);
    assert_eq!(rejected(|p| p["sequence"] = json!([0, 7])), sequence);
    assert_eq!(rejected(|p| p["sequence"] = json!([0.5])), sequence);
    assert_eq!(rejected(|p| p["sequence"] = json!(vec![0; MAX_SEQUENCE + 1])), sequence);
    accepted(|p| p["sequence"] = json!(vec![0; MAX_SEQUENCE]));
    accepted(|p| { p.as_object_mut().unwrap().remove("sequence"); });
}

#[test]
fn zones_and_audio_clips() {
    let zone = json!({ "samplePath": "/a.wav" });
    assert_eq!(rejected(|p| p["instruments"][0]["zones"] = json!(vec![zone.clone(); MAX_ZONES + 1])), "An instrument can contain up to 128 sample zones.");
    accepted(|p| p["instruments"][0]["zones"] = json!(vec![zone.clone(); MAX_ZONES]));
    let clip = json!({ "id": "c", "track": 1, "path": "/a.wav", "startBeat": 0, "durationBeats": 4 });
    accepted(|p| p["audioClips"] = json!(vec![clip.clone(); MAX_AUDIO_CLIPS]));
    assert_eq!(rejected(|p| p["audioClips"] = json!(vec![clip.clone(); MAX_AUDIO_CLIPS + 1])), "A song can contain up to 256 audio clips.");
    let invalid = "An audio clip has an invalid track, position, or duration.";
    for (key, value) in [("track", json!(2)), ("track", json!(-1)), ("startBeat", json!(-0.5)), ("durationBeats", json!(0)), ("durationBeats", json!(-4))] {
        let mut changed = clip.clone();
        changed[key] = value;
        assert_eq!(rejected(|p| p["audioClips"] = json!([changed])), invalid, "{key}");
    }
    assert_eq!(rejected(|p| p["audioClips"] = json!([{ "id": "c", "track": 0, "startBeat": 0 }])), invalid, "a clip needs a duration");
}

#[test]
fn numbers_set_in_code_must_be_finite() {
    let mut project = Project::from_value(base()).unwrap();
    project.tracks.as_mut().unwrap()[0].delay_send = Some(f64::NAN);
    assert_eq!(project.validate().unwrap_err().to_string(), "Project contains values that cannot be saved.");
    let mut project = Project::from_value(base()).unwrap();
    project.instruments.as_mut().unwrap()[0].cutoff = Some(f64::INFINITY);
    assert_eq!(project.validate().unwrap_err().to_string(), "Project contains values that cannot be saved.");
    // JSON itself cannot carry such a number.
    assert!(Project::from_json(r#"{"bpm":1e999}"#).is_err());
}

#[test]
fn routing_identities_and_mix_ranges() {
    assert_eq!(rejected(|p| { p["tracks"][1].as_object_mut().unwrap().remove("routingID"); }), "Track 2 routingID is missing.");
    assert_eq!(rejected(|p| p["tracks"][1]["routingID"] = json!("t0")), "Duplicate track routingID: t0.");
    assert_eq!(rejected(|p| p["tracks"][1]["routingID"] = json!("has space")), "Track 2 routingID must contain 1–128 letters, digits, hyphens, underscores, or periods.");
    assert_eq!(rejected(|p| p["tracks"][1]["routingID"] = json!(7)), "Track 2 routingID must be text.");
    assert_eq!(rejected(|p| p["tracks"][0]["volume"] = json!(2.5)), "Track 1 volume must be between 0 and 2.");
    assert_eq!(rejected(|p| p["tracks"][0]["volume"] = json!("loud")), "Track 1 volume must be a number.");
    assert_eq!(rejected(|p| p["tracks"][0]["pan"] = json!(-1.01)), "Track 1 pan must be between -1 and 1.");
    assert_eq!(rejected(|p| p["tracks"][0]["mute"] = json!(1)), "Track 1 mute must be true or false.");
    assert_eq!(rejected(|p| p["masterVolume"] = json!(3)), "Master volume must be between 0 and 2.");
    // Every project has its routing table; an empty one is tracks into the master.
    assert_eq!(rejected(|p| { p.as_object_mut().unwrap().remove("routing"); }), "The project has no routing table.");
    assert_eq!(rejected(|p| p["routing"] = json!("on")), "The project's routing must be an object.");
    // The table has its four collections, empty or not.
    accepted(|p| p["routing"] = json!({ "buses": [], "outputs": {}, "sends": [], "sidechains": [] }));
    assert_eq!(rejected(|p| p["routing"] = json!({})), "The project's routing table has no buses.");
    for key in ["buses", "outputs", "sends", "sidechains"] {
        assert_eq!(rejected(|p| { p["routing"].as_object_mut().unwrap().remove(key); }), format!("The project's routing table has no {key}."));
    }
    // The routing table has no version or mix model of its own: like any key the format does
    // not know, they are kept and mean nothing.
    accepted(|p| { p["routing"]["version"] = json!(2); p["routing"]["mixModel"] = json!("future"); });
}

#[test]
fn buses_outputs_sends_and_sidechains() {
    let bus = "Bus IDs must be unique and bus type must be group or return.";
    assert_eq!(rejected(|p| p["routing"]["buses"][1]["id"] = json!("g")), bus);
    assert_eq!(rejected(|p| p["routing"]["buses"][1]["type"] = json!("master")), bus);
    assert_eq!(rejected(|p| p["routing"]["buses"][0]["id"] = json!("a:b")), "Bus ID must contain 1–128 letters, digits, hyphens, underscores, or periods.");
    assert_eq!(rejected(|p| p["routing"]["buses"][0]["name"] = json!("  ")), "Bus name must be valid non-control text.");
    assert_eq!(rejected(|p| p["routing"]["buses"][0]["volume"] = json!(-1)), "Bus volume must be between 0 and 2.");
    assert_eq!(rejected(|p| p["routing"]["outputs"]["track:gone"] = json!("master")), "Output source must reference an existing track or bus.");
    assert_eq!(rejected(|p| p["routing"]["outputs"]["track:t0"] = json!("bus:gone")), "Output destination must reference an existing owner.");
    assert_eq!(rejected(|p| p["routing"]["outputs"]["master"] = json!("bus:g")), "Output source must reference an existing track or bus.");
    assert_eq!(rejected(|p| p["routing"]["outputs"] = json!({ "track:t0": 5 })), "Routing outputs must be an object whose values are owners or null.");

    let send = |id: &str, source: &str, destination: &str| json!({ "id": id, "source": source, "tap": "postFader", "destination": destination, "gain": 1, "enabled": true });
    accepted(|p| p["routing"]["sends"] = json!([send("s1", "track:t0", "bus:r"), send("s2", "bus:g", "master")]));
    assert_eq!(rejected(|p| p["routing"]["sends"] = json!([send("s1", "track:t0", "bus:r"), send("s1", "track:t1", "bus:r")])), "Duplicate route ID: s1.");
    assert_eq!(rejected(|p| p["routing"]["sends"] = json!([send("s1", "master", "bus:r")])), "Route source must reference an existing track or bus.");
    assert_eq!(rejected(|p| p["routing"]["sends"] = json!([send("s1", "track:t0", "bus:gone")])), "Send destination must reference an existing owner.");
    assert_eq!(rejected(|p| { p["routing"]["sends"] = json!([send("s1", "track:t0", "bus:r")]); p["routing"]["sends"][0]["tap"] = json!("sideways"); }), "Unknown routing tap.");
    assert_eq!(rejected(|p| { p["routing"]["sends"] = json!([send("s1", "track:t0", "bus:r")]); p["routing"]["sends"][0]["gain"] = json!(2.1); }), "Route gain must be between 0 and 2.");
    assert_eq!(rejected(|p| { p["routing"]["sends"] = json!([send("s1", "track:t0", "bus:r")]); p["routing"]["sends"][0]["enabled"] = json!("yes"); }), "Route enabled must be true or false.");
    let many = |count: usize| json!((0..count).map(|index| send(&format!("s{index}"), "track:t0", "bus:r")).collect::<Vec<_>>());
    accepted(|p| p["routing"]["sends"] = many(MAX_ROUTES));
    assert_eq!(rejected(|p| p["routing"]["sends"] = many(MAX_ROUTES + 1)), "Routing supports at most 128 sends and sidechains combined.");

    let sidechain = json!({ "id": "sc", "source": "track:gone", "tap": "preFader", "destination": "comp" });
    accepted(|p| p["routing"]["sidechains"] = json!([sidechain.clone()]));
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0]["source"] = json!("master"); }), "Sidechain source must reference a track or bus ID.");
    // The destination is the insert, by its instance identity, and nothing else.
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0].as_object_mut().unwrap().remove("destination"); }), "Sidechain destination is missing.");
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0]["destination"] = json!({ "instanceID": "comp" }); }), "Sidechain destination must be text.");
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0]["destination"] = json!("no spaces"); }), "Sidechain destination must contain 1–128 letters, digits, hyphens, underscores, or periods.");
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sends"] = json!([send("sc", "track:t0", "bus:r")]); }), "Duplicate route ID: sc.");
}

#[test]
fn feedback() {
    let send = |id: &str, source: &str, destination: &str, enabled: bool| json!({ "id": id, "source": source, "tap": "preFader", "destination": destination, "enabled": enabled });
    let message = rejected(|p| p["routing"]["outputs"] = json!({ "bus:g": "bus:r", "bus:r": "bus:g" }));
    assert_eq!(message, "Routing feedback is unsupported. Cycle includes: bus:g, bus:r.");
    // track:t0 outputs to bus:g; a send from bus:g back into track:t0 closes a loop. A send from bus:r does not.
    assert_eq!(rejected(|p| p["routing"]["sends"] = json!([send("s", "bus:g", "track:t0", true)])), "Routing feedback is unsupported. Cycle includes: bus:g, track:t0.");
    accepted(|p| p["routing"]["sends"] = json!([send("s", "bus:r", "track:t0", true)]));
    assert!(rejected(|p| p["routing"]["sends"] = json!([send("s", "bus:g", "bus:g", true)])).contains("bus:g"), "a channel cannot send to itself");
    assert!(rejected(|p| { p["routing"]["outputs"]["bus:r"] = json!("bus:g"); p["routing"]["sends"] = json!([send("s", "bus:g", "bus:r", true)]); }).contains("Cycle"));
    // A disabled send is not a connection.
    accepted(|p| { p["routing"]["outputs"]["bus:r"] = json!("bus:g"); p["routing"]["sends"] = json!([send("s", "bus:g", "bus:r", false)]); });
}

#[test]
fn plug_in_descriptors() {
    // Sixteen inserts per channel: slots 0..=15 on a track, a bus and master; not slot 16.
    accepted(|p| {
        p["audioPlugins"] = json!(["track:t0", "bus:g", "master"].iter().flat_map(|owner| (0..16).map(move |slot| insert(owner, slot))).collect::<Vec<_>>());
    });
    assert_eq!(rejected(|p| p["audioPlugins"] = json!([insert("bus:g", 16)])), "Plug-in slot must be a whole number from 0 to 15.");
    assert_eq!(rejected(|p| p["audioPlugins"] = json!([insert("track:gone", 0)])), "Plug-in owner track:gone does not exist.");
    assert_eq!(rejected(|p| p["audioPlugins"] = json!([insert("track:t0", 0), insert("track:t0", 0)])), "Invalid or duplicate plug-in layout: track-t0-0.");
    assert_eq!(rejected(|p| { p["audioPlugins"] = json!([insert("track:t0", 0), insert("track:t0", 0)]); p["audioPlugins"][1]["instanceID"] = json!("other"); }), "Two plug-ins occupy the same routing slot.");
    assert_eq!(rejected(|p| { p["audioPlugins"] = json!([insert("track:t0", 0)]); p["audioPlugins"][0]["kind"] = json!("generator"); }), "Unknown plug-in slot kind.");
    assert_eq!(rejected(|p| { p["audioPlugins"] = json!([insert("track:t0", 0)]); p["audioPlugins"][0]["instanceID"] = json!("no spaces"); }), "Plug-in instance ID must contain 1–128 letters, digits, hyphens, underscores, or periods.");
    assert_eq!(rejected(|p| { p["audioPlugins"] = json!([insert("bus:g", 0)]); p["audioPlugins"][0]["kind"] = json!("instrument"); }), "Instruments require a track's instrument slot.");
    // The owner is the only way a descriptor names its channel: a track index is an unknown key.
    assert_eq!(rejected(|p| p["audioPlugins"] = json!([{ "track": 0, "kind": "effect", "slot": 0 }])), "A plug-in has no owner.");
    assert_eq!(rejected(|p| p["audioPlugins"] = json!([{ "owner": 0, "kind": "effect", "slot": 0 }])), "Plug-in owner must be text.");
    assert_eq!(rejected(|p| p["audioPlugins"] = json!([{ "owner": "instrument:0", "kind": "instrument", "slot": 0 }])), "Instruments require a track's instrument slot.");
    // An instrument and an effect share slot 0 of a track; an instrument's insert chain has its own slots.
    accepted(|p| {
        p["audioPlugins"] = json!([insert("track:t0", 0), insert("instrument:0", 0), insert("instrument:9", 0), { "owner": "track:t0", "kind": "instrument", "slot": 0, "instanceID": "synth", "component": "clap:x" }]);
    });
    assert_eq!(rejected(|p| p["audioPlugins"] = json!({})), "Project plug-ins must be an array of objects.");
}

#[test]
fn automation_lanes() {
    let lane = |owner: &str, target: &str, points: Value| json!({ "id": "l", "owner": owner, "target": target, "interpolation": "linear", "points": points });
    let with = |lanes: Value| move |p: &mut Value| { p["routing"]["sends"] = json!([{ "id": "s1", "source": "track:t0", "tap": "postFader", "destination": "bus:r" }]); p["patterns"][0]["automation"] = lanes; };
    // What the mixer plays is addressed by owner.
    accepted(with(json!([
        lane("bus:g", "volume", json!([{ "row": 0, "value": 2 }, { "row": 16, "value": 0 }])),
        lane("bus:g", "pan", json!([{ "row": 0.5, "value": -1 }])),
        lane("bus:r", "pan", json!([])),
        lane("master", "volume", json!([{ "row": 1, "value": 1 }])),
        lane("track:t0", "send:s1", json!([{ "row": 1, "value": 2 }])),
        lane("track:t0", "send:removed", json!([{ "row": 1, "value": 1 }])),
        lane("track:t0", "plugin:effect:3:42", json!([{ "row": 1, "value": 1 }])),
        lane("track:t0", "plugin:instrument:0:18446744073709551615", json!([{ "row": 1, "value": 0 }])),
        lane("instrument:0", "plugin:effect:2:1", json!([{ "row": 1, "value": 0.5 }])),
    ])));
    assert_eq!(rejected(with(json!([lane("track:gone", "send:s1", json!([]))]))), "Host automation requires an existing owner and a target.");
    assert_eq!(rejected(with(json!([lane("master", "pan", json!([]))]))), "Master pan automation is unsupported.");
    assert_eq!(rejected(with(json!([lane("bus:g", "cutoff", json!([]))]))), "Unknown host automation target: cutoff.");
    assert_eq!(rejected(with(json!([lane("track:t1", "send:s1", json!([]))]))), "Send automation owner does not match its source.");
    assert_eq!(rejected(with(json!([lane("track:t0", "plugin:instrument:1:5", json!([]))]))), "Invalid instrument automation slot.");
    assert_eq!(rejected(with(json!([lane("bus:g", "plugin:effect:16:5", json!([]))]))), "Unknown host automation target: plugin:effect:16:5.");
    assert_eq!(rejected(with(json!([lane("bus:g", "volume", json!([{ "row": 17, "value": 1 }]))]))), "Automation row must be a finite number in 0...16.");
    assert_eq!(rejected(with(json!([lane("bus:g", "volume", json!([{ "row": 1, "value": 2.5 }]))]))), "Automation value must be a finite number in 0...2.");
    assert_eq!(rejected(with(json!([lane("bus:g", "pan", json!([{ "row": 1, "value": 1.5 }]))]))), "Automation value must be a finite number in -1...1.");
    assert_eq!(rejected(with(json!([lane("track:t0", "plugin:effect:0:1", json!([{ "row": 1, "value": 1.5 }]))]))), "Automation value must be a finite number in 0...1.");
    assert_eq!(rejected(with(json!([lane("bus:g", "volume", json!([{ "row": 1, "value": 1 }, { "row": 1, "value": 0 }]))]))), "Duplicate automation point row.");
    assert_eq!(rejected(with(json!([lane("bus:g", "volume", json!([{ "row": 1 }]))]))), "Automation points need a numeric row and value.");
    assert_eq!(rejected(with(json!([lane("bus:g", "volume", json!([])), lane("bus:g", "volume", json!([]))]))), "Two automation lanes of a pattern control the same destination.");
    let dense = |count: usize| json!((0..count).map(|index| json!({ "row": index as f64 / 512.0, "value": 1 })).collect::<Vec<_>>());
    accepted(with(json!([lane("bus:g", "volume", dense(MAX_AUTOMATION_POINTS))])));
    assert_eq!(rejected(with(json!([lane("bus:g", "volume", dense(MAX_AUTOMATION_POINTS + 1))]))), "Automation supports at most 4096 points per lane.");
    assert_eq!(rejected(|p| p["patterns"][0]["automation"] = json!([{ "owner": "bus:g", "target": "volume", "interpolation": "curved" }])), "Unknown automation interpolation.");

    // A track's own parameters are the engine's: their lanes name the track by index, and
    // names the engine does not know are skipped there, not an error here.
    accepted(|p| p["patterns"][0]["automation"] = json!([{ "track": 0, "target": "volume", "points": [{ "row": 0, "value": 1 }] }, { "track": 1, "target": "cutoff", "points": [] }]));
    accepted(|p| p["patterns"][0]["automation"] = json!([{ "track": 0, "target": "warp", "points": [{ "row": 99, "value": 5 }] }]));
    // One way to address each kind of lane.
    let by_owner = "Automation of a track's own parameters names the track by index, not by owner.";
    for target in ["volume", "pan", "cutoff", "warp"] {
        assert_eq!(rejected(with(json!([lane("track:t0", target, json!([]))]))), by_owner);
    }
    let by_index = "Send and plug-in automation lanes name their channel by owner, not by track index.";
    for target in ["send:s1", "plugin:effect:0:1", "plugin:instrument:0:1"] {
        assert_eq!(rejected(with(json!([{ "track": 0, "target": target, "points": [] }]))), by_index);
    }
    assert_eq!(rejected(with(json!([{ "track": 0, "owner": "track:t0", "target": "send:s1", "points": [] }]))), "An automation lane names its channel by owner or its track by index, not both.");
    assert_eq!(native_automation_range("grainScan"), Some((-4.0, 4.0)));
    assert_eq!(native_automation_range("warp"), None);
}

#[test]
fn plug_in_parameter_locks() {
    let lock = |address: Value, value: Value| move |p: &mut Value| p["patterns"][0]["steps"][0] = json!([{ "fx1": { "type": "plugin", "instanceID": "x", "address": address, "value": value } }]);
    accepted(lock(json!("18446744073709551615"), json!(1)));
    accepted(lock(json!("0"), json!(0)));
    let address = "Plug-in step FX address must be an exact UInt64 decimal string.";
    for bad in [json!(5), json!("05"), json!("-1"), json!("18446744073709551616"), json!("1.0"), json!("")] {
        assert_eq!(rejected(lock(bad, json!(0.5))), address);
    }
    let value = "Plug-in step FX value must be a finite normalized number from 0 to 1.";
    for bad in [json!(1.5), json!(-0.1), json!("0.5"), json!(true)] {
        assert_eq!(rejected(lock(json!("7"), bad)), value);
    }
    // Other effects are the engine's to clamp.
    accepted(|p| p["patterns"][0]["steps"][0] = json!([{ "note": 60, "fx1": { "type": "ratchet", "value": 99 }, "fx2": { "type": "unknown" } }]));
}

#[test]
fn live_inputs_of_tracks() {
    let input = |value: Value| move |p: &mut Value| p["tracks"][1]["audioInput"] = value;
    accepted(input(json!({ "enabled": true, "deviceUID": "Interface", "channels": [3], "gain": 1 })));
    accepted(input(json!({ "enabled": true, "deviceUID": "Interface", "channels": [255, 0], "gain": 2, "monitor": false })));
    // A route that is switched off keeps whatever was saved of it.
    accepted(input(json!({ "enabled": false })));
    accepted(input(json!({ "enabled": false, "deviceUID": "", "channels": [1, 2] })));
    let channels = "Track 2: The live input needs one input channel (mono) or two different ones (stereo).";
    for bad in [json!([]), json!([1, 1]), json!([0, 1, 2]), json!([-1]), json!([256])] {
        assert_eq!(rejected(input(json!({ "enabled": true, "deviceUID": "Interface", "channels": bad }))), channels);
    }
    assert_eq!(rejected(input(json!({ "enabled": true, "deviceUID": "Interface", "channels": [0], "gain": 2.5 }))), "Track 2: The live input's gain must be between 0 and 2.");
    assert_eq!(rejected(input(json!({ "enabled": true, "channels": [0] }))), "Track 2: An enabled live input needs its audio device and input channels.");
    assert_eq!(rejected(input(json!({ "enabled": true, "deviceUID": "Interface" }))), "Track 2: An enabled live input needs its audio device and input channels.");
    assert_eq!(rejected(input(json!({ "enabled": true, "deviceUID": "Interface", "channels": "left" }))), "Track 2: The live input has an unreadable setting.");
    assert_eq!(rejected(input(json!("Interface"))), "Track 2: the live input must be an object.");
}

#[test]
fn track_freeze_records() {
    let record = json!({ "id": "f", "path": "/p.wav", "sha256": "", "sampleRate": 48000, "frames": 1000, "musicalFrames": 900, "tailFrames": 100, "musicalEndBeat": 16 });
    let with = |change: &dyn Fn(&mut Value)| {
        let mut changed = record.clone();
        change(&mut changed);
        rejected(|p| p["tracks"][0]["frozen"] = changed)
    };
    accepted(|p| p["tracks"][0]["frozen"] = record.clone());
    assert_eq!(with(&|r| r["path"] = json!("")), "Track 1: Freeze record is missing path.");
    assert_eq!(with(&|r| r["frames"] = json!(999)), "Track 1: Freeze record has inconsistent frame counts.");
    assert_eq!(with(&|r| r["sampleRate"] = json!(4000)), "Track 1: Freeze record has inconsistent frame counts.");
    assert_eq!(with(&|r| r["tailFrames"] = json!(-1)), "Track 1: Freeze record has an invalid tailFrames frame count.");
    assert_eq!(with(&|r| r["frames"] = json!(MAX_FREEZE_FRAMES + 1)), "Track 1: Freeze record has an invalid frames frame count.");
    assert_eq!(with(&|r| { r.as_object_mut().unwrap().remove("musicalEndBeat"); }), "Track 1: Freeze record is missing musicalEndBeat.");
    assert_eq!(rejected(|p| { p["tracks"][0]["name"] = json!("Bass"); p["tracks"][0]["frozen"] = json!(true); }), "Bass: the freeze record must be an object.");
}

/// An instrument is a sampler, a plug-in or an external MIDI instrument; anything else is refused
/// instead of being played as a sampler.
#[test]
fn instrument_types() {
    for kind in ["sample", "plugin", "midi"] {
        accepted(|p| p["instruments"][0]["type"] = json!(kind));
    }
    for kind in [json!("synth"), json!("au"), json!(3), Value::Null] {
        assert_eq!(rejected(|p| p["instruments"][0]["type"] = kind), "Instrument 1 has an unknown type.");
    }
    // Keys the format does not name are kept and ignored.
    accepted(|p| p["instruments"][0]["waveform"] = json!("sine"));
    for zones in [json!("zones"), json!({ "path": "x" }), Value::Null] {
        assert_eq!(rejected(|p| p["instruments"][0]["zones"] = zones), "Sample zones must be a list of zones.");
    }
}

/// A built-in effect has one spelling, the one the plug-in catalogue lists.
#[test]
fn built_in_effect_components() {
    use st_project::model::plugin::{builtin_effect_id, BUILTIN_EFFECT_PREFIX};
    assert_eq!(BUILTIN_EFFECT_PREFIX, "stfx:");
    assert_eq!((builtin_effect_id("stfx:eq"), builtin_effect_id("builtin:eq"), builtin_effect_id("com.vendor.eq")), (Some("eq"), None, None));
    let project = Project::from_value(json!({ "audioPlugins": [{ "component": "stfx:reverb" }, { "component": "builtin:reverb" }, {}] })).unwrap();
    assert_eq!(project.plugin_list().iter().map(|plugin| plugin.is_builtin()).collect::<Vec<_>>(), [true, false, false]);
}

/// A plug-in instrument's descriptor is on the tracks that play it; the instrument keeps it only
/// while none does, and then without an owner or an instance.
#[test]
fn the_plug_in_an_instrument_keeps() {
    let kept = json!({ "kind": "instrument", "slot": 0, "component": "com.vendor.synth", "format": "VST3", "name": "Synth", "state": "AA==" });
    let unplayed = |p: &mut Value| {
        p["instruments"] = json!([{ "id": 0, "type": "sample" }, { "id": 1, "type": "plugin", "plugin": kept.clone() }]);
    };
    accepted(unplayed);
    // Played with its plug-in on the track: the instrument holds no second copy.
    let played = |p: &mut Value| {
        unplayed(p);
        p["tracks"][1]["instrument"] = json!(1);
        p["audioPlugins"] = json!([{ "owner": "track:t1", "kind": "instrument", "slot": 0, "instanceID": "synth-1", "component": "com.vendor.synth" }]);
    };
    assert_eq!(rejected(played), "Instrument 2 keeps a second copy of the plug-in a track plays it with.");
    accepted(|p| { played(p); p["instruments"][1].as_object_mut().unwrap().remove("plugin"); });
    assert_eq!(rejected(|p| { unplayed(p); p["instruments"][1]["type"] = json!("sample"); }), "Instrument 2 keeps a plug-in but is not a plug-in instrument.");
    assert_eq!(rejected(|p| { unplayed(p); p["instruments"][1]["plugin"]["owner"] = json!("track:t0"); }), "Instrument 2: a plug-in kept by its instrument has no owner and no instance.");
    assert_eq!(rejected(|p| { unplayed(p); p["instruments"][1]["plugin"]["instanceID"] = json!("synth-1"); }), "Instrument 2: a plug-in kept by its instrument has no owner and no instance.");
    assert_eq!(rejected(|p| { unplayed(p); p["instruments"][1]["plugin"] = Value::Null; }), "Instrument 2: the plug-in it keeps must be an object.");
}

#[test]
fn identifiers() {
    assert!(is_identifier("a-b_c.9") && is_identifier(&"x".repeat(128)));
    assert!(!is_identifier("") && !is_identifier(&"x".repeat(129)) && !is_identifier("a:b") && !is_identifier("é"));
}

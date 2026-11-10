//! The limits the host enforces. Each case changes one thing in a valid project and expects the
//! message a user would read. Sources: ProjectStore.validate, AudioRouting.parse,
//! AudioRoutingAutomation, AudioPluginStepFX, AudioFreezeTrack.Record.parse (Swift host) and the
//! assertions of tests/ProjectStoreTests.swift that concern validation.

mod common;

use serde_json::{json, Value};
use st_project::validate::*;
use st_project::Project;

/// A valid project on the standard mixer: two tracks, a group bus and a return bus.
fn base() -> Value {
    json!({
        "version": 1, "name": "Valid", "bpm": 120, "masterVolume": 0.7,
        "tracks": [{ "id": 0, "routingID": "t0", "volume": 0.8, "pan": 0 }, { "id": 1, "routingID": "t1" }],
        "instruments": [{ "id": 0, "type": "sample" }],
        "patterns": [{ "id": 0, "length": 16, "steps": [[], []], "automation": [] }],
        "sequence": [0],
        "routing": { "version": 1, "mixModel": "standard",
            "buses": [{ "id": "g", "name": "Group", "type": "group" }, { "id": "r", "name": "Return", "type": "return" }],
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
    json!({ "instanceID": format!("{}-{slot}", owner.replace(':', "-")), "owner": owner, "kind": "effect", "slot": slot, "component": "builtin:eq", "format": "Builtin", "state": "AA==" })
}

#[test]
fn the_base_project_is_valid() {
    accepted(|_| {});
    // The minimal project of the Swift store tests: no routing at all (the flat mixer).
    common::minimal(None).validate().unwrap();
}

#[test]
fn version_tempo_and_sizes() {
    assert_eq!(rejected(|p| p["version"] = json!(100)), "This project uses an unsupported format version.");
    assert_eq!(rejected(|p| p["version"] = json!("1")), "This project uses an unsupported format version.");
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
    assert_eq!(rejected(|p| { p["tracks"][1].as_object_mut().unwrap().remove("routingID"); }), "Track 2 has no stable routingID.");
    assert_eq!(rejected(|p| p["tracks"][1]["routingID"] = json!("t0")), "Duplicate track routingID: t0.");
    assert_eq!(rejected(|p| p["tracks"][1]["routingID"] = json!("has space")), "Track 2 routingID must contain 1–128 letters, digits, hyphens, underscores, or periods.");
    assert_eq!(rejected(|p| p["tracks"][1]["routingID"] = json!(7)), "Track 2 routingID must be text.");
    assert_eq!(rejected(|p| p["tracks"][0]["volume"] = json!(2.5)), "Track 1 volume must be between 0 and 2.");
    assert_eq!(rejected(|p| p["tracks"][0]["volume"] = json!("loud")), "Track 1 volume must be a number.");
    assert_eq!(rejected(|p| p["tracks"][0]["pan"] = json!(-1.01)), "Track 1 pan must be between -1 and 1.");
    assert_eq!(rejected(|p| p["tracks"][0]["mute"] = json!(1)), "Track 1 mute must be true or false.");
    assert_eq!(rejected(|p| p["masterVolume"] = json!(3)), "Master volume must be between 0 and 2.");
    let version = "Routing requires version 1 and a legacy or standard mixModel.";
    assert_eq!(rejected(|p| p["routing"]["version"] = json!(2)), version);
    assert_eq!(rejected(|p| p["routing"]["mixModel"] = json!("future")), version);
    assert_eq!(rejected(|p| p["routing"] = json!("on")), version);
    // Without `routing` (the flat mixer) tracks need no identity.
    accepted(|p| {
        p.as_object_mut().unwrap().remove("routing");
        p["tracks"] = json!([{}, {}]);
    });
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

    let sidechain = json!({ "id": "sc", "source": "track:gone", "tap": "preFader", "destination": { "instanceID": "comp", "inputBus": 1, "layoutSignature": { "name": "", "channels": 2 } } });
    accepted(|p| p["routing"]["sidechains"] = json!([sidechain.clone()]));
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0]["source"] = json!("master"); }), "Sidechain source must reference a track or bus ID.");
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0]["destination"].as_object_mut().unwrap().remove("layoutSignature"); }), "Sidechains require a destination and saved input layout.");
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0]["destination"]["inputBus"] = json!(32); }), "Sidechain input bus must be a whole number from 0 to 31.");
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sidechains"][0]["destination"]["layoutSignature"]["channels"] = json!(0); }), "Sidechain input channels must be a whole number from 1 to 64.");
    assert_eq!(rejected(|p| { p["routing"]["sidechains"] = json!([sidechain.clone()]); p["routing"]["sends"] = json!([send("sc", "track:t0", "bus:r")]); }), "Duplicate route ID: sc.");
}

#[test]
fn feedback_and_the_legacy_mix_model() {
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

    let legacy = "Enable standard routing to use buses, custom sends or sidechains.";
    assert_eq!(rejected(|p| p["routing"]["mixModel"] = json!("legacy")), legacy);
    accepted(|p| p["routing"] = json!({ "version": 1, "mixModel": "legacy", "buses": [], "outputs": { "track:t0": "master" }, "sends": [], "sidechains": [] }));
    assert_eq!(rejected(|p| p["routing"] = json!({ "version": 1, "mixModel": "legacy", "outputs": { "track:t0": null } })), legacy);
    assert_eq!(
        rejected(|p| { p["routing"] = json!({ "version": 1, "mixModel": "legacy" }); p["audioPlugins"] = json!([insert("master", 0)]); }),
        "Enable standard routing to use master inserts."
    );
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
    assert_eq!(rejected(|p| p["audioPlugins"] = json!([{ "track": 5, "kind": "effect", "slot": 0 }])), "Plug-in track does not exist.");
    // An instrument and an effect share slot 0 of a track; an instrument's insert chain has its own slots.
    accepted(|p| {
        p["audioPlugins"] = json!([insert("track:t0", 0), insert("instrument:0", 0), insert("instrument:9", 0), { "owner": "track:t0", "kind": "instrument", "slot": 0, "instanceID": "synth", "component": "clap:x" }]);
    });
    assert_eq!(rejected(|p| p["audioPlugins"] = json!({})), "Project plug-ins must be an array of objects.");
}

#[test]
fn owner_automation_lanes() {
    let lane = |owner: &str, target: &str, points: Value| json!({ "id": "l", "owner": owner, "target": target, "interpolation": "linear", "points": points });
    let with = |lanes: Value| move |p: &mut Value| { p["routing"]["sends"] = json!([{ "id": "s1", "source": "track:t0", "tap": "postFader", "destination": "bus:r" }]); p["patterns"][0]["automation"] = lanes; };
    accepted(with(json!([
        lane("track:t0", "volume", json!([{ "row": 0, "value": 2 }, { "row": 16, "value": 0 }])),
        lane("track:t0", "pan", json!([{ "row": 0.5, "value": -1 }])),
        lane("track:t0", "cutoff", json!([{ "row": 0, "value": 20000 }])),
        lane("bus:g", "pan", json!([])),
        lane("master", "volume", json!([{ "row": 1, "value": 1 }])),
        lane("track:t0", "send:s1", json!([{ "row": 1, "value": 2 }])),
        lane("track:t0", "send:removed", json!([{ "row": 1, "value": 1 }])),
        lane("track:t0", "plugin:effect:3:42", json!([{ "row": 1, "value": 1 }])),
        lane("track:t0", "plugin:instrument:0:18446744073709551615", json!([{ "row": 1, "value": 0 }])),
        lane("instrument:0", "plugin:effect:2:1", json!([{ "row": 1, "value": 0.5 }])),
    ])));
    assert_eq!(rejected(with(json!([lane("track:gone", "volume", json!([]))]))), "Host automation requires an existing owner and a target.");
    assert_eq!(rejected(with(json!([lane("master", "pan", json!([]))]))), "Master pan automation is unsupported.");
    assert_eq!(rejected(with(json!([lane("bus:g", "cutoff", json!([]))]))), "Unknown host automation target: cutoff.");
    assert_eq!(rejected(with(json!([lane("track:t0", "warp", json!([]))]))), "Unknown host automation target: warp.");
    assert_eq!(rejected(with(json!([lane("track:t1", "send:s1", json!([]))]))), "Send automation owner does not match its source.");
    assert_eq!(rejected(with(json!([lane("track:t0", "plugin:instrument:1:5", json!([]))]))), "Invalid instrument automation slot.");
    assert_eq!(rejected(with(json!([lane("bus:g", "plugin:effect:16:5", json!([]))]))), "Unknown host automation target: plugin:effect:16:5.");
    assert_eq!(rejected(with(json!([lane("track:t0", "volume", json!([{ "row": 17, "value": 1 }]))]))), "Automation row must be a finite number in 0...16.");
    assert_eq!(rejected(with(json!([lane("track:t0", "volume", json!([{ "row": 1, "value": 2.5 }]))]))), "Automation value must be a finite number in 0...2.");
    assert_eq!(rejected(with(json!([lane("track:t0", "cutoff", json!([{ "row": 1, "value": 30000 }]))]))), "Automation value must be a finite number in 20...20000.");
    assert_eq!(rejected(with(json!([lane("track:t0", "plugin:effect:0:1", json!([{ "row": 1, "value": 1.5 }]))]))), "Automation value must be a finite number in 0...1.");
    assert_eq!(rejected(with(json!([lane("track:t0", "volume", json!([{ "row": 1, "value": 1 }, { "row": 1, "value": 0 }]))]))), "Duplicate automation point row.");
    assert_eq!(rejected(with(json!([lane("track:t0", "volume", json!([{ "row": 1 }]))]))), "Automation points need a numeric row and value.");
    assert_eq!(rejected(with(json!([lane("track:t0", "volume", json!([])), lane("track:t0", "volume", json!([]))]))), "Two automation lanes of a pattern control the same destination.");
    let dense = |count: usize| json!((0..count).map(|index| json!({ "row": index as f64 / 512.0, "value": 1 })).collect::<Vec<_>>());
    accepted(with(json!([lane("track:t0", "volume", dense(MAX_AUTOMATION_POINTS))])));
    assert_eq!(rejected(with(json!([lane("track:t0", "volume", dense(MAX_AUTOMATION_POINTS + 1))]))), "Automation supports at most 4096 points per lane.");
    assert_eq!(rejected(|p| p["patterns"][0]["automation"] = json!([{ "owner": "track:t0", "target": "volume", "interpolation": "curved" }])), "Unknown automation interpolation.");
    // Track-index lanes are the engine's: unknown targets are skipped there, not an error here.
    accepted(|p| p["patterns"][0]["automation"] = json!([{ "track": 0, "target": "warp", "points": [{ "row": 99, "value": 5 }] }]));
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
fn track_freeze_records() {
    let record = json!({ "version": 1, "id": "f", "path": "/p.wav", "sha256": "", "sampleRate": 48000, "frames": 1000, "musicalFrames": 900, "tailFrames": 100, "musicalEndBeat": 16 });
    let with = |change: &dyn Fn(&mut Value)| {
        let mut changed = record.clone();
        change(&mut changed);
        rejected(|p| p["tracks"][0]["frozen"] = changed)
    };
    accepted(|p| p["tracks"][0]["frozen"] = record.clone());
    assert_eq!(with(&|r| r["version"] = json!(2)), "Track 1: Unsupported freeze record version.");
    assert_eq!(with(&|r| r["path"] = json!("")), "Track 1: Freeze record is missing path.");
    assert_eq!(with(&|r| r["frames"] = json!(999)), "Track 1: Freeze record has inconsistent frame counts.");
    assert_eq!(with(&|r| r["sampleRate"] = json!(4000)), "Track 1: Freeze record has inconsistent frame counts.");
    assert_eq!(with(&|r| r["tailFrames"] = json!(-1)), "Track 1: Freeze record has an invalid tailFrames frame count.");
    assert_eq!(with(&|r| r["frames"] = json!(MAX_FREEZE_FRAMES + 1)), "Track 1: Freeze record has an invalid frames frame count.");
    assert_eq!(with(&|r| { r.as_object_mut().unwrap().remove("musicalEndBeat"); }), "Track 1: Freeze record is missing musicalEndBeat.");
    assert_eq!(rejected(|p| { p["tracks"][0]["name"] = json!("Bass"); p["tracks"][0]["frozen"] = json!(true); }), "Bass: the freeze record must be an object.");
}

/// Older layouts are not converted (no ProjectMigration); they are refused with a message that says why.
#[test]
fn older_layouts_are_refused() {
    let older = |what: &str| format!("This project was saved in an older layout ({what}) that this version no longer reads. Open and save it with a version of SuperTracker that converts it, then open it again.");
    assert_eq!(rejected(|p| p["timing"] = json!({ "version": 1, "tempos": [{ "id": "a", "beat": 0, "bpm": 120 }], "meters": [] })), older("a tempo map"));
    assert_eq!(rejected(|p| p["kits"] = json!([])), older("kit lanes"));
    assert_eq!(rejected(|p| p["tracks"][1]["kind"] = json!("kit")), older("kit lanes"));
    assert_eq!(rejected(|p| p["instruments"][0] = json!({ "type": "synth", "waveform": "saw" })), older("the built-in synth instrument"));
    assert_eq!(rejected(|p| p["instruments"][0] = json!({ "type": "drum", "drumModel": "kick" })), older("the built-in synth instrument"));
    assert_eq!(rejected(|p| p["instruments"][0] = json!({ "name": "Old", "waveform": "sine" })), older("the built-in synth instrument"));
    // `waveform` on a sampler is inert leftover, not an old layout.
    accepted(|p| p["instruments"][0] = json!({ "type": "sample", "waveform": "sine", "samplePath": "builtin:wavetables/Basic/Sine.wav" }));
    accepted(|p| p["instruments"][0] = json!({ "waveform": "sine", "samplePath": "/a.wav" }));
}

#[test]
fn identifiers() {
    assert!(is_identifier("a-b_c.9") && is_identifier(&"x".repeat(128)));
    assert!(!is_identifier("") && !is_identifier(&"x".repeat(129)) && !is_identifier("a:b") && !is_identifier("é"));
}

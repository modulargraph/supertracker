//! Kit packages: save, read, overwrite, cancellation, malformed packages, previews, group
//! headers and taking a group out of a session.

mod common;

use common::TempDir;
use serde_json::{json, Map, Value};
use st_library::kit::{self, group, SaveOptions};
use st_project::Cancel;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn tree_bytes(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, folder: &Path, result: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, result);
            } else {
                result.insert(path.strip_prefix(root).unwrap().to_string_lossy().into_owned(), fs::read(&path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(directory, directory, &mut result);
    result
}

/// A session with one kit group of three members (a layered sampler with two zones, an empty
/// sampler and a plug-in instrument), a track outside the group, the group bus, a return bus
/// the group feeds, and routes into and out of the group.
fn session(sample: &str, zone: &str) -> Value {
    json!({
        "version": 3, "name": "Session", "bpm": 123.5, "linesPerBeat": 4,
        "tracks": [
            { "id": 0, "name": "Lead", "routingID": "lead", "instrument": 0, "volume": 0.8 },
            { "id": 1, "name": "Layered kick", "routingID": "kick", "groupID": "g1", "pad": 0, "padNote": 60, "instrument": 2, "volume": 0.63, "pan": -0.23, "chokeGroup": 2,
              "frozen": { "path": "/prints/kick.wav" } },
            { "id": 2, "name": "Empty", "routingID": "empty", "groupID": "g1", "pad": 1, "instrument": 3 },
            { "id": 3, "name": "Synth", "routingID": "synth", "groupID": "g1", "pad": 5, "instrument": 1 }
        ],
        "instruments": [
            { "id": 0, "type": "sample", "name": "Lead", "samplePath": "/elsewhere/lead.wav" },
            { "id": 1, "type": "plugin", "name": "Synth" },
            { "id": 2, "type": "sample", "name": "Layered kick", "samplePath": sample, "rootNote": 36, "start": 0.03, "end": 0.91, "cutoff": 1532.5,
              "zones": [ { "samplePath": zone, "rootNote": 42, "keyLow": 36, "keyHigh": 60, "volume": 0.71 }, { "samplePath": sample, "rootNote": 48, "velocityLow": 80, "velocityHigh": 127 } ] },
            { "id": 3, "type": "sample", "name": "Empty", "samplePath": "" }
        ],
        "groups": [ { "id": "g1", "name": "Fixture kit", "color": "#e2a44f", "collapsed": true, "busID": "kitbus", "libraryPath": "/Users/x/Kits/Fixture kit.stkit", "importNotes": ["A note"] } ],
        "audioPlugins": [
            { "instanceID": "p-synth", "owner": "track:synth", "kind": "instrument", "slot": 0, "component": "vst3:ABCDABCDABCDABCDABCDABCDABCDABCD", "format": "VST3", "name": "Fixture Synth",
              "state": "AAECAwQFAP/+", "opaqueVendorState": { "samplePath": "/vendor/opaque/do-not-copy.wav", "path": "/vendor/data" } },
            { "instanceID": "p-kick-fx", "owner": "instrument:2", "kind": "effect", "slot": 0, "component": "stfx:eq", "format": "Builtin", "state": "e30=" },
            { "instanceID": "p-bus", "owner": "bus:kitbus", "kind": "effect", "slot": 0, "component": "stfx:compressor", "format": "Builtin" },
            { "instanceID": "p-lead", "owner": "track:lead", "kind": "effect", "slot": 0, "component": "stfx:chorus", "format": "Builtin" },
            { "instanceID": "p-return", "owner": "bus:space", "kind": "effect", "slot": 0, "component": "stfx:plate", "format": "Builtin" }
        ],
        "routing": {
            "buses": [ { "id": "kitbus", "name": "Fixture kit", "type": "group", "groupID": "g1", "volume": 0.83, "pan": -0.1 }, { "id": "space", "name": "Space", "type": "return" }, { "id": "other", "name": "Other", "type": "return" } ],
            "outputs": { "track:kick": "bus:kitbus", "track:synth": "track:lead", "bus:kitbus": "master", "track:lead": "bus:other" },
            "sends": [ { "id": "s1", "source": "track:kick", "tap": "postFader", "destination": "bus:space", "gain": 0.4 }, { "id": "s2", "source": "track:empty", "tap": "postFader", "destination": "track:lead", "gain": 0.2 },
                       { "id": "s3", "source": "track:lead", "tap": "postFader", "destination": "bus:space", "gain": 0.9 } ],
            "sidechains": [ { "id": "c1", "source": "track:kick", "tap": "postFader", "destination": "p-bus" }, { "id": "c2", "source": "track:lead", "tap": "postFader", "destination": "p-bus" },
                            { "id": "c3", "source": "track:kick", "tap": "postFader", "destination": "p-lead" } ]
        },
        "pluginPages": { "p-synth": { "name": "Fixture Synth", "pages": [ { "name": "Tone", "addresses": ["0", null] } ] }, "p-lead": { "name": "Chorus", "pages": [] } },
        "patterns": [ { "id": 0, "length": 4, "steps": [[null, null, null, null], [{ "note": 36 }, null, null, null], [null, null, null, null], [null, null, null, null]] } ],
        "sequence": [0]
    })
}

fn header(fields: Value) -> Map<String, Value> {
    fields.as_object().cloned().unwrap()
}

fn sample_paths(project: &Value) -> [String; 3] {
    let instrument = &project["instruments"][0];
    [
        instrument["samplePath"].as_str().unwrap().to_string(),
        instrument["zones"][0]["samplePath"].as_str().unwrap().to_string(),
        instrument["zones"][1]["samplePath"].as_str().unwrap().to_string(),
    ]
}

#[test]
fn a_group_is_taken_out_of_a_session() {
    let project = session("/fixture/kick.wav", "/fixture/zone.wav");
    let original = project.clone();
    let extracted = group::extract_group(&project, "g1").unwrap();
    assert_eq!(project, original, "the session is not modified");
    let kit = &extracted.project;
    group::validate(kit).unwrap();

    let tracks = kit["tracks"].as_array().unwrap();
    assert_eq!(tracks.iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Layered kick", "Empty", "Synth"]);
    assert_eq!(tracks.iter().map(|t| t["id"].as_i64().unwrap()).collect::<Vec<_>>(), [0, 1, 2], "members are renumbered");
    assert_eq!(tracks.iter().map(|t| t["instrument"].as_i64().unwrap()).collect::<Vec<_>>(), [0, 1, 2], "instruments follow member order");
    assert_eq!((tracks[0]["pad"].as_i64(), tracks[0]["chokeGroup"].as_i64(), tracks[0]["volume"].as_f64(), tracks[2]["pad"].as_i64()), (Some(0), Some(2), Some(0.63), Some(5)));
    assert!(tracks[0].get("frozen").is_none(), "a frozen print stays with its session");
    let instruments = kit["instruments"].as_array().unwrap();
    assert_eq!(instruments.iter().map(|i| i["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Layered kick", "Empty", "Synth"]);
    assert_eq!(instruments.iter().map(|i| i["id"].as_i64().unwrap()).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(instruments[0]["zones"][0]["keyHigh"], 60, "instrument settings travel unchanged");

    let group = &kit["groups"][0];
    assert_eq!((group["id"].as_str(), group["name"].as_str(), group["busID"].as_str()), (Some("g1"), Some("Fixture kit"), Some("kitbus")));
    assert!(group.get("libraryPath").is_none(), "a saved kit leaves out the session's library location");
    assert_eq!((&group["importNotes"], &group["collapsed"]), (&json!(["A note"]), &json!(false)));

    let plugins = kit["audioPlugins"].as_array().unwrap();
    let owners: Vec<&str> = plugins.iter().map(|p| p["owner"].as_str().unwrap()).collect();
    assert_eq!(owners, ["track:synth", "instrument:0", "bus:kitbus", "bus:space"], "plug-ins of members, their instruments, the bus and the buses it feeds");
    assert_eq!(plugins[0]["opaqueVendorState"], project["audioPlugins"][0]["opaqueVendorState"], "opaque plug-in state is untouched");

    let routing = &kit["routing"];
    let buses: Vec<&str> = routing["buses"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(buses, ["kitbus", "space"]);
    assert_eq!(routing["buses"][0]["volume"], 0.83);
    assert_eq!(routing["outputs"], json!({ "track:kick": "bus:kitbus", "track:empty": "bus:kitbus", "track:synth": "master", "bus:kitbus": "master", "bus:space": "master" }));
    assert_eq!(routing["sends"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect::<Vec<_>>(), ["s1"]);
    assert_eq!(routing["sidechains"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect::<Vec<_>>(), ["c1"]);
    assert_eq!(kit["pluginPages"].as_object().unwrap().keys().collect::<Vec<_>>(), ["p-synth"]);
    assert_eq!((kit["bpm"].as_f64(), kit["name"].as_str()), (Some(123.5), Some("Fixture kit")));
    assert!(kit.get("patterns").is_none(), "a kit carries no patterns");
    assert_eq!(
        extracted.warnings,
        ["An output to another track was changed to Master in the saved kit.", "Sends to other tracks are not included in this reusable kit.", "Sidechains driven by other tracks are not included in this reusable kit."]
    );
    assert_eq!(extracted.metadata["name"], "Fixture kit");

    assert_eq!(group::loaded_sound_count(kit), 2, "the empty sampler pad is not a loaded sound");
    let sounds = group::sound_summaries(kit);
    assert_eq!(sounds.iter().map(|s| (s.slot, s.name.as_str(), s.kind.as_str(), s.empty)).collect::<Vec<_>>(), [(0, "Layered kick", "sample", false), (1, "Empty", "sample", true), (5, "Synth", "plugin", false)]);
    assert_eq!(sounds[2].plugin, "Fixture Synth");
    assert!(sounds[0].missing, "a sample that is not on disk is reported");

    assert_eq!(group::extract_group(&project, "nope").unwrap_err().to_string(), "Select a group to save as a kit.");
    let mut crowded = kit.clone();
    crowded["instruments"][0]["zones"] = json!(vec![json!({}); 129]);
    assert!(group::validate(&crowded).is_err(), "zone capacity");
    for malformed in [json!(null), json!("zones"), json!({ "path": "x" })] {
        let mut bad = kit.clone();
        bad["instruments"][0]["zones"] = malformed.clone();
        assert_eq!(group::validate(&bad).unwrap_err().to_string(), "The kit cannot be used: Sample zones must be a list of zones.", "{malformed}");
    }
    // What a project may not hold, a kit may not hold either: one validation for both.
    let mut unrouted = kit.clone();
    unrouted["tracks"][0].as_object_mut().unwrap().remove("routingID");
    assert_eq!(group::validate(&unrouted).unwrap_err().to_string(), "The kit cannot be used: Track 1 routingID is missing.");
    let mut other_version = kit.clone();
    other_version["version"] = json!(1);
    assert!(group::validate(&other_version).unwrap_err().to_string().contains("different version"));
    for key in ["tracks", "instruments", "audioPlugins", "groups"] {
        let mut partial = kit.clone();
        partial.as_object_mut().unwrap().remove(key);
        assert_eq!(group::validate(&partial).unwrap_err().to_string(), format!("The kit document has no {key} list."));
    }
    let mut song = kit.clone();
    song["patterns"] = json!([]);
    assert_eq!(group::validate(&song).unwrap_err().to_string(), "A kit document holds no patterns.");
}

#[test]
fn collected_round_trip() {
    let temp = TempDir::new("kit-collected");
    let kick_bytes: Vec<u8> = (0..97).collect();
    let zone_bytes = vec![173u8; 103];
    // The same base name with different bytes makes file name collisions observable.
    temp.write("Source/Audio/Kick.wav", &kick_bytes);
    temp.write("Source/Layers/Kick.wav", &zone_bytes);
    let song = temp.join("Source/Song.supertracker");
    let extracted = group::extract_group(&session("Audio/Kick.wav", "Layers/Kick.wav"), "g1").unwrap();
    let project = extracted.project;
    let original_project = project.clone();
    let original_source = tree_bytes(&temp.join("Source"));
    temp.mkdir("Destination");
    let package = temp.text("Destination/Portable.stkit");
    let fields = header(json!({ "name": "Portable fixture", "author": "Fixture Author", "tags": ["Warm", "Drums"], "description": "Fixture description" }));
    let options = SaveOptions { copy_samples: true, relative_to: Some(song.clone()), ..Default::default() };
    let saved = kit::save(&project, &fields, &package, &options).unwrap();
    assert_eq!(tree_bytes(&temp.join("Source")), original_source, "collecting a kit never writes the source project or samples");
    assert_eq!(project, original_project, "saving does not modify the caller's project");
    assert_eq!((saved.saved, saved.copied_samples, saved.media_bytes), (true, 2, 200));
    assert_eq!(saved.metadata["path"], package.as_str());

    let manifest: Value = serde_json::from_slice(&fs::read(temp.join("Destination/Portable.stkit/kit.json")).unwrap()).unwrap();
    assert_eq!((manifest["format"].as_str(), manifest["version"].as_u64(), manifest["projectFile"].as_str()), (Some("supertracker-kit"), Some(kit::FORMAT_VERSION), Some("project.json")));
    assert_eq!((manifest["soundCount"].as_u64(), manifest["collectedSamples"].as_bool(), manifest["author"].as_str()), (Some(2), Some(true), Some("Fixture Author")));
    assert!(manifest["id"].as_str().is_some_and(|id| !id.is_empty()) && manifest["createdAt"].as_str().is_some());
    let stored: Value = serde_json::from_slice(&fs::read(temp.join("Destination/Portable.stkit/project.json")).unwrap()).unwrap();
    let stored_paths = sample_paths(&stored);
    assert!(stored_paths.iter().all(|path| path.starts_with("Samples/") && !path.contains("..")), "copied samples use package-relative paths: {stored_paths:?}");
    assert_eq!(stored_paths[0], stored_paths[2], "repeated references to one sample reuse its copy");
    assert_ne!(stored_paths[0], stored_paths[1], "different files with equal names do not collide");
    assert_eq!(fs::read(temp.join("Destination/Portable.stkit").join(&stored_paths[0])).unwrap(), kick_bytes);
    assert_eq!(fs::read(temp.join("Destination/Portable.stkit").join(&stored_paths[1])).unwrap(), zone_bytes);
    // The name is <24 hex digits of the SHA-256>-<original name>.
    let digest = st_project::media::sha256_hex(&kick_bytes);
    assert_eq!(stored_paths[0], format!("Samples/{}-Kick.wav", &digest[..24]));
    assert_eq!(stored["audioPlugins"], project["audioPlugins"], "opaque plug-in state survives collection exactly");
    let mut expected = project.clone();
    expected["instruments"][0]["samplePath"] = json!(stored_paths[0]);
    expected["instruments"][0]["zones"][0]["samplePath"] = json!(stored_paths[1]);
    expected["instruments"][0]["zones"][1]["samplePath"] = json!(stored_paths[2]);
    assert_eq!(stored, expected, "collection changes nothing but the sample paths");

    // A move proves that reading resolves members from the opened package, not the save location.
    temp.mkdir("Relocated");
    let moved = temp.text("Relocated/Moved.stkit");
    fs::rename(&package, &moved).unwrap();
    let loaded = kit::read(&moved).unwrap();
    let loaded_paths = sample_paths(&loaded.project);
    for (index, path) in loaded_paths.iter().enumerate() {
        assert!(path.starts_with(&format!("{moved}/Samples/")), "loaded media stays inside the moved kit: {path}");
        assert_eq!(fs::read(path).unwrap(), if index == 1 { zone_bytes.clone() } else { kick_bytes.clone() });
    }
    assert_eq!(loaded.project["audioPlugins"], project["audioPlugins"], "reading leaves opaque plug-in paths untouched");
    assert!(loaded.warnings.is_empty());
    assert_eq!((loaded.metadata.name.as_str(), loaded.metadata.sound_count, loaded.sounds.len()), ("Portable fixture", Some(2), 3));
    assert_eq!(loaded.metadata.tags, ["Warm", "Drums"]);
    let json = serde_json::to_value(&loaded).unwrap();
    assert!(json["groupProject"]["groups"][0]["id"] == "g1" && json["metadata"]["kitFormat"] == "supertracker-kit" && json["sounds"][0]["type"] == "sample");

    // Saving the opened kit over itself: reference-only would delete the members it refers to.
    let moved_snapshot = tree_bytes(Path::new(&moved));
    let reference_only = SaveOptions { copy_samples: false, overwrite: true, ..Default::default() };
    assert!(kit::save(&loaded.project, &fields, &moved, &reference_only).unwrap_err().to_string().contains("belongs to the kit being replaced"));
    assert_eq!(tree_bytes(Path::new(&moved)), moved_snapshot, "an unsafe self-reference replacement preserves the package");
    // Collected, the samples keep their names: the paths an open session uses stay valid.
    let collected = SaveOptions { copy_samples: true, overwrite: true, ..Default::default() };
    kit::save(&loaded.project, &fields, &moved, &collected).unwrap();
    let again = kit::read(&moved).unwrap();
    assert!(again.warnings.is_empty(), "a collected overwrite keeps its own sample members");
    assert_eq!(sample_paths(&again.project), loaded_paths, "saving a kit over itself keeps sample names stable");
    kit::save(&again.project, &fields, &moved, &collected).unwrap();
    assert_eq!(sample_paths(&kit::read(&moved).unwrap().project), loaded_paths, "and stays stable on every further save");
    assert_eq!(fs::read_dir(temp.join("Relocated/Moved.stkit/Samples")).unwrap().count(), 2, "no second-prefix copies accumulate");
    assert_eq!(fs::read_dir(temp.join("Relocated")).unwrap().count(), 1, "the stage and the replaced package are removed");
    assert_eq!(tree_bytes(&temp.join("Source")), original_source, "reading and relocating a kit never writes original media");

    // A relative reference without its project location is refused.
    let relative = kit::save(&project, &fields, &temp.text("Destination/Relative.stkit"), &SaveOptions { copy_samples: true, ..Default::default() });
    assert!(relative.unwrap_err().to_string().contains("needs its original project location"));
}

#[test]
fn external_references_and_transactions() {
    let temp = TempDir::new("kit-external");
    let kick = temp.write("Source/Kick.wav", &[1, 2, 3, 4]);
    let zone = temp.write("Source/Zone.wav", &[9, 8, 7, 6]);
    let original_source = tree_bytes(&temp.join("Source"));
    let project = group::extract_group(&session(&kick, &zone), "g1").unwrap().project;
    temp.mkdir("Destination");
    let package = temp.text("Destination/External.stkit");
    let reference = SaveOptions::default();
    kit::save(&project, &header(json!({ "name": "External fixture" })), &package, &reference).unwrap();
    let first_snapshot = tree_bytes(Path::new(&package));
    assert_eq!(kit::read(&package).unwrap().project, project, "a reference-only save and read preserves every setting exactly");
    assert!(first_snapshot.keys().all(|name| !name.starts_with("Samples/")), "a reference-only save collects nothing");

    let mut edited = project.clone();
    edited["name"] = json!("Explicit replacement");
    let refused = kit::save(&edited, &header(json!({ "name": "Replacement" })), &package, &reference);
    assert!(refused.unwrap_err().to_string().contains("already exists"), "an existing destination needs an explicit overwrite");
    assert_eq!(tree_bytes(Path::new(&package)), first_snapshot);

    let cancelled = Cancel::new();
    cancelled.cancel();
    let options = SaveOptions { copy_samples: true, overwrite: true, cancel: Some(cancelled.clone()), ..Default::default() };
    assert_eq!(kit::save(&edited, &header(json!({ "name": "Cancelled" })), &package, &options).unwrap_err(), st_library::Error::Cancelled);
    assert_eq!(tree_bytes(Path::new(&package)), first_snapshot, "a cancelled overwrite preserves every byte");
    assert_eq!(fs::read_dir(temp.join("Destination")).unwrap().count(), 1, "a cancelled save removes its staging package");
    let cancelled_new = temp.text("Destination/Cancelled.stkit");
    assert!(kit::save(&edited, &header(json!({})), &cancelled_new, &SaveOptions { cancel: Some(cancelled), ..Default::default() }).is_err());
    assert!(!Path::new(&cancelled_new).exists(), "a cancelled save leaves no partial package");

    let mut missing = project.clone();
    missing["instruments"][0]["samplePath"] = json!(temp.text("Source/Absent.wav"));
    let failed = kit::save(&missing, &header(json!({ "name": "Missing sample" })), &package, &SaveOptions { copy_samples: true, overwrite: true, ..Default::default() });
    assert!(failed.unwrap_err().to_string().contains("Sample is missing"), "a collection error fails the overwrite");
    assert_eq!(tree_bytes(Path::new(&package)), first_snapshot, "a failed sample copy preserves every byte");
    assert_eq!(fs::read_dir(temp.join("Destination")).unwrap().count(), 1);

    kit::save(&edited, &header(json!({ "name": "Replacement" })), &package, &SaveOptions { overwrite: true, ..Default::default() }).unwrap();
    let replaced = kit::read(&package).unwrap();
    assert_eq!(replaced.project, edited, "an explicit overwrite commits the replacement");
    assert_eq!(replaced.metadata.name, "Replacement");
    assert_eq!(tree_bytes(&temp.join("Source")), original_source, "references, refusal, cancellation and overwrite never modify source samples");

    // Only a kit package is ever replaced, and only at a .stkit path in an existing folder.
    temp.mkdir("Destination/NotAKit.stkit");
    let overwrite = SaveOptions { overwrite: true, ..Default::default() };
    assert!(kit::save(&edited, &header(json!({})), &temp.text("Destination/NotAKit.stkit"), &overwrite).is_err(), "an unrelated folder is not replaced");
    assert!(kit::save(&edited, &header(json!({})), &temp.text("Destination/Kit.zip"), &reference).unwrap_err().to_string().contains(".stkit extension"));
    assert!(kit::save(&edited, &header(json!({})), &temp.text("Nowhere/Kit.stkit"), &reference).unwrap_err().to_string().contains("existing kit destination folder"));

    // A missing sample of a reference-only kit is a warning when the kit is read.
    fs::remove_file(&zone).unwrap();
    let warned = kit::read(&package).unwrap();
    assert_eq!(warned.warnings, [format!("Missing or unreadable sample: {zone}")]);
}

/// The smallest kit document: one pad playing `sample`, its group and the group's bus.
fn one_pad_kit(sample: &str) -> Value {
    json!({
        "version": st_project::model::FORMAT_VERSION, "name": "One pad",
        "tracks": [{ "id": 0, "name": "Pad", "routingID": "pad", "groupID": "g", "pad": 0, "instrument": 0 }],
        "instruments": [{ "id": 0, "type": "sample", "samplePath": sample }],
        "groups": [{ "id": "g", "name": "One pad", "busID": "bus" }],
        "audioPlugins": [],
        "routing": { "buses": [{ "id": "bus", "name": "One pad", "type": "group", "groupID": "g" }], "outputs": { "track:pad": "bus:bus" }, "sends": [], "sidechains": [] }
    })
}

#[test]
fn malformed_packages() {
    let temp = TempDir::new("kit-malformed");
    let package = temp.mkdir("Invalid.stkit");
    let empty_project = one_pad_kit("");
    temp.write("Invalid.stkit/project.json", empty_project.to_string().as_bytes());
    let base = json!({ "format": "supertracker-kit", "version": kit::FORMAT_VERSION, "projectFile": "project.json", "name": "Fixture" });
    let reject_manifest = |value: Value, label: &str| {
        temp.write("Invalid.stkit/kit.json", value.to_string().as_bytes());
        assert!(kit::read(&package).is_err(), "{label}");
    };
    temp.write("Invalid.stkit/kit.json", b"{unclosed");
    assert_eq!(kit::read(&package).unwrap_err().to_string(), "Malformed kit JSON: kit.json");
    reject_manifest(json!([]), "a manifest array is not a package");
    let with = |key: &str, value: Value| {
        let mut bad = base.clone();
        bad[key] = value;
        bad
    };
    reject_manifest(with("format", json!("some-other-format")), "foreign manifest format");
    for version in [json!(0), json!(1), json!(kit::FORMAT_VERSION - 1), json!(kit::FORMAT_VERSION + 1), json!(-1), json!(1.5), json!(true), json!(kit::FORMAT_VERSION.to_string()), Value::Null] {
        temp.write("Invalid.stkit/kit.json", with("version", version.clone()).to_string().as_bytes());
        assert_eq!(kit::read(&package).unwrap_err().to_string(), "This kit was made by a different version of SuperTracker and cannot be opened by this one.", "version {version}");
    }
    let outside = temp.write("outside.json", empty_project.to_string().as_bytes());
    for path in ["../outside.json", outside.as_str(), "nested/../../outside.json", "link.json"] {
        reject_manifest(with("projectFile", json!(path)), "a manifest must not read a project outside its package");
    }
    temp.write("Invalid.stkit/kit.json", base.to_string().as_bytes());
    kit::read(&package).expect("the base manifest is valid");
    temp.write("Invalid.stkit/project.json", b"[]");
    assert!(kit::read(&package).is_err(), "the project member must be a JSON object");
    temp.write("Invalid.stkit/project.json", b"{unclosed");
    assert!(kit::read(&package).is_err(), "malformed project JSON");
    #[cfg(unix)]
    {
        fs::remove_file(temp.join("Invalid.stkit/project.json")).unwrap();
        std::os::unix::fs::symlink(&outside, temp.join("Invalid.stkit/project.json")).unwrap();
        assert!(kit::read(&package).unwrap_err().to_string().contains("escapes its package"), "a project member link must not leave its package");
        fs::remove_file(temp.join("Invalid.stkit/project.json")).unwrap();
        // A sample path that leaves the package through a link is refused as well.
        std::os::unix::fs::symlink(temp.path(), temp.join("Invalid.stkit/Samples")).unwrap();
        temp.write("Invalid.stkit/project.json", one_pad_kit("Samples/outside.json").to_string().as_bytes());
        assert!(kit::read(&package).unwrap_err().to_string().contains("escapes its package"));
        fs::remove_file(temp.join("Invalid.stkit/Samples")).unwrap();
    }
    temp.write("Invalid.stkit/project.json", one_pad_kit("../outside.json").to_string().as_bytes());
    assert!(kit::read(&package).unwrap_err().to_string().contains("escapes its package"), "a relative sample path stays inside the package");
    temp.write("Invalid.stkit/project.json", empty_project.to_string().as_bytes());
    temp.write("outside.wav", &[1, 2, 3]);
    reject_manifest(with("previewFile", json!("../outside.wav")), "a manifest preview stays inside its package");
    reject_manifest(with("description", json!("x".repeat(256 * 1024))), "oversized kit metadata fails before indexing");
    reject_manifest(with("tags", json!(vec!["t"; 65])), "too many tags");
    reject_manifest(with("name", json!(7)), "a non-text name");
    temp.write("Invalid.stkit/kit.json", base.to_string().as_bytes());
    let oversized = fs::OpenOptions::new().write(true).open(temp.join("Invalid.stkit/project.json")).unwrap();
    oversized.set_len(64 * 1024 * 1024 + 1).unwrap();
    drop(oversized);
    assert!(kit::read(&package).unwrap_err().to_string().contains("exceeds its supported size"), "oversized project state fails before it is read");
    assert_eq!(fs::read(&outside).unwrap(), empty_project.to_string().as_bytes(), "rejected manifests never modify outside files");

    // A link to a kit is not followed while indexing; other files are not kits.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&package, temp.join("Alias.stkit")).unwrap();
        assert_eq!(kit::metadata(&temp.text("Alias.stkit"), None).unwrap_err().to_string(), "Kit aliases are not followed while indexing.");
    }
    assert!(kit::metadata(&outside, None).is_err());
    assert!(kit::read(&temp.write("Group.mxgrp", b"opaque")).unwrap_err().to_string().contains("Maschine group importer"));
}

#[test]
fn preview_discovery() {
    let temp = TempDir::new("kit-previews");
    let root = temp.text("Library");
    let kit_path = temp.write("Library/Expansion Name/Groups/Drums/Midnight.mxgrp", b"Opaque fixture only");
    let folder = "Library/Expansion Name/Groups/Drums";
    temp.write(&format!("{folder}/Midnight Other.mxgrp.ogg"), &[9]);
    temp.write(&format!("{folder}/.previews/Midnight Other.ogg"), &[9]);
    let details = kit::metadata(&kit_path, Some(&root)).unwrap();
    assert_eq!(details.preview_path, None, "a similarly named kit's preview is not selected");
    assert_eq!((details.name.as_str(), details.kind.as_str(), details.kit_format.as_str(), details.user), ("Midnight", "kit", "maschine-group", false));
    assert_eq!(details.expansion.as_deref(), Some("Expansion Name"));
    assert_eq!(details.tags, ["Expansion Name", "Drums"]);
    assert_eq!(details.metadata_source, "filename and folders");
    for relative in ["Midnight.mxgrp.ogg", "Midnight.ogg", ".previews/Midnight.mxgrp.ogg", ".previews/Midnight.ogg", "Previews/Midnight.mxgrp.ogg"] {
        let preview = temp.write(&format!("{folder}/{relative}"), &[1, 2, 3]);
        assert_eq!(kit::metadata(&kit_path, Some(&root)).unwrap().preview_path.as_deref(), Some(preview.as_str()), "exact preview candidate {relative}");
        fs::remove_file(&preview).unwrap();
        assert_eq!(kit::metadata(&kit_path, Some(&root)).unwrap().preview_path, None, "a deleted preview is no longer offered");
    }
    temp.write(&format!("{folder}/Midnight.mxgrp.ogg"), &[]);
    assert_eq!(kit::metadata(&kit_path, None).unwrap().preview_path, None, "an empty preview is not offered");
    assert_eq!(fs::read(&kit_path).unwrap(), b"Opaque fixture only", "indexing never modifies a group");

    // A kit package: the preview given when saving is copied in and found again.
    let sample = temp.write("Kick.wav", &st_library::probe::fixtures::wav(0.1));
    let project = group::extract_group(&session(&sample, &sample), "g1").unwrap().project;
    let package = temp.text("Previewed.stkit");
    let saved = kit::save(&project, &header(json!({ "previewPath": sample })), &package, &SaveOptions::default()).unwrap();
    assert_eq!(saved.metadata["previewPath"], format!("{package}/Preview.wav"));
    assert_eq!(kit::metadata(&package, None).unwrap().preview_path, Some(format!("{package}/Preview.wav")));
    assert_eq!(kit::metadata(&package, None).unwrap().name, "Previewed", "the package name is the default kit name");
    let bad_preview = kit::save(&project, &header(json!({ "previewPath": temp.text("nothing.wav") })), &temp.text("Other.stkit"), &SaveOptions::default());
    assert!(bad_preview.unwrap_err().to_string().contains("kit preview is unavailable"));

    // A preview rendered while saving: the renderer writes Preview.wav into the package that
    // is being put together, and its note for the user is one of the kit's warnings.
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<std::path::PathBuf>::new()));
    let renderer = |outcome: Result<Option<String>, String>, write: bool| {
        let calls = calls.clone();
        let audio = st_library::probe::fixtures::wav(0.2);
        kit::PreviewRenderer::new(move |path| {
            calls.lock().unwrap().push(path.to_path_buf());
            if write {
                fs::write(path, &audio).unwrap();
            }
            outcome.clone()
        })
    };
    let rendered_package = temp.text("Rendered.stkit");
    let options = SaveOptions { preview: Some(renderer(Ok(Some("One pad is not in the preview.".into())), true)), ..Default::default() };
    let saved = kit::save(&project, &header(json!({})), &rendered_package, &options).unwrap();
    assert_eq!(saved.metadata["previewPath"], format!("{rendered_package}/Preview.wav"));
    assert_eq!(saved.warnings, ["One pad is not in the preview."]);
    assert_eq!(kit::preview_path(&rendered_package), Some(format!("{rendered_package}/Preview.wav")));
    assert_eq!(fs::read(format!("{rendered_package}/Preview.wav")).unwrap(), st_library::probe::fixtures::wav(0.2));
    let staged = calls.lock().unwrap().pop().unwrap();
    assert!(staged.ends_with("Preview.wav") && !staged.starts_with(&rendered_package), "rendered into the staged package: {}", staged.display());
    // A renderer that fails, or writes nothing, costs the preview, not the kit.
    for (outcome, write, reason) in [(Err("the kit plays no note.".to_string()), true, "the kit plays no note."), (Ok(None), false, "wrote no audio file")] {
        let package = temp.text("Plain.stkit");
        let options = SaveOptions { preview: Some(renderer(outcome, write)), overwrite: true, ..Default::default() };
        let saved = kit::save(&project, &header(json!({})), &package, &options).unwrap();
        assert!(saved.warnings.len() == 1 && saved.warnings[0].starts_with("The kit was saved without an audio preview: ") && saved.warnings[0].contains(reason), "{:?}", saved.warnings);
        assert!(saved.metadata.get("previewPath").is_none() && kit::preview_path(&package).is_none());
        assert!(!Path::new(&package).join("Preview.wav").exists());
    }
    // A ready-made preview wins, and a save that is refused never asks for a render.
    calls.lock().unwrap().clear();
    let options = SaveOptions { preview: Some(renderer(Ok(None), true)), overwrite: true, ..Default::default() };
    kit::save(&project, &header(json!({ "previewPath": sample })), &temp.text("Plain.stkit"), &options).unwrap();
    let options = SaveOptions { preview: Some(renderer(Ok(None), true)), ..Default::default() };
    assert!(kit::save(&project, &header(json!({})), &rendered_package, &options).is_err(), "the package exists and may not be replaced");
    assert!(calls.lock().unwrap().is_empty());
}

/// A synthetic NI SoundInfo item as found near the start of a .mxgrp (the archive is never read).
fn group_header_bytes(name: &str, chain: &[&str], types: &[&str], padding: usize) -> Vec<u8> {
    let u32le = |value: u32| value.to_le_bytes().to_vec();
    let text = |value: &str| -> Vec<u8> {
        let units: Vec<u16> = value.encode_utf16().collect();
        let mut bytes = u32le(units.len() as u32);
        bytes.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
        bytes
    };
    let mut bytes = vec![0u8; 48];
    bytes.extend_from_slice(b"hsin");
    bytes.extend_from_slice(&[0u8; 12]);
    bytes.extend_from_slice(b"DSIN");
    bytes.extend_from_slice(&[0x6c, 0, 0, 0, 1, 0, 0, 0, 0x18, 0, 0, 0]);
    bytes.extend_from_slice(&[0u8; 20]);
    for word in [1, 2, 1, 0] {
        bytes.extend(u32le(word));
    }
    for value in [name, "Naitve Instruments", "Native Instruments", ""] {
        bytes.extend(text(value));
    }
    bytes.extend(u32le(0));
    bytes.extend(u32le(u32::MAX));
    bytes.extend(u32le(u32::MAX));
    bytes.extend_from_slice(&[0u8; 20]);
    bytes.extend(u32le(1));
    bytes.extend(u32le(chain.len() as u32));
    bytes.extend(chain.iter().flat_map(|value| text(value)));
    bytes.extend(u32le(types.len() as u32));
    bytes.extend(types.iter().flat_map(|value| text(value)));
    bytes.extend(u32le(0));
    bytes.extend(u32le(0));
    bytes.extend(vec![0u8; padding]);
    bytes
}

#[test]
fn maschine_group_headers() {
    let header = kit::group_header(&group_header_bytes("Acid Kit", &["Maschine", "Maschine 1.8 Library"], &["\\:Kits", "\\:Kits\\:Urban Kit", "\\:Loops\\:Guitar\\:Wah"], 256)).unwrap();
    assert_eq!((header.name.as_str(), header.vendor.as_str(), header.author.as_str()), ("Acid Kit", "Native Instruments", "Naitve Instruments"));
    assert_eq!(header.chain, ["Maschine", "Maschine 1.8 Library"]);
    assert_eq!(header.types, [vec!["Kits", "Hip Hop"], vec!["Loops", "Guitar", "Wah"]], "the most specific type paths, with Maschine 3's name for Urban Kit");
    let full = group_header_bytes("Truncated", &["Orbit"], &["\\:Kits"], 0);
    assert_eq!(kit::group_header(&full[..full.len() - 12]), None, "a truncated header is rejected, not partially read");
    let mut hostile = group_header_bytes("Huge", &[], &[], 256);
    let marker = [b'D', b'S', b'I', b'N', 0x6c, 0, 0, 0];
    let at = hostile.windows(8).position(|window| window == marker).unwrap() + 36 + 16;
    hostile[at..at + 4].copy_from_slice(&[0xff, 0xff, 0xff, 0x7f]);
    assert_eq!(kit::group_header(&hostile), None, "oversized string lengths are rejected");
    assert_eq!(kit::group_header(b"opaque"), None);

    // A library root inside an expansion must not name the expansion after its Groups or Kits folder.
    let temp = TempDir::new("kit-headers");
    let opaque = temp.write("Anima Ascent Library/Groups/Kits/Box Kit.mxgrp", b"opaque NI group");
    let described = temp.write("Anima Ascent Library/Groups/Kits/Acid Kit.mxgrp", &group_header_bytes("Acid Kit", &["Anima Ascent"], &["\\:Kits\\:Urban Kit"], 256));
    let root = temp.text("Anima Ascent Library/Groups");
    assert_eq!(kit::metadata(&opaque, Some(&root)).unwrap().expansion.as_deref(), Some("Anima Ascent Library"), "the fallback expansion is the Library folder, not Kits");
    let meta = kit::metadata(&described, Some(&root)).unwrap();
    assert_eq!((meta.expansion.as_deref(), meta.product.as_deref(), meta.vendor.as_deref()), (Some("Anima Ascent"), Some("Anima Ascent"), Some("Native Instruments")));
    assert_eq!(meta.subtypes, Some(vec!["Kits\u{1f}Hip Hop".to_string()]));
    assert_eq!((meta.types.clone(), meta.metadata_source.as_str()), (Some(vec!["Kits".to_string()]), "Maschine group header"));
    assert_eq!(serde_json::to_value(&meta).unwrap()["compatibility"]["status"], "uninspected");
    // Without a "… Library" folder the expansion is the folder that holds Groups.
    let loose = temp.write("My Expansion/Groups/Loose.mxgrp", b"opaque");
    assert_eq!(kit::metadata(&loose, None).unwrap().expansion.as_deref(), Some("My Expansion"));
}

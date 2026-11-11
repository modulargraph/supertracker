//! The plug-in area as the interface uses it: every command through the bridge, with the real
//! container process and the test plug-ins of `crates/plugin-host`, on the null audio device.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_audio::plugins::testing::{artifacts, clap_gain, vst3_gain, Installed, CLAP_GAIN, CLAP_SINE, VST3_GAIN};
use st_host::areas::audio::{AudioOptions, AudioOutput};
use st_host::areas::plugins::PluginAreaOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};

struct Fixture {
    folder: TempDir,
    host: TestHost,
    services: Host,
    installed: Installed,
}

fn start_in(folder: TempDir) -> Fixture {
    start_scanning(folder, true)
}

/// A host whose catalogue searches the folder of the test plug-ins, or (`listed` false) an
/// empty one: the plug-ins are installed, and the catalogue has never listed them.
fn start_scanning(folder: TempDir, listed: bool) -> Fixture {
    let installed = artifacts().install(&folder.path().join("plugins"));
    let searched = if listed { installed.folder.clone() } else { folder.path().join("nothing here") };
    std::fs::create_dir_all(&searched).expect("the searched folder");
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(folder.path().join("support"), Arc::new(NoDialogs));
        options.autosave_every = None;
        options.audio = Some(AudioOptions { output: AudioOutput::Null });
        options.plugins = Some(PluginAreaOptions { container: Some(artifacts().container.clone()), folders: vec![searched.clone()], only_these_folders: true });
        services = Some(install(bridge, options).expect("install the host"));
    });
    host.request("ready", json!({})).expect("the handshake");
    Fixture { folder, host, services: services.unwrap(), installed }
}

fn start(label: &str) -> Fixture {
    start_in(TempDir::new(label))
}

/// Two tracks (`track:a` plays a looping tone from the built-in sine table, `track:b` is for
/// a plug-in instrument), 16 rows.
fn project() -> Value {
    let mut rows = vec![Value::Null; 16];
    rows[0] = json!({ "note": 57, "instrument": 0, "velocity": 100, "gate": 0 });
    json!({
        "version": 1, "bpm": 120, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": [{ "id": 0, "name": "a", "routingID": "a", "volume": 1, "pan": 0, "instrument": 0 }, { "id": 1, "name": "b", "routingID": "b", "volume": 1, "pan": 0, "instrument": 1 }],
        "instruments": [
            { "id": 0, "type": "sample", "samplePath": "builtin:wavetables/Basic/Sine.wav", "mode": "wavetable", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.01 },
            { "id": 1, "type": "au", "name": "Synth", "volume": 1 },
        ],
        "patterns": [{ "id": 0, "length": 16, "steps": [rows, vec![Value::Null; 16]], "automation": [] }],
        "sequence": [0], "loopSong": true,
        "routing": { "buses": [], "outputs": {}, "sends": [], "sidechains": [] },
        "audioPlugins": [],
    })
}

impl Fixture {
    fn request(&self, command: &str, payload: Value) -> Value {
        self.host.request(command, payload).unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    fn publish(&self, project: &Value) -> Value {
        self.request("engine.project", json!({ "project": project }))
    }

    /// What the interface does after a load: store the descriptors and publish.
    fn adopt(&self, project: &mut Value) {
        project["audioPlugins"] = self.request("plugin.state", json!({}));
        self.publish(project);
    }

    fn clap(&self, id: &str) -> Value {
        json!({ "component": format!("clap:{id}"), "bundlePath": self.installed.clap })
    }

    fn insert(&self, owner: &str) -> Value {
        self.request("routing.status", json!({}))["inserts"].as_array().unwrap().iter().find(|insert| insert["owner"] == owner).cloned().unwrap_or(Value::Null)
    }

    fn status_where(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = Value::Null;
        while Instant::now() < deadline {
            for (name, payload) in self.host.events() {
                if name == "status" {
                    if wanted(&payload) {
                        return payload;
                    }
                    last = payload;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no status event where {what}; the last one was {last}");
    }
}

fn with(mut base: Value, extra: Value) -> Value {
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

fn parameter(list: &Value, address: u64) -> &Value {
    let address = address.to_string();
    list.as_array().unwrap().iter().find(|parameter| parameter["address"] == address.as_str()).unwrap_or_else(|| panic!("parameter {address} in {list}"))
}

#[test]
fn the_catalogue_lists_built_in_effects_and_scanned_plug_ins() {
    let fixture = start("plugins-list");
    let ready = fixture.request("ready", json!({}));
    for command in ["plugins.list", "plugins.scan", "plugin.load", "plugin.loadNKS", "plugin.state", "plugins.recovery", "plugins.recovery.retry"] {
        assert!(ready["commands"].as_array().unwrap().contains(&json!(command)), "{command} is implemented");
    }
    assert_eq!(ready["pluginRecovery"]["suspects"], json!([]));
    assert!(ready["plugins"].as_array().unwrap().iter().any(|plugin| plugin["format"] == "Builtin" && plugin["type"] == "effect" && plugin["id"].as_str().unwrap().starts_with("stfx:")));

    let done = fixture.request("plugins.scan", json!({ "wait": true, "retryFailed": true }));
    assert_eq!((done["scanning"].clone(), done["result"]["total"].clone(), done["result"]["plugins"].clone(), done["blocklist"].clone()), (json!(false), json!(2), json!(5), json!([])));
    let list = fixture.request("plugins.list", json!({}));
    let entry = |id: &str| list.as_array().unwrap().iter().find(|plugin| plugin["id"] == id).unwrap_or_else(|| panic!("{id} is listed")).clone();
    let gain = entry(&format!("clap:{CLAP_GAIN}"));
    assert_eq!((gain["name"].clone(), gain["format"].clone(), gain["type"].clone(), gain["manufacturer"].clone()), (json!("ST Test Gain"), json!("CLAP"), json!("effect"), json!("SuperTracker Tests")));
    assert_eq!(PathBuf::from(gain["bundlePath"].as_str().unwrap()), fixture.installed.clap);
    assert_eq!((entry(&format!("clap:{CLAP_SINE}"))["type"].clone(), entry(&format!("vst3:{VST3_GAIN}"))["format"].clone()), (json!("instrument"), json!("VST3")));
    assert!(fixture.host.wait_for_event("plugins.catalogue", Duration::from_secs(5)).is_some(), "scan progress is an event");
    // A file that is no plug-in fails its scan and is visible, with the reason; nothing else suffers.
    std::fs::write(fixture.installed.folder.join("Broken.clap"), b"not a library").unwrap();
    let done = fixture.request("plugins.scan", json!({ "wait": true }));
    assert_eq!((done["result"]["plugins"].clone(), done["result"]["failed"].clone()), (json!(5), json!(1)));
    let details = fixture.request("plugins.list", json!({ "details": true }));
    let blocked = &details["blocklist"][0];
    assert!(blocked["path"].as_str().unwrap().ends_with("Broken.clap") && blocked["status"] == "failed" && !blocked["reason"].as_str().unwrap().is_empty(), "{blocked}");
    assert_eq!(details["plugins"].as_array().unwrap().len(), list.as_array().unwrap().len());
    // No plug-in is hosted by listing.
    assert!(fixture.services.plugins.as_ref().unwrap().manager().all().is_empty());
}

#[test]
fn inserts_are_loaded_changed_moved_saved_and_restored_through_the_bridge() {
    let fixture = start("plugins-edit");
    let mut project = project();
    fixture.publish(&project);
    let target = json!({ "owner": "track:a", "kind": "effect", "slot": 0 });
    // A plug-in and a built-in effect, as the mixer's rack adds them.
    let loaded = fixture.request("plugin.load", with(target.clone(), fixture.clap(CLAP_GAIN)));
    assert_eq!((loaded["owner"].clone(), loaded["slot"].clone(), loaded["format"].clone(), loaded["name"].clone()), (json!("track:a"), json!(0), json!("CLAP"), json!("ST Test Gain")));
    assert!(loaded.get("track").is_none(), "a descriptor names its channel by owner only");
    assert!(loaded["instanceID"].as_str().is_some_and(|id| !id.is_empty()) && loaded["hostStatus"]["available"] == true);
    let gain = parameter(&loaded["parameters"], clap_gain::GAIN);
    assert_eq!((gain["name"].clone(), gain["value"].clone(), gain["normalized"].clone(), gain["writable"].clone(), gain["display"].clone()), (json!("Gain"), json!(1.0), json!(0.5), json!(true), json!("1.00 x")));
    assert_eq!(parameter(&loaded["parameters"], 10)["writable"], false, "a parameter the plug-in only reports");
    let equalizer = fixture.request("plugin.load", json!({ "owner": "track:a", "kind": "effect", "slot": 3, "component": "stfx:eq" }));
    assert_eq!((equalizer["format"].clone(), equalizer["owner"].clone(), equalizer["slot"].clone()), (json!("Builtin"), json!("track:a"), json!(3)));
    assert!(equalizer["parameters"].as_array().unwrap().len() > 3);
    fixture.adopt(&mut project);
    assert_eq!(project["audioPlugins"].as_array().unwrap().len(), 2);
    assert_eq!(fixture.insert("track:a")["available"], true);
    let instance = fixture.services.plugins.as_ref().unwrap().manager().hosted(loaded["instanceID"].as_str().unwrap()).unwrap().instance.id();

    // Audible: with the song playing, the plug-in's gain at zero silences the track.
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    fixture.status_where("the track is heard", |status| status["playing"] == true && status["peakL"].as_f64().unwrap() > 0.05);
    let written = fixture.request("plugin.parameter", with(target.clone(), json!({ "address": clap_gain::GAIN.to_string(), "value": 0.0, "normalized": true })));
    assert_eq!((written["value"].clone(), written["written"].clone()), (json!(0.0), json!(0.0)));
    fixture.status_where("the gain at zero is silence", |status| status["peakL"] == 0.0);
    let written = fixture.request("plugin.parameter", with(target.clone(), json!({ "address": clap_gain::GAIN.to_string(), "value": 0.25 })));
    assert_eq!((written["value"].clone(), written["display"].clone()), (json!(0.5), json!("0.50 x")));
    fixture.status_where("the track is back", |status| status["peakL"].as_f64().unwrap() > 0.05);
    let listed = fixture.request("plugin.parameters", with(target.clone(), json!({ "addresses": [clap_gain::GAIN.to_string()] })));
    assert_eq!((listed["parameters"].as_array().unwrap().len(), listed["parameters"][0]["normalized"].clone()), (1, json!(0.25)));
    // The built-in effect answers the same commands.
    let slot3 = json!({ "owner": "track:a", "kind": "effect", "slot": 3 });
    let band = fixture.request("plugin.parameter", with(slot3.clone(), json!({ "address": "0", "value": 0.8 })));
    assert_eq!(band["written"], 0.8);
    assert_eq!(fixture.request("plugin.parameters", slot3.clone())["parameters"][0]["normalized"], 0.8);

    // State: one descriptor, with the state read now; unchanged, it reads the same again.
    let state = fixture.request("plugin.state", target.clone());
    assert!(state["state"].as_str().is_some_and(|state| !state.is_empty()) && state.get("stateError").is_none() && state.get("parameters").is_none());
    assert_eq!(fixture.request("plugin.state", target.clone())["state"], state["state"]);
    fixture.adopt(&mut project);
    assert_eq!(project["audioPlugins"][0]["state"], state["state"]);
    assert_eq!(fixture.services.plugins.as_ref().unwrap().manager().hosted(loaded["instanceID"].as_str().unwrap()).unwrap().instance.id(), instance, "publishing the captured project keeps the instance");
    // Bypass, then restoring a saved state over a later edit.
    let bypassed = fixture.request("plugin.parameter", with(target.clone(), json!({ "bypass": true })));
    assert_eq!(bypassed["bypass"], true);
    fixture.request("plugin.parameter", with(target.clone(), json!({ "bypass": false })));
    fixture.request("plugin.parameter", with(target.clone(), json!({ "address": clap_gain::GAIN.to_string(), "value": 1.0 })));
    let restored = fixture.request("plugin.state", with(target.clone(), json!({ "state": state["state"] })));
    assert_eq!(restored["state"], state["state"]);
    assert_eq!(fixture.request("plugin.parameters", target.clone())["parameters"][0]["value"], 0.5);

    // Move: the plug-in takes the equalizer's place; the lane of its gain follows it.
    project["patterns"][0]["automation"] = json!([{ "id": "lane", "owner": "track:a", "target": format!("plugin:effect:0:{}", clap_gain::GAIN), "interpolation": "step", "points": [{ "row": 0, "value": 0.25 }] }]);
    fixture.publish(&project);
    let moved = fixture.request("plugin.move", json!({ "owner": "track:a", "fromSlot": 0, "toSlot": 3 }));
    assert_eq!((moved["moved"].clone(), moved["owner"].clone(), moved["fromSlot"].clone(), moved["toSlot"].clone()), (json!(true), json!("track:a"), json!(0), json!(3)));
    assert_eq!(moved["slotMap"], json!([{ "fromSlot": 0, "toSlot": 3, "instanceID": loaded["instanceID"] }, { "fromSlot": 3, "toSlot": 0, "instanceID": equalizer["instanceID"] }]));
    let slots = |list: &Value| list.as_array().unwrap().iter().map(|item| (item["slot"].as_i64().unwrap(), item["component"].as_str().unwrap().to_string())).collect::<Vec<_>>();
    assert_eq!(slots(&moved["previousPlugins"]), [(0, format!("clap:{CLAP_GAIN}")), (3, "stfx:eq".to_string())]);
    assert_eq!(slots(&moved["plugins"]), [(0, "stfx:eq".to_string()), (3, format!("clap:{CLAP_GAIN}"))]);
    assert!(moved["previousPlugins"][0]["state"] == state["state"] && moved["plugins"][1]["state"] == state["state"]);
    let snapshot = fixture.request("engine.snapshot", json!({}));
    assert_eq!(snapshot["patterns"][0]["automation"][0]["target"], format!("plugin:effect:3:{}", clap_gain::GAIN));
    project["audioPlugins"] = moved["audioPlugins"].clone();
    project["patterns"] = snapshot["patterns"].clone();
    fixture.publish(&project);
    assert_eq!(fixture.services.plugins.as_ref().unwrap().manager().hosted(loaded["instanceID"].as_str().unwrap()).unwrap().instance.id(), instance, "a move reloads nothing");

    // Save captures the state; opening the file brings the plug-in back with it.
    let path = fixture.folder.path().join("song.supertracker");
    let saved = fixture.request("project.save", json!({ "project": project, "path": path }));
    let stored = saved["project"]["audioPlugins"].as_array().unwrap().iter().find(|plugin| plugin["instanceID"] == loaded["instanceID"]).unwrap().clone();
    assert_eq!(stored["state"], state["state"]);
    let slot = json!({ "owner": "track:a", "kind": "effect", "slot": 3 });
    fixture.request("plugin.parameter", with(slot.clone(), json!({ "address": clap_gain::GAIN.to_string(), "value": 0.9 })));
    let removed = fixture.request("plugin.remove", slot.clone());
    assert_eq!((removed["removed"].clone(), removed["instanceIDs"].clone()), (json!(true), json!([loaded["instanceID"]])));
    assert!(fixture.host.request("plugin.parameters", slot.clone()).unwrap_err().contains("slot 4 of this channel is empty"));
    let opened = fixture.request("project.open", json!({ "path": path }));
    fixture.request("engine.project", json!({ "project": opened["project"], "replace": true }));
    assert_eq!(fixture.request("plugin.parameters", slot.clone())["parameters"][0]["value"], 0.5, "the saved gain, not the later 0.9");
    assert_eq!(fixture.request("plugin.remove", json!({ "owner": "track:a", "kind": "effect", "slot": 7 }))["removed"], false);
    // A plug-in that does not exist is refused with a reason, and the engine stays as it was.
    let error = fixture.host.request("plugin.load", json!({ "owner": "master", "kind": "effect", "slot": 0, "component": "clap:org.example.missing" })).unwrap_err();
    assert!(error.contains("is not installed"), "{error}");
    assert_eq!(fixture.insert("master"), Value::Null);
}

// The catalogue's scan runs in the background from the start of the host: a load that comes
// before it has listed the plug-in must not depend on it for the plug-in's name.
#[test]
fn a_plug_in_the_catalogue_has_not_listed_is_named_by_its_own_instance() {
    let fixture = start_scanning(TempDir::new("plugins-unlisted"), false);
    let mut project = project();
    fixture.publish(&project);
    assert!(fixture.request("plugins.scan", json!({ "wait": true }))["plugins"].as_array().unwrap().iter().all(|plugin| plugin["format"] == "Builtin"), "the catalogue lists no plug-in");
    let loaded = fixture.request("plugin.load", with(json!({ "owner": "track:a", "kind": "effect", "slot": 0 }), fixture.clap(CLAP_GAIN)));
    assert_eq!((loaded["name"].clone(), loaded["manufacturer"].clone(), loaded["component"].clone()), (json!("ST Test Gain"), json!("SuperTracker Tests"), json!(format!("clap:{CLAP_GAIN}"))));
    assert_eq!(loaded["hostStatus"]["available"], true);
    // The engine's project has the name too: it is what the interface stores and what a loss is reported with.
    fixture.adopt(&mut project);
    assert_eq!((project["audioPlugins"][0]["name"].clone(), project["audioPlugins"][0]["manufacturer"].clone()), (json!("ST Test Gain"), json!("SuperTracker Tests")));
    assert_eq!(fixture.insert("track:a")["available"], true);
    fixture.services.services.shutdown();
}

#[test]
fn an_instrument_plug_in_an_editor_and_an_nks_preset() {
    let fixture = start("plugins-instrument");
    let mut project = project();
    project["patterns"][0]["steps"][1][0] = json!({ "note": 60, "instrument": 1, "velocity": 100, "gate": 4 });
    project["tracks"][0]["mute"] = json!(true);
    fixture.publish(&project);
    fixture.request("plugins.scan", json!({ "wait": true }));
    let loaded = fixture.request("plugin.load", json!({ "owner": "track:b", "kind": "instrument", "slot": 0, "component": format!("clap:{CLAP_SINE}") }));
    assert_eq!((loaded["kind"].clone(), loaded["owner"].clone(), loaded["name"].clone()), (json!("instrument"), json!("track:b"), json!("ST Test Sine")));
    assert_eq!(PathBuf::from(loaded["bundlePath"].as_str().unwrap()), fixture.installed.clap, "the catalogue knows the file");
    let captured = fixture.request("plugin.state", json!({ "owner": "track:b", "kind": "instrument", "slot": 0, "instanceID": loaded["instanceID"] }));
    assert!(captured["component"] == loaded["component"] && captured.get("stateError").is_none() && captured["state"].is_string());
    fixture.adopt(&mut project);
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    fixture.status_where("the plug-in instrument is heard", |status| status["peakL"].as_f64().unwrap() > 0.05);
    assert_eq!(fixture.insert("track:b")["instrument"], true);
    // A plug-in command names the channel by owner; a track number is not an address.
    let error = fixture.host.request("plugin.parameters", json!({ "track": 1, "kind": "instrument" })).unwrap_err();
    assert!(error.contains("its owner"), "{error}");
    // An effect cannot be an instrument, and the other way round.
    let error = fixture.host.request("plugin.load", json!({ "owner": "track:a", "kind": "instrument", "component": format!("clap:{CLAP_GAIN}") })).unwrap_err();
    assert!(error.contains("is an effect, not an instrument"), "{error}");

    // The editor: the container opens the plug-in's window (the test plug-in keeps count).
    let gain = json!({ "owner": "master", "kind": "effect", "slot": 0 });
    fixture.request("plugin.load", with(gain.clone(), fixture.clap(CLAP_GAIN)));
    let opened = fixture.request("plugin.editor", gain.clone());
    assert_eq!((opened["shown"].clone(), opened["open"].clone(), opened["name"].clone()), (json!(true), json!(true), json!("ST Test Gain")));
    assert_eq!(parameter(&fixture.request("plugin.parameters", gain.clone())["parameters"], 10)["value"], 2.0, "the plug-in's window is shown");
    assert_eq!(fixture.request("plugin.editor", with(gain.clone(), json!({ "open": false })))["open"], false);
    assert_eq!(parameter(&fixture.request("plugin.parameters", gain.clone())["parameters"], 10)["value"], 0.0);
    let error = fixture.host.request("plugin.editor", json!({ "owner": "track:b", "kind": "instrument" })).unwrap_err();
    assert!(error.contains("no editor window"), "{error}");

    // An NKS preset of the VST3 gain: its exact state (gain 0.25 of 0..1) and its pages.
    let mut state = b"STG3".to_vec();
    state.extend(1u32.to_le_bytes());
    for value in [0.25f64, 0.0, 0.0, 0.0, 0.0] {
        state.extend(value.to_le_bytes());
    }
    let preset = st_library::nks::fixtures::preset(
        &json!({ "name": "Half", "vendor": "SuperTracker Tests", "bankchain": ["ST Test Gain VST3"], "deviceType": "FX", "uuid": "0b8c0f6e-0000-4000-8000-000000000001" }),
        Some(&json!({ "VST3.uid": [0x5354_5633u32, 0x4761_696Eu32, 0x5072_6F63u32, 1], "pluginName": "ST Test Gain VST3" })),
        Some(&json!({ "ni8": [[{ "id": vst3_gain::GAIN, "name": "Gain", "section": "Main" }, { "id": 4242, "name": "Elsewhere" }, {}]] })),
        Some(&state),
    );
    let path = fixture.folder.path().join("Half.nksfx");
    std::fs::write(&path, preset).unwrap();
    let loaded = fixture.request("plugin.loadNKS", json!({ "path": path, "owner": "track:a", "kind": "effect", "slot": 1 }));
    assert_eq!((loaded["component"].clone(), loaded["nks"]["name"].clone(), loaded["format"].clone()), (json!(format!("vst3:{VST3_GAIN}")), json!("Half"), json!("VST3")));
    assert_eq!(parameter(&loaded["parameters"], vst3_gain::GAIN)["value"], 0.25, "the preset's state is in the plug-in");
    let slots = &loaded["nks"]["pages"][0]["parameters"];
    assert_eq!((slots[0]["mappingStatus"].clone(), slots[0]["mapped"].clone(), slots[1]["mappingStatus"].clone(), slots[2]["mappingStatus"].clone()), (json!("mapped"), json!(true), json!("parameterUnavailable"), json!("unassigned")));
    assert_eq!(fixture.request("plugin.state", json!({}))[2]["nks"]["pages"], loaded["nks"]["pages"], "the engine's descriptor carries the pages");
    let error = fixture.host.request("plugin.loadNKS", json!({ "path": fixture.folder.path().join("nothing.nksf"), "owner": "track:a" })).unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn a_plug_in_that_crashes_while_playing_is_reported_and_retry_restores_it() {
    let fixture = start("plugins-crash");
    let mut project = project();
    fixture.publish(&project);
    let target = json!({ "owner": "track:a", "kind": "effect", "slot": 0 });
    let loaded = fixture.request("plugin.load", with(target.clone(), fixture.clap(CLAP_GAIN)));
    fixture.request("plugin.parameter", with(target.clone(), json!({ "address": clap_gain::GAIN.to_string(), "value": 0.25 })));
    fixture.adopt(&mut project);
    // Row 8 of the pattern locks the plug-in's "crash" parameter: it aborts inside `process`.
    project["patterns"][0]["steps"][0][8] = json!({ "fx1": { "type": "plugin", "instanceID": loaded["instanceID"], "address": clap_gain::CRASH.to_string(), "value": 0.5 } });
    fixture.publish(&project);
    fixture.host.events();
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    let error = fixture.host.wait_for_event("plugin.error", Duration::from_secs(20)).expect("the loss is announced");
    assert!(error["lost"] == true && error["instanceID"] == loaded["instanceID"] && error["error"].as_str().unwrap().contains("ST Test Gain stopped working"), "{error}");
    let report = fixture.host.wait_for_event("plugins.recovery", Duration::from_secs(5)).expect("the recovery report follows");
    let suspect = &report["suspects"][0];
    assert_eq!((suspect["requiresReview"].clone(), suspect["identity"]["name"].clone(), suspect["identity"]["componentID"].clone()), (json!(true), json!("ST Test Gain"), json!(format!("clap:{CLAP_GAIN}"))));
    let key = suspect["identity"]["key"].as_str().unwrap().to_string();
    // The song plays on, the track passes through its insert, and everyone says so.
    let row = fixture.status_where("playback continues", |status| status["playing"] == true && status["peakL"].as_f64().unwrap() > 0.05)["row"].clone();
    fixture.status_where("the transport still moves", |status| status["row"] != row);
    let insert = fixture.insert("track:a");
    assert_eq!((insert["available"].clone(), insert["lost"].clone()), (json!(false), json!(true)));
    assert_eq!(fixture.request("plugins.recovery", json!({}))["suspects"][0]["identity"]["key"], key);
    let parameters = fixture.request("plugin.parameters", target.clone());
    assert_eq!(parameter(&parameters["parameters"], clap_gain::GAIN)["value"], 0.5, "the last known values still answer");
    assert!(fixture.host.request("plugin.parameter", with(target.clone(), json!({ "address": "1", "value": 0.5 }))).unwrap_err().contains("is not running"));
    // Saving works while the plug-in is down: its descriptor keeps a state.
    let snapshot = fixture.request("engine.snapshot", json!({}));
    assert!(snapshot["audioPlugins"][0]["state"].is_string() && snapshot["audioPlugins"][0].get("stateError").is_none());

    // The user removes the cause and chooses Retry in the recovery dialog.
    fixture.request("transport.stop", json!({}));
    project["patterns"][0]["steps"][0][8] = Value::Null;
    fixture.publish(&project);
    let retried = fixture.request("plugins.recovery.retry", json!({ "keys": [key] }));
    assert_eq!((retried["suspects"].clone(), retried["retryErrors"].clone()), (json!([]), json!([])));
    assert_eq!(fixture.insert("track:a")["available"], true);
    assert_eq!(parameter(&fixture.request("plugin.parameters", target.clone())["parameters"], clap_gain::GAIN)["value"], 0.5, "restored with its settings");
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    fixture.status_where("the restored insert processes", |status| status["playing"] == true && status["peakL"].as_f64().unwrap() > 0.05);
    fixture.request("plugin.parameter", with(target, json!({ "address": clap_gain::GAIN.to_string(), "value": 0.0 })));
    fixture.status_where("and is in the signal path again", |status| status["peakL"] == 0.0);
}

/// There is no command to capture a "base patch": the value the user sets is what a
/// parameter returns to, and a state saved while stopped has it.
#[test]
fn a_locked_parameter_returns_to_what_the_user_set_and_a_state_captured_while_stopped_has_it() {
    let fixture = start("plugins-base");
    let ready = fixture.request("ready", json!({}));
    assert!(!ready["commands"].as_array().unwrap().contains(&json!("plugin.captureBasePatch")));
    assert!(fixture.host.request("plugin.captureBasePatch", json!({})).unwrap_err().contains("is not a command this host knows"));

    let mut project = project();
    fixture.publish(&project);
    let target = json!({ "owner": "track:a", "kind": "effect", "slot": 0 });
    let loaded = fixture.request("plugin.load", with(target.clone(), fixture.clap(CLAP_GAIN)));
    // The user's value: a gain of 1.5 (the parameter's range is 0..2).
    fixture.request("plugin.parameter", with(target.clone(), json!({ "address": clap_gain::GAIN.to_string(), "value": 0.75 })));
    // A step lock halves the gain on each row of the pattern's first half (a second); on the
    // rows after it the gain returns to 1.5.
    let lock = json!({ "type": "plugin", "instanceID": loaded["instanceID"], "address": clap_gain::GAIN.to_string(), "value": 0.25 });
    for row in 0..8 {
        let step = &mut project["patterns"][0]["steps"][0][row];
        *step = if step.is_null() { json!({ "fx1": lock }) } else { with(step.clone(), json!({ "fx1": lock })) };
    }
    fixture.adopt(&mut project);
    let gain = || parameter(&fixture.request("plugin.parameters", target.clone())["parameters"], clap_gain::GAIN)["value"].as_f64().unwrap();
    let wait_for_gain = |what: &str, wanted: f64| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while gain() != wanted {
            assert!(Instant::now() < deadline, "{what}: the gain is {}", gain());
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait_for_gain("the value the user set", 1.5);
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    wait_for_gain("the lock applies while the pattern plays", 0.5);
    wait_for_gain("and the rows without it are back at what the user set", 1.5);
    wait_for_gain("until the pattern starts again", 0.5);
    // Stop and capture at once, in a locked row: the capture waits for the stop to take
    // effect, and its state is the user's patch, and so is the plug-in.
    fixture.request("transport.stop", json!({}));
    let captured = fixture.request("plugin.state", with(target.clone(), json!({ "instanceID": loaded["instanceID"] })));
    assert!(captured["state"].is_string() && captured.get("stateError").is_none(), "{captured}");
    wait_for_gain("the capture brought the parameter back to its base value", 1.5);
    // The state is that patch: loaded into the plug-in again after a change, the gain is 1.5.
    fixture.request("plugin.parameter", with(target.clone(), json!({ "address": clap_gain::GAIN.to_string(), "value": 0.1 })));
    wait_for_gain("another value", 0.2);
    fixture.request("plugin.state", with(target.clone(), json!({ "state": captured["state"] })));
    wait_for_gain("the captured state has the user's value", 1.5);
}

//! The audio area as the interface uses it: every command through the bridge, on the null
//! device, with the status events the page animates from.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_host::areas::audio::{AudioOptions, AudioOutput};
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};

struct Fixture {
    folder: TempDir,
    host: TestHost,
    services: Host,
}

fn start(folder: TempDir) -> Fixture {
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(folder.path().join("support"), Arc::new(NoDialogs));
        options.autosave_every = None;
        options.audio = Some(AudioOptions { output: AudioOutput::Null });
        services = Some(install(bridge, options).expect("install the host"));
    });
    // Events (the 20 Hz status among them) go to interfaces that have sent `ready`.
    host.request("ready", json!({})).expect("the handshake");
    Fixture { folder, host, services: services.unwrap() }
}

/// The web model's demo song (the fixture st-audio's tests use too).
fn demo() -> Value {
    serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).unwrap()
}

/// Waits until a `status` event satisfies `wanted`, and returns it.
fn status_where(host: &TestHost, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        for (name, payload) in host.events() {
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

#[test]
fn the_page_plays_the_demo_song_and_sees_it_move() {
    let fixture = start(TempDir::new("audio-play"));
    let host = &fixture.host;
    let ready = host.request("ready", json!({})).unwrap();
    for command in ["transport.play", "transport.stop", "note.on", "audio.devices", "engine.snapshot", "step.preview"] {
        assert!(ready["commands"].as_array().unwrap().contains(&json!(command)), "{command} is implemented");
    }
    assert_eq!(ready["audio"]["outputUid"], "null");
    assert_eq!(ready["audio"]["outputs"].as_array().unwrap().last().unwrap()["uid"], "null");
    assert!(ready.get("warning").is_none(), "{ready}");

    // Publication reaches the engine: the reply is the status, with the mixer's eight tracks.
    let published = host.request("engine.project", json!({ "project": demo() })).unwrap();
    assert_eq!((published["playing"].clone(), published["sampleRate"].clone()), (json!(false), json!(48000.0)));
    assert_eq!(published["routing"]["strips"].as_array().unwrap().len(), 9);
    assert!(host.events().iter().all(|(name, _)| name != "sample.error"), "the demo's built-in wavetables load");

    let playing = host.request("transport.play", json!({ "mode": "song", "fromStart": true })).unwrap();
    assert_eq!(playing["playing"], true, "Play answers with the requested state");
    let moving = status_where(host, "the song advances with sound", |status| {
        status["playing"] == true && status["row"].as_i64().unwrap() >= 2 && status["peakL"].as_f64().unwrap() > 0.001 && status["peakR"].as_f64().unwrap() > 0.001
    });
    assert_eq!((moving["songMode"].clone(), moving["pattern"].clone(), moving["running"].clone()), (json!(true), json!(0), json!(true)));
    assert!(moving["trackPeaks"].as_array().unwrap().iter().any(|peak| peak.as_f64().unwrap() > 0.001));
    assert!(moving["cpu"].as_f64().unwrap() >= 0.0 && moving["seconds"].as_f64().unwrap() > 0.0);

    // Editing a note while playing is an ordinary publication; playback continues.
    let mut edited = demo();
    edited["patterns"][0]["steps"][0][20] = json!({ "note": 45, "instrument": 0, "velocity": 100, "gate": 0.5 });
    assert_eq!(host.request("engine.project", json!({ "project": edited })).unwrap()["playing"], true);
    let row = moving["row"].as_i64().unwrap();
    status_where(host, "playback continues after the edit", |status| status["playing"] == true && status["row"].as_i64().unwrap() != row);

    let sought = host.request("transport.seek", json!({ "pattern": 1, "row": 40, "sequence": 2 })).unwrap();
    assert_eq!(sought["playing"], true);
    status_where(host, "the seek is heard", |status| status["sequence"] == 2 && status["row"].as_i64().unwrap() >= 40);

    assert_eq!(host.request("transport.queue", json!({ "sequence": 0 })), Ok(json!({ "queued": true, "pattern": -1, "sequence": 0 })));
    assert_eq!(host.request("transport.queue", json!({ "clear": true })), Ok(json!({ "queued": false, "pattern": -1, "sequence": -1 })));
    assert_eq!(host.request("transport.queue", json!({})), Err("Queue a pattern or sequence position.".to_string()));
    assert_eq!(host.request("transport.queue", json!({ "pattern": 1, "mode": "song" })), Err("Queue a sequence position for Song mode or a pattern for Pattern mode.".to_string()));
    assert_eq!(host.request("transport.queue", json!({ "pattern": 1, "mode": "pattern" })), Ok(json!({ "queued": true, "mode": "pattern", "pattern": 1, "sequence": -1 })));

    assert_eq!(host.request("transport.stop", json!({})).unwrap()["playing"], false);
    status_where(host, "the transport has stopped", |status| status["playing"] == false && status["transportPending"] == false);
    assert_eq!(host.request("transport.loop", json!({ "loop": { "start": 4, "end": 7 } })).unwrap()["playing"], false);
    host.request("transport.play", json!({ "mode": "pattern", "loop": { "start": 4, "end": 7 }, "fromStart": true })).unwrap();
    status_where(host, "pattern mode loops rows 4..7", |status| status["playing"] == true && status["songMode"] == false && (4..=7).contains(&status["row"].as_i64().unwrap()));
    host.request("engine.panic", json!({})).unwrap();
    host.request("transport.stop", json!({})).unwrap();
}

#[test]
fn live_notes_previews_and_record_arm_reach_the_engine() {
    let fixture = start(TempDir::new("audio-notes"));
    let host = &fixture.host;
    host.request("engine.project", json!({ "project": demo() })).unwrap();
    // Live expression names the channel of the track it is for.
    let owner = format!("track:{}", demo()["tracks"][1]["routingID"].as_str().unwrap());
    assert_eq!(host.request("note.on", json!({ "note": 60, "velocity": 110, "track": 1, "instrument": 1 })), Ok(json!({ "ok": true })));
    status_where(host, "the held note is heard", |status| status["peakL"].as_f64().unwrap() > 0.001);
    // The sustain pedal holds the release until it comes up.
    assert_eq!(host.request("midi.expression", json!({ "kind": "sustain", "owner": owner, "value": 1 })).unwrap()["sent"], true);
    assert_eq!(host.request("note.off", json!({ "note": 60, "track": 1 })), Ok(json!({ "ok": true, "sustained": true })));
    assert_eq!(host.request("midi.expression", json!({ "kind": "cc", "controller": 64, "owner": owner, "value": 0 })).unwrap()["sent"], true);
    assert_eq!(host.request("note.off", json!({ "note": 60, "track": 1 })), Ok(json!({ "ok": true })));
    assert_eq!(host.request("midi.expression", json!({ "kind": "pitchBend", "owner": owner, "value": 0.5 })).unwrap(), json!({ "sent": true, "owner": owner, "instrument": 1 }));
    // Without an owner the message is for the track of the last note; a track number is not an address.
    assert_eq!(host.request("midi.expression", json!({ "kind": "cc", "controller": 74, "value": 0.5 })).unwrap(), json!({ "sent": false, "owner": owner, "instrument": 1 }));
    assert_eq!(host.request("midi.expression", json!({ "kind": "pitchBend", "track": 5, "value": 0.5 })).unwrap()["owner"], owner);
    assert_eq!(host.request("midi.expression", json!({ "kind": "pitchBend", "owner": "track:gone", "value": 0.5 })).unwrap_err(), "track:gone is not the channel of a track of this project.");
    assert!(host.request("midi.expression", json!({ "kind": "wobble", "value": 0.5 })).unwrap_err().contains("wobble"));

    assert_eq!(host.request("step.preview", json!({ "step": { "note": 64, "instrument": 0, "velocity": 100 }, "track": 0, "duration": 0.2 })), Ok(json!({ "auditioning": true })));
    assert_eq!(host.request("step.preview", json!({ "step": { "fx1": { "type": "volume", "value": 1 } }, "track": 0 })), Ok(json!({ "auditioning": false })));
    assert_eq!(host.request("step.preview", json!({})), Err("Choose a tracker step to audition.".to_string()));
    assert_eq!(host.request("engine.instrumentParameter", json!({ "instrument": 0, "target": "cutoff", "value": 900 })), Ok(json!({ "ok": true })));
    assert_eq!(host.request("engine.instrumentParameter", json!({ "instrument": 0, "target": "nonsense", "value": 1 })), Err("Unsupported live instrument parameter nonsense.".to_string()));

    // A captured key edge is answered once the audio thread has placed it.
    assert_eq!(host.request("transport.recordArm", json!({ "armed": true })), Ok(json!({ "armed": true })));
    assert!(host.request("transport.recordArm", json!({})).is_err(), "armed is required");
    host.request("transport.play", json!({ "mode": "pattern", "fromStart": true })).unwrap();
    status_where(host, "recording runs", |status| status["playing"] == true && status["transportRecording"] == true && status["noteRecordArmed"] == true);
    let captured = host.request("note.on", json!({ "note": 62, "velocity": 99, "track": 0, "instrument": 0, "captureInput": true, "inputOwner": "key:test" })).unwrap();
    let ack = &captured["inputAcknowledgement"];
    assert_eq!((captured["ok"].clone(), ack["status"].clone(), ack["applied"].clone(), ack["recording"].clone()), (json!(true), json!("applied"), json!(true), json!(true)), "{captured}");
    assert_eq!((ack["inputOwner"].clone(), ack["note"].clone(), ack["velocity"].clone(), ack["pattern"].clone()), (json!("key:test"), json!(62), json!(99), json!(0)));
    assert!(ack["row"].is_number() && ack["recordingContext"]["bpm"] == 116.0);
    let released = host.request("note.off", json!({ "note": 62, "track": 0, "instrument": 0, "captureInput": true, "inputOwner": "key:test" })).unwrap();
    assert_eq!(released["inputAcknowledgement"]["on"], false);
    host.request("transport.stop", json!({})).unwrap();
}

#[test]
fn devices_are_listed_configured_and_remembered() {
    let folder = TempDir::new("audio-devices");
    let fixture = start(folder);
    let host = &fixture.host;
    let devices = host.request("audio.devices", json!({})).unwrap();
    assert_eq!((devices["outputDevice"].clone(), devices["outputUid"].clone(), devices["running"].clone(), devices["fallback"].clone()), (json!(0), json!("null"), json!(true), json!(false)));
    assert_eq!((devices["sampleRate"].clone(), devices["bufferSize"].clone(), devices["bufferSizes"].clone()), (json!(48000.0), json!(512), json!([64, 128, 256, 512, 1024, 2048])));
    assert_eq!(devices["lineInputs"], json!([]));
    let null = devices["outputs"].as_array().unwrap().iter().find(|device| device["uid"] == "null").unwrap();
    assert_eq!((null["id"].clone(), null["outputChannels"].clone()), (json!(0), json!(2)));

    let configured = host.request("audio.configure", json!({ "bufferSize": 256, "outputDevice": 0 })).unwrap();
    assert_eq!((configured["bufferSize"].clone(), configured["outputUid"].clone(), configured["running"].clone()), (json!(256), json!("null"), json!(true)));
    assert!(host.request("audio.configure", json!({ "bufferSize": 1 })).unwrap_err().contains("buffer size"));
    assert!(host.request("audio.configure", json!({ "outputDevice": 4242 })).unwrap_err().contains("not connected"));
    // Monitoring and the input's channels are not settings, and no input was chosen: none is
    // remembered (tests/line_inputs.rs chooses one).
    assert_eq!(host.request("audio.configure", json!({ "monitor": true, "inputChannels": [0, 1] })).unwrap()["bufferSize"], 256);
    assert_eq!(host.request("settings.get", json!({})).unwrap()["values"], json!({}), "device settings are not interface settings");
    assert_eq!(fixture.services.settings.get_raw("audio.devices"), Some(json!({ "output": "null", "bufferSize": 256, "sampleRate": 48000.0 })));

    // A second host on the same support folder starts with what was chosen.
    let Fixture { folder, host, services } = fixture;
    drop(host);
    drop(services);
    let again = start(folder);
    assert_eq!(again.host.request("audio.devices", json!({})).unwrap()["bufferSize"], 256);
    let _ = &again.folder;
}

#[test]
fn the_snapshot_and_the_mixer_report_follow_the_project() {
    let fixture = start(TempDir::new("audio-snapshot"));
    let host = &fixture.host;
    assert_eq!(host.request("engine.snapshot", json!({})), Ok(json!({ "audioPlugins": [] })));
    let effects = host.request("builtin.effects", json!({})).unwrap();
    let compressor = effects.as_array().unwrap().iter().find(|effect| effect["id"] == "stfx:compressor").expect("the compressor is built in");
    assert!(compressor["parameters"][0]["address"] == 0 && compressor["parameters"][0]["id"].is_string() && compressor["sidechain"] == true);

    let mut project = demo();
    for (index, track) in project["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
        track["routingID"] = json!(format!("t{index}"));
    }
    project["routing"] = json!({ "buses": [], "outputs": {}, "sends": [], "sidechains": [] });
    project["audioPlugins"] = json!([
        { "owner": "track:t0", "kind": "effect", "slot": 0, "instanceID": "comp", "component": "stfx:compressor", "format": "Builtin", "name": "Compressor" },
        { "owner": "master", "kind": "effect", "slot": 0, "instanceID": "vendor", "component": "com.vendor.eq", "format": "VST2", "name": "Vendor EQ", "state": "b3BhcXVl" },
    ]);
    let project_tracks = project["tracks"].as_array().unwrap().len();
    host.request("engine.project", json!({ "project": project })).unwrap();
    let routing = host.request("routing.status", json!({})).unwrap();
    // What the compiled mixer is, and nothing that could only have one value.
    let mut keys: Vec<&str> = routing.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["inserts", "latencyFrames", "strips", "unavailableSidechains"]);
    assert_eq!(routing["strips"].as_array().unwrap().len(), project_tracks + 1);
    let inserts = routing["inserts"].as_array().unwrap();
    assert_eq!((inserts[0]["instanceID"].clone(), inserts[0]["available"].clone()), (json!("comp"), json!(true)));
    assert_eq!((inserts[1]["instanceID"].clone(), inserts[1]["available"].clone()), (json!("vendor"), json!(false)));
    // A descriptor that names no hostable plug-in passes through, with the reason.
    assert!(inserts[1]["reason"].as_str().unwrap().contains("names a plug-in format this version does not host"), "{}", inserts[1]["reason"]);

    let snapshot = host.request("engine.snapshot", json!({})).unwrap();
    assert_eq!(snapshot["name"], "Midnight transmission");
    let plugins = snapshot["audioPlugins"].as_array().unwrap();
    assert!(plugins[0]["state"].as_str().unwrap().len() > 40, "a built-in effect's state is captured");
    assert_eq!(plugins[1]["state"], "b3BhcXVl", "a plug-in descriptor keeps the state it carries");
    assert!(plugins.iter().all(|plugin| plugin.get("stateError").is_none()));

    // project.new quiets the engine through engine.panic and transport.recordArm, which now exist.
    host.request("transport.recordArm", json!({ "armed": true })).unwrap();
    host.request("project.new", json!({ "blank": true })).unwrap();
    assert_eq!(fixture.services.audio.as_ref().unwrap().engine().status()["noteRecordArmed"], false);
}

#[test]
fn a_sample_that_cannot_be_loaded_is_reported_once() {
    let fixture = start(TempDir::new("audio-missing"));
    let host = &fixture.host;
    let mut project = demo();
    project["instruments"][0]["samplePath"] = json!("/no/such/file.wav");
    project["instruments"][0]["mode"] = json!("oneshot");
    host.request("engine.project", json!({ "project": project.clone() })).unwrap();
    host.request("engine.project", json!({ "project": project.clone() })).unwrap();
    // The file is looked for on a worker thread: its failure arrives a moment later.
    let first = host.wait_for_event("sample.error", Duration::from_secs(10)).expect("the missing file is reported");
    host.request("engine.project", json!({ "project": project })).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let mut errors: Vec<Value> = vec![first];
    errors.extend(host.events().into_iter().filter(|(name, _)| name == "sample.error").map(|(_, payload)| payload));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!((errors[0]["instrument"].clone(), errors[0]["path"].clone()), (json!(0), json!("/no/such/file.wav")));
    assert!(errors[0]["error"].is_string());
}

#[test]
fn routing_changes_are_validated_applied_and_reported() {
    let fixture = start(TempDir::new("audio-routing"));
    let host = &fixture.host;
    let mut project = demo();
    for (index, track) in project["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
        track["routingID"] = json!(format!("t{index}"));
    }
    project["routing"] = json!({ "buses": [], "outputs": {}, "sends": [], "sidechains": [] });
    project["audioPlugins"] = json!([{ "owner": "track:t0", "kind": "effect", "slot": 0, "instanceID": "comp", "component": "stfx:compressor", "format": "Builtin", "name": "Compressor" }]);
    host.request("engine.project", json!({ "project": project })).unwrap();

    // What the mixer offers: one mix model, every channel as a stem, the compressor's key input.
    let capabilities = host.request("routing.capabilities", json!({})).unwrap();
    let mut keys: Vec<&str> = capabilities.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["audioExport", "buses", "exportTargets", "sends", "sidechains"]);
    assert_eq!(capabilities["buses"], json!({ "max": 1024 }));
    assert_eq!(capabilities["sends"], json!({ "taps": ["preFader", "postFader"], "max": 128 }));
    assert_eq!((capabilities["sidechains"]["taps"].clone(), capabilities["sidechains"]["max"].clone()), (json!(["preInserts", "preFader", "postFader"]), json!(128)));
    let targets = capabilities["exportTargets"].as_array().unwrap();
    let tracks = project["tracks"].as_array().unwrap().len();
    assert_eq!(targets.len(), tracks + 1, "every track and Master: {targets:?}");
    assert_eq!(targets[0], json!({ "owner": "track:t0", "name": project["tracks"][0]["name"], "kind": "track" }));
    assert_eq!(targets.last().unwrap(), &json!({ "owner": "master", "name": "Master", "kind": "master" }));
    let inputs = capabilities["sidechains"]["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0], json!({ "instanceID": "comp", "owner": "track:t0", "slot": 0, "pluginName": "Compressor" }), "the insert, and nothing about its one input");
    assert_eq!((capabilities["audioExport"]["container"].clone(), capabilities["audioExport"]["defaultEncoding"].clone()), (json!("wav"), json!("pcm24")));
    assert!(capabilities["audioExport"]["sampleRates"].as_array().unwrap().contains(&json!(96000)));

    // A proposal the project format refuses is answered, not thrown, and changes nothing.
    let mut broken = project.clone();
    broken["routing"]["sends"] = json!([{ "id": "s1", "source": "track:t0", "destination": "bus:nowhere", "tap": "postFader", "gain": 1.0, "enabled": true }]);
    let verdict = host.request("routing.validate", json!({ "project": broken })).unwrap();
    assert_eq!(verdict["valid"], false);
    assert!(!verdict["error"].as_str().unwrap().is_empty());
    assert!(host.request("routing.apply", json!({ "project": broken })).is_err(), "an invalid project is not applied");
    assert_eq!(host.request("routing.capabilities", json!({})).unwrap()["exportTargets"].as_array().unwrap().len(), tracks + 1);

    // A return bus, a send to it and the kick keying the compressor.
    let mut routed = project.clone();
    routed["routing"]["buses"] = json!([{ "id": "room", "name": "Room", "type": "return", "volume": 1.0, "pan": 0.0 }]);
    routed["routing"]["sends"] = json!([{ "id": "s1", "source": "track:t1", "destination": "bus:room", "tap": "postFader", "gain": 0.5, "enabled": true }]);
    routed["routing"]["sidechains"] = json!([{
        "id": "k1", "source": "track:t3", "tap": "preFader", "gain": 1.0, "enabled": true,
        "destination": "comp",
    }]);
    assert_eq!(host.request("routing.validate", json!({ "project": routed })).unwrap(), json!({ "valid": true }));
    let applied = host.request("routing.apply", json!({ "project": routed })).unwrap();
    assert_eq!(applied["project"]["routing"]["buses"][0]["id"], "room");
    assert_eq!(applied["project"]["routing"]["sends"][0]["destination"], "bus:room");
    let after = applied["capabilities"]["exportTargets"].as_array().unwrap();
    assert!(after.iter().any(|target| target["owner"] == "bus:room" && target["kind"] == "return" && target["name"] == "Room"), "{after:?}");
    assert_eq!(applied["status"]["unavailableSidechains"], json!([]), "the sidechain is connected");
    assert!(applied["status"]["strips"].as_array().unwrap().contains(&json!("bus:room")));
    // The session holds what was applied.
    assert_eq!(host.request("engine.snapshot", json!({})).unwrap()["routing"]["sidechains"][0]["id"], "k1");
}

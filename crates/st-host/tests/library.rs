//! The library area as the interface uses it: every command through the bridge, against a
//! folder of generated audio files, with the events the Browser workspace follows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use st_host::areas::library::LibraryOptions;
use st_host::areas::services::ServiceOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, BridgeBuilder, Host, HostOptions, NoPayload, ScriptedDialogs};

struct Fixture {
    host: TestHost,
    services: Host,
    dialogs: Arc<ScriptedDialogs>,
}

/// A host with the library only, on `support`; `extra` registers stand-ins for other areas.
fn start(support: &Path, extra: impl FnOnce(&mut BridgeBuilder)) -> Fixture {
    let dialogs = Arc::new(ScriptedDialogs::from_answers(Vec::new()));
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(support.to_path_buf(), dialogs.clone());
        options.autosave_every = None;
        options.audio = None;
        options.services = ServiceOptions { library: Some(LibraryOptions::for_support(support)), ..ServiceOptions::none() };
        services = Some(install(bridge, options).expect("install the host"));
        extra(bridge);
    });
    host.request("ready", json!({})).expect("the handshake");
    Fixture { host, services: services.unwrap(), dialogs }
}

/// A short 16-bit mono WAV file.
fn write_wav(path: &Path, frames: u32, hertz: f64) {
    let data: Vec<u8> = (0..frames).flat_map(|frame| (((frame as f64 * hertz * std::f64::consts::TAU / 44_100.0).sin() * 12_000.0) as i16).to_le_bytes()).collect();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&88_200u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A folder of drum sounds: `Drums/Kicks/Kick 01..03.wav`, `Drums/Snares/Snare 01..02.wav`, a loop.
fn sounds(root: &Path) -> PathBuf {
    for index in 1..=3 {
        write_wav(&root.join(format!("Drums/Kicks/Kick 0{index}.wav")), 4_410, 55.0 + index as f64);
    }
    for index in 1..=2 {
        write_wav(&root.join(format!("Drums/Snares/Snare 0{index}.wav")), 4_410, 180.0 + index as f64);
    }
    write_wav(&root.join("Loops/Break 120bpm.wav"), 88_200, 220.0);
    root.to_path_buf()
}

/// Waits for the scan that is running to finish; returns the last `library.results` payload.
fn scanned(host: &TestHost) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let results = host.wait_for_event("library.results", deadline.saturating_duration_since(std::time::Instant::now())).expect("the scan reports its results");
        if results["scanning"] == false && results["cached"] != true {
            return results;
        }
    }
}

fn names(result: &Value) -> Vec<String> {
    let mut names: Vec<String> = result["items"].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap().to_string()).collect();
    names.sort();
    names
}

#[test]
fn a_folder_is_added_scanned_searched_and_favorites_survive_the_host() {
    let folder = TempDir::new("library");
    let support = folder.path().join("support");
    let root = sounds(&folder.path().join("Sounds"));
    let root_text = root.to_string_lossy().into_owned();
    let favorite;
    {
        let fixture = start(&support, |_| {});
        let host = &fixture.host;
        let ready = host.request("ready", json!({})).unwrap();
        for command in ["library.search", "library.scan", "library.chooseRoot", "library.favorite", "kit.read", "kit.save", "kit.detail"] {
            assert!(ready["commands"].as_array().unwrap().contains(&json!(command)), "{command} is implemented");
        }
        // A host on a private support folder starts with its own kit folder and nothing else.
        let roots = ready["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert_eq!(roots[0]["path"], json!(support.join("Kits")));

        // Add folder: the chooser, then the scan.
        fixture.dialogs.push(None);
        assert_eq!(host.request("library.chooseRoot", json!({})), Ok(json!({ "cancelled": true })), "Cancel in the chooser");
        fixture.dialogs.push(Some(root.clone()));
        let started = host.request("library.chooseRoot", json!({})).unwrap();
        assert_eq!(started["scanning"], true, "{started}");
        let results = scanned(host);
        assert!(results["count"].as_u64().unwrap() >= 6, "{results}");

        let listed = host.request("library.roots", json!({})).unwrap();
        let added = listed.as_array().unwrap().iter().find(|item| item["path"] == json!(root_text)).expect("the folder is a library root");
        assert_eq!((added["available"].clone(), added["indexedCount"].clone(), added["scanStatus"].clone()), (json!(true), json!(6), json!("complete")), "{added}");

        // Search: by text, by category, everything.
        let kicks = host.request("library.search", json!({ "query": "kick", "limit": 50 })).unwrap();
        assert_eq!(names(&kicks), ["Kick 01", "Kick 02", "Kick 03"], "{kicks}");
        assert_eq!(kicks["total"], 3);
        let all = host.request("library.search", json!({ "limit": 50 })).unwrap();
        assert_eq!(all["total"], 6, "{all}");
        let none = host.request("library.search", json!({ "query": "zither" })).unwrap();
        assert_eq!(none["total"], 0);

        // Favorite: the reply, the event, the filter.
        favorite = kicks["items"][1]["path"].as_str().unwrap().to_string();
        assert_eq!(host.request("library.favorite", json!({ "path": favorite, "favorite": true })), Ok(json!({ "path": favorite, "favorite": true })));
        assert_eq!(host.wait_for_event("library.favorite", Duration::from_secs(5)), Some(json!({ "path": favorite, "favorite": true })));
        let favorites = host.request("library.search", json!({ "favorites": true })).unwrap();
        assert_eq!(favorites["items"].as_array().unwrap().len(), 1);
        assert_eq!(favorites["items"][0]["favorite"], true);

        // What the browser remembers.
        assert_eq!(host.request("library.state.get", json!({})), Ok(json!({})));
        assert_eq!(host.request("library.state.set", json!({ "state": { "tab": "samples", "query": "kick" } })), Ok(json!({ "saved": true })));
        assert!(host.request("library.state.set", json!({ "state": 3 })).unwrap_err().contains("JSON object"));

        // The files live in Library/ of the support folder.
        fixture.services.services.shutdown();
        for file in ["roots.json", "favorites.json", "browser-state.json", "index.bin"] {
            assert!(support.join("Library").join(file).is_file(), "{file} is written");
        }
    }

    // A new host on the same support folder: the root, the index (from its cache) and the favorite are back.
    let fixture = start(&support, |_| {});
    let host = &fixture.host;
    let ready = host.request("ready", json!({})).unwrap();
    assert!(ready["roots"].as_array().unwrap().iter().any(|item| item["path"] == json!(root_text)), "{ready}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let favorites = loop {
        let favorites = host.request("library.search", json!({ "favorites": true })).unwrap();
        if favorites["total"] == 1 || std::time::Instant::now() > deadline {
            break favorites;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(favorites["items"][0]["path"], json!(favorite), "{favorites}");
    assert_eq!(host.request("library.state.get", json!({})), Ok(json!({ "tab": "samples", "query": "kick" })));

    // Removing the root drops its items.
    let left = host.request("library.removeRoot", json!({ "path": root_text })).unwrap();
    assert!(left.as_array().unwrap().iter().all(|item| item["path"] != json!(root_text)));
    assert_eq!(host.request("library.search", json!({})).unwrap()["total"], 0);
}

/// A session with one kit group: a sampler pad and a plug-in pad.
fn session(sample: &Path) -> Value {
    json!({
        "version": 1, "name": "Session", "bpm": 120, "linesPerBeat": 4,
        "tracks": [
            { "id": 0, "name": "Kick", "routingID": "kick", "groupID": "g1", "pad": 0, "padNote": 60, "instrument": 0 },
            { "id": 1, "name": "Synth", "routingID": "synth", "groupID": "g1", "pad": 1, "instrument": 1 }
        ],
        "instruments": [
            { "id": 0, "type": "sample", "name": "Kick", "samplePath": sample },
            { "id": 1, "type": "au", "name": "Synth" }
        ],
        "groups": [ { "id": "g1", "name": "Bridge kit", "busID": "kitbus" } ],
        "audioPlugins": [ { "instanceID": "p-synth", "owner": "track:synth", "track": 1, "kind": "instrument", "slot": 0, "component": "vst3:ABCD", "format": "VST3", "name": "Fixture Synth", "state": "AAEC" } ],
        "routing": { "buses": [ { "id": "kitbus", "name": "Bridge kit", "type": "group", "groupID": "g1" } ],
            "outputs": { "track:kick": "bus:kitbus", "track:synth": "bus:kitbus", "bus:kitbus": "master" }, "sends": [], "sidechains": [] },
        "patterns": [ { "id": 0, "length": 4, "steps": [[null, null, null, null], [null, null, null, null]] } ],
        "sequence": [0]
    })
}

#[test]
fn a_kit_is_saved_from_the_payloads_project_and_read_back() {
    let folder = TempDir::new("library-kit");
    let support = folder.path().join("support");
    let sample = folder.path().join("Sounds/Kick.wav");
    write_wav(&sample, 4_410, 60.0);
    let fixture = start(&support, |_| {});
    let host = &fixture.host;

    assert!(host.request("kit.save", json!({ "groupID": "g1" })).unwrap_err().contains("Select a kit"), "no project anywhere");
    let saved = host.request("kit.save", json!({ "project": session(&sample), "groupID": "g1", "name": "Bridge kit" })).unwrap();
    let path = saved["path"].as_str().expect("the package's path").to_string();
    assert_eq!(Path::new(&path), support.join("Kits/Bridge kit.stkit"), "kits of a private host stay in its support folder");
    assert!(Path::new(&path).join("kit.json").is_file() || Path::new(&path).is_file(), "{saved}");
    // The same name again asks first; with overwrite it replaces.
    let again = host.request("kit.save", json!({ "project": session(&sample), "groupID": "g1", "name": "Bridge kit" })).unwrap();
    assert_eq!((again["exists"].clone(), again["name"].clone()), (json!(true), json!("Bridge kit")));
    assert!(host.request("kit.save", json!({ "project": session(&sample), "groupID": "g1", "name": "Bridge kit", "overwrite": true })).unwrap()["path"].is_string());

    let read = host.request("kit.read", json!({ "path": path })).unwrap();
    assert_eq!(read["groupProject"]["tracks"].as_array().unwrap().len(), 2, "{read}");
    assert_eq!(read["groupProject"]["groups"][0]["name"], "Bridge kit");

    // No plug-in area: plug-in availability is not judged.
    let detail = host.request("kit.detail", json!({ "path": path })).unwrap();
    assert_eq!(detail["compatibility"]["missingPlugins"], json!([]), "{detail}");
    assert_eq!(detail["compatibility"]["summary"], "Ready to load");
    assert_eq!(detail["sounds"].as_array().unwrap().len(), 2);
    // The request can name the components itself.
    let judged = host.request("kit.detail", json!({ "path": path, "availableComponents": ["vst3:OTHER"] })).unwrap();
    assert_eq!(judged["compatibility"]["missingPlugins"], json!(["Fixture Synth"]));

    // The saved kit is in the library (kit.save rescans the kit folder).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let kits = host.request("library.kits.browse", json!({ "user": true })).unwrap();
        if kits["items"].as_array().is_some_and(|items| items.iter().any(|item| item["path"] == json!(path))) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the saved kit never appeared in the kit browser: {kits}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn kits_use_the_plug_in_list_of_the_plug_in_area_once_it_exists() {
    let folder = TempDir::new("library-seams");
    let support = folder.path().join("support");
    let sample = folder.path().join("Sounds/Kick.wav");
    write_wav(&sample, 4_410, 60.0);
    let installed = Arc::new(Mutex::new(vec![json!({ "id": "vst3:ABCD", "name": "Fixture Synth" })]));
    let fixture = start(&support, {
        let installed = installed.clone();
        // A stand-in for the plug-in area.
        move |bridge| bridge.commands().register("plugins.list", move |_: NoPayload| -> st_host::Result<Value> { Ok(Value::Array(installed.lock().unwrap().clone())) })
    });
    let host = &fixture.host;

    let saved = host.request("kit.save", json!({ "project": session(&sample), "groupID": "g1", "name": "Seam kit" })).unwrap();
    let path = saved["path"].as_str().unwrap().to_string();
    let detail = host.request("kit.detail", json!({ "path": path })).unwrap();
    assert_eq!(detail["compatibility"]["missingPlugins"], json!([]), "the plug-in is installed: {detail}");
    // This host has no audio engine: the kit is saved without a preview, and without a word about it.
    assert!(detail.get("previewPath").is_none() && saved["warnings"].as_array().unwrap().iter().all(|warning| !warning.as_str().unwrap().contains("preview")), "{saved}");
    assert_eq!(st_host::areas::library::kit_preview_path(&path), None);

    installed.lock().unwrap().clear();
    let detail = host.request("kit.detail", json!({ "path": path })).unwrap();
    assert_eq!(detail["compatibility"]["missingPlugins"], json!(["Fixture Synth"]));
    assert_eq!(detail["compatibility"]["summary"], "Review import notes");
    assert!(detail["compatibility"]["warnings"].as_array().unwrap().iter().any(|warning| warning.as_str().unwrap().contains("Fixture Synth")));
}

/// A session as the page publishes it: a kit of two pads on a group bus with a built-in
/// effect (a sampled kick on row 0 and the CLAP test sine of `crates/plugin-host` on rows 8
/// and 9), and a bass track outside the kit that plays the same sample on row 4.
fn kit_session(sample: &Path, clap: &Path) -> Value {
    let mut kick = vec![Value::Null; 16];
    kick[0] = json!({ "note": 60, "instrument": 0, "velocity": 100 });
    let mut synth = vec![Value::Null; 16];
    synth[8] = json!({ "note": 60, "instrument": 1, "velocity": 100, "gate": 2 });
    let mut bass = vec![Value::Null; 16];
    bass[4] = json!({ "note": 60, "instrument": 0, "velocity": 100 });
    json!({
        "version": 1, "name": "Session", "bpm": 120, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": [
            { "id": 0, "name": "Kick", "routingID": "kick", "groupID": "g1", "pad": 0, "padNote": 60, "instrument": 0, "volume": 1, "pan": 0 },
            { "id": 1, "name": "Synth", "routingID": "synth", "groupID": "g1", "pad": 1, "instrument": 1, "volume": 1, "pan": 0 },
            { "id": 2, "name": "Bass", "routingID": "bass", "instrument": 0, "volume": 1, "pan": 0 }
        ],
        "instruments": [
            { "id": 0, "type": "sample", "name": "Kick", "samplePath": sample, "mode": "oneshot", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.01 },
            { "id": 1, "type": "au", "name": "Synth", "volume": 1, "pan": 0 }
        ],
        "groups": [ { "id": "g1", "name": "Bridge kit", "busID": "kitbus" } ],
        "audioPlugins": [
            { "instanceID": "p-synth", "owner": "track:synth", "track": 1, "kind": "instrument", "slot": 0, "component": format!("clap:{}", st_audio::plugins::testing::CLAP_SINE), "format": "CLAP", "bundlePath": clap, "name": "ST Test Sine" },
            { "instanceID": "p-bus", "owner": "bus:kitbus", "kind": "effect", "slot": 0, "component": "stfx:eq", "name": "EQ" }
        ],
        "routing": { "buses": [ { "id": "kitbus", "name": "Bridge kit", "type": "group", "groupID": "g1", "volume": 1, "pan": 0 } ],
            "outputs": { "track:kick": "bus:kitbus", "track:synth": "bus:kitbus", "bus:kitbus": "master" }, "sends": [], "sidechains": [] },
        "patterns": [ { "id": 0, "length": 16, "steps": [kick, synth, bass], "automation": [] } ],
        "sequence": [0], "loopSong": true
    })
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn a_saved_kit_carries_a_preview_of_its_group_bus_that_the_browser_auditions() {
    use st_audio::plugins::testing::artifacts;
    use st_host::areas::audio::{AudioOptions, AudioOutput};
    use st_host::areas::plugins::PluginAreaOptions;

    let folder = TempDir::new("library-preview");
    let support = folder.path().join("support");
    let sample = folder.path().join("Sounds/Kick.wav");
    write_wav(&sample, 4_410, 60.0);
    let installed = artifacts().install(&folder.path().join("plugins"));
    // The library with the audio engine on the null device and plug-ins in containers.
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(support.clone(), Arc::new(ScriptedDialogs::from_answers(Vec::new())));
        options.autosave_every = None;
        options.audio = Some(AudioOptions { output: AudioOutput::Null });
        options.plugins = Some(PluginAreaOptions { container: Some(artifacts().container.clone()), folders: vec![installed.folder.clone()], only_these_folders: true });
        options.services = ServiceOptions { library: Some(LibraryOptions::for_support(&support)), ..ServiceOptions::none() };
        services = Some(install(bridge, options).expect("install the host"));
    });
    let ready = host.request("ready", json!({})).expect("the handshake");
    assert!(!ready["commands"].as_array().unwrap().contains(&json!("audio.kitPreview")), "the preview is not a command of its own");
    let long = |command: &str, payload: Value| host.reply_within(host.send(command, payload), Duration::from_secs(120));

    // The page publishes the session and sends it along with kit.save. The song is playing.
    let session = kit_session(&sample, &installed.clap);
    long("engine.project", json!({ "project": session })).unwrap();
    host.request("transport.play", json!({ "mode": "pattern", "fromStart": true })).unwrap();
    let saved = long("kit.save", json!({ "project": session, "groupID": "g1", "name": "Bridge kit" })).unwrap();
    let package = PathBuf::from(saved["path"].as_str().expect("the package's path"));
    assert_eq!(saved["warnings"], json!([]), "{saved}");
    // A kit of that name exists now: asking again renders nothing and changes nothing.
    let modified = std::fs::metadata(package.join("Preview.wav")).expect("the preview is in the package").modified().unwrap();
    assert_eq!(long("kit.save", json!({ "project": session, "groupID": "g1", "name": "Bridge kit" })).unwrap()["exists"], true);
    assert_eq!(std::fs::metadata(package.join("Preview.wav")).unwrap().modified().unwrap(), modified);

    // The file: 16-bit stereo WAV at 48 kHz; the pattern (16 rows, two seconds) and a second of tail.
    let bytes = std::fs::read(package.join("Preview.wav")).unwrap();
    let format = bytes.windows(4).position(|window| window == b"fmt ").expect("a format chunk") + 8;
    let word = |offset: usize| u16::from_le_bytes([bytes[format + offset], bytes[format + offset + 1]]);
    assert_eq!((word(0), word(2), u32::from_le_bytes(bytes[format + 4..format + 8].try_into().unwrap()), word(14)), (1, 2, 48_000, 16));
    let preview = st_audio::samples::read_wav(&bytes).unwrap();
    assert_eq!(preview.frames(), 3 * 48_000);
    // Audio where the kit sounds: the kick on row 0 (a tenth of a second), the plug-in on rows
    // 8 and 9 (a row is 6000 frames). The bass on row 4 is not part of the kit.
    let (row, quiet) = (6_000, 4.0 / 32_768.0);
    assert!(peak(&preview.left[..4_800]) > 0.05, "the kick: {}", peak(&preview.left[..4_800]));
    assert!(peak(&preview.left[row..8 * row]) <= quiet, "nothing between the pads, the bass least of all: {}", peak(&preview.left[row..8 * row]));
    assert!(peak(&preview.left[8 * row..10 * row]) > 0.05 && peak(&preview.right[8 * row..10 * row]) > 0.05, "the plug-in pad");
    assert!(peak(&preview.left[10 * row + 2_400..]) <= quiet, "silence after the last pad and in the tail");

    // The browser finds the preview with the kit and plays it on the preview mixer.
    let detail = host.request("kit.detail", json!({ "path": package })).unwrap();
    assert_eq!(detail["previewPath"], json!(package.join("Preview.wav")), "{detail}");
    assert_eq!(st_host::areas::library::kit_preview_path(&package.to_string_lossy()), Some(package.join("Preview.wav").to_string_lossy().into_owned()));
    let playing = long("sample.preview", json!({ "path": detail["previewPath"] })).unwrap();
    assert_eq!(playing["playing"], true, "{playing}");
    assert!((playing["duration"].as_f64().unwrap() - 3.0).abs() < 0.01, "{playing}");
    host.request("sample.stopPreview", json!({})).unwrap();
    // The live transport was not touched by the render.
    assert_eq!(host.request("transport.stop", json!({})).unwrap()["playing"], false);

    // A kit whose pads play nothing in the selected pattern is saved all the same, without a
    // preview, and the reply says why.
    let mut empty = session.clone();
    empty["patterns"][0]["steps"][0] = json!(vec![Value::Null; 16]);
    empty["patterns"][0]["steps"][1] = json!(vec![Value::Null; 16]);
    long("engine.project", json!({ "project": empty })).unwrap();
    let saved = long("kit.save", json!({ "project": empty, "groupID": "g1", "name": "Quiet kit" })).unwrap();
    let warnings = saved["warnings"].as_array().unwrap();
    assert!(warnings.len() == 1 && warnings[0].as_str().unwrap().starts_with("The kit was saved without an audio preview: the kit plays no note"), "{saved}");
    let quiet_package = PathBuf::from(saved["path"].as_str().unwrap());
    assert!(quiet_package.join("kit.json").is_file() && !quiet_package.join("Preview.wav").exists());
    assert!(host.request("kit.detail", json!({ "path": quiet_package })).unwrap().get("previewPath").is_none());
    services.unwrap().services.shutdown();
}

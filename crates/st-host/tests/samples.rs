//! The sample commands as the interface uses them, on the null device.

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
    host.request("ready", json!({})).expect("the handshake");
    Fixture { folder, host, services: services.unwrap() }
}

/// One track playing instrument 0 (a one-shot sampler) on rows 0 and 8, looping.
fn project(sample: &str) -> Value {
    let mut rows = vec![Value::Null; 16];
    rows[0] = json!({ "note": 60, "instrument": 0, "velocity": 110, "gate": 0 });
    rows[8] = rows[0].clone();
    json!({
        "version": 1, "bpm": 120, "linesPerBeat": 4, "masterVolume": 1,
        "tracks": [{ "id": 0, "name": "a", "routingID": "a", "volume": 1, "pan": 0, "instrument": 0 }],
        "instruments": [{ "id": 0, "type": "sample", "samplePath": sample, "mode": "oneshot", "volume": 1, "attack": 0, "sustain": 1, "release": 0.05 }],
        "patterns": [{ "id": 0, "length": 16, "steps": [rows], "automation": [] }],
        "sequence": [0], "loopSong": true,
        "routing": { "buses": [], "outputs": {}, "sends": [], "sidechains": [] },
        "audioPlugins": [],
    })
}

/// A second of clicks every 100 ms on a quiet 440 Hz tone, with a 50 ms silent lead-in.
fn write_wav(path: &std::path::Path) {
    let mut samples = vec![0.0f32; 48_000];
    for (frame, sample) in samples.iter_mut().enumerate().skip(2_400) {
        let since = (frame - 2_400) % 4_800;
        let burst = if since < 480 { 0.8 * (1.0 - since as f32 / 480.0) } else { 0.0 };
        *sample = (frame as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * (0.02 + burst);
    }
    std::fs::write(path, st_audio::samples::wav_bytes(&samples, None, 48_000)).unwrap();
}

thread_local! {
    /// Every event of this test's host so far, and how far `wait_for` has looked.
    static LOG: std::cell::RefCell<(Vec<(String, Value)>, usize)> = const { std::cell::RefCell::new((Vec::new(), 0)) };
}

/// Every event received so far (also those `wait_for` has looked past).
fn all_events(host: &TestHost) -> Vec<(String, Value)> {
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        log.0.extend(host.events());
        log.0.clone()
    })
}

/// The next event (after the one the previous call returned) that satisfies `wanted`.
fn wait_for(host: &TestHost, what: &str, wanted: impl Fn(&str, &Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let found = LOG.with(|log| {
            let mut log = log.borrow_mut();
            log.0.extend(host.events());
            let from = log.1;
            let found = log.0[from..].iter().position(|(name, payload)| wanted(name, payload)).map(|index| from + index);
            found.map(|index| {
                log.1 = index + 1;
                log.0[index].1.clone()
            })
        });
        if let Some(found) = found {
            return found;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no event where {what}");
}

#[test]
fn a_sample_loads_from_the_page_and_is_heard() {
    let fixture = start(TempDir::new("samples-load"));
    let host = &fixture.host;
    let file = fixture.folder.path().join("clicks.wav");
    write_wav(&file);
    let path = file.to_string_lossy().into_owned();

    let ready = host.request("ready", json!({})).unwrap();
    for command in ["sample.load", "sample.edit", "sample.peaks", "sample.info", "sample.analyzeSlices", "sample.wavetables", "sampler.playheads", "audio.clipInfo", "builtin.drumModels"] {
        assert!(ready["commands"].as_array().unwrap().contains(&json!(command)), "{command} is implemented");
    }

    // The page loads the file into instrument 0, then publishes the project that names it.
    let info = host.request("sample.load", json!({ "path": path, "instrument": 0 })).unwrap();
    assert_eq!((info["path"].clone(), info["name"].clone(), info["frames"].clone(), info["sampleRate"].clone(), info["channels"].clone()), (json!(path), json!("clicks"), json!(48_000), json!(48_000.0), json!(1)));
    assert_eq!(info["peaks"].as_array().unwrap().len(), 512);
    assert!(info["peaks"].as_array().unwrap().iter().any(|peak| peak.as_f64().unwrap() > 0.5));
    assert!((info["duration"].as_f64().unwrap() - 1.0).abs() < 1e-9);

    host.request("engine.project", json!({ "project": project(&path) })).unwrap();
    assert_eq!(fixture.services.audio.as_ref().unwrap().engine().last_report().samples_installed, 1, "the loaded sample is installed by the publication, without another decode");
    host.request("transport.play", json!({ "mode": "pattern", "fromStart": true })).unwrap();
    let heard = wait_for(host, "the master meter moves", |name, payload| name == "status" && payload["peakL"].as_f64().unwrap_or(0.0) > 0.05);
    assert!(heard["trackPeaks"][0].as_f64().unwrap() > 0.05);
    host.request("transport.stop", json!({})).unwrap();

    // The summary is available without decoding again; not for another path or instrument.
    assert_eq!(host.request("sample.info", json!({ "instrument": 0, "path": path })).unwrap()["frames"], 48_000);
    assert_eq!(host.request("sample.info", json!({ "instrument": 0, "path": "/other.wav" })).unwrap(), json!({ "available": false }));
    assert_eq!(host.request("sample.info", json!({ "instrument": 3, "path": path })).unwrap(), json!({ "available": false }));
    assert_eq!(host.request("audio.clipInfo", json!({ "path": path })).unwrap()["frames"], 48_000);
    assert!(host.request("sample.load", json!({ "instrument": 0 })).unwrap_err().contains("No sample path"));
    assert!(host.request("sample.load", json!({ "path": "/no/such/file.wav", "instrument": 0 })).is_err());
    assert!(all_events(host).iter().all(|(name, _)| name != "sample.error"));
}

#[test]
fn a_project_sample_is_decoded_in_the_background_and_announced() {
    let fixture = start(TempDir::new("samples-background"));
    let host = &fixture.host;
    let file = fixture.folder.path().join("clicks.wav");
    write_wav(&file);
    let path = file.to_string_lossy().into_owned();
    // A session opened from disk: the page publishes a project whose samples the host has not seen.
    let mut opened = project(&path);
    opened["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "sample", "samplePath": "/no/such/file.wav", "mode": "oneshot" }));
    opened["audioClips"] = json!([{ "id": "c1", "track": 0, "startBeat": 0, "durationBeats": 2, "path": path }]);
    host.request("engine.project", json!({ "project": opened.clone() })).unwrap();
    let loaded = wait_for(host, "sample.loaded", |name, _| name == "sample.loaded");
    assert_eq!((loaded["instrument"].clone(), loaded["sample"]["path"].clone(), loaded["sample"]["frames"].clone()), (json!(0), json!(path), json!(48_000)));
    assert_eq!(loaded["sample"]["peaks"].as_array().unwrap().len(), 512);
    let engine = fixture.services.audio.as_ref().unwrap().engine().clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.last_report().problems.iter().any(|problem| problem.error.is_none()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    host.request("engine.project", json!({ "project": opened })).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let events = all_events(host);
    let errors: Vec<&Value> = events.iter().filter(|(name, _)| name == "sample.error").map(|(_, payload)| payload).collect();
    assert_eq!(errors.len(), 1, "the missing file is reported once: {errors:?}");
    assert_eq!((errors[0]["instrument"].clone(), errors[0]["path"].clone()), (json!(1), json!("/no/such/file.wav")));
    assert!(events.iter().any(|(name, payload)| name == "audio.clipLoaded" && payload["path"] == json!(path)), "the clip's file is announced: {:?}", events.iter().filter(|(name, _)| name != "status").map(|(name, payload)| format!("{name} {}", payload.to_string().chars().take(80).collect::<String>())).collect::<Vec<_>>());
    host.request("transport.play", json!({ "mode": "pattern", "fromStart": true })).unwrap();
    wait_for(host, "the sample is heard", |name, payload| name == "status" && payload["peakL"].as_f64().unwrap_or(0.0) > 0.05);
}

#[test]
fn edits_write_new_files_and_slices_are_detected() {
    let fixture = start(TempDir::new("samples-edit"));
    let host = &fixture.host;
    let file = fixture.folder.path().join("clicks.wav");
    write_wav(&file);
    let original = std::fs::read(&file).unwrap();
    let path = file.to_string_lossy().into_owned();
    host.request("sample.load", json!({ "path": path, "instrument": 0 })).unwrap();
    host.request("engine.project", json!({ "project": project(&path) })).unwrap();

    // Trim to the second half: a new file in the support folder's Edits directory.
    let edited = host.request("sample.edit", json!({ "instrument": 0, "path": path, "operation": "trim", "start": 0.5, "end": 1.0 })).unwrap();
    let edited_path = edited["path"].as_str().unwrap().to_string();
    assert_eq!(edited["frames"], 24_000);
    assert!(edited_path.starts_with(&*fixture.folder.path().join("support").join("Edits").to_string_lossy()), "{edited_path}");
    assert!(edited_path.contains("clicks-trim-") && std::path::Path::new(&edited_path).is_file());
    assert_eq!(std::fs::read(&file).unwrap(), original, "the source file is never modified");
    // The page then publishes the instrument with the new path: it is ready at once.
    host.request("engine.project", json!({ "project": project(&edited_path) })).unwrap();
    let engine = fixture.services.audio.as_ref().unwrap().engine().clone();
    assert_eq!((engine.last_report().samples_installed, engine.last_report().problems.len()), (1, 0));
    // Without a path the instrument's sample is edited; the result chains from the edit.
    let reversed = host.request("sample.edit", json!({ "instrument": 0, "operation": "reverse", "start": 0, "end": 1 })).unwrap();
    assert!(reversed["path"].as_str().unwrap().contains("-reverse-") && reversed["frames"] == 24_000);
    assert!(host.request("sample.edit", json!({ "instrument": 0, "path": path, "operation": "polish" })).unwrap_err().contains("Unknown sample edit"));
    assert!(host.request("sample.edit", json!({ "instrument": 5, "operation": "reverse" })).unwrap_err().contains("Load a sample"));

    // Transients: a click every 100 ms after 50 ms of silence.
    let slices = host.request("sample.analyzeSlices", json!({ "instrument": 0, "path": path, "sensitivity": 0.5, "minimumGapSeconds": 0.04, "maximumSlices": 64 })).unwrap();
    let points: Vec<f64> = slices["slicePoints"].as_array().unwrap().iter().map(|point| point.as_f64().unwrap()).collect();
    assert_eq!(slices["method"], "transient");
    assert!((9..=12).contains(&points.len()), "ten clicks: {points:?}");
    assert!(points.iter().skip(1).all(|point| ((point * 48_000.0 - 2_400.0) / 4_800.0 - ((point * 48_000.0 - 2_400.0) / 4_800.0).round()).abs() < 0.05), "markers sit on the clicks: {points:?}");

    // The editor's zoomed waveform and zero-crossing snapping.
    let peaks = host.request("sample.peaks", json!({ "instrument": 0, "path": path, "start": 0.0, "end": 0.5, "bins": 100 })).unwrap();
    assert_eq!((peaks["min"].as_array().unwrap().len(), peaks["max"].as_array().unwrap().len(), peaks["frames"].clone()), (100, 100, json!(48_000)));
    assert!(peaks["max"].as_array().unwrap().iter().any(|value| value.as_f64().unwrap() > 0.5));
    let snapped = host.request("sample.peaks", json!({ "instrument": 0, "path": path, "start": 0, "end": 0, "bins": 1, "snap": [0.5], "direction": 0 })).unwrap();
    assert!((snapped["snapped"][0].as_f64().unwrap() - 0.5).abs() < 0.01);
    assert!(host.request("sample.peaks", json!({ "instrument": 0, "path": "/not/loaded.wav" })).unwrap_err().contains("still loading"));

    let tables = host.request("sample.wavetables", json!({})).unwrap();
    assert!(tables.as_array().unwrap().iter().any(|table| table["path"] == "builtin:wavetables/Basic/Saw.wav" && table["frameSize"] == 2048 && table["wavetable"] == true));
    let drums = host.request("builtin.drumModels", json!({})).unwrap();
    assert_eq!(drums.as_array().unwrap().len(), 6);
    assert_eq!((drums[0]["id"].clone(), drums[0]["name"].clone(), drums[0]["parameters"].as_array().unwrap().len()), (json!("kick"), json!("Kick"), 8));
}

#[test]
fn playheads_follow_the_voices_of_the_watched_instrument() {
    let fixture = start(TempDir::new("samples-playheads"));
    let host = &fixture.host;
    let file = fixture.folder.path().join("clicks.wav");
    write_wav(&file);
    let path = file.to_string_lossy().into_owned();
    host.request("sample.load", json!({ "path": path, "instrument": 0 })).unwrap();
    host.request("engine.project", json!({ "project": project(&path) })).unwrap();
    assert_eq!(host.request("sampler.playheads", json!({ "enabled": true, "instrument": 0 })).unwrap(), json!({ "ok": true, "watching": 0 }));
    host.request("note.on", json!({ "note": 60, "velocity": 100, "track": 0, "instrument": 0 })).unwrap();
    let playheads = wait_for(host, "a voice is reported", |name, payload| name == "sampler.playheads" && payload["voices"].as_array().is_some_and(|voices| !voices.is_empty()));
    let voice = &playheads["voices"][0];
    assert_eq!((playheads["instrument"].clone(), voice["instrument"].clone(), voice["note"].clone(), voice["frames"].clone()), (json!(0), json!(0), json!(60), json!(48_000.0)));
    let first = voice["position"].as_f64().unwrap();
    let later = wait_for(host, "the playhead moves", |name, payload| name == "sampler.playheads" && payload["voices"][0]["position"].as_f64().is_some_and(|position| position > first));
    assert!(later["voices"][0]["position"].as_f64().unwrap() <= 48_000.0);
    // The one-shot ends: an empty snapshot clears the display, then the poll is silent.
    wait_for(host, "the voices end", |name, payload| name == "sampler.playheads" && payload["voices"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(host.request("sampler.playheads", json!({ "enabled": false })).unwrap(), json!({ "ok": true, "watching": -2 }));
    std::thread::sleep(Duration::from_millis(100));
    let before = all_events(host).len();
    host.request("note.on", json!({ "note": 60, "velocity": 100, "track": 0, "instrument": 0 })).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(all_events(host)[before..].iter().all(|(name, _)| name != "sampler.playheads"), "no playheads once the editor is hidden");
}

#[test]
fn previews_start_stop_and_are_prefetched() {
    let fixture = start(TempDir::new("samples-preview"));
    let host = &fixture.host;
    let (first, second) = (fixture.folder.path().join("first.wav"), fixture.folder.path().join("second.wav"));
    write_wav(&first);
    std::fs::write(&second, st_audio::samples::wav_bytes(&[0.4; 4_800], None, 48_000)).unwrap();
    let (first, second) = (first.to_string_lossy().into_owned(), second.to_string_lossy().into_owned());

    assert_eq!(host.request("sample.prefetchPreviews", json!({ "items": [first, { "path": second }, { "kit": "/kits/a.stkit" }, 7] })).unwrap(), json!({ "queued": 2 }));
    let deadline = Instant::now() + Duration::from_secs(10);
    while host.request("sample.previewStatus", json!({})).unwrap()["cachedPreviews"] != 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = host.request("sample.preview", json!({ "path": first })).unwrap();
    assert_eq!((started["playing"].clone(), started["cached"].clone()), (json!(true), json!(true)));
    assert!((started["duration"].as_f64().unwrap() - 1.0).abs() < 1e-6 && started["startMs"].as_f64().unwrap() >= 0.0);
    // The null device renders in real time: the voice is taken within a block or two.
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.request("sample.previewStatus", json!({})).unwrap()["voices"] != 1 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.request("sample.previewStatus", json!({})).unwrap()["voices"], 1);
    // The short one replaces it and plays to its end (a tenth of a second).
    assert_eq!(host.request("sample.preview", json!({ "path": second, "volume": 0.2 })).unwrap()["playing"], true);
    let ended = wait_for(host, "the preview ends", |name, _| name == "sample.previewEnded");
    assert_eq!(ended, json!({ "path": second }));
    assert_eq!(host.request("sample.stopPreview", json!({})).unwrap(), json!({ "playing": false }));
    assert!(host.request("sample.preview", json!({})).unwrap_err().contains("Choose a sample"));
    assert!(host.request("sample.preview", json!({ "path": "/no/such/preview.wav" })).is_err());
    let status = host.request("sample.previewStatus", json!({})).unwrap();
    assert_eq!((status["voices"].clone(), status["leakedVoices"].clone()), (json!(0), json!(0)));
}

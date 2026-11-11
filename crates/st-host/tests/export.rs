//! Export, bounce and render selection as the interface uses them, on the null device.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_host::areas::audio::{AudioOptions, AudioOutput};
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};

struct Fixture {
    folder: TempDir,
    host: TestHost,
    #[allow(dead_code)]
    services: Host,
}

fn start(folder: TempDir) -> Fixture {
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(folder.path().join("support"), Arc::new(NoDialogs));
        options.autosave_every = None;
        options.audio = Some(AudioOptions { output: AudioOutput::Null });
        options.plugins = None;
        services = Some(install(bridge, options).expect("install the host"));
    });
    host.request("ready", json!({})).expect("the handshake");
    Fixture { folder, host, services: services.unwrap() }
}

/// A request that renders: it may take longer than the usual ten seconds on a busy machine.
fn long_request(host: &TestHost, command: &str, payload: Value) -> Result<Value, String> {
    let id = host.send(command, payload);
    host.reply_within(id, Duration::from_secs(120))
}

fn demo() -> Value {
    serde_json::from_str(include_str!("../../st-audio/tests/fixtures/demo_project.json")).unwrap()
}

fn read(path: &str) -> st_audio::SamplePcm {
    st_audio::samples::read_wav(&std::fs::read(path).unwrap()).unwrap()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn the_song_is_exported_with_progress_while_the_host_keeps_playing() {
    let fixture = start(TempDir::new("export-song"));
    let host = &fixture.host;
    host.request("engine.project", json!({ "project": demo() })).unwrap();
    host.request("transport.play", json!({ "mode": "song", "fromStart": true })).unwrap();
    let path = fixture.folder.path().join("song.wav").to_string_lossy().into_owned();
    let result = long_request(host, "audio.export", json!({ "mode": "song", "path": path, "tail": 1, "encoding": "pcm24", "sampleRate": 48000 })).unwrap();
    assert_eq!((result["path"].clone(), result["paths"].clone(), result["container"].clone(), result["encoding"].clone(), result["sampleRate"].clone()), (json!(path), json!([path]), json!("wav"), json!("pcm24"), json!(48_000)));
    let file = read(&path);
    assert_eq!(file.frames() as u64, result["frames"].as_u64().unwrap());
    // Four patterns of 64 rows at 116 BPM, four lines per beat, plus a second of tail.
    let sequence = demo()["sequence"].as_array().unwrap().len() as f64;
    let seconds = sequence * 64.0 * 60.0 / 116.0 / 4.0 + 1.0;
    assert!((file.frames() as f64 / 48_000.0 - seconds).abs() < 0.01, "{} s, expected {seconds}", file.frames() as f64 / 48_000.0);
    assert!(peak(&file.left) > 0.05 && peak(&file.right) > 0.05, "the export is not silent");

    let events = host.events();
    let progress: Vec<f64> = events.iter().filter(|(name, payload)| name == "audio.exportProgress" && payload["path"] == json!(path)).map(|(_, payload)| payload["progress"].as_f64().unwrap()).collect();
    assert!(progress.len() > 3 && progress.windows(2).all(|pair| pair[0] <= pair[1]) && *progress.last().unwrap() == 1.0, "{progress:?}");
    assert!(events.iter().any(|(name, payload)| name == "audio.exported" && payload["path"] == json!(path)));
    assert!(events.iter().any(|(name, payload)| name == "status" && payload["exporting"] == true), "the status reports the export");
    // Live playback was never paused: it is still playing, further along.
    let status = host.request("transport.seek", json!({ "pattern": 0, "row": 0 })).unwrap();
    assert_eq!((status["playing"].clone(), status["exporting"].clone()), (json!(true), json!(false)));
    assert!(host.request("audio.export", json!({ "mode": "song" })).unwrap_err().contains("destination"));
}

#[test]
fn stop_cancels_an_export_and_leaves_no_file() {
    let fixture = start(TempDir::new("export-cancel"));
    let host = &fixture.host;
    host.request("engine.project", json!({ "project": demo() })).unwrap();
    let path = fixture.folder.path().join("cancelled.wav").to_string_lossy().into_owned();
    let export = host.send("audio.export", json!({ "mode": "song", "path": path, "tail": 30 }));
    // A second export is refused while the first runs.
    host.wait_for_event("audio.exportProgress", Duration::from_secs(20)).expect("the export renders");
    assert!(host.request("audio.export", json!({ "mode": "song", "path": path })).unwrap_err().contains("current export"));
    host.request("transport.stop", json!({})).unwrap();
    let error = host.reply_within(export, Duration::from_secs(60)).unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!std::path::Path::new(&path).exists());
    let left: Vec<String> = std::fs::read_dir(fixture.folder.path()).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left, ["support"], "no file and no staging folder remain");
    // The next export is not affected by the earlier Stop.
    let again = long_request(host, "audio.export", json!({ "mode": "pattern", "path": path, "tail": 0 })).unwrap();
    assert!(std::path::Path::new(again["path"].as_str().unwrap()).is_file());
}

#[test]
fn stems_a_bounce_and_a_selection_render() {
    let fixture = start(TempDir::new("export-more"));
    let host = &fixture.host;
    host.request("engine.project", json!({ "project": demo() })).unwrap();
    let stems = fixture.folder.path().join("stems").to_string_lossy().into_owned();
    let result = long_request(host, "audio.export", json!({ "mode": "stems", "path": stems, "tail": 0.5, "encoding": "float32" })).unwrap();
    let paths = result["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 8, "one stem per track");
    assert!(paths.iter().all(|path| path.as_str().unwrap().starts_with(&stems) && std::path::Path::new(path.as_str().unwrap()).is_file()));
    assert_eq!((result["stems"][0]["owner"].clone(), result["stems"][0]["boundary"].clone()), (json!("track:#0"), json!("postFaderPreParent")));

    // A bounce goes to the support folder's Renders and comes back as a clip for the timeline.
    let bounce = long_request(host, "audio.bounceTrack", json!({ "track": 3, "tail": 0.5 })).unwrap();
    let path = bounce["path"].as_str().unwrap();
    assert!(path.starts_with(&*fixture.folder.path().join("support").join("Renders").to_string_lossy()) && path.contains("Track-4-"), "{path}");
    let file = read(path);
    assert!(peak(&file.left) > 0.01);
    let duration = bounce["duration"].as_f64().unwrap();
    assert!((duration - file.frames() as f64 / 48_000.0).abs() < 1e-9);
    let clip = &bounce["clip"];
    assert_eq!((clip["path"].clone(), clip["track"].clone(), clip["startBeat"].clone(), bounce["sourceTrackIndex"].clone()), (json!(path), json!(3), json!(0), json!(3)));
    assert!((clip["durationBeats"].as_f64().unwrap() - duration * 116.0 / 60.0).abs() < 1e-6);
    assert_eq!(bounce["sourceTrack"]["name"], demo()["tracks"][3]["name"]);
    assert!(host.request("audio.bounceTrack", json!({ "track": 40 })).unwrap_err().contains("Choose a track"));

    // Rows 0..=15 of the bass track of pattern 0, as a sample for an instrument.
    let track = (0..8).find(|track| demo()["patterns"][0]["steps"][*track].as_array().is_some_and(|rows| rows.iter().take(16).any(|step| step["note"].as_f64().is_some_and(|note| note >= 0.0)))).expect("a track with notes");
    let started = Instant::now();
    let selection = long_request(host, "render.selection", json!({ "pattern": 0, "tracks": [track], "firstRow": 0, "lastRow": 15, "mode": "track", "tail": 2, "sampleRate": 44100, "channels": 1, "normalize": true, "name": "Bass riff" })).unwrap();
    let path = selection["path"].as_str().unwrap();
    assert!(path.starts_with(&*fixture.folder.path().join("support").join("Recordings").to_string_lossy()) && path.contains("Bass riff-"), "{path}");
    let file = read(path);
    assert_eq!((file.sample_rate, file.frames() as u64, selection["channels"].clone(), selection["rootNote"].clone()), (44_100.0, selection["frames"].as_u64().unwrap(), json!(1), json!(60)));
    // Sixteen rows at 116 BPM are 2.069 s; the tail ends at silence within two seconds.
    let musical = 16.0 * 60.0 / 116.0 / 4.0;
    assert!((selection["musicalFrames"].as_f64().unwrap() / 48_000.0 - musical).abs() < 0.001);
    let seconds = file.frames() as f64 / 44_100.0;
    assert!(seconds >= musical - 0.01 && seconds <= musical + 2.01, "{seconds} s");
    assert!((peak(&file.left) - 10f32.powf(-0.3 / 20.0)).abs() < 1e-3, "normalized to -0.3 dBFS: {}", peak(&file.left));
    assert!(["silence", "maximum"].contains(&selection["tailEnd"].as_str().unwrap()));
    let progress: Vec<Value> = host.events().into_iter().filter(|(name, _)| name == "render.selectionProgress").map(|(_, payload)| payload).collect();
    assert!(progress.first().unwrap()["phase"] == "capturing" && progress.last().unwrap()["phase"] == "done" && progress.last().unwrap()["progress"] == 1.0);
    assert!(started.elapsed() < Duration::from_secs(60));
    // The page then loads the sample into a new instrument.
    let loaded = host.request("sample.load", json!({ "path": path, "instrument": 9 })).unwrap();
    assert_eq!(loaded["frames"], selection["frames"]);
    assert_eq!(host.request("render.cancelSelection", json!({})).unwrap(), json!({ "cancelled": false }));
    assert!(host.request("render.selection", json!({ "pattern": 0, "tracks": [track], "firstRow": 0, "lastRow": 200 })).unwrap_err().contains("past the end"));
    assert!(host.request("render.selection", json!({ "pattern": 77, "tracks": [track], "firstRow": 0, "lastRow": 3 })).unwrap_err().contains("no longer exists"));
}

#[test]
fn a_track_is_frozen_reported_and_unfrozen() {
    let fixture = start(TempDir::new("export-freeze"));
    let host = &fixture.host;
    let mut project = demo();
    project["sequence"] = json!([0]);
    project["audioPlugins"] = json!([{ "owner": "track:#1", "kind": "effect", "slot": 0, "instanceID": "comp", "component": "stfx:compressor", "name": "Compressor" }]);
    host.request("engine.project", json!({ "project": project.clone() })).unwrap();
    assert_eq!(host.request("freeze.status", json!({})).unwrap(), json!({ "tracks": [], "busy": false }));
    let result = long_request(host, "freeze.track", json!({ "track": 1, "maxTail": 2 })).unwrap();
    let record = result["record"].clone();
    assert_eq!((record["version"].clone(), record["tailMode"].clone(), record["dormantPluginIDs"].clone(), result["releasedPlugins"].clone()), (json!(1), json!("auto"), json!(["comp"]), json!(1)));
    let print = record["path"].as_str().unwrap();
    assert!(print.starts_with(&*fixture.folder.path().join("support").join("Freezes").to_string_lossy()) && std::path::Path::new(print).is_file(), "{print}");
    assert!(result["audioPlugins"].as_array().is_some_and(|plugins| plugins.len() == 1 && plugins[0]["state"].is_string()), "the plug-in state captured for the print comes back");
    let progress: Vec<Value> = host.events().into_iter().filter(|(name, _)| name == "freeze.progress").map(|(_, payload)| payload).collect();
    assert!(!progress.is_empty() && progress.iter().all(|event| event["track"] == 1 && event["recording"] == false) && progress.last().unwrap()["progress"] == 1.0);

    // The page stores the record and publishes; the engine plays the print without the insert.
    project["tracks"][1]["frozen"] = record.clone();
    project["audioPlugins"] = result["audioPlugins"].clone();
    let status = host.request("engine.project", json!({ "project": project.clone() })).unwrap();
    assert_eq!(status["routing"]["inserts"], json!([]), "the frozen track's insert is unloaded");
    let rows = host.request("freeze.status", json!({})).unwrap();
    assert_eq!((rows["tracks"][0]["track"].clone(), rows["tracks"][0]["id"].clone(), rows["tracks"][0]["available"].clone(), rows["tracks"][0]["stale"].clone()), (json!(1), record["id"].clone(), json!(true), json!(false)));
    host.request("transport.play", json!({ "mode": "song", "fromStart": true })).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut heard = false;
    while !heard && Instant::now() < deadline {
        heard = host.events().iter().any(|(name, payload)| name == "status" && payload["trackPeaks"][1].as_f64().unwrap_or(0.0) > 0.01);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(heard, "the frozen track is heard from its print");
    host.request("transport.stop", json!({})).unwrap();
    // An edit of the printed notes makes it stale.
    let mut edited = project.clone();
    edited["patterns"][0]["steps"][1][0] = json!({ "note": 72, "instrument": 1, "velocity": 100 });
    host.request("engine.project", json!({ "project": edited })).unwrap();
    assert_eq!(host.request("freeze.status", json!({})).unwrap()["tracks"][0]["stale"], true);
    assert!(long_request(host, "freeze.track", json!({ "track": 1 })).unwrap_err().contains("already frozen"));
    // Freeze by recording is in tests/line_inputs.rs; a method the host does not know is refused.
    assert!(host.request("freeze.track", json!({ "track": 0, "method": "resample" })).unwrap_err().contains("offline or record"));

    assert_eq!(host.request("freeze.unfreeze", json!({ "track": 1 })).unwrap()["track"], 1);
    assert!(host.request("freeze.unfreeze", json!({ "track": 0 })).unwrap_err().contains("not frozen"));
    project["tracks"][1].as_object_mut().unwrap().remove("frozen");
    let status = host.request("engine.project", json!({ "project": project })).unwrap();
    assert_eq!(status["routing"]["inserts"].as_array().unwrap().len(), 1, "the live chain is back");
}

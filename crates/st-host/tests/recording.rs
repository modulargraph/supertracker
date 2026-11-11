//! Recording as the interface uses it, on the null device and the null input.

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

fn demo() -> Value {
    serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).unwrap()
}

fn status_where(host: &TestHost, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(status) = host.events().into_iter().filter(|(name, _)| name == "status").map(|(_, payload)| payload).find(|status| wanted(status)) {
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no status where {what}");
}

#[test]
fn the_master_output_is_recorded_to_a_new_file() {
    let fixture = start(TempDir::new("record-master"));
    let host = &fixture.host;
    host.request("engine.project", json!({ "project": demo() })).unwrap();
    assert_eq!(host.request("audio.stopRecord", json!({})).unwrap(), json!({ "recording": false }));
    let started = host.request("audio.record", json!({ "source": "master", "name": "Jam / take: 1" })).unwrap();
    let path = started["path"].as_str().unwrap().to_string();
    assert_eq!((started["recording"].clone(), started["channels"].clone()), (json!(true), json!(2)));
    assert!(path.starts_with(&*fixture.folder.path().join("support").join("Recordings").to_string_lossy()) && path.contains("Jam  take 1-") && path.ends_with(".wav"), "{path}");
    assert!(host.request("audio.record", json!({ "source": "master" })).unwrap_err().contains("already running"));
    assert!(host.request("audio.configure", json!({ "bufferSize": 256 })).unwrap_err().contains("Finish recording"));
    host.request("transport.play", json!({ "mode": "song", "fromStart": true })).unwrap();
    let status = status_where(host, "the take runs and is heard", |status| status["recording"] == true && status["recordingDuration"].as_f64().unwrap_or(0.0) > 0.6 && status["peakL"].as_f64().unwrap_or(0.0) > 0.01);
    assert_eq!(status["recordingStopping"], false);
    let stopped = host.request("audio.stopRecord", json!({})).unwrap();
    assert_eq!((stopped["recording"].clone(), stopped["path"].clone(), stopped["channels"].clone(), stopped["partial"].clone(), stopped["source"].clone()), (json!(false), json!(path), json!(2), json!(false), json!("master")));
    let file = st_audio::samples::read_wav(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((file.frames() as u64, file.sample_rate), (stopped["frames"].as_u64().unwrap(), 48_000.0));
    assert!((stopped["duration"].as_f64().unwrap() - file.frames() as f64 / 48_000.0).abs() < 1e-9 && stopped["duration"].as_f64().unwrap() > 0.6);
    let (start, end): (i64, i64) = (stopped["sourceStartFrame"].as_str().unwrap().parse().unwrap(), stopped["sourceEndFrame"].as_str().unwrap().parse().unwrap());
    assert_eq!(end - start, file.frames() as i64, "the file holds exactly the frames between Record and Stop on the output's clock");
    assert!(file.left.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs())) > 0.01, "the song is in the take");
    let events = host.events();
    let recording: Vec<&Value> = events.iter().filter(|(name, _)| name == "audio.recording").map(|(_, payload)| payload).collect();
    assert!(recording.iter().any(|event| event["recording"] == false && event["path"] == json!(path)));
    status_where(host, "the status is back to idle", |status| status["recording"] == false);
    // The page loads the take like any other sample.
    assert_eq!(host.request("sample.load", json!({ "path": path, "instrument": 2 })).unwrap()["frames"], stopped["frames"]);
}

#[test]
fn the_input_is_chosen_metered_and_recorded() {
    let fixture = start(TempDir::new("record-input"));
    let host = &fixture.host;
    let devices = host.request("audio.devices", json!({})).unwrap();
    // On the null output the null input is offered and is the default.
    assert!(devices["inputs"].as_array().unwrap().iter().any(|input| input["uid"] == "null" && input["id"] == 0 && input["inputChannels"] == 2), "{}", devices["inputs"]);
    assert_eq!((devices["inputChannels"].clone(), devices["inputMetering"].clone(), devices["monitoring"].clone(), devices["inputRunning"].clone()), (json!([0, 1]), json!(false), json!(false), json!(false)));
    // The test input (a 440 Hz sine on channel 0), metered.
    let configured = host.request("audio.configure", json!({ "input": "null:tone", "inputChannels": [0], "inputMeter": true, "monitor": true, "monitorVolume": 0.4 })).unwrap();
    assert_eq!((configured["inputUid"].clone(), configured["inputChannels"].clone(), configured["inputMetering"].clone(), configured["monitoring"].clone(), configured["monitorVolume"].as_f64().map(|volume| (volume * 10.0).round())), (json!("null:tone"), json!([0]), json!(true), json!(true), Some(4.0)));
    assert_eq!(configured["inputRunning"], true);
    let status = status_where(host, "the input meter moves", |status| status["inputMetering"] == true && status["inputPeakLeft"].as_f64().unwrap_or(0.0) > 0.2);
    assert!((status["inputPeakLeft"].as_f64().unwrap() - 0.25).abs() < 0.01);

    let path = fixture.folder.path().join("take.wav").to_string_lossy().into_owned();
    let started = host.request("audio.record", json!({ "source": "input", "path": path, "instrument": 3, "owner": "page-7", "monitor": false })).unwrap();
    assert_eq!((started["path"].clone(), started["channels"].clone()), (json!(path), json!(1)), "one chosen channel is a mono take");
    std::thread::sleep(Duration::from_millis(400));
    let stopped = host.request("audio.stopRecord", json!({})).unwrap();
    assert_eq!((stopped["source"].clone(), stopped["instrument"].clone(), stopped["owner"].clone(), stopped["partial"].clone(), stopped["channels"].clone()), (json!("input"), json!(3), json!("page-7"), json!(false), json!(1)));
    // The null devices have no latency of their own. What is left out of the take's start is
    // the block the input waits on its way to the render thread, which records it.
    assert_eq!((stopped["latencyCompensation"]["frames"].clone(), stopped["sampleRate"].clone()), (json!(512), json!(48_000)));
    let (start, end): (i64, i64) = (stopped["sourceStartFrame"].as_str().unwrap().parse().unwrap(), stopped["sourceEndFrame"].as_str().unwrap().parse().unwrap());
    assert_eq!(end - start, stopped["frames"].as_i64().unwrap(), "an input take is on the output's clock too: one frame per frame rendered");
    let file = st_audio::samples::read_wav(&std::fs::read(&path).unwrap()).unwrap();
    assert!(file.frames() > 9_600 && file.frames() < 48_000, "{} frames", file.frames());
    let peak = file.left.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    let crossings = file.left[..4_800].windows(2).filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0)).count();
    assert!((peak - 0.25).abs() < 0.01 && (86..=90).contains(&crossings), "440 Hz at a quarter of full scale: peak {peak}, {crossings} crossings");
    // An existing file is never recorded over; an unknown input is refused.
    assert!(host.request("audio.record", json!({ "source": "input", "path": path })).is_err());
    assert!(host.request("audio.configure", json!({ "inputDevice": 987_654 })).unwrap_err().contains("not connected"));
    let off = host.request("audio.configure", json!({ "inputMeter": false, "monitor": false })).unwrap();
    assert_eq!((off["inputRunning"].clone(), off["inputMetering"].clone()), (json!(false), json!(false)));
}

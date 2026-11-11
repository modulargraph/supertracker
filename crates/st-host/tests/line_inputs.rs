//! Live line inputs and Freeze by recording as the interface uses them, on the null device with
//! the null input's test tone (440 Hz on channel 0, 880 Hz on channel 1, a quarter of full
//! scale). The null output is the clock of the null input, so the comparisons are exact. One
//! test runs the tone from a second clock at another rate, through the converter.

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
        options.plugins = None;
        services = Some(install(bridge, options).expect("install the host"));
    });
    host.request("ready", json!({})).expect("the handshake");
    Fixture { folder, host, services: services.unwrap() }
}

/// The demo project's tracks and instruments with a song of one second: one pattern of eight
/// rows at 240 BPM, twice, in which only the first track (a saw bass) plays.
fn project() -> Value {
    let mut project: Value = serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).unwrap();
    let tracks = project["tracks"].as_array().unwrap().len();
    let mut steps = vec![json!(vec![Value::Null; 8]); tracks];
    steps[0][0] = json!({ "note": 48, "instrument": 0, "velocity": 110, "gate": 2 });
    steps[0][5] = json!({ "note": 55, "instrument": 0, "velocity": 110, "gate": 2 });
    project["bpm"] = json!(240);
    project["patterns"] = json!([{ "id": 0, "length": 8, "steps": steps, "automation": [] }]);
    project["sequence"] = json!([0, 0]);
    project["activePattern"] = json!(0);
    project["loopSong"] = json!(true);
    for (index, track) in project["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
        track["routingID"] = json!(format!("t{index}"));
    }
    project
}

fn tone(channels: &[u32], gain: f64) -> Value {
    json!({ "enabled": true, "deviceUID": "null:tone", "channels": channels, "gain": gain })
}

fn status_where(host: &TestHost, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(status) = host.events().into_iter().filter(|(name, _)| name == "status").map(|(_, payload)| payload).find(|status| wanted(status)) {
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no status where {what}");
}

/// Publishes `project` and returns the `audio.devices` event the publication sent, if it sent one.
fn publish(host: &TestHost, project: &Value) -> Option<Value> {
    host.events();
    host.request("engine.project", json!({ "project": project })).unwrap();
    host.events().into_iter().filter(|(name, _)| name == "audio.devices").map(|(_, payload)| payload).next_back()
}

fn long_request(host: &TestHost, command: &str, payload: Value) -> Result<Value, String> {
    let id = host.send(command, payload);
    host.reply_within(id, Duration::from_secs(180))
}

fn read(path: &Value) -> st_audio::SamplePcm {
    st_audio::samples::read_wav(&std::fs::read(path.as_str().unwrap()).unwrap()).unwrap()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()))
}

/// The amplitude of the 440 Hz sine in `signal` (measured over a whole number of its cycles at
/// the start) and the largest difference between `signal` and that sine anywhere.
fn sine_fit(signal: &[f32]) -> (f64, f64) {
    let omega = 440.0 * std::f64::consts::TAU / 48_000.0;
    let (mut i, mut q) = (0.0f64, 0.0f64);
    for (frame, sample) in signal[..4_800].iter().enumerate() {
        i += *sample as f64 * (omega * frame as f64).sin();
        q += *sample as f64 * (omega * frame as f64).cos();
    }
    let (amplitude, phase) = ((i * i + q * q).sqrt() / 2_400.0, q.atan2(i));
    let worst = signal.iter().enumerate().map(|(frame, sample)| (*sample as f64 - amplitude * (omega * frame as f64 + phase).sin()).abs()).fold(0.0f64, f64::max);
    (amplitude, worst)
}

#[test]
fn the_project_routes_a_line_input_into_its_track_and_the_host_reports_it() {
    let fixture = start(TempDir::new("line-inputs"));
    let host = &fixture.host;
    let devices = host.request("audio.devices", json!({})).unwrap();
    assert_eq!((devices["lineInputs"].clone(), devices["lineInputError"].clone()), (json!([]), Value::Null));
    // The interface chooses the input (Settings); nothing is opened for that alone.
    let chosen = host.request("audio.configure", json!({ "input": "null:tone" })).unwrap();
    assert_eq!((chosen["inputUid"].clone(), chosen["inputRunning"].clone()), (json!("null:tone"), json!(false)));
    let listed = chosen["inputs"].as_array().unwrap().iter().find(|input| input["uid"] == "null:tone").expect("the chosen test input is listed");
    assert_eq!((listed["id"].clone(), listed["inputChannels"].clone()), (chosen["inputDevice"].clone(), json!(2)));

    // The project is the routing: track 0 takes channel 0 in at 0.8, and says so by itself.
    let mut project = project();
    project["patterns"][0]["steps"][0] = json!(vec![Value::Null; 8]);
    project["tracks"][0]["volume"] = json!(1);
    project["tracks"][0]["audioInput"] = tone(&[0], 0.8);
    let event = publish(host, &project).expect("the publication reports its line input");
    let row = &event["lineInputs"][0];
    assert_eq!((row["track"].clone(), row["state"].clone(), row["active"].clone(), row["monitoring"].clone(), row["latencyFrames"].clone()), (json!(0), json!("live"), json!(true), json!(true), json!(512)));
    assert_eq!((event["lineInputError"].clone(), event["inputRunning"].clone(), event["lineInputs"].as_array().unwrap().len()), (Value::Null, json!(true), 1));
    assert_eq!(host.request("audio.devices", json!({})).unwrap()["lineInputs"][0]["state"], "live");
    // It is heard on the track's meter and on the master, without Play: 0.25 x 0.8.
    let status = status_where(host, "the input shows on its track", |status| status["trackPeaks"][0].as_f64().unwrap_or(0.0) > 0.15);
    assert!((status["trackPeaks"][0].as_f64().unwrap() - 0.2).abs() < 0.01 && status["trackPeaks"][1] == 0.0 && status["playing"] == false, "{status}");
    assert!(status["peakL"].as_f64().unwrap() > 0.05 && status["latencyCompensation"]["frames"] == 512);
    assert!(publish(host, &project).is_none(), "a publication that changes no line input sends no event");

    // Monitoring off: the route stays, the strip is silent.
    project["tracks"][0]["audioInput"]["monitor"] = json!(false);
    let event = publish(host, &project).expect("an event");
    assert_eq!((event["lineInputs"][0]["state"].clone(), event["lineInputs"][0]["monitoring"].clone()), (json!("live"), json!(false)));
    status_where(host, "the track is silent", |status| status["trackPeaks"][0] == 0.0);
    // A stereo route on a second track; one saved for an interface that is not the input.
    project["tracks"][1]["audioInput"] = tone(&[0, 1], 1.0);
    project["tracks"][2]["audioInput"] = json!({ "enabled": true, "deviceUID": "Interface 18i20", "channels": [4, 5], "gain": 1 });
    let event = publish(host, &project).expect("an event");
    let rows = event["lineInputs"].as_array().unwrap();
    assert_eq!(rows.iter().map(|row| row["state"].as_str().unwrap()).collect::<Vec<_>>(), ["live", "live", "paused"]);
    assert_eq!(rows[2]["status"], "Interface 18i20 is not the audio input in use. Its input is paused until it is.");
    assert_eq!(event["lineInputError"], rows[2]["status"]);
    status_where(host, "the second track carries the tone", |status| (status["trackPeaks"][1].as_f64().unwrap_or(0.0) - 0.25 * 0.8).abs() < 0.01);
    // A project whose binding makes no sense is refused like any invalid project.
    let mut invalid = project.clone();
    invalid["tracks"][1]["audioInput"]["channels"] = json!([1, 1]);
    assert!(host.request("engine.project", json!({ "project": invalid })).unwrap_err().contains("one input channel (mono) or two different ones"));
    // Routes switched off: the input closes.
    for track in 0..3 {
        project["tracks"][track]["audioInput"]["enabled"] = json!(false);
    }
    let event = publish(host, &project).expect("an event");
    assert!(event["lineInputs"].as_array().unwrap().iter().all(|row| row["state"] == "off" && row["active"] == false));
    assert_eq!((event["inputRunning"].clone(), event["lineInputError"].clone()), (json!(false), Value::Null));
    // There is no command that routes inputs: the host names the ones it does not have.
    assert!(host.request("audio.inputs", json!({ "inputs": [] })).unwrap_err().contains("audio.inputs"));
    assert!(host.request("audio.inputs.configure", json!({})).unwrap_err().contains("audio.inputs.configure"));

    // The chosen input is remembered: a host started later on the same support folder runs
    // the saved routes without being told the device again.
    assert_eq!(fixture.services.settings.get_raw("audio.devices").unwrap()["input"], "null:tone");
    let Fixture { folder, host, services } = fixture;
    drop(host);
    drop(services);
    let again = start(folder);
    project["tracks"][0]["audioInput"] = tone(&[1], 1.0);
    let event = publish(&again.host, &project).expect("an event");
    assert_eq!((event["inputUid"].clone(), event["lineInputs"][0]["state"].clone()), (json!("null:tone"), json!("live")));
    status_where(&again.host, "the input is heard again", |status| (status["trackPeaks"][0].as_f64().unwrap_or(0.0) - 0.25).abs() < 0.01);
    let _ = &again.folder;
}

#[test]
fn an_input_with_another_rate_and_clock_is_resampled_into_its_track_and_into_a_take() {
    // A second device in real time: the free-running null input delivers the test tone at
    // 44.1 kHz from a timer thread of its own, under the null output at 48 kHz. What is exact
    // about two clocks is measured in simulated time (st-audio, tests/two_clocks.rs); here the
    // threads are real and the machine may be busy, so this test only asks for what holds
    // then too: the route runs through the converter and says so, and the tone keeps its
    // level and its pitch on the track and in a take.
    let fixture = start(TempDir::new("two-clocks"));
    let host = &fixture.host;
    let uid = "null:free:44100";
    let chosen = host.request("audio.configure", json!({ "input": uid, "inputChannels": [0] })).unwrap();
    assert_eq!((chosen["inputUid"].clone(), chosen["inputRunning"].clone()), (json!(uid), json!(false)));
    let mut project = project();
    project["patterns"][0]["steps"][0] = json!(vec![Value::Null; 8]);
    project["tracks"][0]["volume"] = json!(1);
    project["tracks"][0]["audioInput"] = json!({ "enabled": true, "deviceUID": uid, "channels": [0], "gain": 0.8 });
    let event = publish(host, &project).expect("the publication reports its line input");
    let row = &event["lineInputs"][0];
    // 512 output frames are 470.4 of the input's, its block is 470, the filter reads 46
    // ahead and the margin is 5 ms: 1 207 input frames, 1 314 of the engine's.
    assert_eq!((row["state"].clone(), row["clock"].clone(), row["latencyFrames"].clone()), (json!("live"), json!("converted"), json!(1_314)));
    assert!(row["driftPpm"].is_number() && row["underruns"].is_u64() && row["overruns"].is_u64(), "{row}");
    assert_eq!((event["inputRunning"].clone(), event["inputSampleRate"].clone(), event["sampleRate"].clone()), (json!(true), json!(44_100.0), json!(48_000.0)));
    // The tone on the track's meter at its level, 0.25 x 0.8, and the delay in the mixer.
    let status = status_where(host, "the input shows on its track", |status| (status["trackPeaks"][0].as_f64().unwrap_or(0.0) - 0.2).abs() < 0.01);
    assert_eq!(status["latencyCompensation"]["frames"], 1_314);

    // A take of the input is at the engine's rate, and the 440 Hz tone is at 440 Hz in it:
    // 44.1 kHz frames taken for 48 kHz ones would sound at 479 Hz.
    let path = fixture.folder.path().join("take.wav").to_string_lossy().into_owned();
    host.request("audio.record", json!({ "source": "input", "path": path })).unwrap();
    std::thread::sleep(Duration::from_millis(1_500));
    let stopped = host.request("audio.stopRecord", json!({})).unwrap();
    assert_eq!((stopped["sampleRate"].clone(), stopped["channels"].clone(), stopped["latencyCompensation"]["frames"].clone()), (json!(48_000), json!(1), json!(1_314)));
    let file = read(&stopped["path"]);
    assert!(file.sample_rate == 48_000.0 && file.frames() > 48_000, "{} frames at {} Hz", file.frames(), file.sample_rate);
    // Half a second from the middle of the take: 220 cycles, 440 zero crossings (a busy
    // machine may cost a few; unconverted there would be 479).
    let middle = &file.left[file.frames() / 2 - 12_000..file.frames() / 2 + 12_000];
    let crossings = middle.windows(2).filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0)).count();
    assert!((425..=455).contains(&crossings) && (peak(middle) - 0.25).abs() < 0.01, "{crossings} zero crossings, peak {}", peak(middle));
    // The line input's own counters are in every report.
    let row = host.request("audio.devices", json!({})).unwrap()["lineInputs"][0].clone();
    assert!(row["state"] == "live" && row["driftPpm"].as_f64().unwrap().abs() < 2_000.0, "{row}");
}

#[test]
fn a_hardware_track_is_frozen_by_recording_and_plays_its_print() {
    let fixture = start(TempDir::new("freeze-recording"));
    let host = &fixture.host;
    host.request("audio.configure", json!({ "input": "null:tone" })).unwrap();
    let mut project = project();
    // The reference: the track's notes printed offline.
    publish(host, &project);
    let offline = long_request(host, "freeze.track", json!({ "track": 0, "tail": 0.25 })).unwrap();
    assert_eq!((offline["record"]["method"].clone(), offline["record"]["frames"].clone()), (json!("offline"), json!(60_000)));
    assert!(offline["record"]["printGain"].is_null(), "a print is at unity: there is no gain to record");

    // With the tone on its line input (0.25 x 0.8 on both sides) the track cannot be rendered.
    project["tracks"][0]["audioInput"] = tone(&[0], 0.8);
    assert_eq!(publish(host, &project).expect("an event")["lineInputs"][0]["state"], "live");
    assert!(long_request(host, "freeze.track", json!({ "track": 0, "tail": 0.25 })).unwrap_err().contains("Choose Freeze by recording"));
    assert!(long_request(host, "audio.export", json!({ "path": fixture.folder.path().join("song.wav"), "encoding": "float32" })).unwrap_err().contains("line inputs"));
    assert!(host.request("freeze.track", json!({ "track": 0, "method": "bounce" })).unwrap_err().contains("offline or record"));

    // Stop cancels a freeze by recording; nothing is left behind.
    let freezes = fixture.folder.path().join("support").join("Freezes");
    let prints = || std::fs::read_dir(&freezes).map_or(0, |files| files.count());
    let before = prints();
    host.events();
    let cancelled = host.send("freeze.track", json!({ "track": 0, "tail": 0.25, "method": "record" }));
    status_where(host, "the take plays", |status| status["playing"] == true);
    host.request("transport.stop", json!({})).unwrap();
    assert_eq!(host.reply_within(cancelled, Duration::from_secs(60)).unwrap_err(), "Freeze was cancelled. Nothing was changed.");
    assert_eq!(prints(), before);
    assert_eq!(host.request("audio.devices", json!({})).unwrap()["lineInputs"][0]["state"], "live");

    // The freeze: the song once in real time, then each pattern's head and loop.
    host.events();
    let recorded = long_request(host, "freeze.track", json!({ "track": 0, "tail": 0.25, "method": "record", "patterns": true, "monitor": false })).unwrap();
    let record = recorded["record"].clone();
    assert_eq!((record["method"].clone(), record["tailMode"].clone(), record["sampleRate"].clone(), record["frames"].clone(), record["musicalFrames"].clone()), (json!("recorded"), json!("fixed"), json!(48_000.0), json!(60_000), json!(48_000)));
    assert_eq!((recorded["track"].clone(), recorded["unprintedPatterns"].clone(), recorded["notes"].clone(), record["patternPrints"].as_array().unwrap().len()), (json!(0), json!([]), json!([]), 1));
    let progress: Vec<Value> = host.events().into_iter().filter(|(name, _)| name == "freeze.progress").map(|(_, payload)| payload).collect();
    assert!(progress.iter().all(|event| event["track"] == 0 && event["recording"] == true) && progress.last().unwrap()["progress"] == 1.0, "{progress:?}");
    assert!(progress.iter().any(|event| event["phase"] == "song") && progress.iter().any(|event| event["phase"] == "patterns"));
    // What was played is in the print, where it was played: beyond the offline print of the
    // notes there is the tone, one unbroken sine from the first frame to the last. A print
    // that held the notes one frame off would not leave a sine.
    let compare = |recorded: &Value, offline: &Value, what: &str| {
        let (recorded, offline) = (read(recorded), read(offline));
        assert_eq!(recorded.frames(), offline.frames(), "{what}");
        assert!(peak(&offline.left) > 0.05, "{what}: the notes are in the reference");
        for (side, recorded, offline) in [("left", &recorded.left, &offline.left), ("right", &recorded.right, &offline.right)] {
            let difference: Vec<f32> = recorded.iter().zip(offline.iter()).map(|(recorded, offline)| recorded - offline).collect();
            let (amplitude, worst) = sine_fit(&difference);
            assert!((amplitude - 0.2).abs() < 1e-4 && worst < 1e-4, "{what}, {side}: a sine of amplitude {amplitude}, off by at most {worst}");
        }
    };
    compare(&record["path"], &offline["record"]["path"], "the song");
    let (print, reference) = (&record["patternPrints"][0], &offline["record"]["patternPrints"][0]);
    assert_eq!((print["pattern"].clone(), print["passFrames"].clone(), print["passes"].clone()), (json!(0), json!(24_000), reference["passes"].clone()));
    compare(&print["path"], &reference["path"], "the pattern's head");
    compare(&print["loop"]["path"], &reference["loop"]["path"], "the pattern's loop");
    // The mix was not heard during the takes, and is back as the page left it.
    assert_eq!(host.request("engine.snapshot", json!({})).unwrap()["masterVolume"], project["masterVolume"]);
    assert_eq!(host.request("engine.snapshot", json!({})).unwrap()["loopSong"], true);

    // The page stores the record and publishes: the input is closed, the print plays, and the
    // song can be exported.
    project["tracks"][0]["frozen"] = record.clone();
    project["audioPlugins"] = recorded["audioPlugins"].clone();
    let event = publish(host, &project).expect("an event");
    assert_eq!((event["lineInputs"][0]["state"].clone(), event["inputRunning"].clone()), (json!("paused"), json!(false)));
    assert!(event["lineInputs"][0]["status"].as_str().unwrap().contains("frozen"));
    let rows = host.request("freeze.status", json!({})).unwrap();
    assert_eq!((rows["tracks"][0]["available"].clone(), rows["tracks"][0]["stale"].clone(), rows["tracks"][0]["patternPlayback"].clone()), (json!(true), json!(false), json!(true)));
    status_where(host, "the frozen track is silent while stopped", |status| status["trackPeaks"][0] == 0.0);
    host.request("transport.play", json!({ "mode": "song", "fromStart": true })).unwrap();
    status_where(host, "the print is heard", |status| status["trackPeaks"][0].as_f64().unwrap_or(0.0) > 0.1);
    host.request("transport.stop", json!({})).unwrap();
    let exported = long_request(host, "audio.export", json!({ "path": fixture.folder.path().join("song.wav"), "encoding": "float32", "tail": 0.1 })).unwrap();
    assert!(peak(&read(&exported["path"]).left) > 0.1, "the exported song holds the recorded track");

    // Unfrozen, the input is live again.
    assert_eq!(host.request("freeze.unfreeze", json!({ "track": 0 })).unwrap()["track"], 0);
    project["tracks"][0].as_object_mut().unwrap().remove("frozen");
    assert_eq!(publish(host, &project).expect("an event")["lineInputs"][0]["state"], "live");
}

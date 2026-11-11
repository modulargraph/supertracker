//! The demo song and live use of the engine: block-size determinism, incremental publication,
//! transport and status, live notes with acknowledgements, the null device, the snapshot.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::{AudioEngine, DeviceChoice, DeviceRequest, EngineOptions, InputEdge, PlayOptions};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn demo() -> Value {
    serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).expect("the demo project fixture")
}

fn play_song(engine: &AudioEngine) {
    engine.play(&PlayOptions { song: true, from_start: true, ..PlayOptions::default() });
}

#[test]
fn the_demo_song_renders_identically_at_every_block_size() {
    let frames = 3 * 48_000;
    let mut renders = Vec::new();
    for block in [64usize, 127, 512, 4096] {
        let engine = engine();
        let report = engine.publish(&parse(&demo())).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        play_song(&engine);
        renders.push(engine.render_offline(frames, block));
    }
    let (left, right) = &renders[0];
    assert!(peak(left) > 0.05 && peak(right) > 0.05, "the demo song is audible");
    assert!(left != right, "pans and the stereo returns make the channels differ");
    for (index, render) in renders.iter().enumerate().skip(1) {
        assert!(render.0 == *left && render.1 == *right, "block size #{index} differs from 64 frames");
    }
}

#[test]
fn a_project_with_inserts_sends_and_automation_is_deterministic_across_block_sizes() {
    let mut project = project(3);
    project["routing"]["buses"] = json!([bus("g", "group", 0.8), bus("r", "return", 0.7)]);
    project["routing"]["outputs"] = json!({ "track:a": "bus:g", "track:b": "bus:g" });
    project["routing"]["sends"] = json!([{ "id": "s", "source": "track:c", "tap": "preFader", "destination": "bus:r", "gain": 0.5 }]);
    project["audioPlugins"] = json!([
        effect("track:a", 0, "lat", "test:latency:300"), effect("bus:r", 0, "limit", "stfx:limiter"),
        effect("bus:g", 0, "comp", "stfx:compressor"), effect("master", 0, "gain", "test:gain"),
    ]);
    project["tracks"][2]["delaySend"] = json!(0.4);
    project["tracks"][1]["reverbSend"] = json!(0.3);
    project["patterns"][0]["automation"] = json!([
        { "id": "1", "owner": "bus:g", "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.2 }, { "row": 3, "value": 1.0 }] },
        { "id": "2", "owner": "master", "target": "plugin:effect:0:0", "interpolation": "linear", "points": [{ "row": 0, "value": 0.5 }, { "row": 4, "value": 0.3 }] },
        { "id": "3", "track": 2, "target": "pan", "interpolation": "linear", "points": [{ "row": 0, "value": -1.0 }, { "row": 4, "value": 1.0 }] },
    ]);
    let renders: Vec<_> = [64usize, 127, 512, 4096].into_iter().map(|block| render_with(&project, 30_000, block)).collect();
    assert!(peak(&renders[0].0) > 0.05);
    for render in &renders[1..] {
        assert!(render == &renders[0], "the render depends on the block size");
    }
}

#[test]
fn editing_a_note_re_sends_one_pattern_and_keeps_the_mixer() {
    let engine = engine();
    let mut project = demo();
    let first = engine.publish(&parse(&project)).unwrap();
    assert_eq!((first.patterns_sent, first.patterns_reused), (2, 0));
    assert!(first.samples_installed >= 5, "the demo's wavetables are installed once");
    play_song(&engine);
    engine.render_offline(4_800, 512);
    let serial = engine.status()["routing"]["strips"].clone();

    // Nothing changed: both patterns are reused and no sample is installed again.
    let again = engine.publish(&parse(&project)).unwrap();
    assert_eq!((again.patterns_sent, again.patterns_reused, again.samples_installed), (0, 2, 0));

    // A note edit in pattern 1 while playing: one pattern goes to the engine.
    project["patterns"][1]["steps"][0][3] = json!({ "note": 50, "instrument": 0, "velocity": 90, "gate": 0.5 });
    let edit = engine.publish(&parse(&project)).unwrap();
    assert_eq!((edit.patterns_sent, edit.patterns_reused, edit.samples_installed), (1, 1, 0));
    let (left, _) = engine.render_offline(48_000, 512);
    assert!(peak(&left) > 0.05, "playback continues through the edit");
    assert_eq!(engine.status()["playing"], true);
    assert_eq!(engine.status()["routing"]["strips"], serial);

    // A mixer change keeps every pattern; adding a track changes what patterns resolve against.
    project["tracks"][0]["volume"] = json!(0.5);
    let mix = engine.publish(&parse(&project)).unwrap();
    assert_eq!((mix.patterns_sent, mix.patterns_reused), (0, 2));
    project["tracks"].as_array_mut().unwrap().push(json!({ "id": 8, "name": "New", "routingID": "new", "instrument": 0 }));
    for pattern in project["patterns"].as_array_mut().unwrap() {
        pattern["steps"].as_array_mut().unwrap().push(json!(vec![Value::Null; 64]));
    }
    let wider = engine.publish(&parse(&project)).unwrap();
    assert_eq!((wider.patterns_sent, wider.patterns_reused), (2, 0));
}

#[test]
fn a_refused_project_leaves_the_engine_and_the_mixer_as_they_were() {
    let engine = engine();
    engine.publish(&parse(&project(1))).unwrap();
    engine.play(&PlayOptions { from_start: true, ..Default::default() });
    let (before, _) = engine.render_offline(4_800, 512);
    assert!(peak(&before) > 0.05);
    // Routing loops are refused by project validation already; a plan that cannot be compensated
    // is refused by the compiler: more than ten seconds of latency.
    let mut heavy = project(1);
    heavy["audioPlugins"] = json!([effect("track:a", 0, "lat", "test:latency:600000")]);
    let project = st_project::Project::from_value(heavy).unwrap();
    assert!(engine.publish(&project).unwrap_err().contains("compensation capacity"));
    let (after, _) = engine.render_offline(4_800, 512);
    assert!(peak(&after) > 0.05, "the previous project keeps playing");
    assert_eq!(engine.project().unwrap().track_list().len(), 1);
}

#[test]
fn transport_commands_move_the_engine_and_the_status_follows() {
    let engine = engine();
    engine.publish(&parse(&demo())).unwrap();
    let status = engine.status();
    assert_eq!((status["playing"].clone(), status["row"].clone(), status["sampleRate"].clone()), (json!(false), json!(0), json!(48_000.0)));
    for key in ["songMode", "pattern", "sequence", "position", "seconds", "beat", "bpm", "peakL", "peakR", "trackPeaks", "busPeaks", "cpu", "countIn", "transportPending", "queuedPattern", "noteRecordArmed", "running", "routing"] {
        assert!(status.get(key).is_some(), "status has {key}");
    }
    // Play is reported as requested before the audio thread has rendered a block.
    play_song(&engine);
    let pending = engine.status();
    assert_eq!((pending["playing"].clone(), pending["transportPending"].clone()), (json!(true), json!(true)));
    engine.render_offline(48_000, 512);
    engine.tick();
    let playing = engine.status();
    assert_eq!((playing["playing"].clone(), playing["songMode"].clone(), playing["transportPending"].clone()), (json!(true), json!(true), json!(false)));
    assert!(playing["row"].as_i64().unwrap() > 0 && playing["seconds"].as_f64().unwrap() > 0.9);
    assert!(playing["peakL"].as_f64().unwrap() > 0.01 && playing["peakR"].as_f64().unwrap() > 0.01, "master peaks: {playing}");
    assert_eq!(playing["trackPeaks"].as_array().unwrap().len(), 8);
    assert!(playing["trackPeaks"].as_array().unwrap().iter().any(|peak| peak.as_f64().unwrap() > 0.01));
    assert!(playing["busPeaks"]["master"][0].as_f64().unwrap() > 0.01);
    assert!((playing["bpm"].as_f64().unwrap() - 116.0).abs() < 1e-9);

    engine.seek(Some(1), 32, 2);
    engine.render_offline(512, 512);
    let sought = engine.status();
    assert_eq!((sought["pattern"].clone(), sought["sequence"].clone()), (json!(1), json!(2)));
    assert!(sought["row"].as_i64().unwrap() >= 32);

    engine.queue(0, 0, None);
    engine.render_offline(512, 512);
    assert_eq!(engine.status()["queuedSequence"], 0);
    engine.queue(-1, -1, None);

    engine.stop();
    engine.render_offline(512, 512);
    assert_eq!(engine.status()["playing"], false);

    // Pattern mode with a loop range stays inside it.
    engine.play(&PlayOptions { song: false, from_start: true, pattern_loop: Some((4, 7)), count_in: 0 });
    engine.render_offline(2 * 48_000, 512);
    let looped = engine.status();
    assert_eq!(looped["songMode"], false);
    assert!((4..=7).contains(&looped["row"].as_i64().unwrap()), "row {} is inside the loop", looped["row"]);
    engine.stop();
    engine.render_offline(512, 512);

    // Count-in: a bar of clicks before the transport starts.
    engine.play(&PlayOptions { song: true, from_start: true, pattern_loop: None, count_in: 1 });
    let (clicks, _) = engine.render_offline(4_800, 512);
    let counting = engine.status();
    assert!(counting["countIn"].as_i64().unwrap() > 0 && peak(&clicks) > 0.01, "count-in clicks are heard: {counting}");
    engine.render_offline(3 * 48_000, 512);
    assert_eq!((engine.status()["countIn"].clone(), engine.status()["playing"].clone()), (json!(0), json!(true)));

    assert!(engine.record_arm(true));
    engine.render_offline(512, 512);
    let armed = engine.status();
    assert_eq!((armed["noteRecordArmed"].clone(), armed["transportRecording"].clone()), (json!(true), json!(true)));
    engine.panic();
    engine.render_offline(512, 512);
    assert!(engine.duration(true) > 7.0, "four 64-row patterns at 116 BPM");
}

#[test]
fn live_notes_previews_and_expression_sound_without_the_transport() {
    let engine = engine();
    engine.publish(&parse(&project(2))).unwrap();
    assert_eq!(engine.note_destination(Some(9), Some(4)), (1, 0), "destinations are clamped to the project");
    assert_eq!(engine.note_destination(None, None), (1, 0), "and remembered for the next note");
    engine.note_on(1, 0, 60, 100);
    let (held, _) = engine.render_offline(9_600, 512);
    assert!(peak(&held) > 0.05, "a live note sounds while stopped");
    assert!(engine.expression(1, [0xE0, 0x7F, 0x7F]), "the engine bends its own voices");
    assert!(engine.expression(1, [0xB0, 1, 64]), "and takes the modulation wheel");
    assert!(!engine.expression(1, [0xD0, 100, 0]) && !engine.expression(1, [0xB0, 74, 9]) && !engine.expression(1, [0xB0, 123, 0]), "what the engine has no use for goes nowhere");
    engine.note_off(1, 60);
    engine.render_offline(9_600, 512);
    let (released, _) = engine.render_offline(4_800, 512);
    assert!(peak(&released) < 1e-4, "the note is released");

    let step = st_project::model::Step { note: Some(64), instrument: Some(0), velocity: Some(100), ..Default::default() };
    assert!(engine.preview_step(0, &step, 0.1));
    let (audition, _) = engine.render_offline(2_400, 512);
    assert!(peak(&audition) > 0.05);
    assert!(!engine.preview_step(0, &st_project::model::Step::default(), 0.1), "a step without a note stops the audition");

    assert_eq!(engine.instrument_parameter(0, "cutoff", 400.0), Ok(true));
    assert!(engine.instrument_parameter(0, "volume", 1.0).is_err(), "track volume is not an instrument parameter");
    assert!(engine.instrument_parameter(7, "cutoff", 400.0).is_err());
    assert_eq!(engine.status()["playing"], false);
}

#[test]
fn an_input_edge_is_acknowledged_with_the_position_it_landed_on() {
    let engine = engine();
    engine.publish(&parse(&project(1))).unwrap();
    engine.record_arm(true);
    engine.play(&PlayOptions { from_start: true, ..Default::default() });
    engine.render_offline(2 * ROW + 100, 512);
    let (sender, receiver) = mpsc::channel();
    let edge = InputEdge { owner: "key:1".into(), on: true, track: 0, instrument: 0, note: 62, velocity: 90, apply_note: true };
    engine.input_edge(edge.clone(), Box::new(move |ack| sender.send(ack).unwrap()));
    engine.poll_inputs();
    assert!(receiver.try_recv().is_err(), "no acknowledgement before the audio thread has seen the edge");
    engine.render_offline(512, 512);
    engine.poll_inputs();
    let ack = receiver.try_recv().expect("the acknowledgement");
    assert_eq!((ack["status"].clone(), ack["applied"].clone(), ack["playing"].clone(), ack["recording"].clone()), (json!("applied"), json!(true), json!(true), json!(true)));
    assert_eq!((ack["inputOwner"].clone(), ack["note"].clone(), ack["pattern"].clone(), ack["row"].clone()), (json!("key:1"), json!(62), json!(0), json!(2)));
    assert_eq!(ack["recordingContext"]["patterns"], json!([{ "id": 0, "length": 16 }]));
    assert!(ack["requestID"].is_string() && ack["renderFrame"].is_string() && ack["recordingClock"]["distanceBeats"].is_number());

    // An edge without a correlation id is refused at once, and still plays.
    let (sender, receiver) = mpsc::channel();
    engine.input_edge(InputEdge { owner: String::new(), ..edge.clone() }, Box::new(move |ack| sender.send(ack).unwrap()));
    assert_eq!(receiver.try_recv().unwrap()["status"], "unavailable");

    // An edge whose project changes before it is acknowledged no longer counts for recording.
    let (sender, receiver) = mpsc::channel();
    engine.input_edge(InputEdge { on: false, velocity: 0, ..edge }, Box::new(move |ack| sender.send(ack).unwrap()));
    engine.publish(&parse(&project(1))).unwrap();
    engine.render_offline(512, 512);
    engine.poll_inputs();
    let stale = receiver.try_recv().unwrap();
    assert_eq!((stale["status"].clone(), stale["applied"].clone()), (json!("unavailable"), json!(false)));
}

#[test]
fn the_null_device_renders_in_real_time_and_can_be_reconfigured() {
    let mut options = EngineOptions::offline();
    options.samples = samples();
    options.device = DeviceChoice::Null;
    let engine = AudioEngine::new(options).unwrap();
    let devices = engine.devices();
    assert_eq!((devices.output_device, devices.output_uid.as_str(), devices.running, devices.fallback), (0, "null", true, false));
    assert_eq!((devices.sample_rate, devices.buffer_size), (48_000.0, 512));
    assert!(devices.outputs.iter().any(|device| device.uid == "null"), "the null device is always listed");
    engine.publish(&parse(&demo())).unwrap();
    play_song(&engine);
    let started = Instant::now();
    let mut heard = false;
    while started.elapsed() < Duration::from_secs(5) && !heard {
        std::thread::sleep(Duration::from_millis(50));
        engine.tick();
        let status = engine.status();
        heard = status["playing"] == true && status["row"].as_i64().unwrap() > 0 && status["peakL"].as_f64().unwrap() > 0.0;
    }
    assert!(heard, "the null device advances the song and the meters: {}", engine.status());
    assert_eq!(engine.status()["running"], true);

    // Buffer size and sample rate: the engine restarts at the new rate with the same project.
    let changed = engine.configure(&DeviceRequest { buffer_size: Some(128), sample_rate: Some(44_100.0), ..Default::default() }).unwrap();
    assert_eq!((changed.sample_rate, changed.buffer_size, changed.running), (44_100.0, 128, true));
    assert_eq!(engine.status()["sampleRate"], 44_100.0);
    assert_eq!(engine.project().unwrap().track_list().len(), 8);
    assert!(engine.configure(&DeviceRequest { buffer_size: Some(3), ..Default::default() }).is_err());
    // A device that is not connected: the null device stands in and says why.
    let missing = engine.configure(&DeviceRequest { output_uid: Some("No Such Interface".into()), ..Default::default() }).unwrap();
    assert_eq!((missing.output_uid.as_str(), missing.fallback, missing.running), ("null", true, true));
    assert!(missing.device_error.unwrap().contains("No Such Interface"));
    assert!(engine.configure(&DeviceRequest { output_id: Some(123_456), ..Default::default() }).is_err());
    let back = engine.configure(&DeviceRequest { output_id: Some(0), ..Default::default() }).unwrap();
    assert_eq!((back.fallback, back.device_error), (false, None));
    engine.shutdown();
    assert_eq!(engine.status()["running"], false);
}

#[test]
fn the_snapshot_carries_the_project_with_built_in_effect_state() {
    let engine = engine();
    assert_eq!(engine.snapshot(), json!({ "audioPlugins": [] }));
    let mut project = project(1);
    project["audioPlugins"] = json!([effect("track:a", 0, "comp", "stfx:compressor"), effect("master", 1, "vendor", "com.vendor.eq")]);
    project["audioPlugins"][1]["state"] = json!("b3BhcXVl");
    engine.publish(&parse(&project)).unwrap();
    assert!(engine.set_insert_parameter("comp", 0, 0.75));
    assert!(!engine.set_insert_parameter("nothing", 0, 0.75));
    let snapshot = engine.snapshot();
    assert_eq!(snapshot["tracks"], project["tracks"]);
    let plugins = snapshot["audioPlugins"].as_array().unwrap();
    assert_eq!((plugins.len(), plugins[0]["instanceID"].clone(), plugins[1]["state"].clone()), (2, json!("comp"), json!("b3BhcXVl")));
    let state = st_audio::graph::insert::decode_builtin_state("compressor", plugins[0]["state"].as_str().unwrap()).unwrap();
    let first = tracker_engine::effects::catalogue().find(|effect| effect.info.id().to_string_lossy() == "compressor").unwrap().params[0].id().to_string_lossy().into_owned();
    assert!(state.contains(&(first, 0.75)), "the live value is in the captured state: {state:?}");
    // The captured state is what the insert starts from when the project comes back.
    project["audioPlugins"][0]["state"] = plugins[0]["state"].clone();
    engine.publish(&parse(&project)).unwrap();
    let again = st_audio::graph::insert::decode_builtin_state("compressor", engine.snapshot()["audioPlugins"][0]["state"].as_str().unwrap()).unwrap();
    assert_eq!(again.len(), state.len());
    for ((id, value), (other_id, other)) in again.iter().zip(&state) {
        assert!(id == other_id && (value - other).abs() < 1e-12, "{id} survives the round trip");
    }
}

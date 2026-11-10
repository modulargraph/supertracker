//! How much of real time the graph takes: `cargo run --release -p st-audio --example bench`.
//!
//! Renders ten seconds of two projects offline at 48 kHz in 512-frame blocks and prints the
//! render time as a percentage of real time, for the engine alone (through the graph of an
//! empty mixer the tracks are not connected to) and for the whole graph:
//!
//! * the web model's demo song (8 tracks, delay and reverb sends);
//! * a 64-track project: every track plays the demo's parts, in 8 groups, each track with a
//!   built-in compressor and each group with a built-in EQ-type effect, 16 tracks sending to
//!   two return buses with effects, plus the built-in delay and reverb sends.

use serde_json::{json, Value};
use st_audio::{AudioEngine, EngineOptions, MemorySamples, PlayOptions, SamplePcm};
use st_project::Project;
use std::sync::Arc;
use std::time::Instant;

fn samples() -> Arc<MemorySamples> {
    let samples = MemorySamples::new();
    let table = |shape: fn(f64) -> f64| -> SamplePcm {
        let mut pcm = SamplePcm::mono((0..2048).map(|frame| shape(frame as f64 / 2048.0) as f32 * 0.9).collect(), 48_000.0);
        pcm.cycle_frames = Some(2048);
        pcm
    };
    samples.insert("builtin:wavetables/Basic/Sine.wav", table(|phase| (phase * std::f64::consts::TAU).sin()));
    samples.insert("builtin:wavetables/Basic/Saw.wav", table(|phase| 2.0 * phase - 1.0));
    samples.insert("builtin:wavetables/Basic/Triangle.wav", table(|phase| 1.0 - 4.0 * (phase - 0.5).abs()));
    samples.insert("builtin:wavetables/Basic/Square.wav", table(|phase| if phase < 0.5 { 1.0 } else { -1.0 }));
    let mut seed = 0x1234_5678u32;
    let noise: Vec<f32> = (0..48_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.7
        })
        .collect();
    samples.insert("builtin:wavetables/Basic/Noise.wav", SamplePcm::mono(noise, 48_000.0));
    Arc::new(samples)
}

fn demo() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/demo_project.json")).expect("the demo fixture")
}

/// The demo's eight parts eight times over, with a full mixer.
fn sixty_four_tracks() -> Value {
    let mut project = demo();
    let base_tracks = project["tracks"].as_array().unwrap().clone();
    let mut tracks = Vec::new();
    let mut plugins = Vec::new();
    let mut outputs = serde_json::Map::new();
    let mut buses = Vec::new();
    let mut sends = Vec::new();
    for copy in 0..8 {
        buses.push(json!({ "id": format!("g{copy}"), "name": format!("Group {copy}"), "type": "group", "volume": 0.5 }));
        plugins.push(json!({ "owner": format!("bus:g{copy}"), "kind": "effect", "slot": 0, "instanceID": format!("geq{copy}"), "component": "stfx:eq" }));
        for (index, track) in base_tracks.iter().enumerate() {
            let number = copy * 8 + index;
            let mut track = track.clone();
            track["id"] = json!(number);
            track["routingID"] = json!(format!("t{number}"));
            track["volume"] = json!(0.4);
            tracks.push(track);
            outputs.insert(format!("track:t{number}"), json!(format!("bus:g{copy}")));
            plugins.push(json!({ "owner": format!("track:t{number}"), "kind": "effect", "slot": 0, "instanceID": format!("comp{number}"), "component": "stfx:compressor" }));
            if number % 4 == 0 {
                sends.push(json!({ "id": format!("s{number}"), "source": format!("track:t{number}"), "tap": "postFader", "destination": format!("bus:r{}", number % 8 / 4), "gain": 0.3 }));
            }
        }
    }
    for index in 0..2 {
        buses.push(json!({ "id": format!("r{index}"), "name": format!("Return {index}"), "type": "return", "volume": 0.8 }));
        plugins.push(json!({ "owner": format!("bus:r{index}"), "kind": "effect", "slot": 0, "instanceID": format!("rfx{index}"), "component": if index == 0 { "stfx:chorus" } else { "stfx:phaser" } }));
    }
    plugins.push(json!({ "owner": "master", "kind": "effect", "slot": 0, "instanceID": "limit", "component": "stfx:limiter" }));
    project["tracks"] = json!(tracks);
    for pattern in project["patterns"].as_array_mut().unwrap() {
        let steps = pattern["steps"].as_array().unwrap().clone();
        pattern["steps"] = json!((0..8).flat_map(|_| steps.clone()).collect::<Vec<_>>());
        pattern["automation"] = json!([{ "id": "a", "owner": "bus:g0", "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.3 }, { "row": 63, "value": 0.6 }] }]);
    }
    project["routing"] = json!({ "version": 1, "mixModel": "standard", "buses": buses, "outputs": outputs, "sends": sends, "sidechains": [] });
    project["audioPlugins"] = json!(plugins);
    project["masterVolume"] = json!(0.5);
    project
}

/// Renders `seconds` of `project` and returns (percent of real time, peak, unavailable inserts).
fn measure(project: &Value, seconds: usize, block: usize) -> (f64, f32, usize) {
    let mut options = EngineOptions::offline();
    options.samples = samples();
    let engine = AudioEngine::new(options).expect("an engine");
    let project = Project::from_value(project.clone()).expect("a project");
    st_project::validate::validate(&project).expect("a valid project");
    engine.publish(&project).expect("the project is accepted");
    engine.play(&PlayOptions { song: true, from_start: true, ..Default::default() });
    let renderer = engine.renderer();
    let mut renderer = renderer.lock().unwrap();
    let (mut left, mut right) = (vec![0.0f32; block], vec![0.0f32; block]);
    // One second to warm up caches and let the song start.
    for _ in 0..48_000 / block {
        renderer.render(&mut left, &mut right);
    }
    let blocks = seconds * 48_000 / block;
    let mut peak = 0.0f32;
    let started = Instant::now();
    for _ in 0..blocks {
        renderer.render(&mut left, &mut right);
        peak = left.iter().fold(peak, |peak, sample| peak.max(sample.abs()));
    }
    let spent = started.elapsed().as_secs_f64();
    drop(renderer);
    let unavailable = engine.inserts().iter().filter(|insert| !insert.available).count();
    (spent / (blocks * block) as f64 * 48_000.0 * 100.0, peak, unavailable)
}

fn main() {
    let cases: [(&str, Value); 2] = [("demo song (8 tracks)", demo()), ("64 tracks, 75 built-in inserts, 10 buses", sixty_four_tracks())];
    for (name, project) in cases {
        // The same engine work with nothing connected to the mixer: disconnect every track.
        let mut engine_only = project.clone();
        let tracks = engine_only["tracks"].as_array().unwrap().len();
        let mut outputs = serde_json::Map::new();
        for index in 0..tracks {
            let id = engine_only["tracks"][index]["routingID"].as_str().map(str::to_string).unwrap_or_else(|| format!("t{index}"));
            engine_only["tracks"][index]["routingID"] = json!(id);
            engine_only["tracks"][index]["mute"] = json!(true);
            engine_only["tracks"][index]["delaySend"] = json!(0);
            engine_only["tracks"][index]["reverbSend"] = json!(0);
            outputs.insert(format!("track:{id}"), Value::Null);
        }
        engine_only["routing"] = json!({ "version": 1, "mixModel": "standard", "buses": [], "outputs": outputs, "sends": [], "sidechains": [] });
        engine_only["audioPlugins"] = json!([]);
        for pattern in engine_only["patterns"].as_array_mut().unwrap() {
            pattern["automation"].as_array_mut().unwrap().retain(|lane| lane.get("owner").is_none());
        }
        // The best of three runs each: other work on the machine only ever adds time.
        let best = |project: &Value| (0..3).map(|_| measure(project, 10, 512)).min_by(|a, b| a.0.total_cmp(&b.0)).expect("three runs");
        let (engine, _, _) = best(&engine_only);
        let (whole, peak, unavailable) = best(&project);
        assert_eq!(unavailable, 0, "every insert of the benchmark is a built-in effect");
        println!("{name}: {whole:.2}% of real time at 48 kHz / 512 frames (engine alone {engine:.2}%, mixer {:.2}%), peak {peak:.3}", whole - engine);
    }
}

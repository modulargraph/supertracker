//! Shared helpers of the offline render tests: small projects as JSON, generated samples, and a
//! renderer that plays a project from its start.
#![allow(dead_code)]

use serde_json::{json, Value};
use st_audio::graph::insert::testing::TestFactory;
use st_audio::{AudioEngine, EngineOptions, MemorySamples, PlayOptions, SamplePcm};
use st_project::Project;
use std::sync::Arc;

pub const RATE: f64 = 48_000.0;
/// Frames per pattern row at 120 BPM and four lines per beat.
pub const ROW: usize = 6_000;
pub const TONE: &str = "/samples/tone.wav";

/// One second of a 220 Hz sine at half scale (a whole number of cycles, so it loops cleanly),
/// and the built-in wavetables the demo project names.
pub fn samples() -> Arc<MemorySamples> {
    let samples = MemorySamples::new();
    let tone: Vec<f32> = (0..48_000).map(|frame| (frame as f64 * 220.0 * std::f64::consts::TAU / RATE).sin() as f32 * 0.5).collect();
    samples.insert(TONE, SamplePcm::mono(tone, RATE));
    let table = |shape: fn(f64) -> f64| -> SamplePcm {
        let mut pcm = SamplePcm::mono((0..2048).map(|frame| shape(frame as f64 / 2048.0) as f32 * 0.9).collect(), RATE);
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
    samples.insert("builtin:wavetables/Basic/Noise.wav", SamplePcm::mono(noise, RATE));
    Arc::new(samples)
}

/// A project of `tracks` tracks named `a`, `b`, `c`, ... (owners `track:a`, ...) that all play
/// the looping tone from row 0 of a 16-row pattern, with every fader at unity and no sends.
pub fn project(tracks: usize) -> Value {
    let names: Vec<String> = (0..tracks).map(|index| ((b'a' + index as u8) as char).to_string()).collect();
    let note = json!({ "note": 57, "instrument": 0, "velocity": 100, "gate": 0 });
    let steps: Vec<Value> = (0..tracks)
        .map(|_| {
            let mut rows = vec![Value::Null; 16];
            rows[0] = note.clone();
            Value::Array(rows)
        })
        .collect();
    json!({
        "version": 2, "bpm": 120, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": names.iter().enumerate().map(|(index, name)| json!({ "id": index, "name": name, "routingID": name, "volume": 1, "pan": 0, "instrument": 0 })).collect::<Vec<_>>(),
        "instruments": [{ "id": 0, "type": "sample", "samplePath": TONE, "mode": "forward", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.01 }],
        "patterns": [{ "id": 0, "length": 16, "steps": steps, "automation": [] }],
        "sequence": [0], "loopSong": false,
        "delay": { "mix": 1 }, "reverb": { "mix": 1 },
        "routing": { "buses": [], "outputs": {}, "sends": [], "sidechains": [] },
        "audioPlugins": [],
    })
}

pub fn bus(id: &str, kind: &str, volume: f64) -> Value {
    json!({ "id": id, "name": id, "type": kind, "volume": volume, "pan": 0, "mute": false, "solo": false })
}

pub fn effect(owner: &str, slot: usize, instance: &str, component: &str) -> Value {
    json!({ "owner": owner, "kind": "effect", "slot": slot, "instanceID": instance, "component": component, "name": component })
}

/// An offline engine with the test samples and the test insert factory.
pub fn engine() -> Arc<AudioEngine> {
    let mut options = EngineOptions::offline();
    options.samples = samples();
    options.inserts = Arc::new(TestFactory);
    AudioEngine::new(options).expect("an offline engine")
}

pub fn parse(project: &Value) -> Project {
    let project = Project::from_value(project.clone()).expect("a project");
    st_project::validate::validate(&project).expect("a valid project");
    project
}

/// Publishes `project`, plays its pattern from the start and renders `frames` frames in blocks
/// of `block`.
pub fn render_with(project: &Value, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let engine = engine();
    engine.publish(&parse(project)).expect("the project is accepted");
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    engine.render_offline(frames, block)
}

pub fn render(project: &Value, frames: usize) -> (Vec<f32>, Vec<f32>) {
    render_with(project, frames, 512)
}

pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()))
}

/// Asserts `actual[i] == expected(i)` within `tolerance` for every frame.
pub fn assert_signal(actual: &[f32], expected: impl Fn(usize) -> f32, tolerance: f32, what: &str) {
    for (index, sample) in actual.iter().enumerate() {
        let wanted = expected(index);
        assert!((sample - wanted).abs() <= tolerance, "{what}: frame {index} is {sample}, expected {wanted}");
    }
}

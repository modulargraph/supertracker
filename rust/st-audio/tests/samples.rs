//! Samples from files, decoded off the control thread: a project plays silence while its
//! sample is `Pending` and the sample once the worker has decoded it.

mod common;

use common::*;
use st_audio::samples::wav_bytes;
use st_audio::{AudioEngine, DecodedSamples, EngineOptions, PlayOptions};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

fn folder(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("st-audio-it-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn a_pending_sample_is_silent_and_is_heard_once_it_is_decoded() {
    let dir = folder("pending");
    let file = dir.join("tone.wav");
    // The tone of the other offline tests, as a 16-bit file.
    let tone: Vec<f32> = (0..48_000).map(|frame| (frame as f64 * 220.0 * std::f64::consts::TAU / RATE).sin() as f32 * 0.5).collect();
    std::fs::write(&file, wav_bytes(&tone, None, 48_000)).unwrap();
    let path = file.to_string_lossy().into_owned();

    let samples = Arc::new(DecodedSamples::new(2));
    let (sender, decoded) = mpsc::channel();
    let (sender, publisher) = (Mutex::new(sender), std::thread::current().id());
    samples.set_listener(move |event| {
        let _ = sender.lock().unwrap().send((event.clone(), std::thread::current().id()));
    });
    let mut options = EngineOptions::offline();
    options.samples = samples.clone();
    let engine = AudioEngine::new(options).unwrap();

    let mut project = project(1);
    project["instruments"][0]["samplePath"] = serde_json::json!(path);
    let report = engine.publish(&parse(&project)).unwrap();
    assert_eq!(report.samples_installed, 0);
    assert_eq!(report.problems.len(), 1, "the sample is reported as being decoded");
    assert!(report.problems[0].error.is_none() && report.problems[0].path == path);

    // Nothing is installed yet, whatever the worker is doing: the engine was given no PCM.
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    let (left, _) = engine.render_offline(4_800, 512);
    assert_eq!(peak(&left), 0.0, "a pending sample is silent");

    let (event, thread) = decoded.recv_timeout(Duration::from_secs(10)).expect("the decode ends");
    assert_ne!(thread, publisher, "the file was decoded on a worker thread");
    assert_eq!(event.result.as_ref().unwrap().frames, 48_000);
    let report = engine.samples_changed().unwrap();
    assert_eq!((report.samples_installed, report.problems.len()), (1, 0));

    engine.stop();
    engine.render_offline(4_800, 512);
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    let (left, right) = engine.render_offline(24_000, 512);
    assert!(peak(&left) > 0.2 && left == right, "the decoded sample is heard: peak {}", peak(&left));

    // The same audio as the same project with the sample held in memory (16-bit quantisation apart).
    let mut reference = common::project(1);
    reference["instruments"][0]["samplePath"] = serde_json::json!(TONE);
    let (expected, _) = render(&reference, 24_000);
    let start = left.iter().position(|sample| sample.abs() > 1e-4).unwrap();
    let expected_start = expected.iter().position(|sample| sample.abs() > 1e-4).unwrap();
    for index in 0..10_000 {
        assert!((left[start + index] - expected[expected_start + index]).abs() < 2e-4, "frame {index}");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_demo_song_plays_from_the_built_in_tables_of_st_samples() {
    let samples = Arc::new(DecodedSamples::new(2));
    let mut options = EngineOptions::offline();
    options.samples = samples.clone();
    let engine = AudioEngine::new(options).unwrap();
    let demo: serde_json::Value = serde_json::from_str(include_str!("fixtures/demo_project.json")).unwrap();
    // Built-in tables are generated on a worker within the lookup's short wait: the very first
    // publication installs them.
    let report = engine.publish(&parse(&demo)).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert!(report.samples_installed >= 4);
    assert!(samples.wait_idle(Duration::from_secs(20)));
    engine.play(&PlayOptions { song: true, from_start: true, ..PlayOptions::default() });
    let (left, right) = engine.render_offline(48_000, 512);
    assert!(peak(&left) > 0.05 && peak(&right) > 0.05);
}

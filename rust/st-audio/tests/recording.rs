//! Takes of the master output and of an input, through the engine.

mod common;

use common::*;
use st_audio::inputs::{InputRequest, NULL_TONE_INPUT};
use st_audio::live::{RecordRequest, RecordSource};
use st_audio::samples::read_wav;
use st_audio::{AudioEngine, DeviceChoice, EngineOptions, PlayOptions};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn file(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("st-audio-recording-{label}-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn master(path: &std::path::Path) -> RecordRequest {
    RecordRequest { source: RecordSource::Master, path: path.to_path_buf(), monitor: false, monitor_volume: 0.0, compensation_frames: 0 }
}

#[test]
fn a_master_take_is_the_rendered_output_from_the_frame_it_started_at() {
    // No device: the test is the audio thread, so every frame is accounted for.
    let engine = engine();
    engine.publish(&parse(&project(2))).unwrap();
    engine.render_offline(1_000, 512);
    let path = file("exact");
    assert_eq!(engine.live().record(&master(&path)).unwrap(), 2);
    assert!(engine.live().record(&master(&file("second"))).unwrap_err().contains("already running"));
    assert!(engine.status()["recording"] == true && engine.status()["recordingStopping"] == false);
    // Recording starts before playback does: 700 frames of silence, then the song.
    let (mut left, mut right) = engine.render_offline(700, 512);
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    for _ in 0..40 {
        let (l, r) = engine.render_offline(1_000, 512);
        left.extend(l);
        right.extend(r);
    }
    assert!(engine.status()["recordingDuration"].as_f64().unwrap() > 0.5);
    // Stop is acknowledged by the next block the source renders.
    let stopper = std::thread::spawn({
        let engine = engine.clone();
        move || engine.live().stop_record().expect("a take was running")
    });
    while !stopper.is_finished() {
        engine.render_offline(64, 64);
        std::thread::sleep(Duration::from_millis(1));
    }
    let take = stopper.join().unwrap();
    assert_eq!((take.source, take.file.problem.clone(), take.file.dropped, take.file.channels, take.file.sample_rate), (RecordSource::Master, None, 0, 2, 48_000));
    // The take began at graph frame 1 000 (the first block after it was armed).
    assert_eq!(take.file.start_frame, Some(1_000));
    let frames = take.file.frames as usize;
    assert_eq!(take.file.end_frame, Some(1_000 + frames as i64));
    assert!(frames >= left.len() && frames < left.len() + 4_096, "{frames} frames for {} rendered before Stop", left.len());
    let recorded = read_wav(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(recorded.frames(), frames);
    assert!(peak(&left) > 0.2 && peak(&left[..700]) == 0.0);
    // 24-bit PCM: within one quantisation step of what was rendered, frame for frame.
    for index in 0..left.len() {
        assert!((recorded.left[index] - left[index]).abs() < 2.5e-7 && (recorded.right[index] - right[index]).abs() < 2.5e-7, "frame {index}: {} vs {}", recorded.left[index], left[index]);
    }
    assert!(engine.status()["recording"] == false && engine.live().stop_record().is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_master_take_on_the_null_device_reproduces_the_song() {
    let mut options = EngineOptions::offline();
    options.samples = samples();
    options.device = DeviceChoice::Null;
    let live = AudioEngine::new(options).unwrap();
    live.publish(&parse(&project(1))).unwrap();
    let path = file("null");
    let began = Instant::now();
    live.live().record(&master(&path)).unwrap();
    std::thread::sleep(Duration::from_millis(120));
    live.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    std::thread::sleep(Duration::from_millis(700));
    let take = live.live().stop_record().unwrap();
    let elapsed = began.elapsed().as_secs_f64();
    assert_eq!((take.file.problem.clone(), take.file.dropped), (None, 0));
    let recorded = read_wav(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(recorded.frames() as u64, take.file.frames);
    assert_eq!(take.file.end_frame.unwrap() - take.file.start_frame.unwrap(), take.file.frames as i64);
    let seconds = recorded.frames() as f64 / 48_000.0;
    assert!(seconds > 0.7 && seconds < elapsed + 0.05, "{seconds} s recorded in {elapsed} s");
    // Silence until Play took effect (at least the 120 ms waited), then the song as an
    // offline render of it from its start.
    let first = recorded.left.iter().position(|sample| sample.abs() > 1e-4).expect("the song was recorded");
    assert!(first as f64 / 48_000.0 > 0.1 && (first as f64 / 48_000.0) < 0.4, "playback starts {} s into the take", first as f64 / 48_000.0);
    let (reference, _) = render(&project(1), 48_000);
    let start = reference.iter().position(|sample| sample.abs() > 1e-4).unwrap();
    let compared = (recorded.frames() - first).min(24_000);
    assert!(compared > 12_000);
    for index in 0..compared {
        assert!((recorded.left[first + index] - reference[start + index]).abs() < 1e-4, "frame {index} after the start: {} vs {}", recorded.left[first + index], reference[start + index]);
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn an_input_is_metered_monitored_and_recorded_with_its_latency_left_out() {
    let engine = engine();
    engine.publish(&parse(&project(1))).unwrap();
    let live = engine.live();
    live.inputs().set_format(48_000.0, 256, true);
    assert!(live.inputs().null_input().is_some_and(|input| input.uid == "null" && input.id == 0 && input.input_channels == 2), "the null input is offered with the null output");
    // The test input: 440 Hz on channel 0, 880 Hz on channel 1, both at a quarter of full scale.
    live.inputs().configure(&InputRequest { uid: Some(NULL_TONE_INPUT.to_string()), channels: Some(vec![0, 1]), meter: Some(true), ..InputRequest::default() }).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let status = engine.status();
    assert_eq!(status["inputMetering"], true);
    assert!((status["inputPeakLeft"].as_f64().unwrap() - 0.25).abs() < 0.01 && (status["inputPeakRight"].as_f64().unwrap() - 0.25).abs() < 0.01, "{status}");
    let mut devices = serde_json::json!({});
    live.decorate_devices(&mut devices);
    assert_eq!((devices["inputUid"].clone(), devices["inputChannels"].clone(), devices["inputMetering"].clone(), devices["inputRunning"].clone(), devices["monitoring"].clone()), (serde_json::json!(NULL_TONE_INPUT), serde_json::json!([0, 1]), serde_json::json!(true), serde_json::json!(true), serde_json::json!(false)));

    // A stereo take, with the first 480 frames left out (latency compensation), monitored at 0.5.
    let path = file("input");
    let request = RecordRequest { source: RecordSource::Input, path: path.clone(), monitor: true, monitor_volume: 0.5, compensation_frames: 480 };
    assert_eq!(live.record(&request).unwrap(), 2);
    std::thread::sleep(Duration::from_millis(400));
    // The monitored input is mixed into the output (nothing else plays). The test renders
    // faster than the input delivers, so only what has arrived by now is there to hear.
    let (left, right) = engine.render_offline(2_048, 512);
    assert!((peak(&left) - 0.125).abs() < 0.01 && (peak(&right) - 0.125).abs() < 0.01, "monitoring at half volume: {} / {}", peak(&left), peak(&right));
    let take = live.stop_record().unwrap();
    assert_eq!((take.source, take.compensation_frames, take.file.problem.clone(), take.file.channels), (RecordSource::Input, 480, None, 2));
    let recorded = read_wav(&std::fs::read(&path).unwrap()).unwrap();
    assert!(recorded.frames() > 9_600, "{} frames", recorded.frames());
    // The file starts at input frame start + 480: the sine's phase says so.
    let start = take.file.start_frame.unwrap() as f64;
    for index in (0..4_800).step_by(7) {
        let expected = |frequency: f64| ((start + index as f64) * frequency / 48_000.0 * std::f64::consts::TAU).sin() as f32 * 0.25;
        assert!((recorded.left[index] - expected(440.0)).abs() < 1e-4 && (recorded.right[index] - expected(880.0)).abs() < 1e-4, "frame {index}");
    }
    // Monitoring ended with the take's request? No: it stays as configured until changed.
    live.inputs().configure(&InputRequest { monitor: Some(false), meter: Some(false), channels: Some(vec![1]), ..InputRequest::default() }).unwrap();
    let mut devices = serde_json::json!({});
    live.decorate_devices(&mut devices);
    assert_eq!((devices["inputRunning"].clone(), devices["inputChannels"].clone()), (serde_json::json!(false), serde_json::json!([1])), "an input nobody uses is closed");
    // A mono take of channel 1 (the 880 Hz side).
    let mono = file("mono");
    let request = RecordRequest { source: RecordSource::Input, path: mono.clone(), monitor: false, monitor_volume: 0.0, compensation_frames: 0 };
    assert_eq!(live.record(&request).unwrap(), 1);
    std::thread::sleep(Duration::from_millis(150));
    assert!(engine.status()["inputPeakLeft"].as_f64().unwrap() > 0.2, "the take's level is shown while the meter is off");
    let take = live.stop_record().unwrap();
    let recorded = read_wav(&std::fs::read(&mono).unwrap()).unwrap();
    // 880 Hz for a tenth of a second: 176 zero crossings (the 440 Hz side would have 88).
    let crossings = recorded.left[..4_800].windows(2).filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0)).count();
    assert!((174..=178).contains(&crossings) && (peak(&recorded.left) - 0.25).abs() < 0.01, "{crossings} zero crossings");
    assert_eq!(take.file.channels, 1);
    assert!(!live.inputs().watch(), "nothing was lost");
    assert!(live.inputs().configure(&InputRequest { channels: Some(vec![0, 1, 2]), ..InputRequest::default() }).is_err());
    assert!(live.inputs().configure(&InputRequest { id: Some(123_456), ..InputRequest::default() }).is_err());
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(mono);
}

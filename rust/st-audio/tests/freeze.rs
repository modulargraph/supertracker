//! Track Freeze round trips: a frozen track plays the same audio as the live track, in Song and
//! in Pattern mode, and its fingerprint follows content, not paths.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::export::{export, ExportRequest, RenderContext};
use st_audio::freeze::{fingerprint, freeze_track, status, FreezeRequest};
use st_audio::samples::{read_wav, wav_bytes};
use st_audio::{AudioEngine, DecodedSamples, EngineOptions, PlayOptions, SamplePcm};
use st_samples::CancelToken;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Fixture {
    folder: PathBuf,
    samples: Arc<DecodedSamples>,
    live: Arc<AudioEngine>,
    sample: String,
}

fn hash(path: &str) -> Option<String> {
    st_project::media::hash_file(Path::new(path), u64::MAX, None).ok()
}

impl Fixture {
    fn new(label: &str) -> Fixture {
        let folder = std::env::temp_dir().join(format!("st-audio-freeze-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // A decaying 220 Hz pluck, as a file.
        let pluck: Vec<f32> = (0..24_000).map(|frame| (frame as f64 * 220.0 * std::f64::consts::TAU / RATE).sin() as f32 * 0.5 * (-(frame as f32) / 6_000.0).exp()).collect();
        let sample = folder.join("pluck.wav");
        std::fs::write(&sample, wav_bytes(&pluck, None, 48_000)).unwrap();
        let samples = Arc::new(DecodedSamples::new(2));
        let mut options = EngineOptions::offline();
        options.samples = samples.clone();
        let live = AudioEngine::new(options).unwrap();
        Fixture { folder, samples, live, sample: sample.to_string_lossy().into_owned() }
    }

    /// Two tracks and two patterns. Track a (the one to freeze) plays the pluck through a
    /// compressor and a limiter, with a fader, a pan and a volume lane; track b plays other notes.
    fn project(&self) -> Value {
        let mut project = project(2);
        project["instruments"][0]["samplePath"] = json!(self.sample);
        project["instruments"][0]["mode"] = json!("oneshot");
        project["instruments"][0]["release"] = json!(0.2);
        let note = |note: i64, gate: f64| json!({ "note": note, "instrument": 0, "velocity": 110, "gate": gate });
        let rows = |notes: &[(usize, i64)]| -> Value {
            let mut rows = vec![Value::Null; 16];
            for (row, pitch) in notes {
                rows[*row] = note(*pitch, 1.0);
            }
            Value::Array(rows)
        };
        project["patterns"] = json!([
            { "id": 0, "length": 16, "steps": [rows(&[(0, 57), (4, 60), (10, 64), (14, 62)]), rows(&[(2, 45), (8, 48)])], "automation": [
                { "id": "v", "owner": "track:a", "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.9 }, { "row": 15, "value": 0.5 }] },
            ] },
            { "id": 1, "length": 16, "steps": [rows(&[(0, 52), (6, 55), (15, 59)]), rows(&[(4, 40)])], "automation": [] },
        ]);
        project["sequence"] = json!([0, 1]);
        project["tracks"][0]["volume"] = json!(0.8);
        project["tracks"][0]["pan"] = json!(-0.3);
        project["tracks"][0]["delaySend"] = json!(0.2);
        project["tracks"][1]["volume"] = json!(0.6);
        project["patterns"][0]["steps"][0][10]["fx1"] = json!({ "type": "pan", "value": 0.4 });
        project["audioPlugins"] = json!([effect("track:a", 0, "comp", "stfx:compressor"), effect("track:a", 1, "limit", "stfx:limiter")]);
        project
    }

    fn publish(&self, project: &Value) {
        self.samples.load(&self.sample, &CancelToken::new()).unwrap();
        let report = self.live.publish(&parse(project)).unwrap();
        assert!(self.samples.wait_idle(std::time::Duration::from_secs(20)));
        let report = if report.problems.is_empty() { report } else { self.live.samples_changed().unwrap() };
        assert!(report.problems.is_empty(), "{:?}", report.problems);
    }

    fn context<'a>(&'a self, cancel: &'a CancelToken) -> RenderContext<'a> {
        RenderContext { live: &self.live, samples: &self.samples, cancel, progress: &|_, _| {} }
    }

    fn export_song(&self, name: &str) -> SamplePcm {
        let path = self.folder.join(name).to_string_lossy().into_owned();
        let request = ExportRequest { path: path.clone(), mode: "song".to_string(), tail: Some(2.0), format: serde_json::from_value(json!({ "encoding": "float32" })).unwrap(), ..ExportRequest::default() };
        let cancel = CancelToken::new();
        export(&self.context(&cancel), &request).unwrap();
        read_wav(&std::fs::read(path).unwrap()).unwrap()
    }

    /// The live engine (it has no device) looping pattern 0 for `frames` frames.
    fn pattern_loop(&self, frames: usize) -> (Vec<f32>, Vec<f32>) {
        self.live.stop();
        self.live.panic();
        self.live.render_offline(2_048, 512);
        self.live.seek(Some(0), 0, 0);
        self.live.play(&PlayOptions { song: false, ..PlayOptions::default() });
        // The output lags the engine by the mixer's latency (the limiter's look-ahead while
        // the track is live, nothing once it is frozen): compare what was played, not when.
        let (left, right) = self.live.render_offline(frames + 4_096, 512);
        let latency = self.live.status()["latencyCompensation"]["frames"].as_u64().unwrap() as usize;
        assert!(latency <= 4_096);
        let audio = (left[latency..latency + frames].to_vec(), right[latency..latency + frames].to_vec());
        self.live.stop();
        self.live.panic();
        self.live.render_offline(2_048, 512);
        audio
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn largest_difference(a: &[f32], b: &[f32]) -> (usize, f32) {
    a.iter().zip(b).enumerate().map(|(index, (x, y))| (index, (x - y).abs())).fold((0, 0.0), |worst, item| if item.1 > worst.1 { item } else { worst })
}

#[test]
fn a_frozen_track_plays_what_the_live_track_played() {
    let fixture = Fixture::new("roundtrip");
    let mut project = fixture.project();
    fixture.publish(&project);
    let live_song = fixture.export_song("live.wav");
    // One pattern is 16 rows of 6 000 frames; two and a half passes of pattern 0.
    let live_loop = fixture.pattern_loop(240_000);
    assert!(peak(&live_song.left) > 0.05 && peak(&live_loop.0) > 0.05);

    let cancel = CancelToken::new();
    let progress = std::sync::Mutex::new(Vec::new());
    let context = RenderContext { live: &fixture.live, samples: &fixture.samples, cancel: &cancel, progress: &|fraction, phase| progress.lock().unwrap().push((fraction, phase.to_string())) };
    let result = freeze_track(&context, &FreezeRequest { track: 0, tail: None, maximum_tail: 10.0 }, &fixture.folder.join("Freezes"), &hash).unwrap();
    let record = result["record"].clone();
    assert_eq!((result["releasedPlugins"].clone(), record["dormantPluginIDs"].clone(), record["tailMode"].clone(), record["method"].clone()), (json!(2), json!(["comp", "limit"]), json!("auto"), json!("offline")));
    assert_eq!((record["version"].clone(), record["sampleRate"].clone(), record["musicalFrames"].clone(), record["musicalEndBeat"].clone()), (json!(1), json!(48_000.0), json!(192_000), json!(8.0)));
    assert_eq!(record["frames"].as_u64().unwrap(), record["musicalFrames"].as_u64().unwrap() + record["tailFrames"].as_u64().unwrap());
    let tail = record["tailFrames"].as_u64().unwrap();
    assert!(tail > 0 && tail < 480_000, "the auto tail ends where the chain is silent ({tail} frames)");
    // The prints: the song, and a head and a loop print of both patterns, each with its hash.
    let prints = record["patternPrints"].as_array().unwrap();
    assert_eq!(prints.iter().map(|print| print["pattern"].as_i64().unwrap()).collect::<Vec<_>>(), [0, 1]);
    for path in [&record["path"], &prints[0]["path"], &prints[0]["loop"]["path"], &prints[1]["path"], &prints[1]["loop"]["path"]] {
        assert!(Path::new(path.as_str().unwrap()).is_file(), "{path}");
    }
    assert_eq!(record["sha256"], json!(hash(record["path"].as_str().unwrap()).unwrap()));
    assert_eq!(prints[0]["loop"]["sha256"], json!(hash(prints[0]["loop"]["path"].as_str().unwrap()).unwrap()));
    assert_eq!((prints[0]["passFrames"].clone(), prints[0]["loop"]["frames"].clone()), (json!(96_000), prints[0]["frames"].clone()));
    assert!(prints[0]["passes"].as_u64().unwrap() >= 2);
    assert!(progress.lock().unwrap().iter().any(|(_, phase)| phase == "patterns"));

    // The interface stores the record in the track and publishes: the engine plays the prints.
    project["tracks"][0]["frozen"] = record.clone();
    st_project::validate::validate(&st_project::Project::from_value(project.clone()).unwrap()).expect("the record is a valid freeze record");
    fixture.publish(&project);
    assert!(fixture.live.inserts().is_empty(), "the frozen track's inserts are unloaded: {:?}", fixture.live.inserts());
    let frozen_song = fixture.export_song("frozen.wav");
    // A song that does not loop plays the whole print: its end moves out by the printed tail.
    assert_eq!(frozen_song.frames() as u64, live_song.frames() as u64 + tail);
    let (frame, difference) = largest_difference(&frozen_song.left, &live_song.left);
    assert!(difference < 2e-4, "song, left: frame {frame} differs by {difference}");
    let (frame, difference) = largest_difference(&frozen_song.right, &live_song.right);
    assert!(difference < 2e-4, "song, right: frame {frame} differs by {difference}");

    // Pattern mode: the head print, then the loop print on every wrap.
    let frozen_loop = fixture.pattern_loop(240_000);
    let (frame, difference) = largest_difference(&frozen_loop.0, &live_loop.0);
    assert!(difference < 2e-3, "pattern loop, left: frame {frame} differs by {difference}");
    let (frame, difference) = largest_difference(&frozen_loop.1, &live_loop.1);
    assert!(difference < 2e-3, "pattern loop, right: frame {frame} differs by {difference}");

    // Fader, pan, mute and sends stay live on the frozen track.
    project["tracks"][0]["mute"] = json!(true);
    fixture.publish(&project);
    let muted = fixture.export_song("muted.wav");
    project["tracks"][0]["frozen"] = Value::Null;
    project["tracks"][0].as_object_mut().unwrap().remove("frozen");
    fixture.publish(&project);
    let live_muted = fixture.export_song("live-muted.wav");
    assert!(largest_difference(&muted.left, &live_muted.left).1 < 1e-6, "a muted frozen track is as silent as the muted live track");
    assert_eq!(fixture.live.inserts().len(), 2, "unfreezing brings the inserts back");
}

#[test]
fn the_fingerprint_follows_content_not_paths() {
    let fixture = Fixture::new("fingerprint");
    let mut project = fixture.project();
    project["audioClips"] = json!([{ "id": "c", "track": 0, "startBeat": 0, "durationBeats": 1, "path": fixture.sample }]);
    fixture.publish(&project);
    let cancel = CancelToken::new();
    let result = freeze_track(&fixture.context(&cancel), &FreezeRequest { track: 0, tail: Some(0.5), maximum_tail: 10.0 }, &fixture.folder.join("Freezes"), &hash).unwrap();
    let record = result["record"].clone();
    assert_eq!((record["tailMode"].clone(), record["tailFrames"].clone(), record["frames"].clone()), (json!("fixed"), json!(24_000), json!(192_000 + 24_000)));
    project["tracks"][0]["frozen"] = record.clone();
    fixture.publish(&project);
    // The host reads the project as the engine holds it.
    let project = fixture.live.snapshot();
    let rows = status(&project, &hash);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["track"].clone(), rows[0]["id"].clone(), rows[0]["available"].clone(), rows[0]["stale"].clone(), rows[0]["patternPlayback"].clone()), (json!(0), record["id"].clone(), json!(true), json!(false), json!(true)));
    let original = fingerprint(&project, 0, &hash).unwrap();
    assert_eq!(json!(original), record["fingerprint"]);

    // Save As copies the sample into the project's media folder: other path, same content.
    let media = fixture.folder.join("Saved Media");
    std::fs::create_dir_all(&media).unwrap();
    let copy = media.join("pluck copy.wav");
    std::fs::copy(&fixture.sample, &copy).unwrap();
    let mut saved = project.clone();
    saved["instruments"][0]["samplePath"] = json!(copy.to_string_lossy());
    saved["audioClips"][0]["path"] = json!(copy.to_string_lossy());
    assert_eq!(fingerprint(&saved, 0, &hash).unwrap(), original, "a copy of the same audio elsewhere is the same source");
    assert_eq!(status(&saved, &hash)[0]["stale"], false);
    // The mixer's part of the track is not printed: it does not make the print stale either.
    let mut mixed = saved.clone();
    mixed["tracks"][0]["volume"] = json!(0.3);
    mixed["tracks"][0]["name"] = json!("renamed");
    mixed["patterns"][0]["automation"][0]["points"][0]["value"] = json!(0.1);
    mixed["tracks"][1]["instrument"] = json!(0);
    mixed["patterns"][0]["steps"][1][3] = json!({ "note": 50, "instrument": 0, "velocity": 100 });
    assert_eq!(fingerprint(&mixed, 0, &hash).unwrap(), original);
    // Other content under any path, a changed note, a changed insert or tempo: stale.
    let other = fixture.folder.join("other.wav");
    std::fs::write(&other, wav_bytes(&[0.25; 2_000], None, 48_000)).unwrap();
    let mut changes: Vec<Value> = Vec::new();
    let mut changed = project.clone();
    changed["instruments"][0]["samplePath"] = json!(other.to_string_lossy());
    changes.push(changed);
    let mut changed = project.clone();
    changed["patterns"][1]["steps"][0][6]["note"] = json!(56);
    changes.push(changed);
    let mut changed = project.clone();
    changed["audioPlugins"][1]["component"] = json!("stfx:phaser");
    changes.push(changed);
    let mut changed = project.clone();
    changed["bpm"] = json!(121);
    changes.push(changed);
    let mut changed = project.clone();
    changed["instruments"][0]["cutoff"] = json!(0.2);
    changes.push(changed);
    for (index, changed) in changes.iter().enumerate() {
        assert_ne!(fingerprint(changed, 0, &hash).unwrap(), original, "change {index}");
        assert_eq!(status(changed, &hash)[0]["stale"], true, "change {index}");
    }
    // A print that is gone is reported as unavailable; a pattern used later has no print.
    std::fs::remove_file(record["patternPrints"][0]["loop"]["path"].as_str().unwrap()).unwrap();
    assert_eq!(status(&project, &hash)[0]["available"], false);
    let mut more = project.clone();
    more["patterns"].as_array_mut().unwrap().push(json!({ "id": 2, "length": 16, "steps": [[{ "note": 60, "instrument": 0 }], []], "automation": [] }));
    let row = &status(&more, &hash)[0];
    assert_eq!((row["patternPlayback"].clone(), row["unprintedPatterns"].clone()), (json!(false), json!([2])));

    // Refusals.
    let cancel = CancelToken::new();
    let refuse = |request: FreezeRequest| freeze_track(&fixture.context(&cancel), &request, &fixture.folder.join("Freezes"), &hash).unwrap_err();
    assert!(refuse(FreezeRequest { track: 0, tail: None, maximum_tail: 10.0 }).contains("already frozen"));
    assert!(refuse(FreezeRequest { track: 9, tail: None, maximum_tail: 10.0 }).contains("existing track"));
    assert!(FreezeRequest::from_payload(&json!({ "track": 0, "method": "record" })).unwrap_err().contains("recording"));
    assert!(FreezeRequest::from_payload(&json!({ "track": 0, "tail": 31 })).unwrap_err().contains("between 0 and 30"));
    assert_eq!(FreezeRequest::from_payload(&json!({ "track": 1, "maxTail": 4 })).unwrap(), FreezeRequest { track: 1, tail: None, maximum_tail: 4.0 });
}

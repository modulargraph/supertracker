//! Export through the private offline engine: the files equal what the engine renders, stems
//! line up with the mix, and a cancelled export leaves nothing behind.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::export::{export, ExportRequest, RenderContext, CANCELLED};
use st_audio::samples::read_wav;
use st_audio::{AudioEngine, DecodedSamples, EngineOptions, PlayOptions, SamplePcm};
use st_samples::CancelToken;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Fixture {
    folder: PathBuf,
    samples: Arc<DecodedSamples>,
    live: Arc<AudioEngine>,
}

impl Fixture {
    /// A "live" engine (without a device) playing `project`, with samples from `st-samples`.
    fn new(label: &str, project: &Value) -> Fixture {
        let folder = std::env::temp_dir().join(format!("st-audio-export-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let samples = Arc::new(DecodedSamples::new(2));
        // The tone of the offline tests, as audio the host made itself.
        let tone: Vec<f32> = (0..48_000).map(|frame| (frame as f64 * 220.0 * std::f64::consts::TAU / RATE).sin() as f32 * 0.5).collect();
        samples.insert(st_samples::DecodedSample::new(TONE, RATE, tone.clone(), tone, 1));
        let mut options = EngineOptions::offline();
        options.samples = samples.clone();
        let live = AudioEngine::new(options).unwrap();
        let report = live.publish(&parse(project)).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        Fixture { folder, samples, live }
    }

    fn export(&self, request: &ExportRequest, cancel: &CancelToken, progress: &dyn Fn(f64, &str)) -> Result<Value, String> {
        export(&RenderContext { live: &self.live, samples: &self.samples, cancel, progress }, request)
    }

    fn path(&self, name: &str) -> String {
        self.folder.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn float32(path: &str, mode: &str) -> ExportRequest {
    ExportRequest { path: path.to_string(), mode: mode.to_string(), format: serde_json::from_value(json!({ "encoding": "float32", "sampleRate": 48_000 })).unwrap(), ..ExportRequest::default() }
}

fn read(path: impl AsRef<Path>) -> SamplePcm {
    read_wav(&std::fs::read(path).unwrap()).unwrap()
}

fn demo() -> Value {
    serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).unwrap()
}

#[test]
fn the_exported_song_is_what_the_engine_renders_sample_for_sample() {
    let fixture = Fixture::new("song", &demo());
    let progress = Mutex::new(Vec::new());
    let mut request = float32(&fixture.path("song.wav"), "song");
    request.tail = Some(1.5);
    let result = fixture.export(&request, &CancelToken::new(), &|fraction, phase| progress.lock().unwrap().push((fraction, phase.to_string()))).unwrap();

    // The reference: the same project (not looping, no metronome) on an engine of the test's own.
    let mut options = EngineOptions::offline();
    options.samples = fixture.samples.clone();
    let reference = AudioEngine::new(options).unwrap();
    let mut project = demo();
    project["loopSong"] = json!(false);
    project["metronome"] = json!(false);
    reference.publish(&parse(&project)).unwrap();
    let duration = reference.duration(true);
    let musical = (duration * RATE) as usize;
    let frames = ((duration + 1.5) * RATE) as usize;
    reference.seek(None, 0, 0);
    reference.play(&PlayOptions { song: true, ..PlayOptions::default() });
    let (mut left, mut right) = reference.render_offline(musical, 512);
    reference.stop();
    let (tail_left, tail_right) = reference.render_offline(frames - musical, 512);
    left.extend(tail_left);
    right.extend(tail_right);

    let file = read(fixture.path("song.wav"));
    assert_eq!((file.sample_rate, file.frames()), (48_000.0, frames));
    assert!(peak(&file.left) > 0.05 && peak(&file.right) > 0.05, "the song is audible");
    assert!(file.left.iter().zip(&left).all(|(a, b)| a.to_bits() == b.to_bits()), "left channel, sample for sample");
    assert!(file.right.iter().zip(&right).all(|(a, b)| a.to_bits() == b.to_bits()), "right channel, sample for sample");
    assert!(peak(&file.left[musical + 4_800..]) < peak(&file.left[..musical]), "the tail decays after the musical end");

    assert_eq!((result["path"].clone(), result["paths"].clone(), result["mode"].clone()), (json!(fixture.path("song.wav")), json!([fixture.path("song.wav")]), json!("song")));
    assert_eq!((result["frames"].clone(), result["musicalFrames"].clone(), result["tailFrames"].clone()), (json!(frames), json!(musical), json!(frames - musical)));
    assert_eq!((result["container"].clone(), result["encoding"].clone(), result["sampleRate"].clone(), result["isFloat"].clone()), (json!("wav"), json!("float32"), json!(48_000), json!(true)));
    let progress = progress.into_inner().unwrap();
    assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0) && progress.first().unwrap().1 == "rendering" && progress.last().unwrap() == &(1.0, "delivery".to_string()), "{progress:?}");
    // Only the file is left: the staging folder is gone.
    assert_eq!(std::fs::read_dir(&fixture.folder).unwrap().count(), 1);
    // The live engine was not touched: it is still at the start, stopped.
    assert_eq!(fixture.live.status()["playing"], false);
}

#[test]
fn stems_are_captured_in_one_pass_and_sum_to_the_mix() {
    // Three tracks: a and b in a group bus (at 0.5), c straight to the master; no sends, no
    // master inserts, the master fader at 0.8; b has an insert with latency.
    let mut project = project(3);
    project["masterVolume"] = json!(0.8);
    project["tracks"][0]["volume"] = json!(0.7);
    project["tracks"][1]["pan"] = json!(-0.5);
    project["tracks"][2]["volume"] = json!(0.4);
    project["tracks"][2]["pan"] = json!(0.25);
    project["patterns"][0]["steps"][1][4] = json!({ "note": 64, "instrument": 0, "velocity": 90, "gate": 0 });
    project["patterns"][0]["steps"][2][8] = json!({ "note": 52, "instrument": 0, "velocity": 120, "gate": 0 });
    project["routing"]["buses"] = json!([bus("g", "group", 0.5)]);
    project["routing"]["outputs"] = json!({ "track:a": "bus:g", "track:b": "bus:g" });
    let fixture = Fixture::new("stems", &project);

    let mix = fixture.export(&float32(&fixture.path("mix.wav"), "song"), &CancelToken::new(), &|_, _| {}).unwrap();
    let mut request = float32(&fixture.path("stems"), "stems");
    request.owners = Some(vec!["track:a".to_string(), "track:b".to_string(), "track:c".to_string(), "bus:g".to_string(), "master".to_string()]);
    let result = fixture.export(&request, &CancelToken::new(), &|_, _| {}).unwrap();
    let paths: Vec<String> = result["paths"].as_array().unwrap().iter().map(|path| path.as_str().unwrap().to_string()).collect();
    let names: Vec<String> = paths.iter().map(|path| Path::new(path).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["01-a.wav", "02-b.wav", "03-c.wav", "04-g.wav", "05-Master.wav"]);
    assert_eq!(result["stems"][3]["boundary"], "postFaderPreParent");
    assert_eq!((result["stems"][4]["owner"].clone(), result["stems"][4]["boundary"].clone()), (json!("master"), json!("masterOutput")));
    assert_eq!((result["renderPasses"].clone(), result["frames"].clone()), (json!(1), mix["frames"].clone()));

    let mix = read(fixture.path("mix.wav"));
    let stems: Vec<SamplePcm> = paths.iter().map(read).collect();
    assert!(stems.iter().all(|stem| stem.frames() == mix.frames()) && peak(&mix.left) > 0.1);
    // The master stem is the mix, exactly.
    assert!(stems[4].left == mix.left && stems[4].right == mix.right);
    // The group's stem is its tracks' stems through its fader; the top-level channels (the
    // group and track c) through the master fader are the mix.
    for frame in 0..mix.frames() {
        let group = (stems[0].left[frame] + stems[1].left[frame]) * 0.5;
        assert!((stems[3].left[frame] - group).abs() < 1e-6, "group stem, frame {frame}");
        let (left, right) = ((stems[3].left[frame] + stems[2].left[frame]) * 0.8, (stems[3].right[frame] + stems[2].right[frame]) * 0.8);
        assert!((mix.left[frame] - left).abs() < 1e-6 && (mix.right[frame] - right).abs() < 1e-6, "top-level stems sum to the mix, frame {frame}");
    }
    // Without owners: one stem per track.
    let all = fixture.export(&float32(&fixture.path("tracks"), "stems"), &CancelToken::new(), &|_, _| {}).unwrap();
    assert_eq!(all["paths"].as_array().unwrap().len(), 3);
    let mut bad = float32(&fixture.path("bad"), "stems");
    bad.owners = Some(vec!["track:a".to_string(), "track:zzz".to_string()]);
    assert!(fixture.export(&bad, &CancelToken::new(), &|_, _| {}).unwrap_err().contains("unavailable channel"));
    assert!(!Path::new(&fixture.path("bad")).exists());
}

#[test]
fn stems_stay_aligned_with_the_mix_under_delay_compensation() {
    // Track b has an insert with 200 frames of latency: the mixer delays track a to match, and
    // the output lags by 200 frames. Every exported stream starts at the engine's frame 0.
    let mut project = project(2);
    project["audioPlugins"] = json!([effect("track:b", 0, "lat", "test:latency:200")]);
    project["tracks"][1]["pan"] = json!(1.0);
    project["tracks"][0]["pan"] = json!(-1.0);
    let folder = std::env::temp_dir().join(format!("st-audio-export-pdc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let samples = Arc::new(DecodedSamples::new(1));
    let tone: Vec<f32> = (0..48_000).map(|frame| (frame as f64 * 220.0 * std::f64::consts::TAU / RATE).sin() as f32 * 0.5).collect();
    samples.insert(st_samples::DecodedSample::new(TONE, RATE, tone.clone(), tone, 1));
    let mut options = EngineOptions::offline();
    options.samples = samples.clone();
    options.inserts = Arc::new(st_audio::graph::insert::testing::TestFactory);
    let live = AudioEngine::new(options).unwrap();
    live.publish(&parse(&project)).unwrap();
    let cancel = CancelToken::new();
    let context = RenderContext { live: &live, samples: &samples, cancel: &cancel, progress: &|_, _| {} };
    let mut request = float32(&folder.join("stems").to_string_lossy(), "stems");
    request.owners = Some(vec!["track:a".to_string(), "track:b".to_string(), "master".to_string()]);
    let result = export(&context, &request).unwrap();
    assert_eq!(result["latencyCompensation"]["frames"], 200);
    assert_eq!((result["stems"][0]["arrivalFrames"].clone(), result["stems"][1]["arrivalFrames"].clone(), result["stems"][2]["arrivalFrames"].clone()), (json!(0), json!(200), json!(200)));
    let stems: Vec<SamplePcm> = result["paths"].as_array().unwrap().iter().map(|path| read(path.as_str().unwrap())).collect();
    // a is hard left, b hard right: the master's channels are the two stems, frame for frame,
    // and both start with the note at frame 0.
    let first = stems[0].left.iter().position(|sample| sample.abs() > 1e-4).unwrap();
    assert!(first < 8, "the latency was trimmed: the first audible frame is {first}");
    assert_eq!(stems[1].right.iter().position(|sample| sample.abs() > 1e-4).unwrap(), first);
    for frame in 0..stems[2].frames() {
        assert!((stems[2].left[frame] - stems[0].left[frame]).abs() < 1e-6 && (stems[2].right[frame] - stems[1].right[frame]).abs() < 1e-6, "frame {frame}");
    }
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_cancelled_export_leaves_no_file_and_an_existing_file_untouched() {
    let fixture = Fixture::new("cancel", &demo());
    let existing = fixture.path("keep.wav");
    std::fs::write(&existing, b"not audio, but mine").unwrap();
    for target in ["new.wav", "keep.wav"] {
        let cancel = CancelToken::new();
        let calls = AtomicUsize::new(0);
        let error = fixture
            .export(&float32(&fixture.path(target), "song"), &cancel, &|_, _| {
                // Stop is pressed while the song renders.
                if calls.fetch_add(1, Ordering::Relaxed) == 1 {
                    cancel.cancel();
                }
            })
            .unwrap_err();
        assert_eq!(error, CANCELLED);
    }
    assert!(!Path::new(&fixture.path("new.wav")).exists());
    assert_eq!(std::fs::read(&existing).unwrap(), b"not audio, but mine");
    let left: Vec<String> = std::fs::read_dir(&fixture.folder).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left, ["keep.wav"], "no staging folder, no partial file");
    // Cancelled during delivery too.
    let cancel = CancelToken::new();
    let error = fixture
        .export(&float32(&fixture.path("late.wav"), "song"), &cancel, &|_, phase| {
            if phase == "delivery" {
                cancel.cancel();
            }
        })
        .unwrap_err();
    assert_eq!(error, CANCELLED);
    assert_eq!(std::fs::read_dir(&fixture.folder).unwrap().count(), 1);
    // And the same export completes when nobody cancels; an existing file is replaced.
    fixture.export(&float32(&existing, "song"), &CancelToken::new(), &|_, _| {}).unwrap();
    assert!(read(&existing).frames() > 48_000);
}

#[test]
fn a_pattern_a_track_and_other_formats() {
    let fixture = Fixture::new("modes", &demo());
    let none = CancelToken::new();
    // One pattern, 16-bit at 44.1 kHz.
    let mut request = ExportRequest { path: fixture.path("pattern.wav"), mode: "pattern".to_string(), tail: Some(0.0), pattern: Some(1), ..ExportRequest::default() };
    request.format = serde_json::from_value(json!({ "encoding": "pcm16", "sampleRate": 44_100 })).unwrap();
    let result = fixture.export(&request, &none, &|_, _| {}).unwrap();
    let file = read(fixture.path("pattern.wav"));
    assert_eq!((result["encoding"].clone(), result["sampleRate"].clone(), result["bitDepth"].clone(), result["renderSampleRate"].clone()), (json!("pcm16"), json!(44_100), json!(16), json!(48_000)));
    assert_eq!((file.sample_rate, file.frames() as u64), (44_100.0, result["frames"].as_u64().unwrap()));
    // The demo's patterns are 64 rows at 116 BPM, four lines per beat.
    let seconds = 64.0 * 60.0 / 116.0 / 4.0;
    assert!((file.frames() as f64 / 44_100.0 - seconds).abs() < 0.001, "{} s", file.frames() as f64 / 44_100.0);
    assert!(peak(&file.left) > 0.05);
    request.pattern = Some(99);
    assert!(fixture.export(&request, &none, &|_, _| {}).unwrap_err().contains("pattern does not exist"));

    // A track bounce is always 32-bit float at the render rate, whatever is asked.
    let mut bounce = ExportRequest { path: fixture.path("bounce.wav"), mode: "track".to_string(), track: Some(3), tail: Some(0.5), ..ExportRequest::default() };
    bounce.format = serde_json::from_value(json!({ "encoding": "pcm16", "sampleRate": 44_100 })).unwrap();
    let result = fixture.export(&bounce, &none, &|_, _| {}).unwrap();
    assert_eq!((result["encoding"].clone(), result["sampleRate"].clone(), result["stems"][0]["owner"].clone()), (json!("float32"), json!(48_000), json!(format!("track:{}", demo()["tracks"][3]["routingID"].as_str().unwrap()))));
    assert!(peak(&read(fixture.path("bounce.wav")).left) > 0.01);
    bounce.track = Some(40);
    assert!(fixture.export(&bounce, &none, &|_, _| {}).unwrap_err().contains("bounce track does not exist"));

    let mut invalid = float32(&fixture.path("x.wav"), "song");
    invalid.format = serde_json::from_value(json!({ "encoding": "mp3" })).unwrap();
    assert!(fixture.export(&invalid, &none, &|_, _| {}).unwrap_err().contains("PCM 16-bit"));
    assert!(fixture.export(&float32("", "song"), &none, &|_, _| {}).unwrap_err().contains("destination"));
    assert!(fixture.export(&float32(&fixture.path("x.wav"), "everything"), &none, &|_, _| {}).is_err());
    let mut live_input = demo();
    live_input["tracks"][0]["audioInput"] = json!({ "enabled": true, "deviceUID": "Interface", "channels": [0, 1] });
    fixture.live.publish(&parse(&live_input)).unwrap();
    assert!(fixture.export(&float32(&fixture.path("x.wav"), "song"), &none, &|_, _| {}).unwrap_err().contains("line inputs"));
    // Frozen, the track plays its print and its input is closed: the song can be rendered
    // (here the print's file does not exist, so the track is silent).
    live_input["tracks"][0]["frozen"] = json!({ "id": "f", "path": fixture.path("print.wav"), "sha256": "", "sampleRate": 48000, "frames": 1000, "musicalFrames": 900, "tailFrames": 100, "musicalEndBeat": 4 });
    fixture.live.publish(&parse(&live_input)).unwrap();
    fixture.export(&float32(&fixture.path("frozen-input.wav"), "song"), &none, &|_, _| {}).unwrap();
}

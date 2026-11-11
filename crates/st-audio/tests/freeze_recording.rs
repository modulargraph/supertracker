//! Freeze by recording, in real time on the null device and the null input.
//!
//! The null output device is the clock of the null input (one device, one clock), so what the
//! tests compare is exact: a recorded print is the offline print of the same track, plus what
//! the input delivered.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::export::RenderContext;
use st_audio::freeze::{freeze_track, FreezeMethod, FreezeRequest, CANCELLED};
use st_audio::graph::insert::testing::TestFactory;
use st_audio::inputs::{InputRequest, NULL_TONE_INPUT};
use st_audio::samples::{read_wav, wav_bytes};
use st_audio::{AudioEngine, DecodedSamples, DeviceChoice, EngineOptions, PlayOptions, SamplePcm};
use st_samples::CancelToken;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Frames per row at 240 BPM and four lines per beat.
const FAST_ROW: u64 = 3_000;
/// The song: two patterns of eight rows.
const SONG: u64 = 16 * FAST_ROW;
const PASS: u64 = 8 * FAST_ROW;

struct Fixture {
    folder: PathBuf,
    samples: Arc<DecodedSamples>,
    live: Arc<AudioEngine>,
    sample: String,
}

fn hash(path: &str) -> Option<String> {
    st_project::media::hash_file(Path::new(path), u64::MAX, None).ok()
}

fn request(method: FreezeMethod, tail: Option<f64>, maximum_tail: f64, patterns: bool) -> FreezeRequest {
    FreezeRequest { track: 0, tail, maximum_tail, method, patterns, monitor: true }
}

impl Fixture {
    /// A live engine on the null device, with the test tone as its input.
    fn new(label: &str) -> Fixture {
        let folder = std::env::temp_dir().join(format!("st-audio-freeze-recording-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // A decaying 220 Hz pluck, as a file.
        let pluck: Vec<f32> = (0..12_000).map(|frame| (frame as f64 * 220.0 * std::f64::consts::TAU / RATE).sin() as f32 * 0.5 * (-(frame as f32) / 2_000.0).exp()).collect();
        let sample = folder.join("pluck.wav");
        std::fs::write(&sample, wav_bytes(&pluck, None, 48_000)).unwrap();
        let samples = Arc::new(DecodedSamples::new(2));
        let mut options = EngineOptions::offline();
        options.samples = samples.clone();
        options.inserts = Arc::new(TestFactory);
        options.device = DeviceChoice::Null;
        let live = AudioEngine::new(options).unwrap();
        live.live().inputs().configure(&InputRequest { uid: Some(NULL_TONE_INPUT.to_string()), ..InputRequest::default() }).unwrap();
        Fixture { folder, samples, live, sample: sample.to_string_lossy().into_owned() }
    }

    /// Two tracks, two patterns of eight rows at 240 BPM (a song of one second). Track a plays
    /// the pluck through an insert that is 200 frames late and one that multiplies by 1.5,
    /// with a fader, a pan, a mute and a volume lane that a print must not contain.
    fn project(&self) -> Value {
        let mut project = project(2);
        project["bpm"] = json!(240);
        project["instruments"][0]["samplePath"] = json!(self.sample);
        project["instruments"][0]["mode"] = json!("oneshot");
        project["instruments"][0]["release"] = json!(0.05);
        let rows = |notes: &[(usize, i64)]| -> Value {
            let mut rows = vec![Value::Null; 8];
            for (row, pitch) in notes {
                rows[*row] = json!({ "note": pitch, "instrument": 0, "velocity": 110, "gate": 1 });
            }
            Value::Array(rows)
        };
        project["patterns"] = json!([
            { "id": 0, "length": 8, "steps": [rows(&[(0, 57), (3, 60), (6, 64)]), rows(&[(2, 45)])], "automation": [
                { "id": "v", "track": 0, "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.9 }, { "row": 7, "value": 0.5 }] },
            ] },
            { "id": 1, "length": 8, "steps": [rows(&[(1, 52), (7, 59)]), rows(&[(4, 40)])], "automation": [] },
        ]);
        project["sequence"] = json!([0, 1]);
        project["tracks"][0]["volume"] = json!(0.8);
        project["tracks"][0]["pan"] = json!(-0.3);
        project["tracks"][0]["mute"] = json!(true);
        project["audioPlugins"] = json!([
            effect("track:a", 0, "late", "clap:test.latency.200"),
            { "owner": "track:a", "kind": "effect", "slot": 1, "instanceID": "g", "component": "clap:test.gain", "format": "CLAP", "state": "0.75" },
        ]);
        project
    }

    fn publish(&self, project: &Value) {
        self.samples.load(&self.sample, &CancelToken::new()).unwrap();
        let report = self.live.publish(&parse(project)).unwrap();
        assert!(self.samples.wait_idle(Duration::from_secs(20)));
        let report = if report.problems.is_empty() { report } else { self.live.samples_changed().unwrap() };
        assert!(report.problems.is_empty(), "{:?}", report.problems);
    }

    fn freeze(&self, request: &FreezeRequest, cancel: &CancelToken, progress: &dyn Fn(f64, &str)) -> Result<Value, String> {
        let context = RenderContext { live: &self.live, samples: &self.samples, cancel, progress };
        freeze_track(&context, request, &self.folder.join("Freezes"), &hash)
    }

    fn prints(&self) -> usize {
        std::fs::read_dir(self.folder.join("Freezes")).map_or(0, |files| files.count())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn print(path: &Value) -> SamplePcm {
    read_wav(&std::fs::read(path.as_str().unwrap()).unwrap()).unwrap()
}

fn largest_difference(a: &[f32], b: &[f32]) -> (usize, f32) {
    a.iter().zip(b).enumerate().map(|(index, (x, y))| (index, (x - y).abs())).fold((0, 0.0), |worst, item| if item.1 > worst.1 { item } else { worst })
}

fn assert_same(recorded: &SamplePcm, offline: &SamplePcm, what: &str) {
    assert_eq!(recorded.frames(), offline.frames(), "{what}: frames");
    assert!(peak(&offline.left) > 0.05, "{what}: the print holds sound");
    let (frame, difference) = largest_difference(&recorded.left, &offline.left);
    assert!(difference < 1e-5, "{what}, left: frame {frame} differs by {difference}");
    let (frame, difference) = largest_difference(&recorded.right, &offline.right);
    assert!(difference < 1e-5, "{what}, right: frame {frame} differs by {difference}");
}

/// The sine of `frequency` in `signal`: its amplitude and phase over a whole number of cycles
/// at the start, and the largest difference between `signal` and that sine anywhere.
fn sine_fit(signal: &[f32], frequency: f64) -> (f64, f64) {
    let omega = frequency * std::f64::consts::TAU / RATE;
    // 4 800 frames hold a whole number of cycles of 440 Hz and of 880 Hz.
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
fn a_recorded_print_of_a_software_track_is_its_offline_print() {
    let fixture = Fixture::new("software");
    let project = fixture.project();
    fixture.publish(&project);
    let cancel = CancelToken::new();
    let offline = fixture.freeze(&request(FreezeMethod::Offline, Some(0.25), 10.0, true), &cancel, &|_, _| {}).unwrap();
    let before = (fixture.live.project().unwrap().to_value(), fixture.prints());

    let progress = Mutex::new(Vec::new());
    let recorded = fixture.freeze(&request(FreezeMethod::Record, Some(0.25), 10.0, true), &cancel, &|fraction, phase| progress.lock().unwrap().push((fraction, phase.to_string()))).unwrap();
    let (record, reference) = (&recorded["record"], &offline["record"]);
    assert_eq!((record["method"].clone(), record["tailMode"].clone(), record["sampleRate"].clone()), (json!("recorded"), json!("fixed"), json!(48_000.0)));
    assert_eq!((record["musicalFrames"].clone(), record["tailFrames"].clone(), record["frames"].clone(), record["musicalEndBeat"].clone()), (json!(SONG), json!(12_000), json!(SONG + 12_000), json!(4.0)));
    assert_eq!((record["dormantPluginIDs"].clone(), recorded["releasedPlugins"].clone(), record["unprintedPatterns"].clone(), recorded["notes"].clone()), (json!(["late", "g"]), json!(2), json!([]), json!([])));
    assert_eq!(record["fingerprint"], reference["fingerprint"], "the same source, whichever way it is printed");
    assert_eq!(record["sha256"], json!(hash(record["path"].as_str().unwrap()).unwrap()));
    // The strip was recorded before its fader, with its inserts' delay taken out: the print
    // is the offline print, frame for frame, although the track is muted, panned and faded.
    assert_same(&print(&record["path"]), &print(&reference["path"]), "the song");
    let (prints, references) = (record["patternPrints"].as_array().unwrap(), reference["patternPrints"].as_array().unwrap());
    assert_eq!(prints.iter().map(|print| print["pattern"].as_i64().unwrap()).collect::<Vec<_>>(), [0, 1]);
    for (recorded, offline) in prints.iter().zip(references) {
        let number = recorded["pattern"].as_i64().unwrap();
        assert_eq!((recorded["passFrames"].clone(), recorded["passes"].clone(), recorded["frames"].clone()), (json!(PASS), offline["passes"].clone(), json!(PASS + 12_000)), "pattern {number}");
        assert_same(&print(&recorded["path"]), &print(&offline["path"]), &format!("pattern {number}, head"));
        assert_same(&print(&recorded["loop"]["path"]), &print(&offline["loop"]["path"]), &format!("pattern {number}, loop"));
        assert_eq!(recorded["loop"]["sha256"], json!(hash(recorded["loop"]["path"].as_str().unwrap()).unwrap()));
    }
    // Progress for the page: the song, then the patterns, never backwards, and done.
    let progress = progress.lock().unwrap();
    assert!(progress.iter().any(|(_, phase)| phase == "song") && progress.iter().any(|(_, phase)| phase == "patterns"));
    assert!(progress.windows(2).all(|pair| pair[1].0 >= pair[0].0 - 1e-9), "{progress:?}");
    assert_eq!(progress.last().unwrap(), &(1.0, "done".to_string()));
    // Everything is as it was: the transport stopped, the page's project played again.
    assert_eq!(fixture.live.status()["playing"], false);
    assert_eq!(fixture.live.project().unwrap().to_value(), before.0);
    assert_eq!(fixture.prints(), before.1 + 5);

    // The record is one the engine plays: frozen, the track's inserts are gone.
    let mut frozen = project.clone();
    frozen["tracks"][0]["frozen"] = record.clone();
    fixture.publish(&frozen);
    assert!(fixture.live.inserts().is_empty());

    // An automatic tail ends shortly after the strip fell silent; without pattern prints the
    // patterns the track uses are listed as not printed.
    fixture.publish(&project);
    let auto = fixture.freeze(&request(FreezeMethod::Record, None, 2.0, false), &cancel, &|_, _| {}).unwrap();
    let record = &auto["record"];
    assert_eq!((record["tailMode"].clone(), record["patternPrints"].clone(), record["unprintedPatterns"].clone(), auto["unprintedPatterns"].clone()), (json!("auto"), json!([]), json!([0, 1]), json!([0, 1])));
    let tail = record["tailFrames"].as_u64().unwrap();
    assert!(tail > 0 && tail < 48_000, "the tail ends where the strip is silent, well before the two seconds allowed ({tail} frames)");
    let (short, long) = (print(&record["path"]), print(&reference["path"]));
    let common = short.frames().min(long.frames());
    assert!(largest_difference(&short.left[..common], &long.left[..common]).1 < 1e-5);
    assert!(peak(&long.left[common.min(long.frames())..]) < 10f32.powf(-66.0 / 20.0), "what was cut is below the threshold");
}

#[test]
fn a_hardware_return_is_recorded_in_time_with_the_song() {
    let fixture = Fixture::new("hardware");
    let mut project = fixture.project();
    // The reference: track a without its input, printed offline.
    fixture.publish(&project);
    let cancel = CancelToken::new();
    let offline = fixture.freeze(&request(FreezeMethod::Offline, Some(0.5), 10.0, true), &cancel, &|_, _| {}).unwrap();
    assert!(fixture.freeze(&request(FreezeMethod::Record, Some(0.5), 10.0, true), &cancel, &|_, _| {}).is_ok(), "any track can be recorded");

    // Track a takes the tone in (440 Hz left, 880 Hz right, a quarter of full scale) at half
    // gain, not monitored: a print needs the input in the strip all the same.
    project["tracks"][0]["audioInput"] = json!({ "enabled": true, "deviceUID": NULL_TONE_INPUT, "channels": [0, 1], "gain": 0.5, "monitor": false });
    fixture.publish(&project);
    let row = fixture.live.line_inputs().0[0].clone();
    // On the null devices a return is one block late: the feed's cushion.
    assert_eq!((row["state"].clone(), row["monitoring"].clone(), row["latencyFrames"].clone()), (json!("live"), json!(false), json!(512)));
    assert!(fixture.freeze(&request(FreezeMethod::Offline, Some(0.5), 10.0, true), &cancel, &|_, _| {}).unwrap_err().contains("Choose Freeze by recording"));
    let recorded = fixture.freeze(&request(FreezeMethod::Record, Some(0.5), 10.0, true), &cancel, &|_, _| {}).unwrap();
    let (record, reference) = (&recorded["record"], &offline["record"]);
    assert_eq!((record["method"].clone(), record["frames"].clone(), recorded["notes"].clone()), (json!("recorded"), json!(SONG + 24_000), json!([])));
    // What the print holds beyond the offline print of the notes is the input, through the
    // strip's gain (0.25 x 0.5 x 1.5): one unbroken sine per side from the first frame to the
    // last. Had the notes landed a frame off, the difference would not be a sine.
    let check = |recorded: &Value, offline: &Value, what: &str| {
        let (recorded, offline) = (print(recorded), print(offline));
        assert_eq!(recorded.frames(), offline.frames(), "{what}");
        assert!(peak(&offline.left) > 0.05, "{what}: the notes are in the reference");
        for (side, frequency, recorded, offline) in [("left", 440.0, &recorded.left, &offline.left), ("right", 880.0, &recorded.right, &offline.right)] {
            let difference: Vec<f32> = recorded.iter().zip(offline.iter()).map(|(recorded, offline)| recorded - offline).collect();
            let (amplitude, worst) = sine_fit(&difference, frequency);
            assert!((amplitude - 0.1875).abs() < 1e-4 && worst < 1e-4, "{what}, {side}: a sine of amplitude {amplitude}, off by at most {worst}");
        }
    };
    check(&record["path"], &reference["path"], "the song");
    let (prints, references) = (record["patternPrints"].as_array().unwrap(), reference["patternPrints"].as_array().unwrap());
    assert_eq!(prints.len(), 2);
    for (recorded, offline) in prints.iter().zip(references) {
        assert_eq!(recorded["passes"], offline["passes"]);
        check(&recorded["path"], &offline["path"], "a pattern's head");
        check(&recorded["loop"]["path"], &offline["loop"]["path"], "a pattern's loop");
    }
    // The route is as the page left it, and the input never ran dry or over.
    let row = fixture.live.line_inputs().0[0].clone();
    assert_eq!((row["state"].clone(), row["monitoring"].clone(), row["underruns"].clone(), row["overruns"].clone()), (json!("live"), json!(false), json!(0), json!(0)));
    assert_eq!(fixture.live.project().unwrap().to_value()["tracks"][0]["audioInput"]["monitor"], false);

    // Frozen, the input is closed and the print plays in its place.
    let mut frozen = project.clone();
    frozen["tracks"][0]["frozen"] = record.clone();
    st_project::validate::validate(&st_project::Project::from_value(frozen.clone()).unwrap()).expect("a valid freeze record");
    fixture.publish(&frozen);
    assert_eq!(fixture.live.line_inputs().0[0]["state"], "paused");
    let mut devices = json!({});
    fixture.live.live().decorate_devices(&mut devices);
    assert_eq!(devices["inputRunning"], false);

    // An input that never falls silent gets the longest tail allowed, and a silent one a note.
    fixture.publish(&project);
    let auto = fixture.freeze(&request(FreezeMethod::Record, None, 0.25, false), &cancel, &|_, _| {}).unwrap();
    assert_eq!((auto["record"]["tailMode"].clone(), auto["record"]["tailFrames"].clone()), (json!("auto"), json!(12_000)));
    let mut silent = project.clone();
    silent["patterns"][0]["steps"][0] = json!(vec![Value::Null; 8]);
    silent["patterns"][1]["steps"][0] = json!(vec![Value::Null; 8]);
    silent["tracks"][0]["audioInput"]["gain"] = json!(0);
    fixture.publish(&silent);
    let quiet = fixture.freeze(&request(FreezeMethod::Record, None, 0.25, false), &cancel, &|_, _| {}).unwrap();
    assert!(quiet["notes"][0].as_str().unwrap().contains("is silent"), "{}", quiet["notes"]);
    assert_eq!(quiet["record"]["tailFrames"].as_u64().unwrap(), 0, "nothing to wait for after a silent song");
}

#[test]
fn a_take_that_is_disturbed_fails_and_leaves_everything_as_it_was() {
    let fixture = Fixture::new("failures");
    let mut project = fixture.project();
    project["tracks"][0]["audioInput"] = json!({ "enabled": true, "deviceUID": NULL_TONE_INPUT, "channels": [0], "gain": 1 });
    // A song of four seconds, so that a disturbance has time to arrive before its end.
    project["sequence"] = json!([0, 1, 0, 1, 0, 1, 0, 1]);
    fixture.publish(&project);
    let before = fixture.live.project().unwrap().to_value();
    let record = request(FreezeMethod::Record, Some(0.25), 10.0, true);
    // Runs a take and, once the song plays, `disturbance` on another thread.
    let disturbed = |disturbance: &(dyn Fn(&CancelToken) + Sync)| -> String {
        let cancel = CancelToken::new();
        std::thread::scope(|scope| {
            let take = scope.spawn(|| fixture.freeze(&record, &cancel, &|_, _| {}));
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while fixture.live.status()["playing"] != true && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(200));
            disturbance(&cancel);
            take.join().unwrap().unwrap_err()
        })
    };
    let settled = |what: &str| {
        assert_eq!(fixture.live.status()["playing"], false, "{what}: the transport is stopped");
        assert_eq!(fixture.prints(), 0, "{what}: no print is left behind");
        assert_eq!(fixture.live.line_inputs().0[0]["state"], "live", "{what}");
    };

    // Stop cancels the freeze (the host stops the transport, then cancels what is running).
    assert_eq!(disturbed(&|cancel| { fixture.live.stop(); cancel.cancel(); }), CANCELLED);
    settled("cancelled");
    assert_eq!(fixture.live.project().unwrap().to_value(), before);
    // The transport stops by itself, or jumps.
    let error = disturbed(&|_| fixture.live.stop());
    assert!(error.contains("Playback stopped before the end"), "{error}");
    settled("stopped");
    let error = disturbed(&|_| fixture.live.seek(Some(0), 4, 0));
    assert!(error.contains("Playback jumped"), "{error}");
    settled("jumped");
    assert_eq!(fixture.live.project().unwrap().to_value(), before);
    // Somebody publishes meanwhile: the take ends, and that project stays.
    let mut edited = project.clone();
    edited["tracks"][1]["volume"] = json!(0.3);
    let error = disturbed(&|_| {
        fixture.live.publish(&parse(&edited)).unwrap();
    });
    assert!(error.contains("The project changed"), "{error}");
    settled("published");
    assert_eq!(fixture.live.project().unwrap().to_value()["tracks"][1]["volume"], 0.3);

    // Refused before anything plays: playback running, an input that is not running.
    let cancel = CancelToken::new();
    fixture.live.play(&PlayOptions { song: true, from_start: true, ..PlayOptions::default() });
    std::thread::sleep(Duration::from_millis(100));
    assert!(fixture.freeze(&record, &cancel, &|_, _| {}).unwrap_err().contains("Stop playback"));
    fixture.live.stop();
    std::thread::sleep(Duration::from_millis(100));
    project["tracks"][0]["audioInput"]["deviceUID"] = json!("Interface");
    fixture.publish(&project);
    let refused = fixture.freeze(&record, &cancel, &|_, _| {}).unwrap_err();
    assert!(refused.contains("The live input of a is not running: Interface is not the audio input in use"), "{refused}");
    settled_without_input(&fixture);
}

fn settled_without_input(fixture: &Fixture) {
    assert_eq!(fixture.live.status()["playing"], false);
    assert_eq!(fixture.prints(), 0);
    assert_eq!(fixture.live.line_inputs().0[0]["state"], "paused");
}

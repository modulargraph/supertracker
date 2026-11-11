//! The audio preview of a kit: its group bus in the selected pattern, rendered on a private
//! engine with the kit's built-in effects and its plug-ins in containers (the CLAP test gain of
//! `crates/plugin-host`), written as the file the kit package keeps.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::export::RenderContext;
use st_audio::kit_preview::{render, KitPreview, MAXIMUM_SECONDS, SAMPLE_RATE, TAIL_SECONDS};
use st_audio::plugins::testing::{artifacts, clap_gain, CLAP_GAIN};
use st_audio::plugins::{PluginManager, PluginOptions};
use st_audio::samples::read_wav;
use st_audio::{AudioEngine, DecodedSamples, EngineOptions, PlayOptions};
use st_samples::CancelToken;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct Fixture {
    folder: PathBuf,
    samples: Arc<DecodedSamples>,
    clap: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Fixture {
        let folder = std::env::temp_dir().join(format!("st-audio-kit-preview-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("sockets")).unwrap();
        let clap = artifacts().install(&folder.join("plugins")).clap;
        let samples = Arc::new(DecodedSamples::new(2));
        let tone: Vec<f32> = (0..48_000).map(|frame| (frame as f64 * 220.0 * std::f64::consts::TAU / RATE).sin() as f32 * 0.5).collect();
        samples.insert(st_samples::DecodedSample::new(TONE, RATE, tone.clone(), tone, 1));
        Fixture { folder, samples, clap }
    }

    /// An engine without a device that hosts plug-ins, as the host's live engine does.
    fn engine(&self) -> Arc<AudioEngine> {
        let mut options = EngineOptions::offline();
        options.samples = self.samples.clone();
        let engine = AudioEngine::new(options).unwrap();
        let mut plugins = PluginOptions::default();
        plugins.supervisor.container_path = Some(artifacts().container.clone());
        plugins.supervisor.socket_dir = Some(self.folder.join("sockets"));
        plugins.min_deadline = Duration::from_secs(20);
        engine.set_plugins(PluginManager::new(plugins).unwrap());
        engine
    }

    fn preview(&self, live: &AudioEngine, session: &Value, name: &str) -> Result<KitPreview, String> {
        let context = RenderContext { live, samples: &self.samples, cancel: &CancelToken::new(), progress: &|_, _| {} };
        render(&context, session, "kit", &self.folder.join(name))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// A session of three tracks. Tracks a and b are the kit "kit" with the group bus `kitbus`:
/// a plays the tone on rows 4 and 5 through a built-in distortion, b on row 8, and the bus
/// has a CLAP gain that a lane holds at one half. Track c is not part of the kit and plays
/// from row 0 on.
fn session(fixture: &Fixture) -> Value {
    let mut session = project(3);
    session["groups"] = json!([{ "id": "kit", "name": "Kit", "busID": "kitbus" }]);
    for track in 0..2 {
        session["tracks"][track]["groupID"] = json!("kit");
        session["patterns"][0]["steps"][track] = json!(vec![Value::Null; 16]);
    }
    session["patterns"][0]["steps"][0][4] = json!({ "note": 57, "instrument": 0, "velocity": 100, "gate": 2 });
    session["patterns"][0]["steps"][1][8] = json!({ "note": 64, "instrument": 0, "velocity": 100, "gate": 1 });
    let mut group_bus = bus("kitbus", "group", 1.0);
    group_bus["groupID"] = json!("kit");
    session["routing"]["buses"] = json!([group_bus]);
    session["routing"]["outputs"] = json!({ "track:a": "bus:kitbus", "track:b": "bus:kitbus" });
    session["audioPlugins"] = json!([
        effect("track:a", 0, "drive", "stfx:distortion"),
        { "owner": "bus:kitbus", "kind": "effect", "slot": 0, "instanceID": "halve", "component": format!("clap:{CLAP_GAIN}"), "format": "CLAP", "bundlePath": fixture.clap, "name": "ST Test Gain" },
    ]);
    session["patterns"][0]["automation"] = json!([{ "id": "half", "owner": "bus:kitbus", "target": format!("plugin:effect:0:{}", clap_gain::GAIN), "interpolation": "step", "points": [{ "row": 0, "value": 0.25 }] }]);
    session
}

/// The `fmt ` chunk of a WAV file: (format tag, channels, sample rate, bits per sample).
fn wav_format(bytes: &[u8]) -> (u16, u16, u32, u16) {
    let at = bytes.windows(4).position(|window| window == b"fmt ").expect("a format chunk") + 8;
    let word = |offset: usize| u16::from_le_bytes([bytes[at + offset], bytes[at + offset + 1]]);
    (word(0), word(2), u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]), word(14))
}

#[test]
fn a_kit_is_previewed_as_its_group_bus_in_the_selected_pattern() {
    let fixture = Fixture::new("bus");
    let session = session(&fixture);
    let live = fixture.engine();
    live.publish(&parse(&session)).unwrap();
    let preview = fixture.preview(&live, &session, "Preview.wav").unwrap();

    // The pattern is 16 rows, two seconds; one second of tail follows.
    let (musical, frames) = (16 * ROW, 16 * ROW + (TAIL_SECONDS * RATE) as usize);
    assert_eq!(preview, KitPreview { path: fixture.folder.join("Preview.wav").to_string_lossy().into_owned(), owner: "bus:kitbus".into(), pattern: 0, frames: frames as u64, musical_frames: musical as u64, sample_rate: SAMPLE_RATE, warning: None });
    let bytes = std::fs::read(fixture.folder.join("Preview.wav")).expect("the preview file exists");
    assert_eq!(wav_format(&bytes), (1, 2, 48_000, 16), "16-bit PCM, stereo, 48 kHz");
    let file = read_wav(&bytes).unwrap();
    assert_eq!((file.sample_rate, file.frames()), (48_000.0, frames));

    // Audio where the kit sounds, and only there: track c (not in the kit) plays from row 0.
    let quiet = 4.0 / 32_768.0; // dither
    assert!(peak(&file.left[..4 * ROW]) <= quiet, "nothing of the kit before row 4 (peak {})", peak(&file.left[..4 * ROW]));
    assert!(peak(&file.left[4 * ROW..6 * ROW]) > 0.02 && peak(&file.right[4 * ROW..6 * ROW]) > 0.02, "pad a on rows 4 and 5");
    assert!(peak(&file.left[6 * ROW + 2_400..8 * ROW]) <= quiet, "silence between the pads");
    assert!(peak(&file.left[8 * ROW..9 * ROW]) > 0.02, "pad b on row 8");
    assert!(peak(&file.left[9 * ROW + 2_400..]) <= quiet, "silence after the last pad, tail included");

    // It is the group bus as it plays: the same session with everything else muted, heard at
    // the master (the bus goes there at unity), with the distortion and the plug-in.
    let mut alone = session.clone();
    alone["tracks"][2]["mute"] = json!(true);
    let reference = fixture.engine();
    reference.publish(&parse(&alone)).unwrap();
    reference.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    let (mut left, mut right) = reference.render_offline(musical, 512);
    reference.stop();
    let (tail_left, tail_right) = reference.render_offline(frames - musical, 512);
    left.extend(tail_left);
    right.extend(tail_right);
    assert!(peak(&left) > 0.02);
    let step = 1.6 / 32_768.0; // half a step of rounding and the dither
    assert_signal(&file.left, |index| left[index], step, "the left channel is the bus");
    assert_signal(&file.right, |index| right[index], step, "the right channel is the bus");
    // The plug-in on the bus was part of it: without the lane's half the kit is twice as loud.
    let mut full = alone.clone();
    full["patterns"][0]["automation"] = json!([]);
    reference.publish(&parse(&full)).unwrap();
    reference.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    let (loud, _) = reference.render_offline(musical, 512);
    assert!((peak(&loud) - 2.0 * peak(&left)).abs() < 1e-4, "{} against {}", peak(&loud), peak(&left));

    // Nothing but the file is left, and the live engine was not touched.
    let left_over: Vec<String> = std::fs::read_dir(&fixture.folder).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).filter(|name| name != "sockets" && name != "plugins").collect();
    assert_eq!(left_over, ["Preview.wav"]);
    assert_eq!(live.status()["playing"], false);
    // Saved again over the same file.
    assert!(fixture.preview(&live, &session, "Preview.wav").is_ok());
}

#[test]
fn a_long_pattern_is_cut_at_ten_seconds_or_eight_bars() {
    let fixture = Fixture::new("length");
    let mut session = project(1);
    session["groups"] = json!([{ "id": "kit", "name": "Kit", "busID": "kitbus" }]);
    session["tracks"][0]["groupID"] = json!("kit");
    let mut group_bus = bus("kitbus", "group", 1.0);
    group_bus["groupID"] = json!("kit");
    session["routing"]["buses"] = json!([group_bus]);
    session["routing"]["outputs"] = json!({ "track:a": "bus:kitbus" });
    // 128 rows at four lines per beat: 16 seconds at 120 BPM, 8 seconds at 240.
    let mut rows = vec![Value::Null; 128];
    rows[0] = json!({ "note": 57, "instrument": 0, "velocity": 100, "gate": 0 });
    session["patterns"][0] = json!({ "id": 0, "length": 128, "steps": [rows], "automation": [] });
    let live = fixture.engine();
    live.publish(&parse(&session)).unwrap();
    let tail = (TAIL_SECONDS * RATE) as u64;
    // 120 BPM: eight bars are 16 seconds, so the ten seconds apply.
    let preview = fixture.preview(&live, &session, "Ten.wav").unwrap();
    assert_eq!((preview.musical_frames, preview.frames), ((MAXIMUM_SECONDS * RATE) as u64, (MAXIMUM_SECONDS * RATE) as u64 + tail));
    let file = read_wav(&std::fs::read(fixture.folder.join("Ten.wav")).unwrap()).unwrap();
    assert_eq!(file.frames() as u64, preview.frames);
    let end = preview.musical_frames as usize;
    assert!(peak(&file.left[end - 4_800..end]) > 0.02, "the kit plays up to the cut");
    assert!(peak(&file.left[end + 4_800..]) < 1e-3, "and is released there: the tail rings out");
    // 180 BPM in three-four time: eight bars are 24 beats, 8 seconds.
    session["bpm"] = json!(180);
    session["timeSignature"] = json!({ "numerator": 3, "denominator": 4 });
    live.publish(&parse(&session)).unwrap();
    let preview = fixture.preview(&live, &session, "Bars.wav").unwrap();
    assert_eq!((preview.musical_frames, preview.frames), (8 * 48_000, 8 * 48_000 + tail));
}

#[test]
fn a_kit_that_plays_nothing_audible_gets_no_preview_and_says_why() {
    let fixture = Fixture::new("refused");
    let mut session = session(&fixture);
    let live = fixture.engine();
    live.publish(&parse(&session)).unwrap();
    let render = |session: &Value| fixture.preview(&live, session, "Preview.wav");
    let context = RenderContext { live: &live, samples: &fixture.samples, cancel: &CancelToken::new(), progress: &|_, _| {} };
    assert!(st_audio::kit_preview::render(&context, &session, "other", &fixture.folder.join("Preview.wav")).unwrap_err().contains("existing kit"));

    // Pad b becomes an external MIDI instrument: the preview is made without it and says so.
    session["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "midi", "name": "Hardware", "midiChannel": 1 }));
    session["tracks"][1]["instrument"] = json!(1);
    session["patterns"][0]["steps"][1][8]["instrument"] = json!(1);
    let preview = render(&session).unwrap();
    assert!(preview.warning.as_deref().is_some_and(|warning| warning.contains("external MIDI")), "{preview:?}");
    let file = read_wav(&std::fs::read(fixture.folder.join("Preview.wav")).unwrap()).unwrap();
    assert!(peak(&file.left[4 * ROW..6 * ROW]) > 0.02 && peak(&file.left[8 * ROW..9 * ROW]) < 1e-3, "pad a without pad b");
    std::fs::remove_file(fixture.folder.join("Preview.wav")).unwrap();

    // Only the external pad plays; then no pad at all; then a pattern that does not exist.
    session["patterns"][0]["steps"][0] = json!(vec![Value::Null; 16]);
    assert!(render(&session).unwrap_err().contains("external MIDI instruments only"));
    session["patterns"][0]["steps"][1] = json!(vec![Value::Null; 16]);
    assert!(render(&session).unwrap_err().contains("plays no note"));
    session["activePattern"] = json!(7);
    assert!(render(&session).unwrap_err().contains("pattern does not exist"));
    assert!(!fixture.folder.join("Preview.wav").exists(), "a refused preview writes nothing");
    // A kit without a group bus has nothing to capture.
    session["activePattern"] = json!(0);
    session["groups"][0].as_object_mut().unwrap().remove("busID");
    assert!(render(&session).unwrap_err().contains("no group bus"));
}

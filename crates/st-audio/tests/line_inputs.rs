//! Live line inputs in the mixer graph, and the tap on a strip.
//!
//! The tests are the input thread and the audio thread at once: the engine has no device, the
//! input is the manual one (`null:manual`, eight channels), and each step delivers a block of
//! input and then renders a block, as an audio interface with one clock does.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::graph::insert::testing::TestFactory;
use st_audio::graph::tap::{StripTap, TapBlock, NOT_PLAYING};
use st_audio::inputs::{InputRequest, NULL_MANUAL_INPUT};
use st_audio::{AudioEngine, EngineOptions, PlayOptions};
use std::sync::Arc;

/// Frames per block of the tests' device, and with it the cushion of every line feed.
const BLOCK: usize = 64;

/// What the input device captures: every frame of every channel is its own value.
fn input(frame: usize, channel: usize) -> f32 {
    ((frame * (channel + 3)) % 997) as f32 / 997.0 - 0.5
}

struct Rig {
    engine: Arc<AudioEngine>,
    /// Frames delivered and rendered so far.
    time: usize,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Rig {
    fn new() -> Rig {
        let mut options = EngineOptions::offline();
        options.samples = samples();
        options.inserts = Arc::new(TestFactory);
        options.buffer_size = BLOCK as u32;
        let engine = AudioEngine::new(options).unwrap();
        engine.live().inputs().configure(&InputRequest { uid: Some(NULL_MANUAL_INPUT.to_string()), ..InputRequest::default() }).unwrap();
        Rig { engine, time: 0, left: Vec::new(), right: Vec::new() }
    }

    /// Delivers and renders `blocks` blocks; the output is appended to `left` and `right`.
    fn run(&mut self, blocks: usize) {
        for _ in 0..blocks {
            let base = self.time;
            self.engine.live().inputs().hub().deliver(BLOCK, 8, |frame, channel| input(base + frame, channel));
            let (left, right) = self.engine.render_offline(BLOCK, BLOCK);
            self.left.extend(left);
            self.right.extend(right);
            self.time += BLOCK;
        }
    }
}

/// `project(tracks)` without its notes: nothing sounds but what comes in.
fn silent(tracks: usize) -> Value {
    let mut project = project(tracks);
    for rows in project["patterns"][0]["steps"].as_array_mut().unwrap() {
        *rows = json!(vec![Value::Null; 16]);
    }
    project
}

fn line(channels: &[u32], gain: f64) -> Value {
    json!({ "enabled": true, "deviceUID": NULL_MANUAL_INPUT, "channels": channels, "gain": gain })
}

fn row(engine: &AudioEngine, track: usize) -> Value {
    engine.line_inputs().0.into_iter().find(|row| row["track"] == json!(track)).unwrap_or(Value::Null)
}

#[test]
fn a_line_input_runs_through_its_tracks_inserts_fader_and_meter() {
    let mut rig = Rig::new();
    let mut project = silent(3);
    // Track a: channels 2 and 3 at half gain, through an insert that multiplies by 1.5, a
    // fader at 0.8 and a pan to the left. Track b: channel 5, mono. Track c: nothing.
    project["tracks"][0]["audioInput"] = line(&[2, 3], 0.5);
    project["tracks"][0]["volume"] = json!(0.8);
    project["tracks"][0]["pan"] = json!(-0.5);
    project["tracks"][1]["audioInput"] = line(&[5], 1.0);
    project["tracks"][1]["mute"] = json!(true);
    project["audioPlugins"] = json!([{ "owner": "track:a", "kind": "effect", "slot": 0, "instanceID": "g", "component": "clap:test.gain", "format": "CLAP", "state": "0.75" }]);
    rig.engine.publish(&parse(&project)).unwrap();
    assert_eq!((row(&rig.engine, 0)["state"].clone(), row(&rig.engine, 0)["monitoring"].clone(), row(&rig.engine, 0)["latencyFrames"].clone()), (json!("live"), json!(true), json!(BLOCK)));
    assert_eq!(row(&rig.engine, 2), Value::Null, "a track without an input has no row");
    rig.run(40);
    // The first block is silence (the feed waits for its cushion); from then on input frame n
    // is heard at frame n + BLOCK, the latency the mixer reports for it.
    assert_eq!(rig.engine.status()["latencyCompensation"]["frames"], json!(BLOCK));
    assert!(peak(&rig.left[..BLOCK]) == 0.0 && peak(&rig.right[..BLOCK]) == 0.0);
    assert_signal(&rig.left[BLOCK..], |index| input(index, 2) * 0.5 * 1.5 * 0.8, 1e-6, "left: channel 2 through gain, insert and fader");
    assert_signal(&rig.right[BLOCK..], |index| input(index, 3) * 0.5 * 1.5 * 0.8 * 0.5, 1e-6, "right: channel 3, panned away from");
    // The strips' meters see the inputs: track a after its fader, the muted track b not.
    rig.engine.tick();
    let peaks = rig.engine.status()["trackPeaks"].clone();
    assert!(peaks[0].as_f64().unwrap() > 0.2 && peaks[1].as_f64().unwrap() == 0.0 && peaks[2].as_f64().unwrap() == 0.0, "{peaks}");

    // Unmuted, the mono input is on both sides; an input that is not monitored stays routed
    // and is silent in its strip.
    project["tracks"][1]["mute"] = json!(false);
    project["tracks"][0]["audioInput"]["monitor"] = json!(false);
    rig.engine.publish(&parse(&project)).unwrap();
    assert_eq!((row(&rig.engine, 0)["state"].clone(), row(&rig.engine, 0)["monitoring"].clone()), (json!("live"), json!(false)));
    let from = rig.left.len();
    rig.run(20);
    // The feeds were kept across the publication: no gap, no cushion to wait for again.
    assert_signal(&rig.left[from..], |index| input(from - BLOCK + index, 5), 1e-6, "left: channel 5");
    assert_signal(&rig.right[from..], |index| input(from - BLOCK + index, 5), 1e-6, "right: channel 5 again");
    let lines = rig.engine.line_inputs().0;
    assert!(lines.iter().all(|row| row["underruns"] == 0 && row["overruns"] == 0), "{lines:?}");

    // A route of another device is paused with its reason, and so is one whose channel the
    // device does not have; the remaining route keeps running.
    project["tracks"][0]["audioInput"] = json!({ "enabled": true, "deviceUID": "Interface", "channels": [0, 1], "gain": 1 });
    project["tracks"][2]["audioInput"] = line(&[8], 1.0);
    rig.engine.publish(&parse(&project)).unwrap();
    let (rows, problem) = rig.engine.line_inputs();
    assert_eq!((rows[0]["state"].clone(), rows[0]["active"].clone(), rows[1]["state"].clone(), rows[2]["state"].clone()), (json!("paused"), json!(false), json!("live"), json!("paused")));
    assert_eq!(problem.as_deref(), Some("Interface is not the audio input in use. Its input is paused until it is."));
    assert_eq!(rows[2]["status"], "null:manual has 8 input channels: input 9 does not exist.");
    let from = rig.left.len();
    rig.run(10);
    assert_signal(&rig.left[from..], |index| input(from - BLOCK + index, 5), 1e-6, "channel 5 goes on");

    // A switched-off route is "off"; with no route left the input closes and the mixer has no
    // latency left to compensate.
    project["tracks"][0]["audioInput"]["enabled"] = json!(false);
    project["tracks"][1]["audioInput"]["enabled"] = json!(false);
    project["tracks"][2].as_object_mut().unwrap().remove("audioInput");
    rig.engine.publish(&parse(&project)).unwrap();
    let (rows, problem) = rig.engine.line_inputs();
    assert_eq!((rows.len(), rows[0]["state"].clone(), rows[1]["state"].clone(), problem), (2, json!("off"), json!("off"), None));
    let mut devices = json!({});
    rig.engine.live().decorate_devices(&mut devices);
    assert_eq!(devices["inputRunning"], false);
    let from = rig.left.len();
    rig.run(4);
    assert!(peak(&rig.left[from..]) == 0.0);
    assert_eq!(rig.engine.status()["latencyCompensation"]["frames"], 0);
}

#[test]
fn the_mixer_compensates_for_a_line_inputs_latency_and_its_inserts() {
    // Tracks a and b play the tone. The reference is what they sound like by themselves.
    let (reference, _) = render_with(&project(2), 40 * BLOCK, BLOCK);
    assert!(peak(&reference) > 0.5);

    let mut rig = Rig::new();
    let mut project = project(2);
    // Track a also takes channel 1 in, and has an insert that is 200 frames late.
    project["tracks"][0]["audioInput"] = line(&[1], 1.0);
    project["audioPlugins"] = json!([effect("track:a", 0, "late", "clap:test.latency.200")]);
    rig.engine.publish(&parse(&project)).unwrap();
    rig.engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    rig.run(40);
    let latency = BLOCK + 200;
    assert_eq!(rig.engine.status()["latencyCompensation"]["frames"], json!(latency));
    // Everything arrives together: the engine's two tracks as they are without the input, and
    // the input as it was delivered when the engine rendered the same frame.
    assert!(peak(&rig.left[..latency]) == 0.0);
    assert_signal(&rig.left[latency..], |index| reference[index] + input(index, 1), 1e-5, "the tone and the input, aligned");

    // The same with the input on a track of its own: the other tracks wait for it.
    let mut rig = Rig::new();
    let mut project = common::project(3);
    project["patterns"][0]["steps"][2] = json!(vec![Value::Null; 16]);
    project["tracks"][2]["audioInput"] = line(&[6, 7], 0.25);
    rig.engine.publish(&parse(&project)).unwrap();
    rig.engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    rig.run(40);
    assert_signal(&rig.left[BLOCK..], |index| reference[index] + input(index, 6) * 0.25, 1e-5, "left");
    assert_signal(&rig.right[BLOCK..], |index| reference[index] + input(index, 7) * 0.25, 1e-5, "right");
}

#[test]
fn a_frozen_tracks_input_is_closed() {
    let mut rig = Rig::new();
    let mut project = silent(2);
    project["tracks"][0]["audioInput"] = line(&[0, 1], 1.0);
    rig.engine.publish(&parse(&project)).unwrap();
    rig.run(4);
    assert!(peak(&rig.left) > 0.2);
    // A print (any file name: it is not found, which is a sample problem, not an error).
    project["tracks"][0]["frozen"] = json!({ "id": "f", "path": "/nowhere/print.wav", "sha256": "", "sampleRate": 48000, "frames": 1000, "musicalFrames": 900, "tailFrames": 100, "musicalEndBeat": 4 });
    rig.engine.publish(&parse(&project)).unwrap();
    let frozen = row(&rig.engine, 0);
    assert_eq!((frozen["state"].clone(), frozen["status"].clone()), (json!("paused"), json!("The track is frozen: its input is closed while it plays its print.")));
    let mut devices = json!({});
    rig.engine.live().decorate_devices(&mut devices);
    assert_eq!(devices["inputRunning"], false, "nothing else uses the input");
    let from = rig.left.len();
    rig.run(4);
    assert!(peak(&rig.left[from..]) == 0.0);
    assert_eq!(rig.engine.status()["latencyCompensation"]["frames"], 0, "the return's latency left the plan");
}

#[test]
fn a_tap_carries_the_strip_before_its_fader_with_each_blocks_place_in_the_song() {
    let mut rig = Rig::new();
    let mut project = project(2);
    project["tracks"][1]["mute"] = json!(true);
    project["tracks"][0]["volume"] = json!(0.5);
    project["tracks"][0]["audioInput"] = line(&[4], 1.0);
    project["audioPlugins"] = json!([effect("track:a", 0, "late", "clap:test.latency.100")]);
    rig.engine.publish(&parse(&project)).unwrap();
    assert!(rig.engine.set_strip_tap(Some((7, Arc::new(StripTap::new(48_000))))).unwrap_err().contains("no mixer strip"));
    let tap = Arc::new(StripTap::new(48_000));
    rig.engine.set_strip_tap(Some((0, tap.clone()))).unwrap();
    rig.run(8);
    rig.engine.seek(Some(0), 0, 0);
    rig.engine.play(&PlayOptions { song: true, from_start: false, ..PlayOptions::default() });
    rig.run(40);
    assert!(rig.engine.plan_adopted());

    let (mut left, mut right, mut blocks) = (Vec::new(), Vec::new(), Vec::<TapBlock>::new());
    while let Some(block) = tap.pop(&mut left, &mut right) {
        blocks.push(block);
    }
    assert_eq!(blocks.len(), 48);
    // Graph time runs on from block to block; the strip lags the engine by its input's
    // latency and its insert's; the song starts with the ninth block and runs with the graph.
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!((block.time, block.frames, block.arrival), ((index * BLOCK) as u64, BLOCK as u32, (BLOCK + 100) as u32), "block {index}");
        assert_eq!(block.song_frame, if index < 8 { NOT_PLAYING } else { ((index - 8) * BLOCK) as i64 }, "block {index}");
    }
    // What the tap holds is the strip before the fader: the master (only this track is heard,
    // at half volume) is half of it.
    assert_eq!(left.len(), 48 * BLOCK);
    assert!(peak(&left[10 * BLOCK..]) > 0.5);
    assert_signal(&rig.left, |index| left[index] * 0.5, 1e-6, "the master is the tapped strip after its fader");
    assert_eq!(left, right);
    assert_eq!(tap.dropped(), 0);

    // Muted, the strip still plays into its tap; without the tap nothing is captured.
    project["tracks"][0]["mute"] = json!(true);
    rig.engine.publish(&parse(&project)).unwrap();
    let from = rig.left.len();
    rig.run(4);
    left.clear();
    right.clear();
    assert!(tap.pop(&mut left, &mut right).is_some() && peak(&left) > 0.1 && peak(&rig.left[from..]) == 0.0);
    rig.engine.set_strip_tap(None).unwrap();
    tap.clear();
    rig.run(4);
    assert!(tap.pop(&mut left, &mut right).is_none());
}

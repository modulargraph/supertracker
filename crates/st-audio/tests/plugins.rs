//! Plug-ins in the mixer graph, with the real container process and the CLAP and VST3 test
//! plug-ins of `crates/plugin-host` (built on demand, see `st_audio::plugins::testing`).
//!
//! Renders are offline and compared frame by frame against what the test plug-ins are known
//! to compute: the gain effects multiply each sample by their factor from the frame a
//! parameter change lands on, the sine instruments produce `sine_sample` from the frame of a
//! note-on.

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::plugins::testing::{artifacts, clap_gain, clap_sine, vst3_gain, vst3_sine, Installed, CLAP_GAIN, CLAP_NOTE_SINE, CLAP_SINE, VST3_GAIN, VST3_SINE};
use st_audio::plugins::{Catalog, CatalogOptions, PluginEvent, PluginManager, PluginOptions};
use st_audio::{AudioEngine, PlayOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FRAMES: usize = 30_000;

/// A scratch folder with the test plug-ins installed, removed when dropped.
struct Fixture {
    folder: PathBuf,
    installed: Installed,
}

impl Fixture {
    fn new(label: &str) -> Fixture {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let folder = std::env::temp_dir().join(format!("st-plug-{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("sockets")).unwrap();
        let installed = artifacts().install(&folder.join("plugins"));
        Fixture { folder, installed }
    }

    fn options(&self) -> PluginOptions {
        let mut options = PluginOptions::default();
        options.supervisor.container_path = Some(artifacts().container.clone());
        options.supervisor.socket_dir = Some(self.folder.join("sockets"));
        // Functional tests must not depend on the scheduler: no block is ever given up.
        options.min_deadline = Duration::from_secs(20);
        options
    }

    fn manager(&self) -> Arc<PluginManager> {
        PluginManager::new(self.options()).expect("a plug-in manager")
    }

    /// An offline engine (test samples, test inserts) with a plug-in manager attached.
    fn engine(&self) -> (Arc<AudioEngine>, Arc<PluginManager>) {
        let engine = engine();
        let manager = self.manager();
        engine.set_plugins(manager.clone());
        (engine, manager)
    }

    fn clap(&self, owner: &str, kind: &str, slot: usize, instance: &str, id: &str) -> Value {
        json!({ "owner": owner, "kind": kind, "slot": slot, "instanceID": instance, "component": format!("clap:{id}"), "format": "CLAP", "bundlePath": self.installed.clap, "name": "ST Test (CLAP)", "manufacturer": "SuperTracker Tests" })
    }

    fn vst3(&self, owner: &str, kind: &str, slot: usize, instance: &str, id: &str) -> Value {
        json!({ "owner": owner, "kind": kind, "slot": slot, "instanceID": instance, "component": format!("vst3:{id}"), "format": "VST3", "bundlePath": self.installed.vst3, "name": "ST Test (VST3)", "manufacturer": "SuperTracker Tests" })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn play(engine: &AudioEngine, project: &Value, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    engine.publish(&parse(project)).expect("the project is accepted");
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    engine.render_offline(frames, block)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn assert_bits(actual: &[f32], expected: impl Fn(usize) -> f32, what: &str) {
    for (index, sample) in actual.iter().enumerate() {
        let wanted = expected(index);
        assert!(sample.to_bits() == wanted.to_bits() || (*sample == 0.0 && wanted == 0.0), "{what}: frame {index} is {sample}, expected {wanted}");
    }
}

/// The sample the CLAP and VST3 test instruments produce `k` frames after a note-on.
fn sine_sample(note: u8, k: u64, level: f64) -> f32 {
    let frequency = 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
    (level * (std::f64::consts::TAU * frequency * k as f64 / RATE).sin()) as f32
}

fn reference() -> Vec<f32> {
    render(&project(1), FRAMES).0
}

fn inserts(engine: &AudioEngine) -> Vec<Value> {
    engine.routing_status()["inserts"].as_array().cloned().unwrap_or_default()
}

#[test]
fn an_effect_processes_the_strip_bit_exactly_and_its_latency_is_compensated_when_it_changes() {
    let fixture = Fixture::new("effect");
    let reference = reference();
    for (format, block) in [("clap", 512), ("vst3", 333)] {
        let (engine, manager) = fixture.engine();
        let mut project = project(2);
        project["audioPlugins"] = json!([if format == "clap" { fixture.clap("track:a", "effect", 0, "fx", CLAP_GAIN) } else { fixture.vst3("track:a", "effect", 0, "fx", VST3_GAIN) }]);
        engine.publish(&parse(&project)).unwrap();
        let hosted = manager.hosted("fx").expect("the insert has an instance");
        let status = inserts(&engine);
        assert_eq!((status[0]["available"].clone(), status[0]["instanceID"].clone(), status[0]["latencyFrames"].clone()), (json!(true), json!("fx"), json!(0)), "{format}");
        // Half gain: 1.0 of 0..2 in CLAP's own range is 0.5; VST3 is normalized (0.25 of 0..2).
        let (gain, value) = if format == "clap" { (clap_gain::GAIN, 0.5) } else { (vst3_gain::GAIN, 0.25) };
        hosted.set_parameter(gain, value).unwrap();
        // The plug-in halves track a; track b is untouched: bit for bit.
        let (left, right) = play(&engine, &project, FRAMES, block);
        assert_bits(&left, |i| reference[i] * 0.5 + reference[i], &format!("{format}: a through the plug-in plus b"));
        assert_eq!(left, right);

        // The plug-in now reports 64 frames of latency (it does not delay itself): the mixer
        // plans again and holds track b back by 64 frames, so the paths stay aligned for a
        // plug-in that does delay.
        let (latency, value) = if format == "clap" { (clap_gain::LATENCY, 64.0) } else { (vst3_gain::LATENCY, 64.0 / 1024.0) };
        hosted.set_parameter(latency, value).unwrap();
        // The request reaches the plug-in with a block; the restart follows on the manager's thread.
        wait_until("the new latency in the mixer", || {
            engine.render_offline(256, 256);
            inserts(&engine)[0]["latencyFrames"] == 64
        });
        engine.render_offline(256, 256);
        assert_eq!(engine.routing_status()["latencyFrames"], 64, "the plan in use lags by the plug-in's latency");
        let (left, _) = play(&engine, &project, FRAMES, block);
        assert_bits(&left[64..], |i| reference[i + 64] * 0.5 + reference[i], &format!("{format}: b compensated by 64 frames"));
        // A bypassed insert passes its signal on, still 64 frames late like the planner assumed.
        project["audioPlugins"][0]["bypass"] = json!(true);
        let (left, _) = play(&engine, &project, FRAMES, block);
        assert_bits(&left[64..], |i| reference[i] + reference[i], &format!("{format}: bypassed, both paths 64 frames late"));
    }
}

#[test]
fn automation_lanes_and_step_locks_reach_plug_in_parameters_on_their_frame() {
    let fixture = Fixture::new("automation");
    let reference = reference();
    let (engine, _manager) = fixture.engine();
    let mut project = project(1);
    project["audioPlugins"] = json!([fixture.clap("track:a", "effect", 0, "fx", CLAP_GAIN)]);
    // A stepped lane on the gain (address 1): 0.5 is unity, 0.25 half, 1.0 double.
    project["patterns"][0]["automation"] = json!([{ "id": "lane", "owner": "track:a", "target": format!("plugin:effect:0:{}", clap_gain::GAIN), "interpolation": "step",
        "points": [{ "row": 0, "value": 0.5 }, { "row": 1, "value": 0.25 }, { "row": 3, "value": 1.0 }] }]);
    for block in [512, 97, 4096] {
        let (left, _) = play(&engine, &project, FRAMES, block);
        assert_bits(&left, |i| reference[i] * if i < ROW { 1.0 } else if i < 3 * ROW { 0.5 } else { 2.0 }, &format!("stepped lane, blocks of {block}"));
    }
    // A step lock on row 2 holds its value for that row and the lane takes over again after it.
    project["patterns"][0]["automation"] = json!([]);
    project["patterns"][0]["steps"][0][2] = json!({ "fx1": { "type": "plugin", "instanceID": "fx", "address": clap_gain::GAIN.to_string(), "value": 0.25 } });
    let (left, _) = play(&engine, &project, FRAMES, 512);
    assert_bits(&left[..2 * ROW], |i| reference[i], "before the lock");
    assert_bits(&left[2 * ROW..3 * ROW], |i| reference[i + 2 * ROW] * 0.5, "the locked row");
    // A linear lane arrives as the engine's 32-frame staircase, each step on its frame.
    project["patterns"][0]["steps"][0][2] = Value::Null;
    project["patterns"][0]["automation"] = json!([{ "id": "ramp", "owner": "track:a", "target": format!("plugin:effect:0:{}", clap_gain::GAIN), "interpolation": "linear",
        "points": [{ "row": 0, "value": 0.0 }, { "row": 4, "value": 0.5 }] }]);
    let (left, _) = play(&engine, &project, FRAMES, 512);
    let mut steps = 0;
    for start in (0..4 * ROW).step_by(32) {
        let gain = if reference[start + 16].abs() > 1e-4 { left[start + 16] / reference[start + 16] } else { continue };
        let expected = (start as f32 / (4 * ROW) as f32) * 1.0;
        assert!((gain - expected).abs() < 0.01, "ramp at frame {start}: gain {gain}, expected about {expected}");
        steps += 1;
    }
    assert!(steps > 500);
}

#[test]
fn a_plug_in_instrument_plays_the_notes_of_its_track_at_their_frames() {
    let fixture = Fixture::new("instrument");
    for format in ["clap", "vst3"] {
        let (engine, manager) = fixture.engine();
        let mut project = project(2);
        // Track a plays instrument 1, a plug-in; track b keeps the sampled tone.
        project["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "au", "name": "Sine", "volume": 0.5, "pan": 0 }));
        project["tracks"][0]["instrument"] = json!(1);
        project["tracks"][1]["mute"] = json!(true);
        project["audioPlugins"] = json!([if format == "clap" { fixture.clap("track:a", "instrument", 0, "synth", CLAP_SINE) } else { fixture.vst3("track:a", "instrument", 0, "synth", VST3_SINE) }]);
        let mut rows = vec![Value::Null; 16];
        rows[1] = json!({ "note": 60, "instrument": 1, "velocity": 100, "gate": 1 });
        rows[3] = json!({ "note": 67, "instrument": 1, "velocity": 100, "gate": 0 });
        project["patterns"][0]["steps"][0] = json!(rows);
        let (left, right) = play(&engine, &project, FRAMES, 512);
        let hosted = manager.hosted("synth").expect("the instrument has an instance");
        assert!(hosted.instrument);
        // The level parameter of both test instruments defaults to 0.5; the instrument's volume is 0.5.
        let level = 0.5;
        assert_eq!(peak(&left[..ROW]), 0.0, "{format}: silent before the first note");
        assert_bits(&left[ROW..2 * ROW], |i| sine_sample(60, i as u64, level) * 0.5, &format!("{format}: note 60 from row 1"));
        assert_eq!(peak(&left[2 * ROW + 1..3 * ROW]), 0.0, "{format}: the gate of one row ended the note");
        assert_bits(&left[3 * ROW..FRAMES], |i| sine_sample(67, i as u64, level) * 0.5, &format!("{format}: note 67 from row 3"));
        assert_eq!(left, right);
        let status = inserts(&engine);
        assert_eq!((status[0]["instrument"].clone(), status[0]["available"].clone()), (json!(true), json!(true)));

        // Stop ends the held note: the engine sends its note-off, the plug-in falls silent.
        engine.stop();
        let (tail, _) = engine.render_offline(2048, 512);
        assert_eq!(peak(&tail[1024..]), 0.0, "{format}: stop releases the note");
        // The instrument's pan is a balance, and the engine's own channel is mixed with it.
        project["instruments"][1]["pan"] = json!(1.0);
        project["tracks"][1]["mute"] = json!(false);
        let (left, right) = play(&engine, &project, FRAMES, 512);
        let tone = reference();
        assert_bits(&left[ROW..2 * ROW], |i| tone[i + ROW], &format!("{format}: panned right, the left channel is track b alone"));
        assert_bits(&right[ROW..2 * ROW], |i| sine_sample(60, i as u64, level) * 0.5 + tone[i + ROW], &format!("{format}: the right channel has both"));
        // A live note, a pitch bend and all-sound-off through the engine's own calls.
        engine.stop();
        engine.render_offline(1024, 512);
        engine.note_on(0, 1, 72, 100);
        let (_, live) = engine.render_offline(2048, 512);
        assert!(peak(&live) > 0.1, "{format}: a live note sounds");
        engine.panic();
        wait_until("silence after panic", || peak(&engine.render_offline(1024, 512).1) == 0.0);
    }
}

/// The value of one parameter as the plug-in reports it now.
fn value_of(hosted: &st_audio::plugins::manager::Hosted, address: u64) -> f64 {
    hosted.values().unwrap().into_iter().find(|(id, _)| *id == address).map_or(f64::NAN, |(_, value)| value)
}

#[test]
fn controllers_and_pressure_played_live_reach_a_plug_in_instrument_with_its_next_block() {
    let fixture = Fixture::new("expression");
    for format in ["clap", "clap-notes", "vst3"] {
        let (engine, manager) = fixture.engine();
        let mut project = project(2);
        project["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "au", "name": "Sine", "volume": 1, "pan": 0 }));
        project["tracks"][0]["instrument"] = json!(1);
        project["patterns"][0]["steps"][0] = json!(vec![Value::Null; 16]);
        project["patterns"][0]["steps"][1] = json!(vec![Value::Null; 16]);
        project["audioPlugins"] = json!([match format {
            "clap" => fixture.clap("track:a", "instrument", 0, "synth", CLAP_SINE),
            "clap-notes" => fixture.clap("track:a", "instrument", 0, "synth", CLAP_NOTE_SINE),
            _ => fixture.vst3("track:a", "instrument", 0, "synth", VST3_SINE),
        }]);
        engine.publish(&parse(&project)).unwrap();
        let hosted = manager.hosted("synth").expect("the instrument has an instance");

        // A key held live, at the instrument's level of 0.5.
        engine.note_on(0, 1, 60, 100);
        let (before, _) = engine.render_offline(1_000, 512);
        assert_bits(&before, |i| sine_sample(60, i as u64, 0.5), &format!("{format}: the held note"));

        // Channel pressure doubles the level of both test instruments: from the first frame
        // of the next block, not a frame later.
        assert!(engine.expression(0, [0xD0, 127, 0]), "{format}: channel pressure");
        assert!(engine.expression(0, [0xA0, 60, 45]), "{format}: poly pressure");
        assert!(engine.expression(0, [0xB0, 74, 99]), "{format}: a control change");
        assert!(engine.expression(0, [0xE5, 0x00, 0x60]), "{format}: pitch bend (the channel does not matter)");
        let (after, _) = engine.render_offline(1_000, 512);
        assert_bits(&after, |i| sine_sample(60, 1_000 + i as u64, 1.0), &format!("{format}: pressure from the next block's first frame"));

        // What the plug-in received, in its format's own terms.
        let (poly, bend) = ((60 * 128 + 45) as f64, (0x60 << 7) as f64);
        match format {
            "vst3" => {
                // Controllers and channel pressure are the parameters the plug-in maps them to
                // (its edit controller hears of them a moment later), poly pressure is an event.
                wait_until("the mapped parameters", || value_of(&hosted, vst3_sine::PRESSURE) == 1.0 && value_of(&hosted, vst3_sine::BRIGHTNESS) == 99.0 / 127.0);
                wait_until("pitch bend and poly pressure", || value_of(&hosted, vst3_sine::BEND) == bend / 16383.0 && value_of(&hosted, vst3_sine::POLY_PRESSURE) == poly / 16383.0);
            }
            _ => {
                // MIDI for a note port that takes it; note expressions for one that does not
                // (a control change then has nowhere to go).
                let controller = if format == "clap" { (74 * 128 + 99) as f64 } else { 0.0 };
                let probes = [clap_sine::LAST_CONTROLLER, clap_sine::PRESSURE, clap_sine::POLY_PRESSURE, clap_sine::BEND].map(|address| value_of(&hosted, address));
                assert_eq!(probes, [controller, 127.0, poly, bend], "{format}");
            }
        }

        // Pressure released; a program change is taken as well.
        assert!(engine.expression(0, [0xD0, 0, 0]) && engine.expression(0, [0xC0, 3, 0]));
        let (released, _) = engine.render_offline(500, 512);
        assert_bits(&released, |i| sine_sample(60, 2_000 + i as u64, 0.5), &format!("{format}: pressure released"));
        // Only an instrument that the pedal can reach has one of its own: a note port that
        // takes CLAP note events alone has no controllers.
        assert_eq!(engine.plugin_pedal(0).is_some(), format != "clap-notes", "{format}");
        // Notes and channel-mode messages have their own calls; a track without a plug-in
        // instrument has no use for pressure.
        assert!(!engine.expression(0, [0x90, 60, 100]) && !engine.expression(0, [0xB0, 123, 0]) && !engine.expression(0, [0xF8, 0, 0]));
        assert!(!engine.expression(1, [0xD0, 100, 0]), "{format}: the sampler ignores pressure");
        engine.note_off(0, 60);
    }
}

/// The sustain pedal is the plug-in's: it gets controller 64 with its value, before the notes
/// of the block the pedal arrives with, and its note-offs as played.
#[test]
fn the_sustain_pedal_reaches_a_plug_in_instrument_before_the_notes_of_its_block() {
    let fixture = Fixture::new("pedal");
    for format in ["clap", "vst3"] {
        let (engine, manager) = fixture.engine();
        let mut project = project(2);
        project["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "au", "name": "Sine", "volume": 1, "pan": 0 }));
        project["tracks"][0]["instrument"] = json!(1);
        project["patterns"][0]["steps"][0] = json!(vec![Value::Null; 16]);
        project["patterns"][0]["steps"][1] = json!(vec![Value::Null; 16]);
        project["audioPlugins"] = json!([if format == "clap" { fixture.clap("track:a", "instrument", 0, "synth", CLAP_SINE) } else { fixture.vst3("track:a", "instrument", 0, "synth", VST3_SINE) }]);
        engine.publish(&parse(&project)).unwrap();
        let hosted = manager.hosted("synth").expect("the instrument has an instance");
        assert_eq!(engine.instrument_kind(1), Some(st_project::model::InstrumentKind::Plugin));
        let generation = engine.plugin_pedal(0).expect("the instrument has a pedal");
        assert!(hosted.sustain_pedal && generation == hosted.pedal_generation() && engine.plugin_pedal(1).is_none(), "{format}: the instrument's pedal generation; a track without a plug-in has no pedal");

        // A key is held. The pedal goes down and the key comes up before the next block is
        // rendered: the plug-in hears them in that order, and sustains.
        engine.note_on(0, 1, 60, 100);
        let (held, _) = engine.render_offline(1_000, 512);
        assert_bits(&held, |i| sine_sample(60, i as u64, 0.5), &format!("{format}: the held note"));
        assert!(engine.expression(0, [0xB0, 64, 127]), "{format}: the pedal is taken");
        engine.note_off(0, 60);
        let (sustained, _) = engine.render_offline(1_000, 512);
        assert_bits(&sustained, |i| sine_sample(60, 1_000 + i as u64, 0.5), &format!("{format}: the note-off arrived after the pedal, the instrument sustains"));
        let state = |silent: f64, key_down: f64, sustained: f64| -> &'static str {
            let value = value_of(&hosted, if format == "clap" { clap_sine::NOTE_STATE } else { vst3_sine::NOTE_STATE });
            [(silent, "silent"), (key_down, "key down"), (sustained, "sustained")].into_iter().find(|(known, _)| *known == value).map_or("unknown", |(_, name)| name)
        };
        let note_state = || if format == "clap" { state(clap_sine::SILENT, clap_sine::KEY_DOWN, clap_sine::SUSTAINED) } else { state(vst3_sine::SILENT, vst3_sine::KEY_DOWN, vst3_sine::SUSTAINED) };
        assert_eq!(note_state(), "sustained", "{format}: the plug-in has the note-off");
        // Half-pedalling is the instrument's: it gets the value.
        assert!(engine.expression(0, [0xB0, 64, 80]));
        let (half, _) = engine.render_offline(500, 512);
        assert_bits(&half, |i| sine_sample(60, 2_000 + i as u64, 0.5), &format!("{format}: still down"));
        if format == "clap" {
            assert_eq!(value_of(&hosted, clap_sine::LAST_CONTROLLER), (64 * 128 + 80) as f64);
        } else {
            wait_until("the mapped sustain parameter", || value_of(&hosted, vst3_sine::SUSTAIN) == 80.0 / 127.0);
        }
        // Pedal up: silence from the first frame of the next block.
        assert!(engine.expression(0, [0xB0, 64, 0]));
        let (released, _) = engine.render_offline(500, 512);
        assert_eq!(peak(&released), 0.0, "{format}: the pedal let go");
        assert_eq!(note_state(), "silent");
        assert_eq!(engine.plugin_pedal(0), Some(generation), "{format}: nothing made the plug-in forget its pedals");

        // A panic sends the pedals up before "all notes off": afterwards a key release ends
        // its note, and whoever held the pedal down for the plug-in is told to send it again.
        engine.note_on(0, 1, 62, 100);
        engine.render_offline(512, 512);
        assert!(engine.expression(0, [0xB0, 64, 127]) && engine.expression(0, [0xB0, 66, 127]));
        engine.note_off(0, 62);
        assert!(peak(&engine.render_offline(1_024, 512).0) > 0.1, "{format}: sustained");
        engine.panic();
        wait_until("silence after the panic", || peak(&engine.render_offline(1_024, 512).0) == 0.0 && note_state() == "silent");
        assert_ne!(engine.plugin_pedal(0), Some(generation), "{format}: the pedals were reset");
        // The plug-in comes back from its reset a moment later; then it plays, and lets go.
        wait_until("the instrument to play again", || {
            engine.note_on(0, 1, 64, 100);
            let sounds = peak(&engine.render_offline(1_024, 512).0) > 0.1;
            engine.note_off(0, 64);
            let ended = peak(&engine.render_offline(1_024, 512).0[512..]) == 0.0;
            assert!(!sounds || ended, "{format}: a note is still sustained after the panic");
            sounds
        });
    }
}

/// Lanes and step locks move a plug-in's parameter for a while. Where it goes back to is its
/// base value: the value the user left it at, which the host keeps for every plug-in itself.
#[test]
fn a_parameter_a_step_lock_moved_returns_to_the_value_the_user_set() {
    let fixture = Fixture::new("base");
    // The pattern is 16 rows and loops; the reference covers the first rows of its second pass.
    let reference = render(&project(1), 22 * ROW).0;
    let playing = PlayOptions { song: false, from_start: true, ..PlayOptions::default() };
    // (format, the gain's address, its value for a factor of one)
    for (format, gain, unity) in [("clap", clap_gain::GAIN, 1.0), ("vst3", vst3_gain::GAIN, 0.5)] {
        let (engine, manager) = fixture.engine();
        let mut project = project(1);
        project["audioPlugins"] = json!([if format == "clap" { fixture.clap("track:a", "effect", 0, "fx", CLAP_GAIN) } else { fixture.vst3("track:a", "effect", 0, "fx", VST3_GAIN) }]);
        // A lock on row 2 halves the gain. No lane exists for the parameter.
        project["patterns"][0]["steps"][0][2] = json!({ "fx1": { "type": "plugin", "instanceID": "fx", "address": gain.to_string(), "value": 0.25 } });
        let (left, _) = play(&engine, &project, 5 * ROW, 512);
        let hosted = manager.hosted("fx").unwrap();
        let rows = |from: usize, to: usize| from * ROW..to * ROW;
        assert_bits(&left[rows(0, 2)], |i| reference[i], &format!("{format}: before the lock"));
        assert_bits(&left[rows(2, 3)], |i| reference[i + 2 * ROW] * 0.5, &format!("{format}: the locked row"));
        assert_bits(&left[rows(3, 5)], |i| reference[i + 3 * ROW], &format!("{format}: back at the base value after the row"));

        // The user turns the control while the song plays: that is the base from now on,
        // without a publication in between.
        hosted.set_parameter(gain, 1.5 * unity).unwrap();
        let (left, _) = engine.render_offline(13 * ROW, 512);
        assert_bits(&left[rows(1, 13)], |i| reference[i + 6 * ROW] * 1.5, &format!("{format}: rows 6 to 17, the second pass starting at the new value"));
        // (Row 18 is row 2 of the second pass: the lock.) While the lock holds the parameter,
        // reading the values shows the lock's value and does not take it for the user's.
        engine.render_offline(ROW / 2, 512);
        wait_until("the lock's value in the plug-in", || value_of(&hosted, gain) == 0.5 * unity);
        // An offline copy made at this moment (an export, a kit preview) gets the live
        // instance's state, which has the lock's value in it, and still starts from the
        // user's value.
        let copy = common::engine();
        copy.set_plugins(manager.offline().unwrap());
        let (rendered, _) = play(&copy, &project, 3 * ROW, 512);
        assert_bits(&rendered[..2 * ROW], |i| reference[i] * 1.5, &format!("{format}: an offline copy starts at the user's value"));
        assert_bits(&rendered[2 * ROW..], |i| reference[i + 2 * ROW] * 0.5, &format!("{format}: and plays the lock"));
        drop(copy);
        let (left, _) = engine.render_offline(ROW / 2 + 2 * ROW, 512);
        assert_bits(&left[ROW / 2..], |i| reference[i + 19 * ROW] * 1.5, &format!("{format}: after the lock, back at what the user set"));

        // Stopped in the locked row of the next start. Stopping changes nothing in the
        // plug-in: what rings out keeps the values the song left.
        engine.play(&playing);
        engine.render_offline(2 * ROW + ROW / 2, 512);
        engine.stop();
        engine.render_offline(1_024, 512);
        wait_until("the lock's value after stop", || value_of(&hosted, gain) == 0.5 * unity);
        // A state captured now is the patch the user made, not what the song did to it: the
        // capture has the audio thread (here: this one) bring the parameter back first.
        let snapshot = std::thread::scope(|scope| {
            let capture = scope.spawn(|| engine.snapshot());
            while !capture.is_finished() {
                engine.render_offline(512, 512);
            }
            capture.join().unwrap()
        });
        assert!(snapshot["audioPlugins"][0]["state"].is_string() && snapshot["audioPlugins"][0].get("stateError").is_none(), "{snapshot}");
        wait_until("the base value in the captured plug-in", || value_of(&hosted, gain) == 1.5 * unity);
        // A new start, and a seek, begin at the base value as well.
        engine.play(&playing);
        let (left, _) = engine.render_offline(2 * ROW + ROW / 2, 512);
        assert_bits(&left[..2 * ROW], |i| reference[i] * 1.5, &format!("{format}: a new start"));
        engine.seek(None, 0, 0);
        let (left, _) = engine.render_offline(ROW, 512);
        assert_bits(&left, |i| reference[i] * 1.5, &format!("{format}: after a seek out of the locked row"));
        engine.stop();
    }
}

#[test]
fn a_sidechain_reaches_the_plug_ins_second_input_bus() {
    let fixture = Fixture::new("sidechain");
    let reference = reference();
    let (engine, manager) = fixture.engine();
    let mut project = project(2);
    project["audioPlugins"] = json!([fixture.vst3("track:a", "effect", 0, "fx", VST3_GAIN), fixture.clap("track:b", "effect", 0, "plain", CLAP_GAIN)]);
    project["routing"]["outputs"] = json!({ "track:b": null });
    let sidechain = |destination: &str| json!([{ "id": "k", "source": "track:b", "tap": "preInserts", "gain": 0.5, "enabled": true, "destination": destination }]);
    project["routing"]["sidechains"] = sidechain("fx");
    engine.publish(&parse(&project)).unwrap();
    // The plug-in adds its sidechain input, times this parameter, to its output.
    manager.hosted("fx").unwrap().set_parameter(vst3_gain::SIDECHAIN, 1.0).unwrap();
    let (left, _) = play(&engine, &project, FRAMES, 512);
    assert_eq!(engine.routing_status()["unavailableSidechains"], json!([]));
    assert_bits(&left, |i| reference[i] + reference[i] * 0.5, "the key from track b, at the route's gain, is added by the plug-in");
    // Without the route the plug-in hears silence on that bus.
    project["routing"]["sidechains"] = json!([]);
    let (left, _) = play(&engine, &project, FRAMES, 512);
    assert_bits(&left, |i| reference[i], "no sidechain");
    // A plug-in without a sidechain input says so, and the route is reported.
    project["routing"]["sidechains"] = sidechain("plain");
    engine.publish(&parse(&project)).unwrap();
    assert_eq!(engine.routing_status()["unavailableSidechains"][0]["reason"], "The destination has no sidechain input.");
}

#[test]
fn state_is_captured_into_the_snapshot_and_restored_when_the_project_is_opened_again() {
    let fixture = Fixture::new("state");
    let reference = reference();
    let mut project = project(1);
    project["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "au", "name": "Sine" }));
    project["audioPlugins"] = json!([
        fixture.clap("track:a", "effect", 0, "fx", CLAP_GAIN), fixture.vst3("master", "effect", 2, "m", VST3_GAIN), effect("track:a", 1, "eq", "stfx:eq"),
        fixture.clap("instrument:0", "effect", 0, "chain", CLAP_GAIN),
    ]);
    let saved = {
        let (engine, manager) = fixture.engine();
        engine.publish(&parse(&project)).unwrap();
        assert_eq!(manager.all().iter().map(|hosted| hosted.key.as_str()).collect::<Vec<_>>(), ["chain-on-a", "fx", "m"], "an instrument's chain effect runs once per track that plays it");
        manager.hosted("fx").unwrap().set_parameter(clap_gain::GAIN, 0.5).unwrap();
        manager.hosted("m").unwrap().set_parameter(vst3_gain::GAIN, 0.75).unwrap();
        manager.hosted("chain-on-a").unwrap().set_parameter(clap_gain::GAIN, 2.0).unwrap();
        // A CLAP plug-in takes a value set from outside with its next block; audio runs all
        // the time in the application, here the test renders.
        engine.render_offline(1024, 512);
        let first = engine.snapshot();
        let plugins = first["audioPlugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 4);
        for plugin in plugins {
            assert!(plugin["state"].as_str().is_some_and(|state| !state.is_empty()), "{plugin}");
            assert!(plugin.get("stateError").is_none());
        }
        assert_eq!((plugins[0]["instanceID"].clone(), plugins[3]["owner"].clone()), (json!("fx"), json!("instrument:0")), "descriptors keep their place and owner");
        // Unchanged plug-ins give identical bytes; a change gives others.
        let second = engine.snapshot();
        assert_eq!(first["audioPlugins"], second["audioPlugins"]);
        manager.hosted("fx").unwrap().set_parameter(clap_gain::GAIN, 0.25).unwrap();
        assert_eq!(manager.hosted("fx").unwrap().values().unwrap().iter().find(|(id, _)| *id == clap_gain::GAIN).unwrap().1, 0.25, "a value reads as set before the plug-in applied it");
        engine.render_offline(1024, 512);
        let third = engine.snapshot();
        assert_ne!(third["audioPlugins"][0]["state"], first["audioPlugins"][0]["state"]);
        assert_eq!(third["audioPlugins"][1], first["audioPlugins"][1]);
        // Publishing what was captured does not touch the plug-ins (no state is loaded again).
        let mut captured = project.clone();
        captured["audioPlugins"] = third["audioPlugins"].clone();
        let before = manager.hosted("fx").unwrap().instance.id();
        engine.publish(&parse(&captured)).unwrap();
        assert_eq!(manager.hosted("fx").unwrap().instance.id(), before, "the instance is kept across publications");
        captured
    };
    // Another session: the saved project brings every plug-in back with its state.
    let (engine, manager) = fixture.engine();
    let (left, _) = play(&engine, &saved, FRAMES, 512);
    let value = |key: &str, id: u64| manager.hosted(key).unwrap().values().unwrap().into_iter().find(|(address, _)| *address == id).unwrap().1;
    assert_eq!((value("fx", clap_gain::GAIN), value("m", vst3_gain::GAIN), value("chain-on-a", clap_gain::GAIN)), (0.25, 0.75, 2.0));
    // chain (x2), then fx (x0.25), the built-in EQ at its defaults, then the master's x1.5.
    let flat = render(&{ let mut flat = project.clone(); flat["audioPlugins"] = json!([effect("track:a", 1, "eq", "stfx:eq")]); flat }, FRAMES).0;
    assert_signal(&left, |i| flat[i] * 2.0 * 0.25 * 1.5, 1e-6, "the restored chain");
    assert!(peak(&reference) > 0.2);
    // Opening the project again (a forced publication) loads the saved states over later edits.
    manager.hosted("fx").unwrap().set_parameter(clap_gain::GAIN, 1.5).unwrap();
    engine.render_offline(1024, 512);
    engine.publish(&parse(&saved)).unwrap();
    assert_eq!(value("fx", clap_gain::GAIN), 1.5, "an ordinary publication keeps what the user changed since");
    engine.publish_with(&parse(&saved), true).unwrap();
    assert_eq!(value("fx", clap_gain::GAIN), 0.25);
}

#[test]
fn an_offline_copy_renders_what_the_live_engine_renders() {
    let fixture = Fixture::new("offline");
    let (engine, manager) = fixture.engine();
    let mut project = project(2);
    project["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "au", "name": "Sine", "volume": 0.8 }));
    project["tracks"][1]["instrument"] = json!(1);
    project["patterns"][0]["steps"][1] = json!((0..16).map(|row| if row % 2 == 0 { json!({ "note": 60 + row, "instrument": 1, "velocity": 100, "gate": 1 }) } else { Value::Null }).collect::<Vec<_>>());
    project["audioPlugins"] = json!([
        fixture.clap("track:a", "effect", 0, "fx", CLAP_GAIN), fixture.vst3("track:b", "instrument", 0, "synth", VST3_SINE), fixture.vst3("master", "effect", 0, "m", VST3_GAIN),
    ]);
    project["patterns"][0]["automation"] = json!([{ "id": "lane", "owner": "master", "target": format!("plugin:effect:0:{}", vst3_gain::GAIN), "interpolation": "linear",
        "points": [{ "row": 0, "value": 0.2 }, { "row": 4, "value": 0.6 }] }]);
    engine.publish(&parse(&project)).unwrap();
    manager.hosted("fx").unwrap().set_parameter(clap_gain::GAIN, 0.7).unwrap();
    let live = play(&engine, &project, FRAMES, 512);
    assert!(peak(&live.0) > 0.1);

    // The export engine: its own instances without deadlines, started from the live states
    // (the project still carries none: the 0.7 comes from the live instance).
    let offline_manager = manager.offline().unwrap();
    assert!(offline_manager.is_offline());
    let offline_engine = common::engine();
    offline_engine.set_plugins(offline_manager.clone());
    let offline = play(&offline_engine, &project, FRAMES, 1024);
    assert_ne!(offline_manager.hosted("fx").unwrap().instance.id(), u64::MAX);
    assert_bits(&offline.0, |i| live.0[i], "offline equals real time, left");
    assert_bits(&offline.1, |i| live.1[i], "offline equals real time, right");
    // The live instances are untouched by the copy.
    assert!(manager.hosted("fx").unwrap().lost().is_none() && manager.all().len() == 3);
}

#[test]
fn a_crash_inside_a_plug_in_leaves_the_song_playing_and_restore_brings_it_back_with_its_state() {
    let fixture = Fixture::new("crash");
    let reference = reference();
    let (engine, manager) = fixture.engine();
    let events: Arc<Mutex<Vec<PluginEvent>>> = Arc::default();
    let seen = events.clone();
    manager.listen(Arc::new(move |event| seen.lock().unwrap().push(event.clone())));
    let mut project = project(1);
    project["audioPlugins"] = json!([fixture.clap("track:a", "effect", 0, "fx", CLAP_GAIN), fixture.vst3("master", "effect", 0, "m", VST3_GAIN)]);
    engine.publish(&parse(&project)).unwrap();
    let hosted = manager.hosted("fx").unwrap();
    hosted.set_parameter(clap_gain::GAIN, 0.5).unwrap();
    engine.play(&PlayOptions { song: false, from_start: true, ..PlayOptions::default() });
    let (before, _) = engine.render_offline(4096, 512);
    assert_bits(&before, |i| reference[i] * 0.5, "the plug-in halves the track");

    // The plug-in aborts inside `process` (the value arrives as an automation event would).
    assert!(engine.set_insert_parameter("fx", clap_gain::CRASH, 0.5));
    let (after, _) = engine.render_offline(8192, 512);
    // The block that crashed and every block after it carry the dry signal: the song plays on.
    assert_bits(&after, |i| reference[4096 + i], "the insert passes through");
    wait_until("the loss to be reported", || events.lock().unwrap().iter().any(|event| matches!(event, PluginEvent::Lost { key, .. } if key == "fx")));
    let lost = events.lock().unwrap().iter().find_map(|event| if let PluginEvent::Lost { reason, .. } = event { Some(reason.clone()) } else { None }).unwrap();
    assert!(lost.contains("crashed") && lost.contains("signal 6"), "{lost}");
    let status = inserts(&engine);
    assert_eq!((status[0]["available"].clone(), status[0]["lost"].clone()), (json!(false), json!(true)));
    assert_eq!(status[1]["available"], true, "the other container is unaffected");
    assert!(status[0]["reason"].as_str().unwrap().contains("stopped working"));
    let report = manager.recovery().report();
    assert_eq!((report["suspects"][0]["requiresReview"].clone(), report["pendingInstances"].clone()), (json!(true), json!(1)));
    let key = report["suspects"][0]["identity"]["key"].as_str().unwrap().to_string();
    // Saving while the plug-in is down keeps a state for it (the last one known), no error.
    let snapshot = engine.snapshot();
    assert!(snapshot["audioPlugins"][0].get("stateError").is_none());
    assert!(hosted.values().is_ok(), "the last known values answer while the instance is lost");
    assert!(hosted.set_parameter(clap_gain::GAIN, 1.0).unwrap_err().contains("not running"));

    // Retry: the container starts again and the instance gets its settings back.
    assert_eq!(manager.restore(Some(&[key])), Vec::<(String, String)>::new());
    assert_eq!(manager.recovery().report()["suspects"], json!([]));
    assert_eq!(inserts(&engine)[0]["available"], true);
    let (restored, _) = play(&engine, &project, 4096, 512);
    assert_bits(&restored, |i| reference[i] * 0.5, "restored with its gain");
    assert!(events.lock().unwrap().iter().any(|event| matches!(event, PluginEvent::Changed { key, what: "restored" } if key == "fx")));
}

#[test]
fn a_plug_in_that_cannot_be_loaded_passes_through_and_one_that_keeps_crashing_is_held_back() {
    let fixture = Fixture::new("missing");
    let reference = reference();
    let mut options = fixture.options();
    options.recovery_path = Some(fixture.folder.join("plugin-recovery.json"));
    let engine = engine();
    let manager = PluginManager::new(options.clone()).unwrap();
    engine.set_plugins(manager.clone());
    let events: Arc<Mutex<Vec<PluginEvent>>> = Arc::default();
    let seen = events.clone();
    manager.listen(Arc::new(move |event| seen.lock().unwrap().push(event.clone())));
    let mut project = project(1);
    let mut missing = fixture.clap("track:a", "effect", 0, "gone", "org.example.not-installed");
    missing["state"] = json!("c3RhdGU=");
    // A VST3 module whose file name makes it abort while it is being loaded.
    let crashing = fixture.folder.join("plugins").join("CrashOnLoad.vst3");
    let binaries = crashing.join("Contents").join(format!("{}-linux", std::env::consts::ARCH));
    std::fs::create_dir_all(&binaries).unwrap();
    std::fs::copy(&artifacts().vst3_library, binaries.join("CrashOnLoad.so")).unwrap();
    let mut crash = fixture.vst3("track:a", "effect", 1, "crash", VST3_GAIN);
    crash["bundlePath"] = json!(crashing);
    crash["name"] = json!("Crasher");
    project["audioPlugins"] = json!([missing, crash]);
    let (left, _) = play(&engine, &project, FRAMES, 512);
    assert_bits(&left, |i| reference[i], "both inserts pass through");
    let status = inserts(&engine);
    assert_eq!((status[0]["available"].clone(), status[1]["available"].clone()), (json!(false), json!(false)));
    assert!(status[1]["reason"].as_str().unwrap().contains("Crasher stopped working while loading"), "{}", status[1]["reason"]);
    assert_eq!(events.lock().unwrap().iter().filter(|event| matches!(event, PluginEvent::LoadFailed { .. })).count(), 2);
    // The snapshot keeps exactly what the descriptors carry.
    assert_eq!(engine.snapshot()["audioPlugins"][0]["state"], "c3RhdGU=");
    // The same project again does not try the failed loads again; opening it does, and the
    // second crash at load holds the plug-in back.
    engine.publish(&parse(&project)).unwrap();
    assert_eq!(events.lock().unwrap().iter().filter(|event| matches!(event, PluginEvent::LoadFailed { .. })).count(), 2);
    assert!(manager.recovery().report()["suspects"][0]["requiresReview"] == false);
    engine.publish_with(&parse(&project), true).unwrap();
    let report = manager.recovery().report();
    assert_eq!((report["suspects"][0]["requiresReview"].clone(), report["suspects"][0]["identity"]["name"].clone()), (json!(true), json!("Crasher")));
    drop((engine, manager));

    // The next launch does not load it at all: the project opens, the insert says why.
    let engine = common::engine();
    let manager = PluginManager::new(options).unwrap();
    engine.set_plugins(manager.clone());
    let started = Instant::now();
    engine.publish_with(&parse(&project), true).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    let reason = inserts(&engine)[1]["reason"].as_str().unwrap().to_string();
    assert!(reason.contains("held back") && reason.contains("Review interrupted plug-ins"), "{reason}");
    assert_eq!(manager.recovery().pending_keys().len(), 1);
}

#[test]
fn the_catalogue_lists_the_test_plug_ins_without_loading_them_into_this_process() {
    let fixture = Fixture::new("catalogue");
    let options = CatalogOptions {
        container_path: Some(artifacts().container.clone()),
        socket_dir: Some(fixture.folder.join("sockets")),
        cache_path: Some(fixture.folder.join("plugins.stpcscan")),
        folders: vec![fixture.installed.folder.clone()],
        standard_folders: false,
        timeout: None,
    };
    let catalog = Catalog::new(options.clone()).unwrap();
    assert!(catalog.plugins().is_empty(), "nothing is known before the first scan");
    let mut progress = Vec::new();
    let done = catalog.scan(false, false, &mut |state| progress.push(state.clone())).unwrap();
    assert_eq!((done.scanning, done.total, done.scanned, done.failed, done.plugins), (false, 2, 2, 0, 5));
    assert!(progress.first().unwrap().scanning && progress.iter().any(|state| state.completed == 1 && state.total == 2));
    let plugins = catalog.plugins();
    let found = |format: &str, id: &str| plugins.iter().find(|plugin| plugin.format == format && plugin.id == id).unwrap_or_else(|| panic!("{format}:{id} is listed"));
    assert_eq!(found("clap", CLAP_GAIN).name, "ST Test Gain");
    assert!(st_audio::plugins::catalog::is_instrument(found("clap", CLAP_SINE)) && !st_audio::plugins::catalog::is_instrument(found("vst3", VST3_GAIN)));
    assert_eq!(found("vst3", VST3_SINE).vendor, "SuperTracker Tests");
    assert_eq!(std::path::Path::new(&found("vst3", VST3_GAIN).path), fixture.installed.vst3);
    // The plug-in libraries were read by container processes, never mapped into this one.
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
    assert!(!maps.contains("ST Test") && !maps.contains("stpc_clap_test_plugin") && !maps.contains("stpc_vst3_test_plugin"));
    // A second scan answers from the cache; another process reads the cache file.
    let again = catalog.scan(false, false, &mut |_| {}).unwrap();
    assert_eq!((again.scanned, again.cached, again.plugins), (0, 2, 5));
    assert_eq!(Catalog::new(options).unwrap().plugins().len(), 5);
    assert!(catalog.blocked().is_empty());
    // A descriptor that names no file is found through the catalogue.
    let manager = fixture.manager();
    manager.set_catalog(Arc::new(catalog));
    let engine = engine();
    engine.set_plugins(manager.clone());
    let mut project = project(1);
    project["audioPlugins"] = json!([{ "owner": "track:a", "kind": "effect", "slot": 0, "instanceID": "fx", "component": format!("clap:{CLAP_GAIN}") }]);
    engine.publish(&parse(&project)).unwrap();
    assert_eq!(manager.hosted("fx").unwrap().identity.name, "ST Test Gain");
}

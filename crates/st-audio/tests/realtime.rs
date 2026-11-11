//! The render path does not allocate.
//!
//! This test binary replaces the global allocator by one that counts allocations made while a
//! thread-local switch is on. The switch is on only around `Renderer::render`, on the thread
//! that renders, so the control thread's work (publishing projects, compiling plans) is free to
//! allocate, as it does in the application.

mod common;

use common::*;
use serde_json::json;
use st_audio::PlayOptions;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static COUNTED: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

fn count() {
    if ARMED.try_with(|armed| armed.get()).unwrap_or(false) {
        COUNTED.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: defers to the system allocator; only counts on the way.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Renders `blocks` blocks with the allocation counter armed and returns the peak.
fn render_counted(engine: &st_audio::AudioEngine, blocks: usize, frames: usize, left: &mut [f32], right: &mut [f32]) -> f32 {
    let renderer = engine.renderer();
    let mut renderer = renderer.lock().unwrap();
    let mut loudest = 0.0f32;
    for _ in 0..blocks {
        ARMED.with(|armed| armed.set(true));
        renderer.render(&mut left[..frames], &mut right[..frames]);
        ARMED.with(|armed| armed.set(false));
        loudest = loudest.max(peak(&left[..frames]));
    }
    loudest
}

/// The tests of this file share the counter: one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// As [`render_counted`], with a block of the manual input delivered before every render (the
/// input thread's part, counted too) when `deliver` says so for the block.
fn render_with_input(engine: &st_audio::AudioEngine, blocks: usize, frames: usize, left: &mut [f32], right: &mut [f32], deliver: impl Fn(usize) -> usize) -> f32 {
    let renderer = engine.renderer();
    let mut renderer = renderer.lock().unwrap();
    let hub = engine.live().inputs().hub().clone();
    let mut loudest = 0.0f32;
    for block in 0..blocks {
        ARMED.with(|armed| armed.set(true));
        for _ in 0..deliver(block) {
            hub.deliver(frames, 8, |frame, channel| ((frame + channel * 7) % 64) as f32 / 256.0);
        }
        renderer.render(&mut left[..frames], &mut right[..frames]);
        ARMED.with(|armed| armed.set(false));
        loudest = loudest.max(peak(&left[..frames]));
    }
    loudest
}

#[test]
fn line_inputs_and_a_strip_tap_do_not_allocate_on_the_input_or_the_render_thread() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut left, mut right) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let engine = engine();
    engine.live().inputs().configure(&st_audio::inputs::InputRequest { uid: Some(st_audio::inputs::NULL_MANUAL_INPUT.to_string()), ..Default::default() }).unwrap();
    let mut project: serde_json::Value = serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).unwrap();
    for (index, track) in project["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
        track["routingID"] = json!(format!("t{index}"));
    }
    let line = |channels: &[u32]| json!({ "enabled": true, "deviceUID": st_audio::inputs::NULL_MANUAL_INPUT, "channels": channels, "gain": 0.8 });
    project["tracks"][0]["audioInput"] = line(&[0, 1]);
    project["tracks"][2]["audioInput"] = line(&[5]);
    project["audioPlugins"] = json!([effect("track:t0", 0, "lat", "test:latency:200"), effect("track:t0", 1, "comp", "stfx:compressor")]);
    engine.publish(&parse(&project)).unwrap();
    assert_eq!(engine.line_inputs().0.iter().filter(|row| row["state"] == "live").count(), 2);
    engine.play(&PlayOptions { song: true, from_start: true, ..Default::default() });
    render_with_input(&engine, 4, 512, &mut left, &mut right, |_| 1);
    COUNTED.store(0, Ordering::Relaxed);

    // In step; then the input stalls (an underrun and a new start) and floods (an overrun).
    let mut loudest = render_with_input(&engine, 100, 512, &mut left, &mut right, |_| 1);
    render_with_input(&engine, 12, 512, &mut left, &mut right, |block| usize::from(block >= 6));
    render_with_input(&engine, 12, 512, &mut left, &mut right, |block| if block == 2 { 8 } else { 1 });
    let rows = engine.line_inputs().0;
    assert!(rows[0]["underruns"].as_u64().unwrap() >= 1 && rows[0]["overruns"].as_u64().unwrap() >= 1, "{rows:?}");
    // A tap on the first strip, read by this thread between the counted renders.
    let tap = std::sync::Arc::new(st_audio::graph::tap::StripTap::new(48_000));
    engine.set_strip_tap(Some((0, tap.clone()))).unwrap();
    let (mut tapped, mut other, mut blocks, mut heard) = (Vec::new(), Vec::new(), 0, 0.0f32);
    for _ in 0..40 {
        loudest = loudest.max(render_with_input(&engine, 5, 512, &mut left, &mut right, |_| 1));
        while let Some(block) = tap.pop(&mut tapped, &mut other) {
            assert!(block.frames == 512 && block.arrival >= 712);
            blocks += 1;
            heard = heard.max(peak(&tapped));
            tapped.clear();
            other.clear();
        }
    }
    assert!(blocks == 200 && heard > 0.05 && tap.dropped() == 0, "{blocks} blocks, peak {heard}");
    // A tap nobody reads fills up and drops, without a word from the allocator.
    render_with_input(&engine, 120, 512, &mut left, &mut right, |_| 1);
    assert!(tap.dropped() > 0);
    // The routes change (one feed goes, one comes), then the tap goes.
    project["tracks"][0].as_object_mut().unwrap().remove("audioInput");
    project["tracks"][3]["audioInput"] = line(&[2, 3]);
    engine.publish(&parse(&project)).unwrap();
    render_with_input(&engine, 20, 512, &mut left, &mut right, |_| 1);
    engine.set_strip_tap(None).unwrap();
    render_with_input(&engine, 20, 512, &mut left, &mut right, |_| 1);
    engine.stop();
    render_with_input(&engine, 10, 512, &mut left, &mut right, |_| 1);

    assert!(loudest > 0.02, "the song and the inputs were really rendered (peak {loudest})");
    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the input or the render path");
    engine.tick();
}

/// Two devices in simulated time: an input at `input_rate` frames per second in blocks of 441
/// and an output at 48 kHz in blocks of 512, each delivering or rendering when its clock says.
/// Both sides run on this thread with the allocation counter armed.
struct TwoClocks {
    input_rate: f64,
    delivered: u64,
    rendered: u64,
    /// Seconds of input the device captured and never delivered.
    lost: f64,
    /// Input blocks that arrive before this time are delivered at it.
    held_until: f64,
}

impl TwoClocks {
    fn now(&self) -> f64 {
        1.0 + (self.rendered * 512) as f64 / 48_000.0
    }

    /// `blocks` output callbacks; with `render` false the output stands still meanwhile.
    fn run(&mut self, engine: &st_audio::AudioEngine, blocks: usize, render: bool, left: &mut [f32], right: &mut [f32]) -> f32 {
        let renderer = engine.renderer();
        let mut renderer = renderer.lock().unwrap();
        let hub = engine.live().inputs().hub().clone();
        let mut loudest = 0.0f32;
        for _ in 0..blocks {
            let time = self.now();
            ARMED.with(|armed| armed.set(true));
            loop {
                let arrival = (((self.delivered + 1) * 441) as f64 / self.input_rate + self.lost).max(self.held_until);
                if arrival > time {
                    break;
                }
                let base = self.delivered * 441;
                hub.deliver_at((arrival * 1e9) as u64, 441, 8, |frame, channel| ((base as usize + frame + channel * 7) % 64) as f32 / 256.0);
                self.delivered += 1;
            }
            if render {
                renderer.render_at((time * 1e9) as u64, &mut left[..512], &mut right[..512]);
            }
            ARMED.with(|armed| armed.set(false));
            self.rendered += 1;
            loudest = loudest.max(peak(&left[..512]));
        }
        loudest
    }
}

#[test]
fn an_input_on_a_clock_of_its_own_does_not_allocate_on_the_input_or_the_render_thread() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut left, mut right) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let engine = engine();
    // A 44.1 kHz device, 300 ppm fast: two line inputs, the monitored channels and a take
    // all go through the converter.
    let uid = "null:manual:44100:441";
    engine.live().inputs().configure(&st_audio::inputs::InputRequest { uid: Some(uid.to_string()), monitor: Some(true), ..Default::default() }).unwrap();
    let mut project: serde_json::Value = serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).unwrap();
    for (index, track) in project["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
        track["routingID"] = json!(format!("t{index}"));
    }
    let line = |channels: &[u32]| json!({ "enabled": true, "deviceUID": uid, "channels": channels, "gain": 0.8 });
    project["tracks"][0]["audioInput"] = line(&[0, 1]);
    project["tracks"][2]["audioInput"] = line(&[5]);
    project["audioPlugins"] = json!([effect("track:t0", 0, "comp", "stfx:compressor")]);
    engine.publish(&parse(&project)).unwrap();
    let rows = engine.line_inputs().0;
    assert!(rows.len() == 2 && rows.iter().all(|row| row["state"] == "live" && row["clock"] == "converted"), "{rows:?}");
    let path = std::env::temp_dir().join(format!("st-audio-realtime-take-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&path);
    engine.live().record(&st_audio::live::RecordRequest { source: st_audio::live::RecordSource::Input, path: path.clone(), monitor: true, monitor_volume: 0.5, output_latency_frames: 0 }).unwrap();
    engine.play(&PlayOptions { song: true, from_start: true, ..Default::default() });
    let mut clocks = TwoClocks { input_rate: 44_100.0 * 1.0003, delivered: 43_000 / 441, rendered: 0, lost: 0.0, held_until: 0.0 };
    clocks.run(&engine, 4, true, &mut left, &mut right);
    COUNTED.store(0, Ordering::Relaxed);

    // In step for ten seconds; then everything that makes a feed start over, jump or drop.
    let mut loudest = clocks.run(&engine, 940, true, &mut left, &mut right);
    // The input stalls and delivers what it held back in one burst.
    clocks.held_until = clocks.now() + 0.2;
    clocks.run(&engine, 100, true, &mut left, &mut right);
    // The input loses 150 ms.
    clocks.lost += 0.15;
    clocks.run(&engine, 100, true, &mut left, &mut right);
    // The output stands still for 150 ms, and then for three seconds (the rings run full).
    clocks.run(&engine, 14, false, &mut left, &mut right);
    clocks.run(&engine, 100, true, &mut left, &mut right);
    clocks.run(&engine, 282, false, &mut left, &mut right);
    clocks.run(&engine, 100, true, &mut left, &mut right);
    // The input is gone for two seconds and comes back.
    clocks.lost += 2.0;
    loudest = loudest.max(clocks.run(&engine, 400, true, &mut left, &mut right));
    let rows = engine.line_inputs().0;
    assert!(rows.iter().all(|row| row["underruns"].as_u64().unwrap() >= 3 && row["overruns"].as_u64().unwrap() >= 2), "{rows:?}");
    assert!((rows[0]["driftPpm"].as_f64().unwrap() - 300.0).abs() < 60.0, "{rows:?}");
    // A route goes and one comes while the others play on; monitoring is switched off.
    project["tracks"][0].as_object_mut().unwrap().remove("audioInput");
    project["tracks"][3]["audioInput"] = line(&[2, 3]);
    engine.publish(&parse(&project)).unwrap();
    clocks.run(&engine, 100, true, &mut left, &mut right);
    engine.live().inputs().configure(&st_audio::inputs::InputRequest { monitor: Some(false), ..Default::default() }).unwrap();
    clocks.run(&engine, 50, true, &mut left, &mut right);

    assert!(loudest > 0.02, "the song and the inputs were really rendered (peak {loudest})");
    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the input or the render path");
    // The take is ended by another thread while this one goes on rendering.
    let stopper = std::thread::spawn({
        let engine = engine.clone();
        move || engine.live().stop_record().expect("a take was running")
    });
    while !stopper.is_finished() {
        clocks.run(&engine, 1, true, &mut left, &mut right);
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
    let take = stopper.join().unwrap();
    assert!(take.file.problem.is_none() && take.file.frames > 1_800 * 512, "{:?}", take.file);
    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the input or the render path");
    engine.tick();
    let _ = std::fs::remove_file(path);
}

#[test]
fn rendering_never_allocates_or_frees_even_across_project_and_plan_changes() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // The counter itself works: an allocation made while armed is seen.
    ARMED.with(|armed| armed.set(true));
    drop(std::hint::black_box(vec![0u8; 64]));
    ARMED.with(|armed| armed.set(false));
    assert!(COUNTED.swap(0, Ordering::Relaxed) >= 2, "the counting allocator counts");

    let (mut left, mut right) = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
    let engine = engine();
    let mut project: serde_json::Value = serde_json::from_str(include_str!("../../st-project/tests/fixtures/demo.json")).unwrap();
    // The demo song with a mixer that uses every kind of stage.
    for (index, track) in project["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
        track["routingID"] = json!(format!("t{index}"));
    }
    project["routing"] = json!({
        "buses": [bus("drums", "group", 0.9), bus("space", "return", 0.8)],
        "outputs": { "track:t3": "bus:drums", "track:t4": "bus:drums" },
        "sends": [{ "id": "s", "source": "track:t1", "tap": "preFader", "destination": "bus:space", "gain": 0.5 }],
        "sidechains": [{ "id": "k", "source": "track:t3", "tap": "postFader", "gain": 1,
            "destination": "comp" }],
    });
    project["audioPlugins"] = json!([
        effect("track:t0", 0, "comp", "stfx:compressor"), effect("bus:space", 0, "lim", "stfx:limiter"),
        effect("track:t2", 0, "lat", "test:latency:200"), effect("master", 0, "gain", "test:gain"), effect("instrument:1", 0, "igain", "test:gain"),
    ]);
    project["patterns"][0]["automation"].as_array_mut().unwrap().extend([
        json!({ "id": "a", "owner": "bus:drums", "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.5 }, { "row": 63, "value": 1.0 }] }),
        json!({ "id": "b", "owner": "master", "target": "plugin:effect:0:0", "interpolation": "linear", "points": [{ "row": 0, "value": 0.4 }, { "row": 63, "value": 0.5 }] }),
        json!({ "id": "c", "track": 2, "target": "volume", "interpolation": "linear", "points": [{ "row": 0, "value": 0.3 }, { "row": 63, "value": 0.9 }] }),
    ]);
    project["patterns"][0]["steps"][0][4] = json!({ "fx1": { "type": "plugin", "instanceID": "comp", "address": "0", "value": 0.3 }, "fx2": { "type": "pan", "value": -0.5 } });
    engine.publish(&parse(&project)).unwrap();
    engine.play(&PlayOptions { song: true, from_start: true, ..Default::default() });

    // Warm up outside the count (nothing lazy is expected, but the claim is about steady state
    // and about swaps, which are counted below).
    render_counted(&engine, 4, 512, &mut left, &mut right);
    COUNTED.store(0, Ordering::Relaxed);

    let mut loudest = render_counted(&engine, 200, 512, &mut left, &mut right);
    // A note edit (engine project swap), then a mixer change (new plan adopted, old one retired),
    // live notes, a live insert parameter, a seek and a panic, each followed by counted renders.
    project["patterns"][0]["steps"][0][8] = json!({ "note": 45, "instrument": 0, "velocity": 100, "gate": 1 });
    engine.publish(&parse(&project)).unwrap();
    loudest = loudest.max(render_counted(&engine, 50, 127, &mut left, &mut right));
    project["tracks"][0]["volume"] = json!(0.6);
    project["routing"]["buses"][0]["solo"] = json!(true);
    project["audioPlugins"].as_array_mut().unwrap().push(effect("bus:drums", 0, "phase", "stfx:phaser"));
    engine.publish(&parse(&project)).unwrap();
    loudest = loudest.max(render_counted(&engine, 50, 4096, &mut left, &mut right));
    engine.note_on(1, 1, 64, 100);
    assert!(engine.set_insert_parameter("gain", 0, 0.45));
    engine.seek(Some(1), 16, 2);
    render_counted(&engine, 20, 64, &mut left, &mut right);
    engine.note_off(1, 64);
    engine.panic();
    render_counted(&engine, 20, 512, &mut left, &mut right);
    engine.stop();
    render_counted(&engine, 20, 512, &mut left, &mut right);
    // A new start: the inserts go back to their base values, the compressor's step lock
    // returns to its base at the end of its row; a pitch bend and the wheel for the engine's voices.
    engine.play(&PlayOptions { song: true, from_start: true, ..Default::default() });
    assert!(engine.expression(1, [0xE0, 0, 0x50]) && engine.expression(1, [0xB0, 1, 90]));
    loudest = loudest.max(render_counted(&engine, 60, 512, &mut left, &mut right));

    assert!(loudest > 0.02, "the song was really rendered (peak {loudest})");
    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the render path");
    // The control thread frees what the render retired.
    engine.tick();
}

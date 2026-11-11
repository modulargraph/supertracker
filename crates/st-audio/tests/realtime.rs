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
    let mut project: serde_json::Value = serde_json::from_str(include_str!("fixtures/demo_project.json")).unwrap();
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
    let mut project: serde_json::Value = serde_json::from_str(include_str!("fixtures/demo_project.json")).unwrap();
    // The demo song with a mixer that uses every kind of stage.
    for (index, track) in project["tracks"].as_array_mut().unwrap().iter_mut().enumerate() {
        track["routingID"] = json!(format!("t{index}"));
    }
    project["routing"] = json!({
        "version": 1, "mixModel": "standard", "buses": [bus("drums", "group", 0.9), bus("space", "return", 0.8)],
        "outputs": { "track:t3": "bus:drums", "track:t4": "bus:drums" },
        "sends": [{ "id": "s", "source": "track:t1", "tap": "preFader", "destination": "bus:space", "gain": 0.5 }],
        "sidechains": [{ "id": "k", "source": "track:t3", "tap": "postFader", "gain": 1,
            "destination": { "instanceID": "comp", "inputBus": 1, "layoutSignature": { "name": "Sidechain", "channels": 2 } } }],
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

    assert!(loudest > 0.02, "the song was really rendered (peak {loudest})");
    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the render path");
    // The control thread frees what the render retired.
    engine.tick();
}

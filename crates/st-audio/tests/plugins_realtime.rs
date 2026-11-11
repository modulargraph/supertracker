//! The render path does not allocate with plug-ins in the graph.
//!
//! Like `realtime.rs`: the global allocator counts while a thread-local switch is on, and the
//! switch is on only around `Renderer::render`. Here the mixer holds a CLAP effect, a VST3
//! effect with a sidechain and automation, and a CLAP instrument playing a pattern; the count
//! covers steady playback, a plan swap, live notes, controllers and pressure, a stop, a
//! seek and a new start (parameters go back to their base values), a panic, and a plug-in
//! that crashes in the middle (its insert then passes through).

mod common;

use common::*;
use serde_json::{json, Value};
use st_audio::plugins::testing::{artifacts, clap_gain, vst3_gain, CLAP_GAIN, CLAP_SINE, VST3_GAIN};
use st_audio::plugins::{PluginManager, PluginOptions};
use st_audio::PlayOptions;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

fn render_counted(engine: &st_audio::AudioEngine, blocks: usize, frames: usize) -> f32 {
    let (mut left, mut right) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let renderer = engine.renderer();
    let mut renderer = renderer.lock().unwrap();
    let mut loudest = 0.0f32;
    for _ in 0..blocks {
        ARMED.with(|armed| armed.set(true));
        renderer.render(&mut left, &mut right);
        ARMED.with(|armed| armed.set(false));
        loudest = loudest.max(peak(&left));
    }
    loudest
}

#[test]
fn rendering_with_plug_ins_never_allocates_also_when_one_of_them_crashes() {
    ARMED.with(|armed| armed.set(true));
    drop(std::hint::black_box(vec![0u8; 64]));
    ARMED.with(|armed| armed.set(false));
    assert!(COUNTED.swap(0, Ordering::Relaxed) >= 2, "the counting allocator counts");

    let folder = std::env::temp_dir().join(format!("st-plug-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(folder.join("sockets")).unwrap();
    let installed = artifacts().install(&folder.join("plugins"));
    let mut options = PluginOptions::default();
    options.supervisor.container_path = Some(artifacts().container.clone());
    options.supervisor.socket_dir = Some(folder.join("sockets"));
    options.min_deadline = Duration::from_secs(20);
    let manager = PluginManager::new(options).unwrap();
    let engine = engine();
    engine.set_plugins(manager.clone());

    let plugin = |owner: &str, kind: &str, instance: &str, format: &str, id: &str| {
        json!({ "owner": owner, "kind": kind, "slot": 0, "instanceID": instance, "component": format!("{format}:{id}"), "bundlePath": if format == "clap" { &installed.clap } else { &installed.vst3 }, "name": instance })
    };
    let mut project = project(3);
    project["instruments"].as_array_mut().unwrap().push(json!({ "id": 1, "type": "au", "name": "Sine" }));
    project["tracks"][2]["instrument"] = json!(1);
    project["patterns"][0]["steps"][2] = json!((0..16).map(|row| if row % 2 == 0 { json!({ "note": 48 + row, "instrument": 1, "velocity": 100, "gate": 1 }) } else { Value::Null }).collect::<Vec<_>>());
    project["audioPlugins"] = json!([
        plugin("track:a", "effect", "clap-fx", "clap", CLAP_GAIN), plugin("track:b", "effect", "vst3-fx", "vst3", VST3_GAIN),
        plugin("track:c", "instrument", "synth", "clap", CLAP_SINE), plugin("master", "effect", "last", "clap", CLAP_GAIN),
    ]);
    project["routing"]["sidechains"] = json!([{ "id": "k", "source": "track:a", "tap": "postFader", "gain": 1, "enabled": true,
        "destination": { "instanceID": "vst3-fx", "inputBus": 1, "layoutSignature": { "name": "Sidechain", "channels": 2 } } }]);
    project["patterns"][0]["automation"] = json!([
        { "id": "a", "owner": "track:b", "target": format!("plugin:effect:0:{}", vst3_gain::GAIN), "interpolation": "linear", "points": [{ "row": 0, "value": 0.3 }, { "row": 15, "value": 0.6 }] },
        { "id": "b", "owner": "track:c", "target": "plugin:instrument:0:1", "interpolation": "linear", "points": [{ "row": 0, "value": 0.2 }, { "row": 15, "value": 0.9 }] },
    ]);
    project["patterns"][0]["steps"][0][4] = json!({ "fx1": { "type": "plugin", "instanceID": "clap-fx", "address": clap_gain::GAIN.to_string(), "value": 0.3 } });
    engine.publish(&parse(&project)).unwrap();
    assert_eq!(manager.all().len(), 4);
    engine.play(&PlayOptions { song: false, from_start: true, ..Default::default() });

    render_counted(&engine, 4, 512);
    COUNTED.store(0, Ordering::Relaxed);
    let mut loudest = render_counted(&engine, 200, 512);
    // A mixer change (plan swap: the processors stay), live notes and controllers, a panic.
    project["tracks"][0]["volume"] = json!(0.6);
    project["instruments"][1]["volume"] = json!(0.9);
    engine.publish(&parse(&project)).unwrap();
    loudest = loudest.max(render_counted(&engine, 50, 127));
    engine.note_on(2, 1, 72, 100);
    // Pitch bend, the modulation wheel, another controller, channel and poly pressure, more
    // of them than one block's queue takes.
    for step in 0..100u8 {
        for message in [[0xE0, step, 64], [0xB0, 1, step], [0xB0, 74, step], [0xD0, step, 0], [0xA0, 72, step]] {
            engine.expression(2, message);
        }
    }
    assert!(engine.set_insert_parameter("last", clap_gain::GAIN, 0.4));
    loudest = loudest.max(render_counted(&engine, 50, 4096));
    assert!(engine.expression(2, [0xD0, 0, 0]), "the queue was drained on the render path");
    engine.note_off(2, 72);
    // Stop, seek and start again: the parameters the lanes and the lock moved go back to
    // their base values each time.
    engine.stop();
    render_counted(&engine, 4, 512);
    engine.seek(None, 3, 0);
    render_counted(&engine, 4, 512);
    engine.play(&PlayOptions { song: false, from_start: true, ..Default::default() });
    loudest = loudest.max(render_counted(&engine, 40, 512));
    engine.panic();
    render_counted(&engine, 20, 64);
    // The effect on track a aborts inside `process`; rendering goes on with its fallback.
    assert!(engine.set_insert_parameter("clap-fx", clap_gain::CRASH, 0.5));
    loudest = loudest.max(render_counted(&engine, 100, 512));
    engine.stop();
    render_counted(&engine, 20, 512);

    assert!(loudest > 0.02, "the song was really rendered (peak {loudest})");
    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the render path");
    assert!(manager.hosted("clap-fx").unwrap().lost().is_some(), "the crash happened");
    engine.tick();
    drop((engine, manager));
    let _ = std::fs::remove_dir_all(&folder);
}

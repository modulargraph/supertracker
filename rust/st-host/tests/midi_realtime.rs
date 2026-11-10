//! MIDI output on the render thread does not allocate.
//!
//! Like `st-audio/tests/realtime.rs`: the global allocator of this test binary counts what is
//! allocated or freed while a thread-local switch is on, and the switch is on only around
//! `Renderer::render`. Here the renderer has the host's MIDI hook
//! (`st_host::areas::midi::RenderHook` over `st_midi::RenderOutput`): notes of an external
//! instrument, MIDI clock with transport messages, and all-notes-off.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};
use st_audio::{AudioEngine, EngineOptions, PlayOptions};
use st_host::areas::midi::connect_output;
use st_midi::memory::MemoryPorts;
use st_midi::MidiService;
use st_project::Project;

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

const SYNTH: i32 = 700;

/// A track that plays an external MIDI instrument on every other row.
fn project() -> Value {
    let rows: Vec<Value> = (0..16).map(|row| if row % 2 == 0 { json!({ "note": 48 + row, "instrument": 0, "velocity": 100 }) } else { Value::Null }).collect();
    json!({
        "version": 1, "name": "MIDI", "bpm": 174, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": [{ "id": 0, "name": "Synth", "routingID": "synth", "volume": 1, "pan": 0, "instrument": 0 }],
        "instruments": [{ "id": 0, "type": "midi", "name": "Synth", "midiOutput": SYNTH, "midiChannel": 1 }],
        "patterns": [{ "id": 0, "length": 16, "steps": [rows], "automation": [] }],
        "sequence": [0], "loopSong": true
    })
}

fn render_counted(engine: &AudioEngine, blocks: usize) {
    let renderer = engine.renderer();
    let mut renderer = renderer.lock().unwrap();
    let (mut left, mut right) = ([0.0f32; 256], [0.0f32; 256]);
    for _ in 0..blocks {
        ARMED.with(|armed| armed.set(true));
        renderer.render(&mut left, &mut right);
        ARMED.with(|armed| armed.set(false));
    }
}

#[test]
fn midi_output_from_the_render_thread_never_allocates() {
    ARMED.with(|armed| armed.set(true));
    drop(std::hint::black_box(vec![0u8; 64]));
    ARMED.with(|armed| armed.set(false));
    assert!(COUNTED.swap(0, Ordering::Relaxed) >= 2, "the counting allocator counts");

    let ports = MemoryPorts::new();
    ports.add_output(SYNTH, "Synth");
    let service = MidiService::new(st_midi::MidiOptions::in_memory(&ports)).unwrap();
    service.clock_command(serde_json::from_value(json!({ "enabled": true, "devices": [{ "id": SYNTH, "clock": true, "transport": true }] })).unwrap()).unwrap();
    let engine = AudioEngine::new(EngineOptions::offline()).unwrap();
    connect_output(&engine, &service);
    let project = project();
    engine.publish(&Project::from_value(project.clone()).unwrap()).unwrap();
    service.set_project(&project);

    // Stopped (clock only), playing (notes, clock, Start), stopped again (Stop, all notes off), panic.
    render_counted(&engine, 50);
    engine.play(&PlayOptions { song: true, from_start: true, ..PlayOptions::default() });
    render_counted(&engine, 400);
    engine.stop();
    render_counted(&engine, 20);
    engine.panic();
    render_counted(&engine, 20);
    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the render thread");

    // And it did send: notes, clock pulses, Start and Stop, all notes off.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let sent = ports.sent_messages(SYNTH);
    for (what, wanted) in [("note-ons", 0x90u8), ("clock pulses", 0xF8), ("Start", 0xFA), ("Stop", 0xFC), ("all notes off", 0xB0)] {
        assert!(sent.iter().any(|message| message[0] == wanted), "{what} were sent: {} messages", sent.len());
    }
    engine.set_midi_output(None);
}

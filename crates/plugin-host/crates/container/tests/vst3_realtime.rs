//! The VST3 backend's audio path allocates nothing: its `Processor` is driven here inside
//! the test process (the module loaded with `dlopen`, no container), under an allocator
//! that counts the calls of this thread. What is counted is the backend's own work per
//! block: building the parameter queues and event lists from the host's events, the
//! process context, the bus buffers, and passing output parameters and events back. The
//! plug-in library has its own allocator and is not counted (its process code does not
//! allocate either).

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::*;
use stpc_container::vst3::Vst3Backend;
use stpc_runtime::{CreateRequest, HostHandle, PluginBackend, ProcessStatus, TestBlock};
use stpc_vst3_test_plugin as plug;

struct CountingAllocator;

thread_local! {
    static WATCHED: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<u64> = const { Cell::new(0) };
}

fn note_call() {
    let _ = WATCHED.try_with(|watched| {
        if watched.get() {
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }
    });
}

// SAFETY: defers to the system allocator; the bookkeeping does not allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_call();
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_call();
        System.dealloc(ptr, layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_call();
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_call();
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocator_calls<R>(f: impl FnOnce() -> R) -> (u64, R) {
    CALLS.with(|calls| calls.set(0));
    WATCHED.with(|watched| watched.set(true));
    let result = f();
    WATCHED.with(|watched| watched.set(false));
    (CALLS.with(|calls| calls.get()), result)
}

fn request<'a>(id: &'a str, path: &'a str, inputs: &'a [u32], outputs: &'a [u32]) -> CreateRequest<'a> {
    CreateRequest {
        plugin_id: id,
        name: "test",
        options: path,
        sample_rate: 48_000.0,
        max_frames: 1024,
        input_buses: inputs,
        output_buses: outputs,
        event_capacity: 1024,
        out_event_capacity: 256,
    }
}

#[test]
fn processing_does_not_allocate() {
    // The yardstick works: an allocation on this thread is seen.
    assert!(allocator_calls(|| vec![0u8; 100]).0 >= 1);
    let scratch = Scratch::new("vst3-rt");
    let bundle = vst3_bundle(&scratch.dir, "STTest").to_string_lossy().into_owned();
    let backend = Vst3Backend::new();
    let transport = Transport {
        flags: TRANSPORT_PLAYING | TRANSPORT_TEMPO_VALID | TRANSPORT_PPQ_VALID,
        tempo_bpm: 120.0,
        ..Transport::default()
    };
    let mut rng = Rng(0xA110C);

    // The effect: main and sidechain input, dense automation of several parameters,
    // untimed changes queued from the main thread, output parameters every block.
    let handle = HostHandle::detached();
    let mut gain = backend.create(&request(plug::GAIN_CLASS_ID, &bundle, &[2, 2], &[2]), handle.clone()).unwrap();
    let mut processor = gain.activate(48_000.0, 1024).unwrap();
    let mut block = TestBlock::new(&[2, 2], &[2], 1024, 256);
    for channel in 0..4 {
        block.input_mut(channel).copy_from_slice(&rng.signal(1024));
    }
    let events: Vec<Event> = (0..512u32)
        .map(|index| {
            let id = if index % 3 == 0 { plug::PARAM_SIDECHAIN } else { plug::PARAM_GAIN };
            Event::param(index * 2, id as u64, (index % 17) as f64 / 16.0)
        })
        .collect();
    // Warm up (first-use paths), then measure.
    assert_eq!(block.process(processor.as_mut(), 1024, &events, &transport), ProcessStatus::Ok);
    let mut blocks = 0;
    for round in 0..200usize {
        // Main thread work between blocks is not on the audio path; it may allocate.
        gain.set_parameter(plug::PARAM_GAIN as u64, (round % 10) as f64 / 10.0).unwrap();
        if handle.take_callback_request() {
            gain.idle();
        }
        let frames = [1usize, 64, 1024, 333][round % 4];
        let slice = &events[..events.iter().position(|e| e.offset as usize >= frames).unwrap_or(events.len())];
        let (calls, status) = allocator_calls(|| block.process(processor.as_mut(), frames, slice, &transport));
        assert_eq!(status, ProcessStatus::Ok);
        assert_eq!(calls, 0, "block {round} ({frames} frames, {} events)", slice.len());
        blocks += 1;
    }
    assert_eq!(blocks, 200);
    // The audio is real: the last gain event of the last full block decides the tail.
    block.process(processor.as_mut(), 1024, &[Event::param(0, plug::PARAM_GAIN as u64, 1.0)], &transport);
    assert!(block.output(0).iter().any(|&s| s != 0.0));
    gain.deactivate(processor);
    drop(gain);

    // The instrument: notes, controllers that map to parameters, pitch bend, events out.
    let handle = HostHandle::detached();
    let mut sine = backend.create(&request(plug::SINE_CLASS_ID, &bundle, &[], &[2]), handle.clone()).unwrap();
    sine.set_parameter(plug::PARAM_THRU as u64, 1.0).unwrap();
    let mut processor = sine.activate(48_000.0, 1024).unwrap();
    let mut block = TestBlock::new(&[], &[2], 1024, 256);
    let mut events = Vec::new();
    for index in 0..60u32 {
        let offset = index * 16;
        events.push(Event::note_on(offset, (index % 4) as u8, 40 + index as u8, 100));
        events.push(Event::midi(offset + 3, &[0xB0, 7, (index * 2) as u8]));
        events.push(Event::midi(offset + 5, &[0xE0, index as u8, 64]));
        events.push(Event::note_off(offset + 9, (index % 4) as u8, 40 + index as u8));
    }
    assert_eq!(block.process(processor.as_mut(), 1024, &events, &transport), ProcessStatus::Ok);
    assert_eq!(block.out_events().len(), 180, "notes and pitch bend came back");
    for round in 0..100usize {
        if handle.take_callback_request() {
            sine.idle();
        }
        let (calls, status) = allocator_calls(|| block.process(processor.as_mut(), 1024, &events, &transport));
        assert_eq!(status, ProcessStatus::Ok);
        assert_eq!(calls, 0, "instrument block {round}");
        assert_eq!(block.out_events().len(), 180);
    }
    // A held note and "all notes off" (the backend emits the note-offs itself).
    let held = [Event::note_on(0, 0, 60, 100), Event::note_on(1, 1, 61, 100)];
    let (calls, _) = allocator_calls(|| block.process(processor.as_mut(), 64, &held, &transport));
    assert_eq!(calls, 0);
    let panic = [Event::midi(0, &[0xB0, 123, 0]), Event::midi(0, &[0xB1, 120, 0])];
    let (calls, _) = allocator_calls(|| block.process(processor.as_mut(), 64, &panic, &transport));
    assert_eq!(calls, 0);
    assert!(block.output(0)[..64].iter().all(|&s| s == 0.0), "both held notes were released");
    assert_eq!(block.output_silence & 3, 3);
    sine.deactivate(processor);
}

//! Parameter locks on steps (a step effect that sets a parameter of the host for the row), their
//! interaction with host automation lanes, and the events they emit.
//!
//! Allocations made by the audio callback, including event consumption, are counted (test setup,
//! result collection and normal control-thread project copies are excluded); every group ends
//! with the C++ program's "zero render/event-consumer allocations" assertion for its own thread.
#![cfg(feature = "engine")]
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations};
use engine_support_b::engine::Engine;
use tracker_engine::engine::*;

/// C++ `struct Engine`: 8 kHz, exactly 100 samples per row, three tracks, two 8-row patterns and
/// an AU instrument, with the update left open until `play`.
fn fixture() -> Engine {
    let mut p = Engine::with_rate(8000.0);
    p.begin_update();
    p.set_timing(300.0, 16, 0.0, 1.0); // Exactly 100 samples per row.
    p.set_track_count(3);
    p.set_pattern(0, 8);
    p.set_pattern(1, 8);
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_AU;
    p.set_instrument(0, instrument);
    p
}

fn play(engine: &mut Engine) {
    engine.commit_update();
    engine.play(0);
}

#[derive(Clone, Copy)]
struct TimedEvent {
    frame: u64,
    event: STEvent,
}

type Events = Vec<TimedEvent>;

#[derive(Clone, Copy)]
struct Destination {
    track: i32,
    slot: i32,
    host: i32,
    address: u64,
}

impl Destination {
    fn matches(&self, event: &STEvent) -> bool {
        event.address == self.address && event.type_ == ST_EVENT_HOST_PARAMETER && event.target == self.host && event.track == -1
    }
}

fn destination(track: i32, slot: i32, host: i32, address: u64) -> Destination {
    Destination { track, slot, host, address }
}

fn drain(engine: &mut Engine, base: u64) -> Events {
    let mut events = Vec::new();
    while let Some(event) = realtime(|| engine.pop_event()) {
        assert!(event.frame_offset >= 0);
        events.push(TimedEvent { frame: base + event.frame_offset as u64, event });
    }
    events
}

fn render_without_drain(engine: &mut Engine, frames: usize) {
    let (mut left, mut right) = (vec![0.0f32; 16384], vec![0.0f32; 16384]);
    assert!(frames <= left.len());
    realtime(|| engine.render(&mut left[..frames], &mut right[..frames]));
    for n in 0..frames {
        assert!(left[n].is_finite() && right[n].is_finite());
    }
}

fn render(engine: &mut Engine, frames: usize, base: u64, block: usize) -> Events {
    let mut result = Vec::new();
    let mut at = 0;
    while at < frames {
        render_without_drain(engine, block.min(frames - at));
        result.extend(drain(engine, base + at as u64));
        at += block;
    }
    result
}

/// `render(engine, frames)` with the C++ defaults `base = 0`, `block = 127`.
fn render_from_zero(engine: &mut Engine, frames: usize) -> Events {
    render(engine, frames, 0, 127)
}

fn render_bus(engine: &mut Engine, bus: i32, time: u64, frames: usize) -> Events {
    let (mut left, mut right) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);
    assert!(frames <= left.len());
    realtime(|| engine.render_bus(bus, time, &mut left[..frames], &mut right[..frames]));
    drain(engine, time)
}

fn select(events: &Events, destination: Destination) -> Events {
    events.iter().copied().filter(|event| destination.matches(&event.event)).collect()
}

#[track_caller]
fn at_within(event: &TimedEvent, frame: u64, value: f64, tolerance: f64) {
    if event.frame != frame || (event.event.value - value).abs() > tolerance {
        eprintln!(
            "Expected frame {} value {:.17e}, got frame {} value {:.17e} (event {} target {} address {})",
            frame, value, event.frame, event.event.value, event.event.type_, event.event.target, event.event.address
        );
    }
    assert!(event.frame == frame);
    assert!((event.event.value - value).abs() <= tolerance);
}

#[track_caller]
fn at(event: &TimedEvent, frame: u64, value: f64) {
    at_within(event, frame, value, 0.0);
}

fn step_note(engine: &mut Engine, track: i32, row: i32, note: i32) {
    let mut value = default_step();
    value.note = note;
    value.gate = 10.0;
    engine.set_step(0, track, row, value, &[]);
}

/// `step(engine, track, row)`: an effect-only cell (note -3).
fn step(engine: &mut Engine, track: i32, row: i32) {
    step_note(engine, track, row, -3);
}

#[track_caller]
fn lock_column(engine: &mut Engine, destination: Destination, row: i32, value: f64, column: i32) {
    assert!(engine.set_plugin_step_fx(
        0,
        destination.track,
        row,
        column,
        destination.slot,
        destination.host,
        destination.address,
        value
    ));
}

#[track_caller]
fn lock(engine: &mut Engine, destination: Destination, row: i32, value: f64) {
    lock_column(engine, destination, row, value, 0);
}

fn lane_with(engine: &mut Engine, destination: Destination, row: f64, value: f64, interpolation: i32) {
    engine.add_host_automation_point(0, destination.host, destination.address, interpolation, row, value);
}

/// `lane(engine, destination, row, value)`: stepped (interpolation 1).
fn lane(engine: &mut Engine, destination: Destination, row: f64, value: f64) {
    lane_with(engine, destination, row, value, 1);
}

/// "sample-exact rows, full-width addresses and double values"
#[test]
fn exact_rows_and_opaque_addresses() {
    let mut engine = fixture();
    let destination = destination(0, -1, 511, u64::MAX);
    let (first, second) = (0.12345678901234567, 0.9876543210987654);
    step(&mut engine, 0, 1);
    lock(&mut engine, destination, 1, first);
    step(&mut engine, 0, 3);
    lock(&mut engine, destination, 3, second);
    play(&mut engine);
    let events = select(&render(&mut engine, 401, 0, 73), destination);
    assert!(events.len() == 2);
    at(&events[0], 100, first);
    at(&events[1], 300, second);
    assert!(engine.status().dropped_events == 0);
    assert!(render_allocations() == 0);
}

/// "host suppression is local to destination and address"
#[test]
fn host_suppression_uses_destination_and_address() {
    let mut engine = fixture();
    let locked = destination(0, 2, 127, u64::MAX);
    let other_address = destination(0, 2, 127, u64::MAX - 1);
    let other_owner = destination(0, 2, 128, u64::MAX);
    lane(&mut engine, locked, 0.0, 0.25);
    lane_with(&mut engine, other_address, 0.0, -12.25, 0);
    lane_with(&mut engine, other_address, 4.0, 7.75, 0);
    lane(&mut engine, other_owner, 0.0, 13.75);
    step(&mut engine, 0, 0);
    lock(&mut engine, locked, 0, 0.875);
    play(&mut engine);
    let events = render_from_zero(&mut engine, 101);
    let target = select(&events, locked);
    assert!(target.len() == 2);
    at(&target[0], 0, 0.875);
    at(&target[1], 100, 0.25);
    let owner = select(&events, other_owner);
    assert!(owner.len() == 1);
    at(&owner[0], 0, 13.75);
    let other = select(&events, other_address);
    assert!(other.len() == 4);
    for n in 0..other.len() {
        at_within(&other[n], n as u64 * 32, -12.25 + (n * 32) as f64 * 20.0 / 400.0, 1e-12);
    }
    assert!(render_allocations() == 0);
}

/// "FX2 duplicate coalescing and distinct-column order"
#[test]
fn duplicate_columns_coalesce_to_fx2() {
    let mut engine = fixture();
    let both = destination(0, 7, 5, u64::MAX);
    step(&mut engine, 0, 0);
    lock_column(&mut engine, both, 0, 0.125, 0);
    lock_column(&mut engine, both, 0, 0.875, 1);
    lane(&mut engine, both, 0.0, 0.25);
    play(&mut engine);
    let mut events = select(&render_from_zero(&mut engine, 50), both);
    assert!(events.len() == 1);
    at(&events[0], 0, 0.875);
    engine.refresh_automation();
    events = select(&render(&mut engine, 1, 50, 127), both);
    assert!(events.len() == 1);
    at(&events[0], 50, 0.875);
    let mut engine = fixture();
    let (first, second) = (destination(0, 0, 10, 17), destination(0, 1, 11, 17));
    step(&mut engine, 0, 0);
    lock_column(&mut engine, first, 0, 0.125, 0);
    lock_column(&mut engine, second, 0, 0.875, 1);
    play(&mut engine);
    let mut events = render_from_zero(&mut engine, 1);
    events.retain(|event| event.event.type_ == ST_EVENT_HOST_PARAMETER);
    assert!(events.len() == 2 && first.matches(&events[0].event) && second.matches(&events[1].event));
    at(&events[0], 0, 0.125);
    at(&events[1], 0, 0.875);
    assert!(render_allocations() == 0);
}

/// "unchanged stepped and linear lanes resume at next row"
#[test]
fn unchanged_step_and_linear_lanes_resume_at_next_row() {
    for interpolation in [0, 1] {
        let mut engine = fixture();
        let destination = destination(0, -1, 511, u64::MAX);
        lane_with(&mut engine, destination, 0.0, 0.125, interpolation);
        lane_with(&mut engine, destination, 4.0, if interpolation != 0 { 0.125 } else { 0.625 }, interpolation);
        step(&mut engine, 0, 1);
        lock(&mut engine, destination, 1, 0.875);
        play(&mut engine);
        let events = select(&render(&mut engine, 257, 0, 43), destination);
        for event in &events {
            assert!(event.frame <= 100 || event.frame >= 200);
        }
        let locked = events.iter().find(|event| event.frame == 100);
        let resumed = events.iter().find(|event| event.frame == 200);
        assert!(locked.is_some() && resumed.is_some());
        at(locked.unwrap(), 100, 0.875);
        at_within(resumed.unwrap(), 200, if interpolation != 0 { 0.125 } else { 0.375 }, 1e-7);
        if interpolation != 0 {
            assert!(events.len() == 3);
            at(&events[0], 0, 0.125);
        } else {
            assert!(events.len() == 8);
            for event in &events {
                if event.frame != 100 {
                    at_within(event, event.frame, 0.125 + event.frame as f64 * 0.5 / 400.0, 1e-7);
                }
            }
        }
    }
    assert!(render_allocations() == 0);
}

/// "standalone parameter values hold without synthetic resets"
#[test]
fn standalone_locks_do_not_invent_resets() {
    let mut engine = fixture();
    let destination = destination(0, -1, 511, u64::MAX);
    step(&mut engine, 0, 1);
    lock(&mut engine, destination, 1, 0.75);
    step(&mut engine, 0, 3);
    lock(&mut engine, destination, 3, 0.25);
    play(&mut engine);
    let events = select(&render_from_zero(&mut engine, 701), destination);
    assert!(events.len() == 2);
    at(&events[0], 100, 0.75);
    at(&events[1], 300, 0.25);
    engine.refresh_automation();
    assert!(select(&render_from_zero(&mut engine, 1), destination).is_empty());
    assert!(render_allocations() == 0);
}

/// "OFF and CUT rows apply parameter locks"
#[test]
fn off_and_cut_rows_still_apply_locks() {
    let mut engine = fixture();
    let destination = destination(0, -1, 13, u64::MAX);
    step_note(&mut engine, 0, 0, 60);
    step_note(&mut engine, 0, 1, -1);
    lock(&mut engine, destination, 1, 0.25);
    step_note(&mut engine, 0, 2, -2);
    lock(&mut engine, destination, 2, 0.75);
    play(&mut engine);
    let events = render_from_zero(&mut engine, 301);
    let parameters = select(&events, destination);
    assert!(parameters.len() == 2);
    at(&parameters[0], 100, 0.25);
    at(&parameters[1], 200, 0.75);
    let off = events.iter().find(|event| event.event.type_ == ST_EVENT_NOTE_OFF && event.event.note == 60);
    assert!(off.is_some() && off.unwrap().frame == 100);
    assert!(render_allocations() == 0);
}

/// "stop, seek and project replacement clear stale locks"
#[test]
fn stop_seek_and_project_replacement_clear_active_locks() {
    let mut engine = fixture();
    let destination = destination(0, -1, 17, u64::MAX);
    lane(&mut engine, destination, 0.0, 0.25);
    step(&mut engine, 0, 0);
    lock(&mut engine, destination, 0, 0.875);
    play(&mut engine);
    let mut events = select(&render_from_zero(&mut engine, 50), destination);
    assert!(events.len() == 1);
    at(&events[0], 0, 0.875);
    engine.stop();
    render_from_zero(&mut engine, 1);
    engine.refresh_automation();
    assert!(select(&render_from_zero(&mut engine, 1), destination).is_empty());
    engine.seek(0, 2, 0);
    engine.play(0);
    events = select(&render_from_zero(&mut engine, 1), destination);
    assert!(events.len() == 1);
    at(&events[0], 0, 0.25);
    engine.seek(0, 0, 0);
    events = select(&render_from_zero(&mut engine, 1), destination);
    assert!(events.len() == 1);
    at(&events[0], 0, 0.875);
    engine.begin_update();
    step(&mut engine, 0, 0);
    engine.commit_update();
    events = select(&render_from_zero(&mut engine, 1), destination);
    assert!(events.len() == 1);
    at(&events[0], 0, 0.25);
    engine.begin_update();
    engine.clear_project();
    engine.commit_update();
    engine.refresh_automation();
    assert!(select(&render_from_zero(&mut engine, 1), destination).is_empty());
    assert!(render_allocations() == 0);
}

/// "coalesced refresh preserves the effective active lock"
#[test]
fn mailbox_refresh_reemits_effective_lock_without_queueing() {
    let mut engine = fixture();
    let destination = destination(0, -1, 511, u64::MAX);
    lane(&mut engine, destination, 0.0, 0.25);
    step(&mut engine, 0, 0);
    lock(&mut engine, destination, 0, 0.875);
    play(&mut engine);
    render_from_zero(&mut engine, 47);
    realtime(|| {
        for _ in 0..100000 {
            engine.refresh_automation();
        }
    });
    let mut events = select(&render(&mut engine, 1, 47, 127), destination);
    assert!(events.len() == 1);
    at(&events[0], 47, 0.875);
    assert!(select(&render(&mut engine, 52, 48, 127), destination).is_empty());
    events = select(&render(&mut engine, 1, 100, 127), destination);
    assert!(events.len() == 1);
    at(&events[0], 100, 0.25);
    assert!(engine.status().dropped_commands == 0 && engine.status().dropped_events == 0);
    assert!(render_allocations() == 0);
}

/// A refresh replays the active lock once; reading further buses of a block replays nothing.
#[test]
fn a_refresh_replays_the_active_lock_once() {
    let mut engine = fixture();
    let destination = destination(0, -1, 31, u64::MAX);
    step(&mut engine, 0, 1);
    lock(&mut engine, destination, 1, 0.75);
    step(&mut engine, 0, 3);
    lock(&mut engine, destination, 3, 0.25);
    play(&mut engine);
    let mut events = select(&render_bus(&mut engine, 0, 0, 150), destination);
    assert!(events.len() == 1);
    at(&events[0], 100, 0.75);
    let before = engine.status().seconds;
    assert!(render_bus(&mut engine, return_bus(ST_MIN_RETURN_BUS), 0, 150).is_empty());
    assert!(engine.status().seconds == before);
    assert!(select(&render_bus(&mut engine, 1, 150, 25), destination).is_empty());
    engine.refresh_automation();
    assert!(render_bus(&mut engine, 2, 150, 25).is_empty()); // The same block: the mailbox stays pending.
    events = select(&render_bus(&mut engine, 0, 175, 51), destination);
    assert!(events.len() == 1);
    at(&events[0], 175, 0.75);
    events = select(&render_bus(&mut engine, 1, 226, 125), destination);
    assert!(events.len() == 1);
    at(&events[0], 300, 0.25);
    assert!(render_bus(&mut engine, return_bus(ST_MIN_RETURN_BUS), 226, 125).is_empty());
    assert!(render_allocations() == 0);
}

/// "event pressure retries a still-active lock"
#[test]
fn saturated_queue_retries_active_lock() {
    let mut engine = fixture();
    engine.set_pattern(0, 128);
    for destination in 0..256 {
        engine.add_host_automation_point(0, destination, 1, 0, 0.0, 0.0);
        engine.add_host_automation_point(0, destination, 1, 0, 128.0, 1.0);
    }
    let destination = destination(0, -1, 511, u64::MAX);
    step(&mut engine, 0, 120);
    lock(&mut engine, destination, 120, 0.75);
    play(&mut engine);
    render_without_drain(&mut engine, 12001);
    assert!(engine.status().dropped_events > 0);
    assert!(select(&drain(&mut engine, 0), destination).is_empty());
    let events = select(&render(&mut engine, 64, 12001, 127), destination);
    assert!(events.len() == 1 && events[0].frame >= 12001 && events[0].frame < 12065);
    assert!(events[0].event.value == 0.75);
    assert!(render_allocations() == 0);
}

/// "setter validation and step replacement"
///
/// Dropped from the C++ group: the call with a null engine handle (it cannot be expressed).
#[test]
fn setter_validation_and_step_replacement() {
    let mut engine = fixture();
    let destination = destination(0, -1, 0, u64::MAX);
    step(&mut engine, 0, 0);
    let mut set = |pattern: i32, track: i32, row: i32, column: i32, slot: i32, host: i32, value: f64| {
        engine.set_plugin_step_fx(pattern, track, row, column, slot, host, u64::MAX, value)
    };
    assert!(!set(-1, 0, 0, 0, -1, 0, 0.5));
    assert!(!set(256, 0, 0, 0, -1, 0, 0.5));
    assert!(!set(0, -1, 0, 0, -1, 0, 0.5));
    assert!(!set(0, ST_TRACK_SANITY_LIMIT, 0, 0, -1, 0, 0.5));
    assert!(!set(0, 0, -1, 0, -1, 0, 0.5));
    assert!(!set(0, 0, 8, 0, -1, 0, 0.5));
    assert!(!set(0, 0, 0, -1, -1, 0, 0.5));
    assert!(!set(0, 0, 0, 2, -1, 0, 0.5));
    assert!(!set(0, 0, 0, 0, -2, 0, 0.5));
    assert!(!set(0, 0, 0, 0, ST_MAX_INSERTS, 0, 0.5));
    assert!(!set(0, 0, 0, 0, -1, -2, 0.5));
    assert!(!set(0, 0, 0, 0, -1, ST_MAX_HOST_DESTINATIONS, 0.5));
    assert!(!set(0, 0, 0, 0, -1, 0, -0.001));
    assert!(!set(0, 0, 0, 0, -1, 0, 1.001));
    assert!(!set(0, 0, 0, 0, -1, 0, f64::NAN));
    assert!(!set(0, 0, 0, 0, -1, 0, f64::INFINITY));
    // Sixteen inserts per channel: slots 8...15 are valid insert locks.
    assert!(set(0, 0, 0, 0, 8, 0, 0.5));
    assert!(set(0, 0, 0, 0, ST_MAX_INSERTS - 1, 0, 0.5));
    lock(&mut engine, destination, 0, 0.75);
    step(&mut engine, 0, 0); // Replacing the STStep removes its separately stored locks.
    step(&mut engine, 0, 2);
    lock(&mut engine, destination, 2, 0.0);
    step(&mut engine, 0, 3);
    lock(&mut engine, destination, 3, 1.0);
    play(&mut engine);
    let events = select(&render_from_zero(&mut engine, 401), destination);
    assert!(events.len() == 2);
    at(&events[0], 200, 0.0);
    at(&events[1], 300, 1.0);
    assert!(render_allocations() == 0);
}

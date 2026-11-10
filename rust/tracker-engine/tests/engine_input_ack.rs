//! Port of `tests/engine_input_ack_tests.cpp`: acknowledged input notes (`input_note` /
//! `pop_input_ack`): where the render thread applied each edge, run identity across transport
//! and project changes, early placement, capacity, and concurrent delivery.
//!
//! Renders marked `realtime` in the C++ run as the render thread here; every group ends with the
//! program's "zero measured render-thread allocations" assertion for its own thread.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations};
use engine_support_b::engine::{st_instrument_capacity, Engine};
use engine_support_b::tone::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tracker_engine::engine::control::Control;
use tracker_engine::engine::state::Shared;
use tracker_engine::engine::*;

fn render(e: &mut Engine, count: u32) {
    realtime(|| e.render_null(count));
}

fn tempos() -> [STClockTempoEvent; 3] {
    [
        STClockTempoEvent { beat: 0.0, bpm: 120.0 },
        STClockTempoEvent { beat: 0.5, bpm: 60.0 },
        STClockTempoEvent { beat: 1.5, bpm: 180.0 },
    ]
}

fn meters() -> [STClockMeterEvent; 2] {
    [
        STClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 },
        STClockMeterEvent { beat: 0.75, numerator: 7, denominator: 8 },
    ]
}

/// C++ `fixture(e, mapped = true, rows = 4, songLoop = false, swing = 0, variable = true)`.
fn fixture_with(e: &mut Engine, mapped: bool, rows: i32, song_loop: bool, swing: f32, variable: bool) {
    e.begin_update();
    e.clear_project();
    e.set_track_count(1);
    e.set_timing(120.0, 4, swing, 1.0);
    e.set_pattern(0, rows);
    e.set_pattern(1, rows);
    e.set_sequence(&[0, 1], song_loop, 0);
    let mut instrument = test_tone_instrument(TEST_SINE);
    load_test_tone(e, 0, TEST_SINE);
    instrument.attack = 0.0;
    instrument.decay = 0.0;
    instrument.sustain = 1.0;
    instrument.release = 0.01;
    instrument.volume = 0.3;
    e.set_instrument(0, instrument);
    let mut track = default_track();
    track.volume = 1.0;
    track.delay_send = 0.0;
    track.reverb_send = 0.0;
    e.set_track(0, track);
    e.set_host_mixing(true, 1);
    if mapped {
        let (tempos, meters) = (tempos(), meters());
        assert!(e
            .set_timing_map(&tempos[..if variable { 3 } else { 1 }], &meters[..if variable { 2 } else { 1 }], 91)
            .is_ok());
    }
    assert!(e.commit_update_checked().is_ok());
}

/// `fixture(e)`
fn fixture(e: &mut Engine) {
    fixture_with(e, true, 4, false, 0.0, true);
}

/// `fixture(e, mapped)`
fn fixture_mapped(e: &mut Engine, mapped: bool) {
    fixture_with(e, mapped, 4, false, 0.0, true);
}

/// C++ `edge(e, id, type = ST_INPUT_NOTE_ON, apply = false)`.
fn edge_with(e: &mut Engine, id: u64, kind: i32, apply: bool) -> bool {
    e.input_note(id, kind, 0, 0, 60, if kind == ST_INPUT_NOTE_ON { 100 } else { 0 }, apply as i32)
}

/// `edge(e, id)`: a note-on that is only acknowledged.
fn edge(e: &mut Engine, id: u64) -> bool {
    edge_with(e, id, ST_INPUT_NOTE_ON, false)
}

/// `edge(e, id, ST_INPUT_NOTE_OFF)`
fn edge_off(e: &mut Engine, id: u64) -> bool {
    edge_with(e, id, ST_INPUT_NOTE_OFF, false)
}

#[track_caller]
fn ack(e: &mut Engine) -> STInputAck {
    let a = e.pop_input_ack();
    assert!(a.is_some());
    a.unwrap()
}

#[track_caller]
fn empty(e: &mut Engine) {
    assert!(e.pop_input_ack().is_none());
}

#[track_caller]
fn equal(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-11);
}

fn peak(e: &mut Engine, frames: usize) -> f32 {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    e.render(&mut l, &mut r);
    l.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

/// The program's final `assert(rtAllocations==0)`.
fn zero_render_allocations() {
    assert!(render_allocations() == 0);
}

#[test]
fn exact_applied_onset_row_and_quarter_beat_distance_across_tempo_meter_boundaries() {
    let mut e = Engine::new();
    fixture(&mut e);
    e.play(1);
    assert!(edge(&mut e, 1));
    render(&mut e, 1);
    let first = ack(&mut e);
    assert!(first.status == ST_INPUT_ACK_APPLIED && first.pattern == 0 && first.row == 0 && first.sequence == 0);
    assert!(
        first.request_id == 1 && first.timing_revision == 91 && first.graph_frame == i64::MIN && first.render_frame == 0
    );
    equal(first.distance_beats, 0.0);
    render(&mut e, 5999);
    assert!(edge_off(&mut e, 2));
    render(&mut e, 1);
    let row = ack(&mut e);
    assert!(row.row == 1 && row.render_frame == 6000 && row.run_id == first.run_id);
    equal(row.distance_beats, 0.25);
    render(&mut e, 57999);
    assert!(edge(&mut e, 3));
    render(&mut e, 1);
    let late = ack(&mut e);
    assert!(
        late.pattern == 1 && late.row == 3 && late.sequence == 1 && late.render_frame == 64000 && late.run_id == first.run_id
    );
    equal(late.distance_beats, 1.75);
    empty(&mut e);
    zero_render_allocations();
}

#[test]
fn swung_pattern_onset_arm_changes_and_three_complete_loops_retain_recording_run_identity() {
    let mut e = Engine::new();
    fixture_with(&mut e, true, 2, false, 0.5, false);
    e.begin_update();
    assert!(e.set_pattern_timing_origin(0.25));
    e.commit_update();
    e.play(0);
    assert!(edge(&mut e, 1));
    render(&mut e, 1);
    let first = ack(&mut e);
    equal(first.distance_beats, 0.25);
    render(&mut e, 8999);
    assert!(edge(&mut e, 2));
    e.set_recording(true);
    render(&mut e, 1);
    let swung = ack(&mut e);
    assert!(swung.row == 1 && swung.run_id == first.run_id && (swung.flags & ST_INPUT_CLOCK_RECORDING) != 0);
    equal(swung.distance_beats, 0.625);
    render(&mut e, 26999);
    assert!(edge_off(&mut e, 3));
    render(&mut e, 1);
    let looped = ack(&mut e);
    equal(looped.distance_beats, 1.75);
    assert!(looped.row == 0 && looped.run_id == first.run_id);
    zero_render_allocations();
}

#[test]
fn mapped_fixed_song_wraps_and_unwrapped_legacy_pattern_travel_count_every_cycle() {
    for mapped in [false, true] {
        let mut e = Engine::new();
        fixture_with(&mut e, mapped, 2, true, 0.0, false);
        e.play(1);
        assert!(edge(&mut e, 1));
        render(&mut e, 1);
        let first = ack(&mut e);
        render(&mut e, 71999);
        assert!(edge_off(&mut e, 2));
        render(&mut e, 1);
        let last = ack(&mut e);
        equal(last.distance_beats - first.distance_beats, 3.0);
        assert!(last.run_id == first.run_id);
    }
    let mut legacy = Engine::new();
    fixture_with(&mut legacy, false, 2, false, 0.0, true);
    legacy.play(0);
    assert!(edge(&mut legacy, 1));
    render(&mut legacy, 36001);
    let first = ack(&mut legacy);
    assert!(edge(&mut legacy, 2));
    render(&mut legacy, 1);
    let last = ack(&mut legacy);
    equal(last.distance_beats - first.distance_beats, 36001.0 / 24000.0);
    zero_render_allocations();
}

#[test]
fn command_order_discontinuities_stopped_endpoints_restore_and_zero_audio_flush_outcomes() {
    let mut e = Engine::new();
    fixture(&mut e);
    e.play(1);
    assert!(edge(&mut e, 1));
    e.seek(0, 2, 0);
    assert!(edge_off(&mut e, 2));
    render(&mut e, 1);
    let (mut before, mut after) = (ack(&mut e), ack(&mut e));
    assert!(
        before.status == ST_INPUT_ACK_DISCONTINUITY
            && after.status == ST_INPUT_ACK_APPLIED
            && before.run_id != after.run_id
            && after.row == 2
    );
    assert!(edge(&mut e, 3));
    e.stop();
    assert!(edge_off(&mut e, 4));
    render(&mut e, 1);
    before = ack(&mut e);
    after = ack(&mut e);
    assert!(
        before.status == ST_INPUT_ACK_DISCONTINUITY
            && after.status == ST_INPUT_ACK_APPLIED
            && (after.flags & ST_INPUT_CLOCK_PLAYING) == 0
            && before.run_id != after.run_id
    );
    let saved = e.capture_transport();
    let stopped_run = after.run_id;
    e.restore_transport(saved, false);
    assert!(edge(&mut e, 5));
    render(&mut e, 1);
    assert!(ack(&mut e).run_id != stopped_run);
    assert!(edge(&mut e, 6));
    render(&mut e, 0);
    assert!(ack(&mut e).status == ST_INPUT_ACK_CONTROL_FLUSH);
    assert!(edge(&mut e, 7));
    let _ = e.capture_transport();
    assert!(ack(&mut e).status == ST_INPUT_ACK_CONTROL_FLUSH);
    zero_render_allocations();
}

#[test]
fn ordinary_publication_and_identical_timing_retain_run_semantic_map_edits_break_it() {
    let mut e = Engine::new();
    fixture(&mut e);
    e.play(1);
    assert!(edge(&mut e, 1));
    render(&mut e, 1);
    let first = ack(&mut e);
    e.begin_update();
    let mut p = test_tone_instrument(TEST_SINE);
    p.volume = 0.4;
    e.set_instrument(0, p);
    e.commit_update();
    assert!(edge(&mut e, 2));
    render(&mut e, 1);
    assert!(ack(&mut e).run_id == first.run_id);
    e.begin_update();
    let (mut t, m) = (tempos(), meters());
    assert!(e.set_timing_map(&t, &m, 91).is_ok());
    e.commit_update();
    assert!(edge(&mut e, 3));
    render(&mut e, 1);
    assert!(ack(&mut e).run_id == first.run_id);
    e.begin_update();
    t[1].bpm = 70.0;
    assert!(e.set_timing_map(&t, &m, 92).is_ok());
    e.commit_update();
    assert!(edge(&mut e, 4));
    render(&mut e, 1);
    let changed = ack(&mut e);
    assert!(changed.run_id != first.run_id && changed.timing_revision == 92);
    let pcm = [0.0f32; 16];
    e.begin_update();
    assert!(e.set_audio_asset(0, &pcm, Some(&pcm), 48000.0));
    e.add_audio_clip(0, 0, 4.0, 0.25, 0.0, 1.0, 0.0, 0.0, false);
    e.commit_update();
    assert!(edge(&mut e, 5));
    render(&mut e, 1);
    assert!(ack(&mut e).run_id != changed.run_id);
    let mut unsequenced = Engine::new();
    fixture(&mut unsequenced);
    unsequenced.begin_update();
    unsequenced.set_pattern(2, 2);
    unsequenced.set_sequence(&[0, 1], false, 2);
    unsequenced.commit_update();
    unsequenced.play(0);
    assert!(edge(&mut unsequenced, 1));
    render(&mut unsequenced, 1);
    let mut prior = ack(&mut unsequenced);
    unsequenced.begin_update();
    unsequenced.set_pattern(2, 3);
    unsequenced.commit_update();
    assert!(edge(&mut unsequenced, 2));
    render(&mut unsequenced, 1);
    assert!(ack(&mut unsequenced).run_id != prior.run_id);
    let mut collision = Engine::new();
    fixture(&mut collision);
    collision.play(1);
    assert!(edge(&mut collision, 1));
    render(&mut collision, 1);
    prior = ack(&mut collision);
    let same_length = [
        STClockTempoEvent { beat: 0.0, bpm: 120.0 },
        STClockTempoEvent { beat: 0.5, bpm: 80.0 },
        STClockTempoEvent { beat: 1.0, bpm: 48.0 },
        STClockTempoEvent { beat: 1.5, bpm: 180.0 },
    ];
    collision.begin_update();
    assert!(collision.set_timing_map(&same_length, &m, 91).is_ok());
    collision.commit_update();
    assert!(edge(&mut collision, 2));
    render(&mut collision, 1);
    assert!(ack(&mut collision).run_id != prior.run_id);
    zero_render_allocations();
}

/// `memcmp` over two float buffers: equal bit patterns.
fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn opt_in_note_pcm_equals_ordinary_apis_capture_only_sustain_release_leaves_voice_sounding() {
    let (mut ordinary, mut tracked) = (Engine::new(), Engine::new());
    fixture_mapped(&mut ordinary, false);
    fixture_mapped(&mut tracked, false);
    ordinary.note_on(0, 0, 60, 100);
    assert!(edge_with(&mut tracked, 1, ST_INPUT_NOTE_ON, true));
    let (mut al, mut ar, mut bl, mut br) = ([0.0f32; 1024], [0.0f32; 1024], [0.0f32; 1024], [0.0f32; 1024]);
    let mut compare = |ordinary: &mut Engine, tracked: &mut Engine, bl: &mut [f32; 1024]| {
        realtime(|| {
            ordinary.render(&mut al, &mut ar);
            tracked.render(bl, &mut br);
        });
        assert!(same_bits(&al, bl) && same_bits(&ar, &br));
    };
    compare(&mut ordinary, &mut tracked, &mut bl);
    assert!(ack(&mut tracked).apply_note == 1);
    assert!(edge_with(&mut tracked, 2, ST_INPUT_NOTE_OFF, false));
    compare(&mut ordinary, &mut tracked, &mut bl);
    assert!(ack(&mut tracked).apply_note == 0);
    let mut energy = 0.0f64;
    for x in bl {
        energy += (x * x) as f64;
    }
    assert!(energy > 1.0);
    ordinary.note_off(0, 60);
    assert!(edge_with(&mut tracked, 3, ST_INPUT_NOTE_OFF, true));
    compare(&mut ordinary, &mut tracked, &mut bl);
    assert!(ack(&mut tracked).apply_note == 1);
    compare(&mut ordinary, &mut tracked, &mut bl);
    zero_render_allocations();
}

#[test]
fn cached_overlapping_pulls_neither_duplicate_nor_prematurely_apply_acks_reset_breaks_run() {
    let mut e = Engine::new();
    fixture(&mut e);
    e.play(1);
    assert!(edge(&mut e, 1));
    realtime(|| e.render_bus_null(0, 0, 4097));
    let first = ack(&mut e);
    assert!(first.graph_frame == 0);
    assert!(edge(&mut e, 2));
    e.render_bus_null(16, 0, 4096);
    empty(&mut e);
    e.render_bus_null(0, 4097, 4096);
    let second = ack(&mut e);
    assert!(second.graph_frame == 4097 && second.render_frame == 4097 && second.run_id == first.run_id);
    e.reset_render_cache();
    assert!(edge(&mut e, 3));
    e.render_bus_null(0, 0, 1);
    assert!(ack(&mut e).run_id != first.run_id);
    zero_render_allocations();
}

#[test]
fn a_note_on_in_the_second_half_of_a_row_is_placed_on_the_next_row() {
    let mut e = Engine::new();
    fixture_mapped(&mut e, false);
    e.play(0);
    render(&mut e, 1000);
    assert!(edge(&mut e, 1));
    render(&mut e, 1);
    let first = ack(&mut e);
    assert!(first.row == 0 && first.early == 0);
    render(&mut e, 3000);
    assert!(edge(&mut e, 2));
    render(&mut e, 1);
    let early = ack(&mut e);
    assert!(early.row == 1 && early.early != 0 && early.pattern == 0);
    // At the last row nothing is ahead in the pattern: the note stays on its row.
    render(&mut e, 22000 - 4002);
    assert!(edge(&mut e, 3));
    render(&mut e, 1);
    let last = ack(&mut e);
    assert!(last.row == 3 && last.early == 0);
    zero_render_allocations();
}

fn row_one_note(e: &mut Engine) {
    e.begin_update();
    let mut step = default_step();
    step.note = 60;
    step.instrument = 0;
    e.set_step(0, 0, 1, step, &[]);
    e.commit_update();
}

#[test]
fn the_placed_row_skips_its_own_trigger_of_the_early_note_once() {
    for early in [false, true] {
        let mut e = Engine::new();
        fixture_mapped(&mut e, false);
        row_one_note(&mut e);
        e.play(0);
        render(&mut e, 4000);
        if early {
            assert!(edge(&mut e, 1));
        }
        let _before = peak(&mut e, 1990);
        let row1 = peak(&mut e, 3000);
        assert!(if early { row1 == 0.0 } else { row1 > 0.01f32 });
    }
    zero_render_allocations();
}

/// Seek, restore and Panic end the live voice the early note stood for, so the placed row
/// triggers normally.
#[test]
fn seek_restore_and_panic_forget_a_pending_early_note_its_row_triggers() {
    for transport in 0..3 {
        let mut e = Engine::new();
        fixture_mapped(&mut e, false);
        row_one_note(&mut e);
        e.play(0);
        render(&mut e, 4000);
        assert!(edge_with(&mut e, 1, ST_INPUT_NOTE_ON, true));
        render(&mut e, 1);
        assert!(ack(&mut e).early != 0);
        if transport == 0 {
            e.seek(0, 1, 0); // row 1 starts at once
        } else if transport == 1 {
            let saved = e.capture_transport();
            e.restore_transport(saved, true);
            render(&mut e, 1999); // resumes mid-row 0
        } else {
            e.panic();
            render(&mut e, 1999); // playback continues into row 1
        }
        assert!(peak(&mut e, 3000) > 0.01f32);
    }
    zero_render_allocations();
}

#[test]
fn strict_tokens_argument_bounds_and_reserved_ack_capacity_with_command_failure_rollback() {
    let mut e = Engine::new();
    assert!(!edge(&mut e, 0));
    assert!(!e.input_note(1, 3, 0, 0, 60, 100, 1));
    assert!(!e.input_note(1, 1, st_channel_count(ST_MIN_RETURN_BUS), 0, 60, 100, 1));
    assert!(!e.input_note(1, 1, 0, st_instrument_capacity(64), 60, 100, 1));
    assert!(!e.input_note(1, 1, 0, 0, 128, 100, 1));
    assert!(!e.input_note(1, 1, 0, 0, 60, 0, 1));
    assert!(!e.input_note(1, 1, 0, 0, 60, 100, 2));
    assert!(edge(&mut e, 1));
    assert!(!edge(&mut e, 1));
    render(&mut e, 1);
    assert!(ack(&mut e).request_id == 1);
    for id in 2..=4096u64 {
        assert!(edge(&mut e, id));
        if id % 1000 == 0 {
            render(&mut e, 1);
        }
    }
    render(&mut e, 1);
    assert!(!edge(&mut e, 4097));
    assert!(ack(&mut e).request_id == 2);
    assert!(edge(&mut e, 4097));
    render(&mut e, 1);
    for id in 3..=4097u64 {
        assert!(ack(&mut e).request_id == id);
    }
    empty(&mut e);
    assert!(edge(&mut e, u64::MAX));
    render(&mut e, 1);
    assert!(ack(&mut e).request_id == u64::MAX);
    assert!(!edge(&mut e, 1));
    assert!(e.status().dropped_events == 0);
    let mut full = Engine::new();
    for _ in 0..2047 {
        full.pitch_bend(0, 0.0);
    }
    assert!(!edge(&mut full, 1));
    render(&mut full, 1);
    assert!(edge(&mut full, 1));
    render(&mut full, 1);
    assert!(ack(&mut full).request_id == 1);
    zero_render_allocations();
}

/// The control half of an engine, lent to a producer thread while this thread renders.
///
/// TODO(api): the safe API cannot express the engine's own threading model ("one serial control
/// thread", "the render thread"): `Control` is not `Send` (a published project holds a raw
/// `retired_next` pointer) and `Shared` is not `Sync` (the voice snapshot's `UnsafeCell`), so a
/// host cannot hand the control half to its control thread without `unsafe`.
struct ControlHalf<'a> {
    control: &'a mut Control,
    shared: &'a Shared,
}

// SAFETY: this is the engine's documented contract (`engine/mod.rs`, "Threading model"): one
// thread uses the control half and the command ring's producer side while another renders;
// everything they share is in `Shared`, whose atomics and single-producer/single-consumer rings
// are built for exactly that. The producer thread is joined before the engine is used again.
unsafe impl Send for ControlHalf<'_> {}

#[test]
fn concurrent_single_producer_audio_consumer_20000_edge_delivery_without_lost_acks() {
    let mut e = Engine::new();
    let done = AtomicBool::new(false);
    let STEngine { shared, control, render, .. } = &mut *e.e;
    let shared: &Shared = shared;
    let producer_half = ControlHalf { control: control.get_mut(), shared };
    let render = render.get_mut();
    let mut expected: u64 = 1;
    std::thread::scope(|scope| {
        let done = &done;
        let producer = scope.spawn(move || {
            let half = producer_half;
            for id in 1..=20000u64 {
                while !half.control.input_note(half.shared, id, ST_INPUT_NOTE_ON, 0, 0, 60, 100, 0) {
                    std::thread::yield_now();
                }
            }
            done.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) || expected <= 20000 {
            realtime(|| render.render(shared, None, None, 1));
            while let Some(a) = shared.pop_input_ack() {
                assert!(a.request_id == expected && a.status == ST_INPUT_ACK_APPLIED);
                expected += 1;
            }
        }
        producer.join().expect("producer thread");
    });
    assert!(expected == 20001);
    empty(&mut e);
    zero_render_allocations();
}

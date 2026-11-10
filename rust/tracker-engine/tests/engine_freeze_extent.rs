//! Port of `tests/engine_freeze_extent_tests.cpp`: the frozen song's exact extent
//! (`set_frozen_song_extent`): admission, exact ends, removed clips, checked commits, loops,
//! restores and the admitted maximum.
//!
//! Renders and span queries run as the render thread; every group ends with the program's "zero
//! measured render/query allocation/deallocation" assertion for its own thread. The injected
//! allocation failure is the test allocator returning null once, which `set_frozen_song_extent`
//! reports as `Err("std::bad_alloc")`.
//!
//! Dropped from the C++ program (C pointer mechanics): the call with a null engine handle, and
//! the two-byte error buffer of capacity 1 (`tiny`) that must be terminated without touching the
//! byte after it; the failing call itself is kept.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{
    allocation_failure_pending, fail_next_allocation, realtime, render_allocations, render_deallocations,
};
use engine_support_b::engine::Engine;
use tracker_engine::engine::*;

fn commit(e: &mut Engine) {
    if let Err(error) = e.commit_update_checked() {
        panic!("Commit failed: {error}");
    }
}

/// C++ `extent(e, frames, publication = 7)`.
#[track_caller]
fn extent_for(e: &mut Engine, frames: i64, publication: u64) {
    assert!(e.set_frozen_song_extent(publication, frames).is_ok());
    commit(e);
}

#[track_caller]
fn extent(e: &mut Engine, frames: i64) {
    extent_for(e, frames, 7);
}

fn render(e: &mut Engine, at: u64, n: u32) {
    realtime(|| e.render_bus_null(0, at, n));
}

/// C++ `span(e, at, n = 65536, latency = 0)`.
#[track_caller]
fn span_with(e: &mut Engine, at: i64, n: u32, latency: i64) -> STFreezeSpan {
    let mut s = STFreezeSpan::default();
    let status = realtime(|| e.freeze_span(at, n, latency, &mut s));
    assert!(status == ST_CLOCK_OK && s.frames > 0);
    s
}

#[track_caller]
fn span(e: &mut Engine, at: i64) -> STFreezeSpan {
    span_with(e, at, 65536, 0)
}

fn onsets(e: &mut Engine) -> i32 {
    let mut n = 0;
    while let Some(event) = e.pop_event() {
        if event.type_ == ST_EVENT_NOTE_ON {
            n += 1;
        }
    }
    n
}

/// C++ `fixture(e, mapped = false, loop = false, bpm = 120, rows = 1, swing = 0)`.
fn fixture_with(e: &mut Engine, mapped: bool, loop_song: bool, bpm: f64, rows: i32, swing: f32) {
    e.begin_update();
    e.clear_project();
    e.set_track_count(1);
    e.set_timing(bpm, 4, swing, 1.0);
    e.set_host_mixing(true, 1);
    e.set_pattern(0, rows);
    e.set_sequence(&[0], loop_song, 0);
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_AU;
    e.set_instrument(0, instrument);
    let mut step = default_step();
    step.note = 60;
    step.instrument = 0;
    step.gate = 0.01;
    for row in 0..rows {
        e.set_step(0, 0, row, step, &[]);
    }
    if mapped {
        let tempos = [
            STClockTempoEvent { beat: 0.0, bpm: 120.0 },
            STClockTempoEvent { beat: 0.25, bpm: 60.0 },
            STClockTempoEvent { beat: 0.5, bpm: 180.0 },
        ];
        let meters = [
            STClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 },
            STClockMeterEvent { beat: 0.75, numerator: 7, denominator: 8 },
        ];
        assert!(e.set_timing_map(&tempos, &meters, 51).is_ok());
    }
    assert!(e.set_frozen_song_policy(1, 7).is_ok());
    commit(e);
}

/// `fixture(e)`
fn fixture(e: &mut Engine) {
    fixture_with(e, false, false, 120.0, 1, 0.0);
}

/// `fixture(e, mapped)`
fn fixture_mapped(e: &mut Engine, mapped: bool) {
    fixture_with(e, mapped, false, 120.0, 1, 0.0);
}

/// The program's final `assert(allocations==0&&deallocations==0)`.
fn zero_render_allocations() {
    assert!(render_allocations() == 0 && render_deallocations() == 0);
}

#[test]
fn bounded_extent_publication_admission_allocation_failure_idempotence_and_policy_reset() {
    let mut e = Engine::new();
    assert!(e.set_frozen_song_extent(7, 7000).is_err_and(|error| !error.is_empty()));
    fixture(&mut e);
    for bad in [i64::MIN, -1, 0, 100000001, i64::MAX] {
        assert!(e.set_frozen_song_extent(7, bad).is_err());
    }
    assert!(e.set_frozen_song_extent(0, 7000).is_err());
    assert!(e.set_frozen_song_extent(8, 7000).is_err());
    assert!(e.set_frozen_song_extent(0, 7000).is_err());
    fail_next_allocation();
    assert!(
        e.set_frozen_song_extent(7, 7000).is_err_and(|error| error == "std::bad_alloc")
            && !allocation_failure_pending()
    );
    assert!(e.duration(true) == 6000.0 / 48000.0);
    extent(&mut e, 7000);
    assert!(e.set_frozen_song_extent(7, 7000).is_ok());
    assert!(e.set_frozen_song_extent(7, 7001).is_err());
    commit(&mut e);
    assert!(e.duration(true) == 7000.0 / 48000.0);
    assert!(e.set_frozen_song_policy(1, 7).is_ok());
    commit(&mut e);
    assert!(e.duration(true) == 7000.0 / 48000.0);
    assert!(e.set_frozen_song_policy(1, 8).is_ok());
    commit(&mut e);
    assert!(e.duration(true) == 6000.0 / 48000.0);
    extent_for(&mut e, 8000, 8);
    assert!(e.set_frozen_song_policy(0, 8).is_ok());
    commit(&mut e);
    assert!(e.duration(true) == 6000.0 / 48000.0);
    assert!(e.set_frozen_song_extent(8, 8000).is_err());
    assert!(e.set_frozen_song_policy(1, 8).is_ok());
    commit(&mut e);
    assert!(e.duration(true) == 6000.0 / 48000.0);
    extent_for(&mut e, 8000, 8);
    e.clear_project();
    commit(&mut e);
    assert!(e.duration(true) == 8.0);
    assert!(e.set_frozen_song_extent(8, 8000).is_err());
    zero_render_allocations();
}

#[test]
fn exact_6005_6007_ends_avoid_both_floating_roundtrip_directions_in_legacy_and_mapped_songs() {
    for mapped in [false, true] {
        for end in [6005i64, 6007] {
            let mut e = Engine::new();
            fixture_mapped(&mut e, mapped);
            extent(&mut e, end);
            assert!(e.duration(true) == end as f64 / 48000.0);
            assert!(e.duration(false) == 6000.0 / 48000.0);
            e.play(1);
            render(&mut e, 0, (end + 50) as u32);
            assert!(onsets(&mut e) == 1);
            let (before, tail) = (span(&mut e, end - 1), span(&mut e, end));
            assert!(before.frames == 1 && before.context.phase == ST_FREEZE_SONG);
            assert!(tail.context.phase == ST_FREEZE_NATURAL_TAIL && tail.context.cause == ST_FREEZE_CAUSE_NATURAL_END);
            assert!(
                tail.context.transition_project_frame == end
                    && tail.context.transition_content_graph_frame == end
                    && tail.context.project_frame == end
            );
            assert!(
                span_with(&mut e, end + 20, 1, 0).context.project_frame == end + 20
                    && span_with(&mut e, end + 20, 1, 20).context.project_frame == end
            );
            assert!(e.status().seconds == end as f64 / 48000.0 && e.status().playing == 0);
        }
    }
    zero_render_allocations();
}

#[test]
fn removed_source_limited_clips_keep_exact_full_song_end_without_retaining_original_pcm() {
    for mapped in [false, true] {
        let mut e = Engine::new();
        fixture_mapped(&mut e, mapped);
        let pcm = vec![0.2f32; 1000];
        e.begin_update();
        assert!(e.set_audio_asset(0, &pcm, None, 48000.0));
        e.add_audio_clip(0, 0, 0.25, 0.0, 0.0, 1.0, 0.0, 0.0, false);
        commit(&mut e);
        assert!(e.duration(true) == 7000.0 / 48000.0);
        assert!(e.set_frozen_song_extent(7, 6999).is_ok());
        assert!(e.commit_update_checked().is_err_and(|error| error.contains("shorter")));
        assert!(e.duration(true) == 7000.0 / 48000.0);
        e.cancel_update();
        extent(&mut e, 7000);
        e.begin_update();
        e.clear_audio_clips();
        assert!(e.clear_audio_asset(0));
        commit(&mut e);
        assert!(e.duration(true) == 7000.0 / 48000.0);
        e.play(1);
        render(&mut e, 0, 7050);
        assert!(
            span_with(&mut e, 6999, 1, 0).context.phase == ST_FREEZE_SONG
                && span(&mut e, 7000).context.transition_project_frame == 7000
        );
        assert!(onsets(&mut e) == 1);
    }
    zero_render_allocations();
}

#[test]
fn checked_commit_rejects_later_live_arrangement_changes_atomically_and_compares_integer_boundaries() {
    let mut e = Engine::new();
    fixture(&mut e);
    extent(&mut e, 6007);
    e.begin_update();
    e.set_pattern(0, 2);
    assert!(e.commit_update_checked().is_err());
    assert!(e.duration(true) == 6007.0 / 48000.0);
    e.cancel_update();
    let pcm = vec![0.2f32; 7000];
    e.begin_update();
    assert!(e.set_audio_asset(0, &pcm, None, 48000.0));
    e.add_audio_clip(0, 0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, false);
    assert!(e.commit_update_checked().is_err());
    e.cancel_update();
    e.play(1);
    render(&mut e, 0, 6020);
    assert!(span(&mut e, 6007).context.transition_project_frame == 6007 && onsets(&mut e) == 1);
    let mut fractional = Engine::new();
    fixture(&mut fractional);
    fractional.begin_update();
    assert!(fractional.set_audio_asset(0, &pcm, None, 48000.0));
    fractional.add_audio_clip(0, 0, 0.0, 0.25000000002, 0.0, 1.0, 0.0, 0.0, false);
    assert!(fractional.set_frozen_song_extent(7, 6000).is_ok());
    assert!(fractional.commit_update_checked().is_err());
    fractional.cancel_update();
    fractional.begin_update();
    assert!(fractional.set_audio_asset(0, &pcm, None, 48000.0));
    fractional.add_audio_clip(0, 0, 0.0, 0.25000000002, 0.0, 1.0, 0.0, 0.0, false);
    extent(&mut fractional, 6001);
    fractional.play(1);
    render(&mut fractional, 0, 6020);
    assert!(span(&mut fractional, 6001).context.transition_project_frame == 6001);
    // Mapped projected integer6007 must not ceil a frame->seconds roundtrip.
    for is_mapped in [false, true] {
        let mut projected = Engine::new();
        fixture_mapped(&mut projected, is_mapped);
        let exact = vec![0.2f32; 6007];
        projected.begin_update();
        assert!(projected.set_audio_asset(0, &exact, None, 48000.0));
        projected.add_audio_clip(0, 0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, false);
        extent(&mut projected, 6007);
        projected.play(1);
        render(&mut projected, 0, 6008);
        assert!(span_with(&mut projected, 6007, 1, 0).context.transition_project_frame == 6007);
    }
    let mut tiny = Engine::with_rate(8000.0);
    fixture_with(&mut tiny, false, false, 960.0, 1, 0.0);
    let one = [0.2f32];
    tiny.begin_update();
    assert!(tiny.set_audio_asset(0, &one, None, 8000.0));
    tiny.add_audio_clip(0, 0, 100000.0, 1e-12, 0.0, 1.0, 0.0, 0.0, false);
    assert!(tiny.set_frozen_song_extent(7, 50000000).is_ok());
    assert!(tiny.commit_update_checked().is_err());
    tiny.cancel_update();
    tiny.begin_update();
    assert!(tiny.set_audio_asset(0, &one, None, 8000.0));
    tiny.add_audio_clip(0, 0, 100000.0, 1e-12, 0.0, 1.0, 0.0, 0.0, false);
    extent(&mut tiny, 50000001);
    zero_render_allocations();
}

#[test]
fn exact_multi_loop_span_clock_boundaries_preserve_independent_continuous_input_ack_run() {
    for mapped in [false, true] {
        let mut e = Engine::new();
        fixture_with(&mut e, mapped, true, 120.0, 1, 0.0);
        extent(&mut e, 6007);
        e.play(1);
        assert!(e.input_note(1, ST_INPUT_NOTE_ON, 0, 0, 60, 100, 0));
        render(&mut e, 0, 18022);
        let first = e.pop_input_ack();
        assert!(first.is_some());
        let first = first.unwrap();
        let (initial, next, third) = (span_with(&mut e, 0, 60000, 0), span(&mut e, 6007), span(&mut e, 12014));
        assert!(initial.frames == 6007 && next.frames == 6007 && third.frames == 6007);
        assert!(
            next.context.project_frame == 0
                && next.context.cause == ST_FREEZE_CAUSE_LOOP
                && next.context.playback_id > initial.context.playback_id
                && third.context.playback_id > next.context.playback_id
        );
        assert!(e.input_note(2, ST_INPUT_NOTE_OFF, 0, 0, 60, 0, 0));
        render(&mut e, 18022, 1);
        let last = e.pop_input_ack();
        assert!(last.is_some_and(|last| last.run_id == first.run_id && last.distance_beats > first.distance_beats));
        let mut clock = STClockSpan::default();
        assert!(e.clock_span(6006, 1, 0, &mut clock) == ST_CLOCK_OK);
        let expected = if mapped { 0.25 + 7.0 / 48000.0 } else { 6007.0 / 24000.0 };
        assert!((clock.context.loop_end_beat - expected).abs() < 1e-12);
    }
    zero_render_allocations();
}

#[test]
fn mid_extension_restore_keeps_cursor_without_retrigger_and_stop_preempts_natural_completion() {
    for mapped in [false, true] {
        let mut e = Engine::new();
        fixture_mapped(&mut e, mapped);
        extent(&mut e, 9007);
        e.play(1);
        render(&mut e, 0, 7003);
        let original = e.capture_transport();
        assert!(original.playing != 0 && original.sequence_finished != 0 && original.transport_frames == 7003.0);
        e.restore_transport(original, true);
        render(&mut e, 0, 2005);
        assert!(onsets(&mut e) == 0);
        assert!(
            span_with(&mut e, 2003, 1, 0).context.project_frame == 9006
                && span_with(&mut e, 2004, 1, 0).context.phase == ST_FREEZE_NATURAL_TAIL
                && span_with(&mut e, 2004, 1, 0).context.transition_project_frame == 9007
        );
        e.restore_transport(original, true);
        render(&mut e, 0, 2003);
        e.stop();
        render(&mut e, 2003, 2);
        assert!(
            span(&mut e, 2003).context.phase == ST_FREEZE_STOPPED
                && span(&mut e, 2003).context.cause == ST_FREEZE_CAUSE_STOP
                && onsets(&mut e) == 0
        );
    }
    zero_render_allocations();
}

#[test]
fn incompatible_replacement_restored_cursors_clamp_stopped_without_future_notes_loops_or_fabricated_tails() {
    for mapped in [false, true] {
        let mut e = Engine::new();
        fixture_with(&mut e, mapped, true, 120.0, 1, 0.0);
        extent(&mut e, 12007);
        e.play(1);
        render(&mut e, 0, 9000);
        onsets(&mut e);
        e.begin_update();
        assert!(e.set_frozen_song_policy(1, 8).is_ok());
        extent_for(&mut e, 7007, 8);
        render(&mut e, 9000, 100);
        let s = span(&mut e, 9000);
        assert!(
            s.context.phase == ST_FREEZE_STOPPED
                && s.context.cause == ST_FREEZE_CAUSE_REPLACEMENT
                && s.context.project_frame == 7007
                && e.status().playing == 0
                && onsets(&mut e) == 0
        );
        let mut invalid = e.capture_transport();
        invalid.playing = 1;
        invalid.transport_frames = 9000.0;
        e.restore_transport(invalid, true);
        render(&mut e, 0, 100);
        assert!(
            span(&mut e, 0).context.phase == ST_FREEZE_STOPPED
                && span(&mut e, 0).context.cause == ST_FREEZE_CAUSE_RESTORE
                && span(&mut e, 0).context.project_frame == 7007
                && e.status().playing == 0
                && onsets(&mut e) == 0
        );
    }
    zero_render_allocations();
}

#[test]
fn fractional_row_checkpoint_rollback_republishes_old_policy_and_pattern_audition_ignores_full_song_override() {
    let mut e = Engine::new();
    fixture_with(&mut e, false, false, 123.0, 4, 0.3);
    e.play(1);
    e.seek(0, 1, 0);
    render(&mut e, 0, 111);
    let original = e.capture_transport();
    extent(&mut e, 30007);
    e.restore_transport(original, true);
    render(&mut e, 0, 100);
    assert!(onsets(&mut e) == 0);
    let during = e.capture_transport();
    assert!(during.transport_frames == original.transport_frames + 100.0);
    assert!(e.set_frozen_song_policy(0, 0).is_ok());
    commit(&mut e);
    e.restore_transport(original, true);
    let rollback = e.capture_transport();
    assert!(
        rollback.transport_frames == original.transport_frames
            && rollback.row_remaining == original.row_remaining
            && rollback.playing == original.playing
    );
    assert!(e.set_frozen_song_policy(1, 9).is_ok());
    extent_for(&mut e, 30007, 9);
    e.restore_transport(during, true);
    assert!(e.duration(true) == 30007.0 / 48000.0);
    e.play(0);
    render(&mut e, 0, 1);
    assert!(span(&mut e, 0).context.phase == ST_FREEZE_UNSUPPORTED && e.duration(false) < e.duration(true));
    zero_render_allocations();
}

#[test]
fn maximum_admitted_extent_sample_rate_bounds_render_exact_end_without_long_duration_allocation() {
    for rate in [8000.0, 48000.0, 384000.0] {
        let mut e = Engine::with_rate(rate);
        fixture(&mut e);
        extent_for(&mut e, 100000000, 7);
        e.play(1);
        let mut near = e.capture_transport();
        near.transport_frames = 99999999.0;
        near.sequence_finished = 1;
        near.row_has_started = 1;
        near.row_remaining = 1e100;
        e.restore_transport(near, true);
        render(&mut e, 0, 2);
        assert!(
            span_with(&mut e, 0, 1, 0).context.project_frame == 99999999
                && span_with(&mut e, 1, 1, 0).context.transition_project_frame == 100000000
        );
        assert!(e.duration(true) == 100000000.0 / rate);
    }
    zero_render_allocations();
}

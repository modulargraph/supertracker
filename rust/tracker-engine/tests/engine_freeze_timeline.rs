//! Port of `tests/engine_freeze_timeline_tests.cpp`: the frozen-song timeline (`freeze_span`):
//! policy, exact project frames across seeks/loops/restores, natural tails, retained history and
//! query bounds.
//!
//! Renders and span queries run as the render thread; every group ends with the program's "zero
//! measured render/query allocations" assertion for its own thread. The C++ program injected an
//! allocation failure by making its replaced `operator new` throw once; here the test binary's
//! allocator returns null once, which `set_frozen_song_policy` reports as `Err("std::bad_alloc")`.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{allocation_failure_pending, fail_next_allocation, realtime, render_allocations};
use engine_support_b::engine::Engine;
use tracker_engine::engine::*;

fn commit(e: &mut Engine) {
    assert!(e.commit_update_checked().is_ok());
}

/// C++ `policy(e, enable = true, publication = 7)`.
fn policy_with(e: &mut Engine, enable: bool, publication: u64) {
    assert!(e.set_frozen_song_policy(enable as i32, publication).is_ok());
    commit(e);
}

fn policy(e: &mut Engine) {
    policy_with(e, true, 7);
}

fn render(e: &mut Engine, at: u64, count: u32) {
    realtime(|| e.render_bus_null(0, at, count));
}

/// C++ `span(e, at, n = 65536, latency = 0)`.
#[track_caller]
fn span_with(e: &mut Engine, at: i64, n: u32, latency: i64) -> STFreezeSpan {
    let mut result = STFreezeSpan::default();
    let status = realtime(|| e.freeze_span(at, n, latency, &mut result));
    assert!(status == ST_CLOCK_OK && result.frames > 0);
    result
}

#[track_caller]
fn span(e: &mut Engine, at: i64) -> STFreezeSpan {
    span_with(e, at, 65536, 0)
}

fn notes(e: &mut Engine, offset: i64) -> Vec<i64> {
    let mut result = Vec::new();
    while let Some(v) = e.pop_event() {
        if v.type_ == ST_EVENT_NOTE_ON {
            result.push(offset + v.frame_offset as i64);
        }
    }
    result
}

/// C++ `fixture(e, bpm = 123, rows = 4, swing = .3, loop = false, frozen = true, mapped = false)`.
fn fixture_with(e: &mut Engine, bpm: f64, rows: i32, swing: f32, loop_song: bool, frozen: bool, mapped: bool) {
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
    step.gate = 0.1;
    for row in 0..rows {
        e.set_step(0, 0, row, step, &[]);
    }
    if mapped {
        let tempos = [
            STClockTempoEvent { beat: 0.0, bpm: 120.0 },
            STClockTempoEvent { beat: 0.5, bpm: 60.0 },
            STClockTempoEvent { beat: 1.5, bpm: 180.0 },
        ];
        let meters = [
            STClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 },
            STClockMeterEvent { beat: 0.75, numerator: 7, denominator: 8 },
        ];
        assert!(e.set_timing_map(&tempos, &meters, 51).is_ok());
    }
    if frozen {
        assert!(e.set_frozen_song_policy(1, 7).is_ok());
    }
    commit(e);
}

/// `fixture(e)`: 123 BPM, four rows, swing .3, frozen.
fn fixture(e: &mut Engine) {
    fixture_with(e, 123.0, 4, 0.3, false, true, false);
}

/// `fixture(e, bpm, rows, swing)`: not looping, frozen, unmapped.
fn fixture_plain(e: &mut Engine, bpm: f64, rows: i32, swing: f32) {
    fixture_with(e, bpm, rows, swing, false, true, false);
}

/// The program's final `assert(allocations==0)`.
fn zero_render_allocations() {
    assert!(render_allocations() == 0);
}

const SWING: f64 = 0.3f32 as f64;

#[test]
fn default_off_policy_bounded_allocation_failure_and_invalid_policy_rollback() {
    let mut e = Engine::new();
    let mut out = STFreezeSpan { frames: 123, ..STFreezeSpan::default() };
    assert!(e.freeze_span(0, 1, 0, &mut out) == ST_FREEZE_NOT_CONFIGURED && out.frames == 123);
    assert!(e.set_frozen_song_policy(2, 7).is_err_and(|error| !error.is_empty()));
    assert!(e.set_frozen_song_policy(1, 0).is_err());
    fail_next_allocation();
    assert!(e.set_frozen_song_policy(1, 7).is_err_and(|error| error == "std::bad_alloc") && !allocation_failure_pending());
    assert!(e.freeze_span(0, 1, 0, &mut out) == ST_FREEZE_NOT_CONFIGURED);
    fixture_plain(&mut e, 120.0, 4, 0.0);
    render(&mut e, 0, 1);
    assert!(span(&mut e, 0).context.phase == ST_FREEZE_UNSUPPORTED);
    assert!(e.set_frozen_song_policy(1, 0).is_err());
    render(&mut e, 1, 1);
    assert!(span(&mut e, 1).context.publication_id == 7);
    policy_with(&mut e, false, 7);
    render(&mut e, 2, 1);
    assert!(span(&mut e, 2).context.phase == ST_FREEZE_DISABLED);
    zero_render_allocations();
}

#[test]
fn fractional_swung_row_seek_rounds_once_carries_remainder_and_pdc_startup_is_silent() {
    let mut e = Engine::new();
    fixture(&mut e);
    e.play(1);
    e.seek(0, 1, 0);
    render(&mut e, 0, 16000);
    let base = 48000.0 * 60.0 / 123.0 / 4.0;
    let start = (1.0 + SWING) * base;
    let first = span(&mut e, 0);
    assert!(first.context.project_frame == start.ceil() as i64 && first.context.cause == ST_FREEZE_CAUSE_SEEK);
    let events = notes(&mut e, 0);
    let expected =
        vec![0, ((2.0 * base).ceil() - start.ceil()) as i64, (((3.0 + SWING) * base).ceil() - start.ceil()) as i64];
    assert!(events == expected);
    let pre = span_with(&mut e, 0, 200, 100);
    assert!(
        pre.frames == 100
            && pre.context.phase == ST_FREEZE_STOPPED
            && (pre.context.flags & ST_FREEZE_PREROLL) != 0
            && pre.context.content_graph_frame == -100
    );
    assert!(span_with(&mut e, 100, 1, 100).context.project_frame == first.context.project_frame);
    zero_render_allocations();
}

#[test]
fn mid_row_restore_avoids_retrigger_exact_rollback_and_pending_row_start_remainder_survive() {
    let mut e = Engine::new();
    fixture_with(&mut e, 123.0, 4, 0.3, false, false, false);
    e.play(1);
    e.seek(0, 1, 0);
    e.render_null(101);
    let original = e.capture_transport();
    assert!(original.row_has_started != 0 && original.transport_frames != original.transport_frames.ceil());
    policy(&mut e);
    e.restore_transport(original, true);
    let aligned = e.capture_transport();
    let delta = original.transport_frames.ceil() - original.transport_frames;
    assert!(
        aligned.transport_frames == original.transport_frames.ceil()
            && (aligned.row_remaining - (original.row_remaining - delta)).abs() < 1e-9
            && aligned.row_has_started != 0
    );
    e.restore_transport(aligned, true);
    render(&mut e, 0, 1);
    assert!(notes(&mut e, 0).is_empty());
    assert!(span(&mut e, 0).context.cause == ST_FREEZE_CAUSE_RESTORE);
    policy_with(&mut e, false, 7);
    e.restore_transport(original, true);
    let rollback = e.capture_transport();
    assert!(
        rollback.transport_frames == original.transport_frames
            && rollback.row_remaining == original.row_remaining
            && rollback.row_has_started == original.row_has_started
    );
    let mut start = Engine::new();
    fixture(&mut start);
    start.play(1);
    start.seek(0, 1, 0);
    let before = start.capture_transport();
    assert!(before.row_has_started == 0 && before.row_remaining < 0.0);
    start.restore_transport(before, true);
    let after = start.capture_transport();
    assert!(after.row_remaining == before.row_remaining && after.transport_frames == before.transport_frames);
    start.restore_transport(after, true);
    render(&mut start, 0, 16000);
    let restored_notes = notes(&mut start, 0);
    let base = 48000.0 * 60.0 / 123.0 / 4.0;
    let origin = ((1.0 + SWING) * base).ceil();
    assert!(
        restored_notes == vec![0, ((2.0 * base).ceil() - origin) as i64, (((3.0 + SWING) * base).ceil() - origin) as i64]
    );
    zero_render_allocations();
}

#[test]
fn authoritative_natural_tail_transition_versus_stop_one_sample_before_completion() {
    let (mut natural, mut stopped) = (Engine::new(), Engine::new());
    fixture_plain(&mut natural, 120.0, 1, 0.0);
    fixture_plain(&mut stopped, 120.0, 1, 0.0);
    natural.play(1);
    stopped.play(1);
    render(&mut natural, 0, 6100);
    render(&mut stopped, 0, 5999);
    stopped.stop();
    render(&mut stopped, 5999, 101);
    let (before, tail) = (span(&mut natural, 5998), span(&mut natural, 6000));
    assert!(before.frames == 2 && before.context.phase == ST_FREEZE_SONG);
    assert!(
        tail.context.phase == ST_FREEZE_NATURAL_TAIL
            && tail.context.cause == ST_FREEZE_CAUSE_NATURAL_END
            && tail.context.transition_project_frame == 6000
            && tail.context.transition_content_graph_frame == 6000
            && tail.context.playback_id == before.context.playback_id
    );
    assert!(span(&mut natural, 6050).context.project_frame == 6050);
    let stop = span(&mut stopped, 5999);
    assert!(
        stop.context.phase == ST_FREEZE_STOPPED
            && stop.context.cause == ST_FREEZE_CAUSE_STOP
            && stop.context.project_frame == 5999
            && stop.frames == 101
    );
    natural.stop();
    render(&mut natural, 6100, 1);
    assert!(
        span(&mut natural, 6100).context.phase == ST_FREEZE_STOPPED
            && span(&mut natural, 6100).context.cause == ST_FREEZE_CAUSE_STOP
    );
    zero_render_allocations();
}

#[test]
fn mapped_project_frame_identity_and_clip_extended_exact_musical_end() {
    let mut e = Engine::new();
    fixture_with(&mut e, 120.0, 8, 0.0, false, true, true);
    e.play(1);
    render(&mut e, 0, 65536);
    render(&mut e, 65536, 2564);
    let tail = span(&mut e, 68000);
    assert!(tail.context.phase == ST_FREEZE_NATURAL_TAIL && tail.context.transition_project_frame == 68000);
    let _onsets = notes(&mut e, 0); // last pull offsets differ, so query native retained mapping separately.
    assert!(span_with(&mut e, 64000, 1, 0).context.project_frame == 64000);
    let mut clips = Engine::new();
    fixture_plain(&mut clips, 120.0, 1, 0.0);
    let pcm = vec![0.2f32; 1000];
    clips.begin_update();
    assert!(clips.set_audio_asset(0, &pcm, Some(&pcm), 48000.0));
    clips.add_audio_clip(0, 0, 0.25, 0.0, 0.0, 1.0, 0.0, 0.0, false);
    commit(&mut clips);
    assert!(clips.duration(true) == 7000.0 / 48000.0);
    clips.play(1);
    render(&mut clips, 0, 7100);
    let extended = span(&mut clips, 7000);
    assert!(extended.context.phase == ST_FREEZE_NATURAL_TAIL && extended.context.transition_project_frame == 7000);
    let mut fractional = Engine::new();
    fixture_plain(&mut fractional, 120.0, 1, 0.0);
    let longer = vec![0.2f32; 7000];
    fractional.begin_update();
    assert!(fractional.set_audio_asset(0, &longer, Some(&longer), 48000.0));
    fractional.add_audio_clip(0, 0, 0.0, 0.25000000002, 0.0, 1.0, 0.0, 0.0, false);
    commit(&mut fractional);
    assert!((fractional.duration(true) * 48000.0).ceil() == 6001.0);
    fractional.play(1);
    render(&mut fractional, 0, 6010);
    assert!(
        span(&mut fractional, 6000).context.phase == ST_FREEZE_SONG
            && span(&mut fractional, 6001).context.transition_project_frame == 6001
            && span(&mut fractional, 6001).context.phase == ST_FREEZE_NATURAL_TAIL
    );
    zero_render_allocations();
}

#[test]
fn many_loops_per_pull_split_every_playback_identity_while_ack_run_remains_continuous() {
    let mut e = Engine::new();
    fixture_with(&mut e, 999.0, 1, 0.0, true, true, false);
    e.begin_update();
    e.set_timing(999.0, 32, 0.0, 1.0);
    commit(&mut e);
    e.play(1);
    assert!(e.input_note(1, ST_INPUT_NOTE_ON, 0, 0, 60, 100, 0));
    render(&mut e, 0, 65000);
    let first = e.pop_input_ack();
    assert!(first.is_some());
    let first = first.unwrap();
    let mut cursor: i64 = 0;
    let mut prior: u64 = 0;
    let mut loops = 0;
    let length = (48000.0 * 60.0 / 999.0 / 32.0f64).ceil() as i64;
    while cursor < 65000 {
        let s = span(&mut e, cursor);
        assert!(s.context.phase == ST_FREEZE_SONG && s.context.project_frame == 0 && s.context.playback_id > prior);
        assert!(s.frames as i64 == length.min(65000 - cursor));
        if cursor != 0 {
            assert!(s.context.cause == ST_FREEZE_CAUSE_LOOP);
        }
        prior = s.context.playback_id;
        cursor += s.frames as i64;
        loops += 1;
    }
    assert!(loops > 700);
    assert!(e.input_note(2, ST_INPUT_NOTE_OFF, 0, 0, 60, 0, 0));
    render(&mut e, 65000, 1);
    let last = e.pop_input_ack();
    assert!(last.is_some_and(|last| last.run_id == first.run_id && last.distance_beats > first.distance_beats));
    zero_render_allocations();
}

#[test]
fn retained_publications_integer_pdc_seek_panic_reset_causes_and_pattern_mode_exclusion() {
    let mut e = Engine::new();
    fixture_plain(&mut e, 120.0, 64, 0.0);
    e.play(1);
    render(&mut e, 1000, 4097);
    let initial = span(&mut e, 1000);
    policy_with(&mut e, true, u64::MAX);
    render(&mut e, 5097, 4096);
    let old = span_with(&mut e, 7096, 4096, 2000);
    assert!(old.context.publication_id == 7 && old.frames == 1);
    let changed = span_with(&mut e, 7097, 4096, 2000);
    assert!(
        changed.context.publication_id == u64::MAX
            && changed.context.cause == ST_FREEZE_CAUSE_REPLACEMENT
            && changed.context.project_frame == 4097
            && changed.context.playback_id != initial.context.playback_id
    );
    e.seek(0, 3, 0);
    render(&mut e, 9193, 2);
    let seek = span(&mut e, 9193);
    assert!(seek.context.cause == ST_FREEZE_CAUSE_SEEK && seek.context.project_frame == 18000);
    e.panic();
    render(&mut e, 9195, 1);
    assert!(span(&mut e, 9195).context.cause == ST_FREEZE_CAUSE_PANIC);
    e.reset_render_cache();
    render(&mut e, 0, 1);
    assert!(span(&mut e, 0).context.cause == ST_FREEZE_CAUSE_GRAPH_RESET);
    e.play(0);
    render(&mut e, 1, 1);
    assert!(span(&mut e, 1).context.phase == ST_FREEZE_UNSUPPORTED);
    zero_render_allocations();
}

#[test]
fn retained_window_exhaustion_overflow_bounds_and_no_fabricated_pre_allocation_history() {
    let mut e = Engine::new();
    fixture_with(&mut e, 120.0, 64, 0.0, true, true, false);
    e.play(1);
    render(&mut e, 0, 65536);
    let mut at: u64 = 65536;
    while at < 327680 {
        render(&mut e, at, 65536);
        at += 65536;
    }
    let mut out = STFreezeSpan { frames: 77, ..STFreezeSpan::default() };
    assert!(e.freeze_span(65535, 1, 0, &mut out) == ST_ENGINE_CLOCK_NOT_RETAINED && out.frames == 77);
    assert!(span_with(&mut e, 65536, 1, 0).context.content_graph_frame == 65536);
    assert!(e.freeze_span(327680, 1, 0, &mut out) == ST_ENGINE_CLOCK_NOT_PREPARED);
    assert!(e.freeze_span(0, 0, 0, &mut out) == ST_CLOCK_INVALID);
    assert!(e.freeze_span(0, 65537, 0, &mut out) == ST_CLOCK_INVALID);
    assert!(e.freeze_span(0, 1, -1, &mut out) == ST_CLOCK_INVALID);
    assert!(e.freeze_span(i64::MAX, 1, 0, &mut out) == ST_CLOCK_INVALID);
    assert!(e.freeze_span(0, 1, (1i64 << 30) + 1, &mut out) == ST_CLOCK_INVALID);
    let mut late = Engine::new();
    fixture_with(&mut late, 120.0, 4, 0.0, false, false, false);
    late.play(1);
    render(&mut late, 0, 10);
    policy(&mut late);
    render(&mut late, 10, 10);
    assert!(late.freeze_span(0, 1, 0, &mut out) == ST_ENGINE_CLOCK_NOT_RETAINED);
    assert!(late.freeze_span(-1, 1, 0, &mut out) == ST_ENGINE_CLOCK_NOT_RETAINED);
    assert!(span(&mut late, 10).context.publication_id == 7);
    let mut limit = Engine::new();
    fixture_plain(&mut limit, 120.0, 4, 0.0);
    limit.play(1);
    render(&mut limit, 1u64 << 52, 1);
    assert!(span_with(&mut limit, 1i64 << 52, 128, 100).frames == 1);
    zero_render_allocations();
}

#[test]
fn mix_only_edits_retain_natural_tail_replacement_and_restore_cannot_synthesize_completion() {
    let mut e = Engine::new();
    fixture_plain(&mut e, 120.0, 1, 0.0);
    e.play(1);
    render(&mut e, 0, 6010);
    let ended = span(&mut e, 6000).context;
    e.begin_update();
    let mut track = default_track();
    track.volume = 0.4;
    e.set_track(0, track);
    commit(&mut e);
    render(&mut e, 6010, 10);
    let unchanged = span(&mut e, 6010).context;
    assert!(
        unchanged.phase == ST_FREEZE_NATURAL_TAIL
            && unchanged.transition_id == ended.transition_id
            && unchanged.project_frame == 6010
    );
    policy_with(&mut e, true, 8);
    render(&mut e, 6020, 1);
    assert!(
        span(&mut e, 6020).context.phase == ST_FREEZE_STOPPED
            && span(&mut e, 6020).context.cause == ST_FREEZE_CAUSE_REPLACEMENT
    );
    let saved = e.capture_transport();
    e.restore_transport(saved, false);
    render(&mut e, 0, 1);
    assert!(
        span(&mut e, 0).context.phase == ST_FREEZE_STOPPED && span(&mut e, 0).context.cause == ST_FREEZE_CAUSE_RESTORE
    );
    zero_render_allocations();
}

#[test]
fn query_never_advances_transport_silent_anchor_prepares_exactly_once_across_overlapping_pulls() {
    let mut e = Engine::new();
    fixture_plain(&mut e, 120.0, 4, 0.0);
    e.begin_update();
    e.set_host_mixing(true, 0);
    commit(&mut e);
    e.play(1);
    assert!(e.input_note(1, ST_INPUT_NOTE_ON, 0, 0, 60, 100, 0));
    let mut pending = STFreezeSpan::default();
    assert!(e.freeze_span(0, 4097, 0, &mut pending) == ST_ENGINE_CLOCK_NOT_PREPARED);
    assert!(e.status().playing == 0 && e.pop_input_ack().is_none());
    // A silent transport anchor may prepare any bus with null PCM outputs.
    // It does not require an unfrozen track or an audible source node.
    realtime(|| e.render_bus_null(st_return_bus(ST_MIN_RETURN_BUS), 0, 4097));
    assert!(e.pop_input_ack().is_some_and(|ack| ack.graph_frame == 0));
    assert!(span_with(&mut e, 0, 4097, 0).frames == 4097 && span(&mut e, 0).context.phase == ST_FREEZE_SONG);
    let seconds = e.status().seconds;
    render(&mut e, 0, 4096);
    render(&mut e, 0, 4097);
    assert!(e.status().seconds == seconds && e.pop_input_ack().is_none());
    render(&mut e, 4096, 4096);
    assert!(span(&mut e, 4096).context.project_frame == 4096 && e.status().seconds == 8192.0 / 48000.0);
    zero_render_allocations();
}

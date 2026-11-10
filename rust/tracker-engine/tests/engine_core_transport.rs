//! Port of the former C++ test program `engine_tests.cpp`, part 5 of 5: pattern loop ranges, queued jumps and mode
//! switches, count-in, the transport status that follows queued commands, the seek status
//! revision (with a concurrent status reader), the mod wheel, forgetting external voices with
//! the sequenced flag, and note columns.
//!
//! One `#[test]` per C++ suite (`testFooBar` is `test_foo_bar`), calling the engine's safe Rust
//! API through `engine_support_a::Engine`. Every C++ `assert` is one `assert!` here, with the
//! same inputs, expected values and tolerances; every test ends with the C++ program's final
//! "zero render-thread allocations" check.
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop, clippy::manual_is_multiple_of, clippy::type_complexity)]

mod engine_support_a;

use engine_support_a::core_helpers::*;
use engine_support_a::tone::*;
use engine_support_a::{assert_zero_render_allocations, Engine, SetOnDrop};
use std::sync::atomic::{AtomicBool, Ordering};
use tracker_engine::clock::{ClockMeterEvent, ClockSpan, ClockTempoEvent, CLOCK_LOOPING, CLOCK_OK};
use tracker_engine::engine::*;

// Pattern loop range: rows 0..15 each carry note 60+row; one row is 6000 frames at 120 BPM, 4 LPB.
#[test]
fn test_pattern_loop_range() {
    // No range: unchanged pattern playback and wrap.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.play(0);
        let on = note_ons(&mut e, 20 * 6000);
        assert!(on.len() == 20);
        for i in 0..on.len() {
            assert!(on[i].frame == i as u64 * 6000);
            assert!(on[i].e.note == 60 + (i % 16) as i32);
        }
    }
    // Range before Play: a cursor outside starts at the first loop row and wraps exactly at row boundaries.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.set_pattern_loop(4, 7);
        e.play(0);
        let mut on = note_ons(&mut e, 12 * 6000);
        assert!(on.len() == 12);
        for i in 0..on.len() {
            assert!(on[i].frame == i as u64 * 6000);
            assert!(on[i].e.note == 64 + (i % 4) as i32);
        }
        let status = e.status();
        assert!(status.playing != 0 && status.row >= 4 && status.row <= 7);
        // Stop and Play keep the range; a seek inside the range starts there.
        e.stop();
        e.seek(0, 6, 0);
        e.play(0);
        on = note_ons(&mut e, 4 * 6000);
        assert_notes(&on, &[(0, 66), (6000, 67), (12000, 64), (18000, 65)]);
        // Clearing resumes normal progression from the current row.
        e.set_pattern_loop(-1, -1);
        on = note_ons(&mut e, 4 * 6000);
        assert_notes(&on, &[(0, 66), (6000, 67), (12000, 68), (18000, 69)]);
    }
    // Set while playing past the range: wraps at the next row boundary, not at the pattern end.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.play(0);
        let mut on = note_ons(&mut e, 9 * 6000 + 100);
        assert!(on.len() == 10 && on.last().unwrap().e.note == 69);
        e.set_pattern_loop(2, 3);
        on = note_ons(&mut e, 4 * 6000);
        assert_notes(&on, &[(6000 - 100, 62), (12000 - 100, 63), (18000 - 100, 62)]);
    }
    // End row is clamped to the pattern length; one-row loops repeat that row.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.set_pattern_loop(14, 40);
        e.play(0);
        assert_notes(&note_ons(&mut e, 4 * 6000), &[(0, 74), (6000, 75), (12000, 74), (18000, 75)]);
    }
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.set_pattern_loop(9, 9);
        e.play(0);
        assert_notes(&note_ons(&mut e, 3 * 6000), &[(0, 69), (6000, 69), (12000, 69)]);
    }
    // A start row beyond the pattern disables the range.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.set_pattern_loop(20, 30);
        e.play(0);
        assert_notes(&note_ons(&mut e, 3 * 6000), &[(0, 60), (6000, 61), (12000, 62)]);
    }
    // Song mode ignores the range.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.set_pattern_loop(4, 7);
        e.play(1);
        let on = note_ons(&mut e, 16 * 6000);
        assert!(on.len() == 16);
        for i in 0..on.len() {
            assert!(on[i].frame == i as u64 * 6000 && on[i].e.note == 60 + i as i32);
        }
    }
    // Swing keeps per-row lengths: odd rows 4500 frames, even rows 7500.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.25);
        e.set_pattern_loop(1, 2);
        e.play(0);
        assert_notes(&note_ons(&mut e, 24000), &[(0, 61), (4500, 62), (12000, 61), (16500, 62)]);
    }
    // Mapped timing: the loop is the clock's loop domain and musical beats stay continuous for scheduling.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.begin_update();
        let tempos = [ClockTempoEvent { beat: 0.0, bpm: 120.0 }];
        let meters = [ClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 }];
        assert!(e.set_timing_map(&tempos, &meters, 5).is_ok());
        assert!(e.commit_update_checked().is_ok());
        e.set_pattern_loop(4, 7);
        e.play(0);
        let on = note_ons(&mut e, 12 * 6000);
        assert!(on.len() == 12);
        for i in 0..on.len() {
            assert!((on[i].frame as i64 - (i * 6000) as i64).abs() <= 1);
            assert!(on[i].e.note == 64 + (i % 4) as i32);
        }
        note_ons(&mut e, 1000);
        let status = e.status();
        assert!(status.row == 4 && status.beat >= 1.0 && status.beat < 1.1);
        let mut span = ClockSpan::default();
        let mut a = Audio::new(512);
        bus_render(&mut e, 0, 0, &mut a);
        assert!(e.clock_span(100, 64, 0, &mut span) == CLOCK_OK);
        assert!(span.context.flags & CLOCK_LOOPING != 0);
        assert!((span.context.loop_start_beat - 1.0).abs() < 1e-9 && (span.context.loop_end_beat - 2.0).abs() < 1e-9);
    }
    assert_zero_render_allocations();
}

// Queued jumps: pattern 0 = 16 rows (notes 60+row), pattern 1 = 4 rows (notes 80+row); 6000 frames per row.
#[test]
fn test_queued_jumps() {
    // Pattern mode: queued mid-pattern, honoured exactly at the pattern end; the new pattern then loops.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[0]);
        e.play(0);
        let mut on = note_ons(&mut e, 3 * 6000 + 100);
        assert!(on.len() == 4);
        e.queue_jump(1, -1);
        note_ons(&mut e, 100);
        let mut status = e.status();
        assert!(status.queued_pattern == 1 && status.queued_sequence == -1 && status.pattern == 0);
        on = note_ons(&mut e, 18 * 6000);
        assert!(on.len() == 18);
        for i in 0..12 {
            assert!(on[i].e.note == 64 + i as i32);
        }
        for i in 12..18 {
            assert!(on[i].e.note == 80 + ((i - 12) % 4) as i32);
            assert!(on[i].frame + 16 * 6000 - on[12].frame == 16 * 6000 + (i as u64 - 12) * 6000);
        }
        assert!(on[12].frame == 16 * 6000 - (3 * 6000 + 200));
        status = e.status();
        assert!(status.pattern == 1 && status.queued_pattern == -1);
    }
    // A later queue replaces the target; both <0 clears; Stop clears; stopped queues are ignored.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[0]);
        e.play(0);
        note_ons(&mut e, 1000);
        e.queue_jump(1, -1);
        e.queue_jump(-1, -1);
        let on = note_ons(&mut e, 17 * 6000);
        assert!(on.len() == 17 && on[15].e.note == 60);
        assert!(e.status().queued_pattern == -1);
        e.queue_jump(1, -1);
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == 1);
        e.stop();
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == -1);
        e.queue_jump(1, -1);
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == -1);
        e.play(0);
        note_ons(&mut e, 100);
        e.queue_jump(1, -1);
        e.seek(0, 0, 0);
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == -1);
    }
    // Pattern loop range: the jump happens at the end of the range instead of wrapping.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[0]);
        e.set_pattern_loop(4, 7);
        e.play(0);
        note_ons(&mut e, 100);
        e.queue_jump(1, -1);
        let on = note_ons(&mut e, 6 * 6000);
        assert_notes(&on, &[(6000 - 100, 65), (12000 - 100, 66), (18000 - 100, 67), (24000 - 100, 80), (30000 - 100, 81)]);
    }
    // Song mode: jump by sequence index at the end of the current block; then playback continues from there.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[0, 1, 0, 1]);
        e.play(1);
        note_ons(&mut e, 6000);
        e.queue_jump(-1, 2);
        note_ons(&mut e, 100);
        let mut status = e.status();
        assert!(status.queued_sequence == 2 && status.queued_pattern == 0);
        let on = note_ons(&mut e, 32 * 6000);
        assert!(on[14].e.note == 60 && on[14].frame == 16 * 6000 - 6100);
        status = e.status();
        assert!(status.sequence == 3 && status.pattern == 1 && status.queued_sequence == -1);
        for i in 14..30 {
            assert!(on[i].e.note == 60 + (i - 14) as i32);
        }
        assert!(on[30].e.note == 80 && on[30].frame == on[14].frame + 16 * 6000);
    }
    // Song mode by pattern: first occurrence; a natural end of the song is replaced by the jump.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[1, 0]);
        e.play(1);
        note_ons(&mut e, 100);
        e.queue_jump(0, -1);
        note_ons(&mut e, 100);
        assert!(e.status().queued_sequence == 1);
        e.queue_jump(-1, -1);
        note_ons(&mut e, 4 * 6000);
        e.queue_jump(1, -1);
        note_ons(&mut e, 100);
        assert!(e.status().queued_sequence == 0);
        let on = note_ons(&mut e, 16 * 6000);
        assert!(on.last().unwrap().e.note == 80 && e.status().sequence == 0 && e.status().playing != 0);
    }
    // Mode switch, Song to Pattern: the current block's pattern keeps looping from its end instead of advancing.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[0, 1]);
        e.play(1);
        note_ons(&mut e, 3 * 6000 + 100);
        e.queue_mode(0, 0, -1);
        note_ons(&mut e, 100);
        let mut status = e.status();
        assert!(status.queued_pattern == 0 && status.song_mode == 1);
        let on = note_ons(&mut e, 18 * 6000);
        assert!(on.len() == 18);
        for i in 0..12 {
            assert!(on[i].e.note == 64 + i as i32);
        }
        for i in 12..18 {
            assert!(on[i].e.note == 60 + (i - 12) as i32);
        }
        assert!(on[12].frame == 16 * 6000 - (3 * 6000 + 200));
        status = e.status();
        assert!(status.song_mode == 0 && status.pattern == 0 && status.queued_pattern == -1 && status.playing != 0);
    }
    // Mode switch, Pattern to Song: at the pattern end the song continues at the given block, then ends normally.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[1, 0, 1]);
        e.play(0);
        note_ons(&mut e, 6000);
        e.queue_mode(1, -1, 2);
        note_ons(&mut e, 100);
        let mut status = e.status();
        assert!(status.queued_sequence == 2 && status.queued_pattern == 1 && status.song_mode == 0);
        let on = note_ons(&mut e, 19 * 6000);
        assert!(on[14].e.note == 80 && on[14].frame == 16 * 6000 - 6100);
        status = e.status();
        assert!(status.song_mode == 1 && status.sequence == 2 && status.pattern == 1);
        for i in 14..18 {
            assert!(on[i].e.note == 80 + (i - 14) as i32);
        }
    }
    // A matching mode, a stopped engine or an invalid target queues nothing; Stop clears a pending switch.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[0, 1]);
        e.play(1);
        note_ons(&mut e, 100);
        e.queue_mode(1, -1, 1);
        e.queue_mode(0, -1, -1);
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == -1);
        e.queue_mode(0, 1, -1);
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == 1);
        e.stop();
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == -1);
        e.queue_mode(0, 1, -1);
        note_ons(&mut e, 100);
        assert!(e.status().queued_pattern == -1);
        e.play(1);
        note_ons(&mut e, 100);
        e.queue_mode(0, 1, -1);
        e.queue_jump(-1, 1);
        let _on = note_ons(&mut e, 17 * 6000);
        assert!(e.status().song_mode == 1 && e.status().sequence == 1);
    }
    // Mapped timing: the jump is sample-accurate on the clock and keeps musical time continuous.
    {
        let mut e = Engine::new();
        configure_queue_music(&mut e, &[0]);
        e.begin_update();
        let tempos = [ClockTempoEvent { beat: 0.0, bpm: 120.0 }];
        let meters = [ClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 }];
        assert!(e.set_timing_map(&tempos, &meters, 9).is_ok());
        assert!(e.commit_update_checked().is_ok());
        e.play(0);
        note_ons(&mut e, 6000);
        e.queue_jump(1, -1);
        let on = note_ons(&mut e, 17 * 6000);
        assert!(on.len() == 17);
        assert!(on[14].e.note == 75);
        assert!(on[15].e.note == 80 && (on[15].frame as i64 - (15 * 6000) as i64).abs() <= 1);
        assert!(on[16].e.note == 81 && (on[16].frame as i64 - (16 * 6000) as i64).abs() <= 1);
        assert!(e.status().pattern == 1);
    }
    // Live voices keep sounding over a queued pattern change.
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.set_pattern(0, 2);
        e.set_pattern(1, 2);
        let sequence = [0];
        e.set_sequence(&sequence, 0, 0);
        e.commit_update();
        e.play(0);
        e.note_on(0, 0, 69, 100);
        let mut a = Audio::new(1000);
        render(&mut e, &mut a);
        e.queue_jump(1, -1);
        let mut b = Audio::new(12000);
        render(&mut e, &mut b);
        assert!(e.status().pattern == 1);
        assert!(b.energy_in(11000, 12000) > 0.001);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_count_in() {
    // 120 BPM 4/4: four clicks 24000 frames apart while stopped, then Play exactly one bar later.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.play_count_in(0, 1);
        let mut a = Audio::new(96000);
        render(&mut e, &mut a);
        let status = e.status();
        assert!(status.playing == 0 && status.count_in == 1);
        for beat in 0..4usize {
            assert!(a.energy_in(beat * 24000, beat * 24000 + 500) > 1e-4);
            assert!(a.energy_in(beat * 24000 + 2000, beat * 24000 + 23000) == 0.0);
        }
        let (mut crossings_a, mut crossings_b) = (0, 0);
        for i in 1..1400 {
            if a.l[i - 1] <= 0.0 && a.l[i] > 0.0 {
                crossings_a += 1;
            }
            if a.l[24000 + i - 1] <= 0.0 && a.l[24000 + i] > 0.0 {
                crossings_b += 1;
            }
        }
        assert!(crossings_a > crossings_b);
        while let Some(v) = e.pop_event() {
            assert!(v.type_ != ST_EVENT_NOTE_ON);
        }
        let on = note_ons(&mut e, 2 * 6000);
        assert_notes(&on, &[(0, 60), (6000, 61)]);
        assert!(e.status().playing != 0 && e.status().count_in == 0);
    }
    // Stop cancels; Play while playing ignores a count-in; metronome volume and meter set the clicks.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.play_count_in(0, 1);
        note_ons(&mut e, 30000);
        assert!(e.status().count_in == 3);
        e.stop();
        let mut on = note_ons(&mut e, 96000);
        assert!(on.is_empty() && e.status().count_in == 0 && e.status().playing == 0);
        e.begin_update();
        e.set_metronome(0, 3, 8, 0.5);
        e.commit_update();
        e.play_count_in(1, 2);
        note_ons(&mut e, 100);
        assert!(e.status().count_in == 6);
        on = note_ons(&mut e, 6 * 12000);
        assert!(on.len() == 1 && on[0].frame == 6 * 12000 - 100);
        e.play_count_in(0, 1);
        note_ons(&mut e, 100);
        assert!(e.status().count_in == 0 && e.status().song_mode != 0);
    }
    // Live notes played during the count-in sound on after the downbeat.
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.play_count_in(0, 1);
        let mut a = Audio::new(1000);
        render(&mut e, &mut a);
        e.note_on(0, 0, 69, 100);
        let mut b = Audio::new(96000);
        render(&mut e, &mut b);
        assert!(e.status().playing != 0);
        assert!(b.energy_in(95500, 96000) > 0.001);
    }
    assert_zero_render_allocations();
}

/// Play/Stop are applied by the render thread. Between queueing and the next render (or
/// indefinitely while the audio device is not rendering) the rendered status is stale; the
/// control thread must see the requested transport: a stale "playing" after Stop turned the
/// app's next Play press into another Stop.
#[test]
fn test_transport_status_follows_queued_commands() {
    let playing = |e: &Engine| -> bool {
        let s = e.status();
        s.transport_playing()
    };
    let count_in = |e: &Engine| -> i32 {
        let s = e.status();
        s.count_in_shown()
    };
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let mut a = Audio::new(1000);
        e.play(0);
        let mut s = e.status();
        assert!(s.transport_pending != 0 && s.requested_playing != 0 && s.playing == 0 && playing(&e));
        render(&mut e, &mut a);
        s = e.status();
        assert!(s.playing != 0 && s.transport_pending == 0 && playing(&e));
        e.stop();
        s = e.status();
        // rendered is stale; requested is stopped
        assert!(s.playing != 0 && s.transport_pending != 0 && !playing(&e));
        // No render runs (device stopped): status stays stopped, and a later Play is reported as requested.
        for _ in 0..3 {
            assert!(!playing(&e));
        }
        e.play(0);
        assert!(playing(&e));
        e.stop();
        assert!(!playing(&e));
        render(&mut e, &mut a);
        s = e.status();
        assert!(s.playing == 0 && s.transport_pending == 0 && !playing(&e));
        e.play(0);
        render(&mut e, &mut a);
        assert!(playing(&e) && e.status().transport_pending == 0);
    }
    // Count-in: a pending count-in is not playing and shows no stale count; Stop during it is immediate.
    {
        let mut e = Engine::new();
        configure_loop_music(&mut e, 0.0);
        e.play_count_in(0, 1);
        assert!(!playing(&e) && count_in(&e) == 0 && e.status().transport_pending != 0);
        note_ons(&mut e, 30000);
        assert!(!playing(&e) && count_in(&e) == 3 && e.status().transport_pending == 0);
        e.stop();
        let mut s = e.status();
        assert!(s.count_in == 3 && s.transport_pending != 0 && !playing(&e) && count_in(&e) == 0);
        let on = note_ons(&mut e, 96000);
        s = e.status();
        assert!(on.is_empty() && !playing(&e) && s.count_in == 0 && s.transport_pending == 0);
    }
    // A queued jump left by the old playback is flagged stale (pending) once Stop is requested, and gone after it renders.
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.set_pattern(0, 2);
        e.set_pattern(1, 2);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(1000);
        render(&mut e, &mut a);
        e.queue_jump(1, -1);
        render(&mut e, &mut a);
        assert!(e.status().queued_pattern == 1);
        e.stop();
        assert!(!playing(&e) && e.status().transport_pending != 0);
        render(&mut e, &mut a);
        let s = e.status();
        assert!(!playing(&e) && s.queued_pattern == -1 && s.transport_pending == 0);
    }
    // A capture/restore (export) flush publishes the applied serial with the transport state.
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.play(0);
        let mut a = Audio::new(1000);
        render(&mut e, &mut a);
        e.stop();
        let mut state = e.capture_transport();
        assert!(state.playing == 0);
        let s = e.status();
        assert!(s.playing == 0 && s.transport_pending == 0);
        e.play(0);
        state = e.capture_transport();
        assert!(state.playing != 0 && e.status().transport_pending == 0 && e.status().playing != 0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_seek_status_revision() {
    let mut e = Engine::new();
    configure(&mut e, TONE);
    let mut block = Audio::new(64);
    render(&mut e, &mut block);
    let initial = e.status();
    e.seek(0, 6, 0);
    let queued = e.status();
    assert!(queued.transport_seek_revision == initial.transport_seek_revision && queued.row == initial.row);
    render(&mut e, &mut block);
    let admitted = e.status();
    assert!(admitted.transport_seek_revision == initial.transport_seek_revision + 1 && admitted.row == 6);
    e.play(0);
    render(&mut e, &mut block);
    e.stop();
    render(&mut e, &mut block);
    assert!(e.status().transport_seek_revision == admitted.transport_seek_revision);
    e.seek(0, 2, 0);
    render(&mut e, &mut block);
    let back = e.status();
    assert!(back.transport_seek_revision == admitted.transport_seek_revision + 1 && back.row == 2);
    // The marker and row must remain one publication even with a concurrent status reader.
    let done = AtomicBool::new(false);
    {
        let (mut control, mut render_half, shared) = e.split();
        let done = &done;
        std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    let status = shared.status();
                    if status.transport_seek_revision > back.transport_seek_revision {
                        assert!(
                            status.row
                                == if (status.transport_seek_revision - back.transport_seek_revision) % 2 != 0 {
                                    5
                                } else {
                                    2
                                }
                        );
                    }
                }
            });
            {
                let _finish = SetOnDrop(done);
                for n in 1..=2000 {
                    control.seek(0, if n % 2 != 0 { 5 } else { 2 }, 0);
                    render_half.render_into(&mut block.l, &mut block.r);
                }
            }
            reader.join().unwrap();
        });
    }
    assert_zero_render_allocations();
}

#[test]
fn test_mod_wheel() {
    let energy = |wheel: f32, panic: bool| -> f64 {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        let mut i = test_tone_instrument(TEST_SAW);
        load_test_tone(&mut e, 0, TEST_SAW);
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        i.volume = 0.5;
        e.set_instrument(0, i);
        e.commit_update();
        if wheel > 0.0 {
            e.mod_wheel(0, wheel);
        }
        e.mod_wheel(1, 1.0);
        if panic {
            e.panic();
        }
        e.note_on(0, 0, 93, 100);
        let mut a = Audio::new(24000);
        render(&mut e, &mut a);
        a.energy_in(12000, 24000)
    };
    // Wheel up closes the filter; track 1's wheel does not affect track 0; panic resets it.
    let (open, closed, reset) = (energy(0.0, false), energy(1.0, false), energy(1.0, true));
    assert!(open > 0.001 && closed < open * 0.3 && (reset - open).abs() < open * 0.01);
    assert_zero_render_allocations();
}

/// Pad reorder: a host that released a moved plug-in's notes itself can end the old channel's
/// external voices silently, so the channel's new occupant gets no stray release. Note events
/// mark sequencer ownership in slot (1) versus live/preview (0).
#[test]
fn test_forget_external_voices_and_sequenced_flag() {
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_AU);
        e.note_on(0, 0, 60, 100);
        let mut ev = note_events(render_events(&mut e, 64));
        assert!(ev.len() == 1 && ev[0].e.type_ == ST_EVENT_NOTE_ON && ev[0].e.slot == 0);
        assert!(
            e.forget_external_voices(-1) == 0
                && e.forget_external_voices(channel_count(ST_MIN_RETURN_BUS)) == 0
                && e.forget_external_voices(1) == 0
        );
        assert!(e.forget_external_voices(0) == 1);
        e.note_off(0, 60);
        assert!(note_events(render_events(&mut e, 256)).is_empty());
        e.note_on(0, 0, 62, 100);
        e.note_off(0, 62);
        ev = note_events(render_events(&mut e, 64));
        assert!(ev.len() == 2 && ev[1].e.type_ == ST_EVENT_NOTE_OFF && ev[1].e.slot == 0);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_AU);
        e.begin_update();
        let mut s = default_step();
        s.gate = 0.5;
        e.set_step(0, 0, 0, s, &[]);
        e.commit_update();
        e.play(0);
        let ev = note_events(render_events(&mut e, 6000));
        assert!(ev.len() == 2);
        for v in &ev {
            assert!(v.e.slot == 1);
        }
    }
    assert_zero_render_allocations();
}

// Note columns: each extra column of a track is its own lane on the track's channel
// (`Control::set_step_column`).

/// C++ `columnNote(note,gate)` (default gate 0).
fn column_note(note: i32, gate: f32) -> STStep {
    let mut s = default_step();
    s.note = note;
    s.velocity = 100;
    s.gate = gate;
    s
}

/// C++ `columnEvents(e,frames,type)`: (frame, note) of every note event of one type.
fn column_events(e: &mut Engine, frames: usize, type_: i32) -> Vec<(u64, i32)> {
    let mut out = Vec::new();
    for v in note_events(render_events(e, frames)) {
        if v.e.type_ == type_ {
            out.push((v.frame, v.e.note));
        }
    }
    out
}

/// C++ `offFrame(offs,note)`: `u64::MAX` (C++ `UINT64_MAX`) when the note never ended.
fn off_frame(offs: &[(u64, i32)], note: i32) -> u64 {
    for o in offs {
        if o.1 == note {
            return o.0;
        }
    }
    u64::MAX
}

#[test]
fn test_note_columns() {
    // Two columns sustain together; a new note in column 1 releases only column 1; OFF in column 0 and CUT in column 1
    // end only their own column; a gated column-2 note stops after its gate while column 0's next note sounds on.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.begin_update();
        e.set_pattern(0, 16);
        let seq = [0];
        e.set_sequence(&seq, 0, 0);
        e.set_step(0, 0, 0, column_note(60, 0.0), &[]);
        e.set_step_column(0, 0, 0, 1, column_note(64, 0.0));
        e.set_step_column(0, 0, 2, 1, column_note(67, 0.0));
        e.set_step(0, 0, 4, column_note(-1, 0.0), &[]);
        e.set_step_column(0, 0, 5, 1, column_note(-2, 0.0));
        e.set_step(0, 0, 6, column_note(48, 0.0), &[]);
        e.set_step_column(0, 0, 6, 2, column_note(72, 1.0));
        e.commit_update();
        e.play(0);
        let events = note_events(render_events(&mut e, 6000 * 9));
        let (mut on, mut off): (Vec<(u64, i32)>, Vec<(u64, i32)>) = (Vec::new(), Vec::new());
        for v in &events {
            (if v.e.type_ == ST_EVENT_NOTE_ON { &mut on } else { &mut off }).push((v.frame, v.e.note));
        }
        assert!(on.len() == 5 && on[0].0 == 0 && on[1].0 == 0 && on[2] == (12000u64, 67));
        assert!(
            off_frame(&off, 64) == 12000
                && off_frame(&off, 60) == 24000
                && off_frame(&off, 67) == 30000
                && off_frame(&off, 72) == 42000
                && off_frame(&off, 48) == u64::MAX
        );
        for v in &events {
            assert!(v.e.track == 0 && v.e.slot == 1);
        }
    }
    // Gate per column: column 0 gated one row, column 1 without a gate sounds on; a new column-0 note leaves column 1 alone.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.begin_update();
        e.set_pattern(0, 8);
        let seq = [0];
        e.set_sequence(&seq, 0, 0);
        e.set_step(0, 0, 0, column_note(60, 1.0), &[]);
        e.set_step_column(0, 0, 0, 1, column_note(64, 0.0));
        e.set_step(0, 0, 2, column_note(62, 0.0), &[]);
        e.commit_update();
        e.play(0);
        let off = column_events(&mut e, 6000 * 4, ST_EVENT_NOTE_OFF);
        assert!(off_frame(&off, 60) == 6000 && off_frame(&off, 64) == u64::MAX && off_frame(&off, 62) == u64::MAX);
    }
    // Row FX: column 0's ratchet and microtiming apply to the extra column's note; one probability roll drops the whole row.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.begin_update();
        e.set_pattern(0, 4);
        let seq = [0];
        e.set_sequence(&seq, 0, 0);
        let mut s = column_note(60, 0.0);
        s.fx1 = fx(ST_FX_RATCHET, 2.0);
        e.set_step(0, 0, 0, s, &[]);
        e.set_step_column(0, 0, 0, 1, column_note(64, 0.0));
        let mut fx_only = default_step();
        fx_only.note = -3;
        fx_only.fx1 = fx(ST_FX_MICROTIMING, 0.5);
        e.set_step(0, 0, 1, fx_only, &[]);
        e.set_step_column(0, 0, 1, 1, column_note(65, 0.0));
        let mut drop = default_step();
        drop.note = -3;
        drop.fx1 = fx(ST_FX_PROBABILITY, 0.0);
        e.set_step(0, 0, 2, drop, &[]);
        e.set_step_column(0, 0, 2, 1, column_note(66, 0.0));
        e.commit_update();
        e.play(0);
        let on = column_events(&mut e, 6000 * 4, ST_EVENT_NOTE_ON);
        let (mut hits64, mut hits65): (Vec<u64>, Vec<u64>) = (Vec::new(), Vec::new());
        let mut hits66 = 0;
        for o in &on {
            if o.1 == 64 {
                hits64.push(o.0);
            }
            if o.1 == 65 {
                hits65.push(o.0);
            }
            hits66 += (o.1 == 66) as i32;
        }
        assert!(hits64 == [0u64, 3000] && hits65 == [9000u64] && hits66 == 0);
    }
    // A sampler track: two columns sound at once (more than three times the energy of one).
    {
        let energy = |two: bool| -> f64 {
            let mut e = Engine::new();
            configure(&mut e, TONE);
            e.begin_update();
            e.set_pattern(0, 4);
            e.set_step(0, 0, 0, column_note(69, 0.0), &[]);
            if two {
                e.set_step_column(0, 0, 0, 1, column_note(69, 0.0));
            }
            e.commit_update();
            e.play(0);
            let mut a = Audio::new(4800);
            render(&mut e, &mut a);
            a.energy_in(2400, 4800)
        };
        let (one, both) = (energy(false), energy(true));
        assert!(one > 0.001 && both > one * 3.0);
    }
    // Copy-on-write and resize: a reused pattern keeps its columns, a shortened pattern drops columns past its end,
    // clear_step clears every column, and the scheduler ignores columns of removed tracks.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.begin_update();
        e.set_pattern(0, 8);
        let seq = [0];
        e.set_sequence(&seq, 1, 0);
        e.set_step_column(0, 0, 0, 1, column_note(64, 0.0));
        e.set_step_column(0, 0, 1, 3, column_note(65, 0.0));
        e.set_step_column(0, 0, 6, 7, column_note(66, 0.0));
        e.set_step_column(0, 1, 0, 1, column_note(70, 0.0));
        e.set_step_column(0, 0, 2, 8, column_note(99, 0.0));
        e.set_step_column(0, 0, 2, -1, column_note(99, 0.0));
        e.commit_update();
        e.begin_update();
        e.clear_project();
        e.set_track_count(2);
        assert!(e.reuse_pattern(0));
        e.set_sequence(&seq, 1, 0);
        e.commit_update();
        e.play(0);
        let mut on = column_events(&mut e, 6000 * 8, ST_EVENT_NOTE_ON);
        let mut notes: Vec<i32> = Vec::new();
        for o in &on {
            notes.push(o.1);
        }
        notes.sort();
        assert!(notes == [64, 65, 66, 70]);
        e.stop();
        render_events(&mut e, 256);
        e.begin_update();
        e.set_pattern(0, 4);
        e.clear_step(0, 0, 1);
        e.set_track_count(1);
        e.commit_update();
        e.play(0);
        on = column_events(&mut e, 6000 * 4, ST_EVENT_NOTE_ON);
        assert!(on.len() == 1 && on[0].1 == 64);
    }
    assert_zero_render_allocations();
}

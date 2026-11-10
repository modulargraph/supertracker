//! Port of `tests/engine_timing_tests.cpp`, part 2 of 2: the retained clock history behind
//! `clock_span` (spans, revisions, upstream latency, expiry), mapped startup preroll and the
//! lifetime of the graph origin's map, legacy fractional preroll with a stopped origin, legacy
//! fractional song boundaries, concurrent origin ownership (publication and collection on the
//! control thread against resets, pulls and queries on the audio thread), the recording context
//! with transport checkpoints, and the projected grid of a short loop.
//!
//! One `#[test]` per C++ suite (`testFooBar` is `test_foo_bar`), calling the engine's safe Rust
//! API through `engine_support_a::Engine`. Every C++ `assert` is one `assert!` here and every
//! `expectFrames` one `expect_frames`, with the same inputs and expected values; every test
//! ends with the C++ program's final "zero render/query allocations" check.
//!
//! At 48 kHz the two-tempo fixture maps beat b to 24000*b through b=.375, then to 48000*b-9000.
//! All explicit expectations below are exact integers.

mod engine_support_a;

use engine_support_a::timing_helpers::*;
use engine_support_a::{assert_zero_render_allocations, Engine, SetOnDrop};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tracker_engine::clock::*;
use tracker_engine::engine::*;

fn tempo(beat: f64, bpm: f64) -> STClockTempoEvent {
    STClockTempoEvent { beat, bpm }
}

fn meter(beat: f64, numerator: i32, denominator: i32) -> STClockMeterEvent {
    STClockMeterEvent { beat, numerator, denominator }
}

#[test]
fn test_retained_clock_spans() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 4, 0.0);
    for row in 0..4 {
        put_note(&mut engine, row, 60 + row, 0.5);
    }
    start(&mut engine, 1);
    expect_clock_error(&mut engine, 0, 0, ST_ENGINE_CLOCK_NOT_PREPARED);
    prepare_bus(&mut engine, 0, 12000);
    let status = engine.status();
    // This command and the queued notes must survive all read-only queries.
    engine.note_on(2, 0, 72, 100);
    let before_change = prepared_span_at(&mut engine, 8999, 4096, 0);
    assert!(before_change.frames == 1);
    assert!(before_change.context.bpm == 120.0 && before_change.context.revision == 1);
    assert!(before_change.context.numerator == 4 && before_change.context.denominator == 4);
    assert!(before_change.context.graph_frame == 8999 && before_change.context.content_graph_frame == 8999);
    assert!(before_change.context.song_frame == 8999 && before_change.context.next_boundary_frame == 9000);
    assert!(
        (before_change.context.next_boundary_kinds & (ST_CLOCK_BOUNDARY_TEMPO | ST_CLOCK_BOUNDARY_METER))
            == (ST_CLOCK_BOUNDARY_TEMPO | ST_CLOCK_BOUNDARY_METER)
    );

    let old_revision = prepared_span(&mut engine, 9000);
    assert!(old_revision.frames == 64 && old_revision.context.beat == 0.375);
    assert!(old_revision.context.bpm == 60.0 && old_revision.context.revision == 1);
    assert!(old_revision.context.numerator == 7 && old_revision.context.denominator == 8);
    assert!(old_revision.context.bar == 2 && old_revision.context.bar_start_beat == 0.375);
    assert!(old_revision.context.flags & ST_CLOCK_PLAYING != 0);

    let delayed = prepared_span_at(&mut engine, 9127, 256, 128);
    assert!(delayed.frames == 1 && delayed.context.graph_frame == 9127);
    assert!(delayed.context.content_graph_frame == 8999 && delayed.context.song_frame == 8999);
    assert!(delayed.context.beat == before_change.context.beat);
    assert!(delayed.context.next_boundary_frame == 9128);
    if before_change.context.next_quarter_beat_frame != i64::MAX {
        assert!(delayed.context.next_quarter_beat_frame == before_change.context.next_quarter_beat_frame + 128);
    }
    expect_clock_error(&mut engine, 12000, 0, ST_ENGINE_CLOCK_NOT_PREPARED);
    let startup = prepared_span(&mut engine, -1);
    assert!(startup.frames == 1 && startup.context.song_frame == -1);
    assert!(startup.context.flags & ST_CLOCK_PREROLL != 0);
    let delayed_startup = prepared_span_at(&mut engine, 0, 64, 1);
    assert!(delayed_startup.frames == 1 && delayed_startup.context.content_graph_frame == -1);
    assert!(delayed_startup.context.next_boundary_frame == 1);
    assert!(status_key(&status) == status_key(&engine.status()));
    let mut pending_notes: Vec<Note> = Vec::new();
    drain_notes(&mut engine, 0, &mut pending_notes);
    expect_frames(&pending_notes, ST_EVENT_NOTE_ON, &[0, 6000]);
    expect_frames(&pending_notes, ST_EVENT_NOTE_OFF, &[3000, 9000]);

    let tempos = [tempo(0.0, 240.0)];
    let meters = [meter(0.0, 3, 4)];
    engine.begin_update();
    assert!(engine.set_timing_map(&tempos, &meters, 2).is_ok());
    commit(&mut engine);
    // Pending publication cannot change cached history or consume the live note.
    let pending_revision = prepared_span(&mut engine, 9000);
    assert!(context_key(&pending_revision.context) == context_key(&old_revision.context));
    assert!(status_key(&status) == status_key(&engine.status()));
    prepare_bus(&mut engine, 12000, 4096);
    let mut new_notes: Vec<Note> = Vec::new();
    drain_notes(&mut engine, 12000, &mut new_notes);
    let live = new_notes.iter().find(|note| note.type_ == ST_EVENT_NOTE_ON && note.note == 72 && note.track == 2);
    assert!(live.is_some_and(|note| note.frame == 12000));

    let current_revision = prepared_span(&mut engine, 12000);
    assert!(current_revision.context.revision == 2 && current_revision.context.bpm == 240.0);
    assert!(current_revision.context.numerator == 3 && current_revision.context.denominator == 4);
    assert!(current_revision.context.beat == 0.4375 && current_revision.context.song_frame == 5250);
    let retained_revision = prepared_span(&mut engine, 9000);
    assert!(context_key(&retained_revision.context) == context_key(&old_revision.context));
    let revision_edge = prepared_span_at(&mut engine, 11999, 256, 0);
    assert!(revision_edge.frames == 1 && revision_edge.context.revision == 1);
    let available_prefix = prepared_span_at(&mut engine, 16080, 256, 0);
    assert!(available_prefix.frames == 16 && available_prefix.context.revision == 2);
    expect_clock_error(&mut engine, 16096, 0, ST_ENGINE_CLOCK_NOT_PREPARED);

    // A96k upstream delay still addresses retained clock history after its PCM
    // has expired from the separate65536-frame audio cache.
    prepare_bus(&mut engine, 16096, 65536);
    prepare_bus(&mut engine, 81632, 30368);
    let long_latency = prepared_span_at(&mut engine, 105000, 64, 96000);
    assert!(long_latency.context.content_graph_frame == 9000);
    assert!(long_latency.context.revision == 1 && long_latency.context.beat == 0.375);
    assert!(long_latency.context.bpm == 60.0 && long_latency.context.numerator == 7);

    // Expired post-origin history must not be synthesized as startup preroll.
    let final_frame: u64 = 16096 + ST_ENGINE_CLOCK_HISTORY_FRAMES as u64;
    let mut at: u64 = 112000;
    while at < final_frame {
        let count = 65536u64.min(final_frame - at) as u32;
        prepare_bus(&mut engine, at, count);
        at += count as u64;
    }
    expect_clock_error(&mut engine, 16095, 0, ST_ENGINE_CLOCK_NOT_RETAINED);
    assert!(prepared_span(&mut engine, 16096).context.revision == 2);
    assert!(prepared_span(&mut engine, (final_frame - 1) as i64).frames == 1);
    expect_clock_error(&mut engine, final_frame as i64, 0, ST_ENGINE_CLOCK_NOT_PREPARED);
    assert!(prepared_span(&mut engine, -1).context.revision == 1);
    assert!(engine.status().dropped_commands == 0 && engine.status().dropped_events == 0);
    assert_zero_render_allocations();
}

#[test]
fn test_mapped_startup_preroll_and_origin_lifetime() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 8, 0.0);
    put_note(&mut engine, 2, 62, 0.5);
    let sequence = [0];
    engine.set_sequence(&sequence, 1, 0);
    commit(&mut engine);
    engine.seek(0, 2, 0);
    engine.play(1);
    let pending_status = engine.status();
    expect_clock_error(&mut engine, 1000, 6000, ST_ENGINE_CLOCK_NOT_PREPARED);
    assert!(status_key(&pending_status) == status_key(&engine.status()));
    prepare_bus(&mut engine, 1000, 64);
    let status = engine.status();

    // Startup seeks to beat.5/frame15000. Latency looks backward through the
    // actual map, including its tempo and meter event at song frame9000.
    let preceding = prepared_span_at(&mut engine, 1000, 64, 6001);
    assert!(preceding.frames == 1 && preceding.context.song_frame == 8999);
    assert!(preceding.context.bpm == 120.0 && preceding.context.numerator == 4);
    assert!(preceding.context.denominator == 4 && preceding.context.next_boundary_frame == 1001);
    assert!(preceding.context.flags & ST_CLOCK_PREROLL != 0);
    let boundary = prepared_span_at(&mut engine, 1000, 64, 6000);
    assert!(boundary.context.song_frame == 9000 && boundary.context.beat == 0.375);
    assert!(boundary.context.bpm == 60.0 && boundary.context.numerator == 7 && boundary.context.denominator == 8);
    assert!(boundary.context.flags & ST_CLOCK_PLAYING != 0);
    let negative = prepared_span_at(&mut engine, 1000, 64, 16000);
    assert!(negative.context.song_frame == -1000 && negative.context.beat < 0.0);
    assert!(negative.context.bar == 0 && negative.context.bar_end_beat == 0.0);
    assert!(
        (negative.context.flags & (ST_CLOCK_PREROLL | ST_CLOCK_PLAYING | ST_CLOCK_LOOPING))
            == (ST_CLOCK_PREROLL | ST_CLOCK_PLAYING | ST_CLOCK_LOOPING)
    );
    assert!(negative.context.loop_iteration == 0);
    let edge = prepared_span_at(&mut engine, 1000, 64, 1);
    assert!(edge.frames == 1 && edge.context.next_boundary_frame == 1001);
    assert!(edge.context.next_boundary_kinds & ST_CLOCK_BOUNDARY_ORIGIN != 0);
    assert!(status_key(&status) == status_key(&engine.status()));
    let mut notes: Vec<Note> = Vec::new();
    drain_notes(&mut engine, 1000, &mut notes);
    expect_frames(&notes, ST_EVENT_NOTE_ON, &[1000]);

    // Even retired project snapshots can be collected while this graph's
    // immutable startup origin keeps its original map alive.
    for revision in 2..=3u64 {
        let tempos = [tempo(0.0, if revision == 2 { 240.0 } else { 96.0 })];
        let meters = [meter(0.0, if revision == 2 { 3 } else { 5 }, if revision == 2 { 4 } else { 8 })];
        engine.begin_update();
        assert!(engine.set_timing_map(&tempos, &meters, revision).is_ok());
        commit(&mut engine);
        engine.collect();
        assert!(context_key(&prepared_span_at(&mut engine, 1000, 64, 6001).context) == context_key(&preceding.context));
        prepare_bus(&mut engine, 1064 + (revision - 2) * 64, 64);
        engine.collect();
        assert!(context_key(&prepared_span_at(&mut engine, 1000, 64, 6001).context) == context_key(&preceding.context));
    }

    // A stopped graph reset establishes a fresh origin only after queued seek
    // and Play are applied by the first pull, not by the read-only query.
    engine.seek(0, 3, 0);
    engine.play(1);
    engine.reset_render_cache();
    expect_clock_error(&mut engine, 200000, 7500, ST_ENGINE_CLOCK_NOT_PREPARED);
    prepare_bus(&mut engine, 200000, 64);
    engine.collect();
    let reset = prepared_span_at(&mut engine, 200000, 64, 7500);
    assert!(reset.context.revision == 3 && reset.context.epoch != preceding.context.epoch);
    assert!(reset.context.song_frame == 15000 && reset.context.beat == 0.5);
    assert!(reset.context.bpm == 96.0 && reset.context.numerator == 5 && reset.context.denominator == 8);
    assert!(reset.context.flags & ST_CLOCK_PREROLL != 0);
    assert!(prepared_span(&mut engine, 1000).context.revision == 3);
    let furthest = prepared_span(&mut engine, 200000 - (1i64 << 30));
    assert!(furthest.frames > 0 && (furthest.context.flags & ST_CLOCK_PREROLL) != 0);
    expect_clock_error(&mut engine, 200000 - (1i64 << 30) - 1, 0, ST_ENGINE_CLOCK_NOT_RETAINED);
    assert_zero_render_allocations();
}

#[test]
fn test_legacy_fractional_preroll_and_stopped_origin() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 64, 0.0);
    engine.clear_timing_map();
    engine.set_timing(999.0, 4, 0.25, 1.0);
    engine.set_metronome(0, 3, 8, 0.3);
    put_note(&mut engine, 1, 61, 0.5);
    put_note(&mut engine, 4, 64, 0.5);
    commit(&mut engine);
    engine.seek(0, 1, 0);
    engine.play(1);
    prepare_bus(&mut engine, 4096, 1);

    // Row one begins at .3125 beats =900000/999 source frames. Preserve that
    // fractional origin: latency1000 reaches the song-zero boundary in100,
    // rather than99, graph samples.
    let preroll = prepared_span_at(&mut engine, 4096, 256, 1000);
    assert!(preroll.frames == 100 && preroll.context.next_boundary_frame == 4196);
    assert!(preroll.context.next_quarter_beat_frame == 4196);
    assert!((preroll.context.beat - (-0.034375)).abs() < 1e-14);
    assert!(preroll.context.bpm == 999.0 && preroll.context.bar == 0);
    assert!(preroll.context.bar_start_beat == -1.5 && preroll.context.bar_end_beat == 0.0);
    assert!(preroll.context.flags & ST_CLOCK_PREROLL != 0);
    let before_zero = prepared_span_at(&mut engine, 4195, 64, 1000);
    let after_zero = prepared_span_at(&mut engine, 4196, 64, 1000);
    assert!(before_zero.frames == 1 && before_zero.context.beat < 0.0);
    assert!(after_zero.context.bar == 1 && after_zero.context.beat > 0.0);
    let negative_bar = prepared_span_at(&mut engine, 4096, 1024, 6000);
    assert!(negative_bar.frames == 775 && negative_bar.context.next_boundary_frame == 4871);
    assert!(negative_bar.context.bar == -1 && negative_bar.context.bar_start_beat == -3.0 && negative_bar.context.bar_end_beat == -1.5);
    let mut notes: Vec<Note> = Vec::new();
    drain_notes(&mut engine, 4096, &mut notes);
    expect_frames(&notes, ST_EVENT_NOTE_ON, &[4096]);

    engine.stop();
    engine.seek(0, 1, 0);
    engine.reset_render_cache();
    expect_clock_error(&mut engine, 10000, 1000, ST_ENGINE_CLOCK_NOT_PREPARED);
    prepare_bus(&mut engine, 10000, 64);
    let held = prepared_span_at(&mut engine, 10000, 64, 1000);
    let held_farther = prepared_span_at(&mut engine, 10000, 64, 1000000);
    assert!((held.context.beat - 0.3125).abs() < 1e-14 && held.context.beat == held_farther.context.beat);
    assert!(held.context.song_frame == 900 && held.context.bpm == 999.0);
    assert!((held.context.flags & (ST_CLOCK_PLAYING | ST_CLOCK_PREROLL)) == 0);
    assert!(held.context.next_quarter_beat_frame == i64::MAX);
    notes.clear();
    drain_notes(&mut engine, 10000, &mut notes);
    expect_frames(&notes, ST_EVENT_NOTE_ON, &[]);

    let held_status = engine.status();
    engine.seek(0, 4, 0);
    engine.play(1);
    assert!(context_key(&prepared_span_at(&mut engine, 10000, 64, 1000).context) == context_key(&held.context));
    assert!(status_key(&held_status) == status_key(&engine.status()));
    notes.clear();
    drain_notes(&mut engine, 10000, &mut notes);
    assert!(notes.is_empty());
    prepare_bus(&mut engine, 10064, 1);
    notes.clear();
    drain_notes(&mut engine, 10064, &mut notes);
    expect_frames(&notes, ST_EVENT_NOTE_ON, &[10064]);
    assert!(notes.last().unwrap().note == 64);
    assert!(context_key(&prepared_span_at(&mut engine, 10000, 64, 1000).context) == context_key(&held.context));
    let playing = prepared_span(&mut engine, 10064);
    assert!(playing.context.flags & ST_CLOCK_PLAYING != 0);
    assert!((playing.context.beat - 1.0).abs() < 1e-14);
    assert_zero_render_allocations();
}

#[test]
fn test_legacy_fractional_song_boundary_partitioning() {
    for looping in [0, 1] {
        let mut engine = Engine::new();
        begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 7, 0.0);
        engine.clear_timing_map();
        engine.set_timing(123.0, 4, 0.25, 1.0);
        for row in 0..7 {
            put_note(&mut engine, row, 60 + row, 0.5);
        }
        let sequence = [0];
        engine.set_sequence(&sequence, looping, 0);
        commit(&mut engine);
        engine.seek(0, 1, 0);
        engine.play(1);
        prepare_bus(&mut engine, 1000, 36000);
        let mut notes: Vec<Note> = Vec::new();
        drain_notes(&mut engine, 1000, &mut notes);

        // Seek beat.3125 has a fractional source-frame origin. The seven-row
        // swung pattern ends atbeat1.8125: ceil(1.5*48000*60/123)=35122 frames
        // later, not ceil(endFrame)-floor(originFrame)=35123.
        const BOUNDARY: i64 = 1000 + 35122;
        let before = prepared_span_at(&mut engine, BOUNDARY - 1, 256, 0);
        assert!(before.frames == 1 && (before.context.flags & ST_CLOCK_PLAYING) != 0);
        assert!(before.context.next_boundary_frame == BOUNDARY);
        assert!(before.context.next_boundary_kinds & (if looping != 0 { ST_CLOCK_BOUNDARY_LOOP } else { ST_CLOCK_BOUNDARY_DOMAIN }) != 0);
        let after = prepared_span_at(&mut engine, BOUNDARY, 256, 0);
        assert!(after.frames > 0 && ((after.context.flags & ST_CLOCK_PLAYING) != 0) == (looping != 0));
        assert!(after.context.epoch != before.context.epoch);
        if looping != 0 {
            assert!(after.context.loop_iteration == 1 && after.context.beat == 0.0);
            let mut loop_onsets: Vec<u64> = Vec::new();
            for note in &notes {
                if note.type_ == ST_EVENT_NOTE_ON && note.note == 60 {
                    loop_onsets.push(note.frame);
                }
            }
            assert!(loop_onsets == [BOUNDARY as u64]);
        }

        // Every retained sample belongs to a positive span; none crosses an
        // actual stop or wrap even when row accumulation has fractional carry.
        let mut at: i64 = 1000;
        while at < 37000 {
            let span = prepared_span_at(&mut engine, at, 65536i64.min(37000 - at) as u32, 0);
            assert!(span.frames > 0 && at + span.frames as i64 <= 37000);
            let last = prepared_span_at(&mut engine, at + span.frames as i64 - 1, 1, 0);
            assert!(last.context.epoch == span.context.epoch);
            assert!(last.context.loop_iteration == span.context.loop_iteration);
            assert!((last.context.flags & ST_CLOCK_PLAYING) == (span.context.flags & ST_CLOCK_PLAYING));
            at += span.frames as i64;
        }
    }
    assert_zero_render_allocations();
}

#[test]
fn test_concurrent_origin_ownership() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 64, 0.0);
    start(&mut engine, 1);
    let (published, completed) = (AtomicU32::new(0), AtomicU32::new(0));
    // Set when the control thread leaves its loop, also by a failed assertion, so that the
    // audio thread never waits for a publication that will not come.
    let control_gone = AtomicBool::new(false);
    {
        let (mut control, mut audio_half, _) = engine.split();
        let (published, completed, control_gone) = (&published, &completed, &control_gone);
        std::thread::scope(|scope| {
            let audio = scope.spawn(move || {
                let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
                for round in 0..64u32 {
                    while published.load(Ordering::Acquire) <= round {
                        if control_gone.load(Ordering::SeqCst) {
                            return;
                        }
                        std::thread::yield_now();
                    }
                    let graph: u64 = 4096 + round as u64 * 1024;
                    for origin in [graph, graph + 512] {
                        audio_half.reset_render_cache();
                        expect_clock_error(&mut audio_half, origin as i64, 256, ST_ENGINE_CLOCK_NOT_PREPARED);
                        audio_half.render_bus_into(0, origin, &mut left, &mut right);
                        let preroll = prepared_span_at(&mut audio_half, origin as i64, 64, 256);
                        assert!(preroll.context.revision == round as u64 + 2);
                        assert!(preroll.context.flags & ST_CLOCK_PREROLL != 0);
                        assert!(preroll.frames > 0 && preroll.context.beat.is_finite());
                    }
                    while audio_half.pop_event().is_some() {}
                    completed.store(round + 1, Ordering::Release);
                }
            });
            // Collection races origin acquisition/release on the audio thread. Only
            // this control thread publishes projects; render/reset/query stay serialized.
            {
                let _gone = SetOnDrop(control_gone);
                'publications: for round in 0..64u32 {
                    let tempos = [tempo(0.0, if round % 2 != 0 { 96.0 } else { 240.0 })];
                    let meters = [meter(0.0, if round % 2 != 0 { 5 } else { 3 }, 4)];
                    control.begin_update();
                    assert!(control.set_timing_map(&tempos, &meters, round as u64 + 2).is_ok());
                    commit_control(&mut control);
                    published.store(round + 1, Ordering::Release);
                    loop {
                        control.collect();
                        std::thread::yield_now();
                        if completed.load(Ordering::Acquire) > round {
                            break;
                        }
                        // The audio thread failed an assertion: report it instead of waiting.
                        if audio.is_finished() {
                            break 'publications;
                        }
                    }
                }
            }
            audio.join().unwrap();
        });
    }
    engine.collect();
    assert!(completed.load(Ordering::SeqCst) == 64);
    assert!(engine.status().dropped_commands == 0 && engine.status().dropped_events == 0);
    assert_zero_render_allocations();
}

#[test]
fn test_recording_context_and_checkpoint() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 4, 0.0);
    commit(&mut engine);
    engine.seek(0, 1, 0);
    prepare_bus(&mut engine, 0, 32);
    let held = prepared_span_at(&mut engine, 0, 32, 0);
    assert!(held.context.beat == 0.25);
    assert!(held.context.next_quarter_beat_frame == i64::MAX);
    assert!(held.context.next_quarter_grid_frame == 33000 && (held.context.flags & ST_CLOCK_GRID_PROJECTION) != 0);
    assert!((held.context.flags & ST_CLOCK_RECORDING) == 0 && engine.status().recording == 0);

    engine.set_recording(1);
    // Publishing intent cannot rewrite already-generated, possibly PDC-delayed
    // context or consume the command during a read-only span query.
    assert!(context_key(&prepared_span_at(&mut engine, 0, 32, 0).context) == context_key(&held.context));
    assert!(engine.status().recording == 0);
    prepare_bus(&mut engine, 32, 32);
    let (before, armed) = (prepared_span_at(&mut engine, 0, 64, 0), prepared_span_at(&mut engine, 32, 32, 0));
    assert!(before.frames == 32 && before.context.epoch != armed.context.epoch);
    assert!(armed.context.flags & ST_CLOCK_RECORDING != 0);
    assert!((armed.context.flags & ST_CLOCK_PLAYING) == 0 && engine.status().recording == 1);
    let delayed = prepared_span_at(&mut engine, 96, 32, 64);
    assert!(delayed.context.flags & ST_CLOCK_RECORDING != 0);
    assert!(delayed.context.next_quarter_grid_frame == 33096);
    engine.set_recording(1);
    prepare_bus(&mut engine, 64, 32);
    // Redundant intent adds no seam.
    assert!(prepared_span_at(&mut engine, 32, 64, 0).frames == 64);
    engine.play(1);
    prepare_bus(&mut engine, 96, 32);
    assert!(
        (prepared_span_at(&mut engine, 96, 32, 0).context.flags & (ST_CLOCK_PLAYING | ST_CLOCK_RECORDING))
            == (ST_CLOCK_PLAYING | ST_CLOCK_RECORDING)
    );
    engine.stop();
    prepare_bus(&mut engine, 128, 32);
    let stopped = prepared_span_at(&mut engine, 128, 32, 0);
    assert!((stopped.context.flags & ST_CLOCK_PLAYING) == 0 && (stopped.context.flags & ST_CLOCK_RECORDING) != 0);

    let saved = engine.capture_transport();
    assert!(saved.recording == 1 && saved.playing == 0);
    engine.set_recording(0);
    let temporary = engine.capture_transport();
    assert!(temporary.recording == 0 && engine.status().recording == 0);
    engine.restore_transport(saved, 0);
    assert!(engine.status().recording == 1);
    prepare_bus(&mut engine, 0, 32);
    engine.set_recording(0);
    prepare_bus(&mut engine, 32, 32);
    assert!((prepared_span_at(&mut engine, 32, 32, 0).context.flags & ST_CLOCK_RECORDING) == 0);
    // The cache's pinned stopped origin keeps its original recording context.
    let origin = prepared_span_at(&mut engine, 0, 32, 64);
    assert!(origin.context.flags & ST_CLOCK_RECORDING != 0);
    assert!(origin.context.flags & ST_CLOCK_GRID_PROJECTION != 0);
    assert!(origin.context.next_quarter_grid_frame > origin.context.graph_frame);
    assert_zero_render_allocations();
}

#[test]
fn test_engine_short_loop_projected_grid() {
    let mut engine = Engine::new();
    begin_fixture(&mut engine, ST_INSTRUMENT_MIDI, 1, 0.0);
    engine.set_timing(120.0, 32, 0.0, 1.0);
    assert!(engine.set_pattern_timing_origin(0.1));
    start(&mut engine, 0);
    prepare_bus(&mut engine, 0, 1600);
    for frame in [0i64, 17, 749, 750, 800, 1500] {
        let c = prepared_span_at(&mut engine, frame, 1, 0).context;
        assert!(c.flags & ST_CLOCK_LOOPING != 0);
        assert!(c.flags & ST_CLOCK_GRID_PROJECTION != 0);
        assert!(c.next_quarter_beat_frame == i64::MAX);
        assert!(c.next_quarter_grid_frame == frame + 39000 - c.song_frame);
    }
    let delayed = prepared_span_at(&mut engine, 864, 1, 64).context;
    assert!(delayed.next_quarter_grid_frame == 864 + 39000 - delayed.song_frame);
    assert_zero_render_allocations();
}

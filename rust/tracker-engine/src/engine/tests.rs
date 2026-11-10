//! Unit tests of the engine through its C API (the acceptance tests are the C++ programs in
//! `tests/`, linked against this library; these cover the same ground in miniature so that
//! `cargo test` alone catches gross breakage).

use super::capi::*;
use super::*;
use std::ptr::{null, null_mut};

const RATE: f64 = 48000.0;

/// An engine handle that is destroyed when the test ends.
struct Engine(*mut STEngine);

impl Engine {
    fn new() -> Engine {
        let engine = st_engine_create(RATE);
        assert!(!engine.is_null());
        Engine(engine)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { st_engine_destroy(self.0) }
    }
}

/// One track, one 16-row pattern at 120 BPM / 4 lines per beat: a row lasts 6000 frames.
unsafe fn begin_project(e: *mut STEngine, instrument: STInstrument) {
    unsafe {
        st_engine_begin_update(e);
        st_engine_clear_project(e);
        assert_eq!(st_engine_set_track_count(e, 1), 1);
        st_engine_set_timing(e, 120.0, 4, 0.0, 1.0);
        st_engine_set_pattern(e, 0, 16);
        let sequence = [0];
        st_engine_set_sequence(e, sequence.as_ptr(), 1, 0, 0);
        st_engine_set_instrument(e, 0, instrument);
        st_engine_set_track(e, 0, st_default_track());
    }
}

unsafe fn commit(e: *mut STEngine) {
    let mut error = [0 as std::ffi::c_char; 256];
    assert_eq!(unsafe { st_engine_commit_update_checked(e, error.as_mut_ptr(), 256) }, 1);
}

unsafe fn load_sine(e: *mut STEngine, instrument: i32) {
    let wave: Vec<f32> = (0..4800).map(|n| (n as f32 * 0.05).sin() * 0.5).collect();
    assert_eq!(unsafe { st_engine_set_sample(e, instrument, wave.as_ptr(), null(), wave.len() as u64, RATE) }, 1);
}

unsafe fn set_note(e: *mut STEngine, row: i32, note: i32) {
    let mut step = st_default_step();
    step.note = note;
    step.instrument = 0;
    unsafe { st_engine_set_step(e, 0, 0, row, step, null(), 0) };
}

unsafe fn render(e: *mut STEngine, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut done = 0;
    while done < frames {
        let count = block.min(frames - done);
        unsafe { st_engine_render(e, l[done..].as_mut_ptr(), r[done..].as_mut_ptr(), count as u32) };
        done += count;
    }
    (l, r)
}

unsafe fn events(e: *mut STEngine) -> Vec<STEvent> {
    let mut list = Vec::new();
    let mut event = STEvent::default();
    while unsafe { st_engine_pop_event(e, &mut event) } != 0 {
        list.push(event);
    }
    list
}

#[test]
fn defaults_match_the_header() {
    let track = st_default_track();
    assert_eq!((track.volume, track.pan, track.instrument), (0.8, 0.0, 0));
    let instrument = st_default_instrument();
    assert_eq!(instrument.type_, ST_INSTRUMENT_SAMPLE);
    assert_eq!((instrument.root_note, instrument.slice_count, instrument.bit_depth), (60, 16, 16));
    assert_eq!((instrument.volume, instrument.end, instrument.loop_end, instrument.cutoff), (0.7, 1.0, 1.0, 18000.0));
    assert_eq!((instrument.grain_density, instrument.lfo_beats, instrument.lfo2_beats), (2, 1.0, 1.0));
    let step = st_default_step();
    assert_eq!((step.note, step.instrument, step.velocity, step.gate), (60, -1, 100, 0.8));
    assert_eq!(st_return_bus(8), 16);
    assert_eq!(st_return_bus(40), 40);
    assert_eq!(st_output_bus_count(8), 17);
}

#[test]
fn rejects_invalid_engines_and_null_handles() {
    assert!(st_engine_create(100.0).is_null());
    assert!(st_engine_create(f64::NAN).is_null());
    unsafe {
        st_engine_destroy(null_mut());
        st_engine_play(null_mut(), 1);
        assert_eq!(st_engine_set_track_count(null_mut(), 4), 0);
        assert_eq!(st_engine_status(null_mut()), STStatus::default());
        let mut buffer = [1.0f32; 8];
        st_engine_render(null_mut(), buffer.as_mut_ptr(), null_mut(), 8);
        assert_eq!(buffer, [0.0; 8]);
    }
}

#[test]
fn sampler_output_is_audible_and_independent_of_block_size() {
    let render_with = |block: usize| unsafe {
        let engine = Engine::new();
        let e = engine.0;
        begin_project(e, st_default_instrument());
        load_sine(e, 0);
        set_note(e, 0, 60);
        set_note(e, 2, 67);
        commit(e);
        st_engine_play(e, 0);
        render(e, 20000, block)
    };
    let (l, r) = render_with(20000);
    assert!(l.iter().any(|&x| x.abs() > 0.01) && r.iter().any(|&x| x.abs() > 0.01));
    assert!(l.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    for block in [1, 17, 256, 4097] {
        let (bl, br) = render_with(block);
        assert!(bl.iter().zip(&l).all(|(a, b)| a.to_bits() == b.to_bits()), "left differs at block size {block}");
        assert!(br.iter().zip(&r).all(|(a, b)| a.to_bits() == b.to_bits()), "right differs at block size {block}");
    }
}

#[test]
fn external_notes_are_frame_exact() {
    unsafe {
        let engine = Engine::new();
        let e = engine.0;
        let mut instrument = st_default_instrument();
        instrument.type_ = ST_INSTRUMENT_AU;
        begin_project(e, instrument);
        set_note(e, 0, 60);
        set_note(e, 2, 64);
        commit(e);
        st_engine_play(e, 0);
        let mut onsets = Vec::new();
        let mut offs = Vec::new();
        for block in 0..4 {
            render(e, 5000, 5000);
            for event in events(e) {
                let frame = block * 5000 + event.frame_offset;
                match event.type_ {
                    ST_EVENT_NOTE_ON => onsets.push((frame, event.note, event.slot)),
                    ST_EVENT_NOTE_OFF => offs.push((frame, event.note)),
                    _ => {}
                }
            }
        }
        // Rows last 6000 frames; the default gate is 0.8 rows.
        assert_eq!(onsets, vec![(0, 60, 1), (12000, 64, 1)]);
        assert_eq!(offs, vec![(4800, 60), (16800, 64)]);
        let status = st_engine_status(e);
        assert_eq!((status.playing, status.row, status.track_count), (1, 3, 1));
    }
}

#[test]
fn queued_transport_is_reported_until_rendered() {
    unsafe {
        let engine = Engine::new();
        let e = engine.0;
        st_engine_play(e, 0);
        let status = st_engine_status(e);
        assert_eq!((status.playing, status.transport_pending, status.requested_playing), (0, 1, 1));
        render(e, 16, 16);
        let status = st_engine_status(e);
        assert_eq!((status.playing, status.transport_pending), (1, 0));
        st_engine_stop(e);
        assert_eq!(st_engine_status(e).transport_pending, 1);
        render(e, 16, 16);
        assert_eq!(st_engine_status(e).playing, 0);
    }
}

#[test]
fn bus_history_is_not_rendered_twice() {
    unsafe {
        let engine = Engine::new();
        let e = engine.0;
        begin_project(e, st_default_instrument());
        load_sine(e, 0);
        set_note(e, 0, 60);
        commit(e);
        st_engine_play(e, 0);
        let (mut first, mut again, mut tail) = (vec![0.0f32; 512], vec![9.0f32; 512], vec![0.0f32; 300]);
        st_engine_render_bus(e, 0, 1000, first.as_mut_ptr(), null_mut(), 512);
        assert!(first.iter().any(|&x| x != 0.0));
        // The same range again, and an overlapping pull of another size: copies, not new audio.
        st_engine_render_bus(e, 0, 1000, again.as_mut_ptr(), null_mut(), 512);
        assert_eq!(first, again);
        st_engine_render_channel(e, 0, 1300, tail.as_mut_ptr(), null_mut(), 300);
        assert_eq!(&first[300..], &tail[..212]);
        // The return bus of an 1-track project is bus 16; buses beyond it are silent.
        let mut beyond = vec![5.0f32; 64];
        st_engine_render_bus(e, 17, 1000, beyond.as_mut_ptr(), null_mut(), 64);
        assert!(beyond.iter().all(|&x| x == 0.0));
        let mut span = STClockSpan::default();
        assert_eq!(st_engine_clock_span(e, 1000, 100, 0, &mut span), ST_CLOCK_OK);
        assert_eq!((span.frames, span.context.graph_frame, span.context.bpm), (100, 1000, 120.0));
        assert_eq!(st_engine_clock_span(e, 5000, 100, 0, &mut span), ST_ENGINE_CLOCK_NOT_PREPARED);
        let mut freeze = STFreezeSpan::default();
        assert_eq!(st_engine_freeze_span(e, 1000, 100, 0, &mut freeze), ST_FREEZE_NOT_CONFIGURED);
    }
}

#[test]
fn checked_commit_reports_problems_and_keeps_the_published_project() {
    unsafe {
        let engine = Engine::new();
        let e = engine.0;
        begin_project(e, st_default_instrument());
        commit(e);
        let duration = st_engine_duration(e, 1);
        assert_eq!(duration, 2.0); // 16 rows at 8 rows per second
        let mut error = [0 as std::ffi::c_char; 256];
        let tempos = [STClockTempoEvent { beat: 0.0, bpm: 5.0 }];
        let meters = [STClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 }];
        assert_eq!(st_engine_set_timing_map(e, tempos.as_ptr(), 1, meters.as_ptr(), 1, 1, error.as_mut_ptr(), 256), 0);
        assert_ne!(error[0], 0);
        assert_eq!(st_engine_set_frozen_song_policy(e, 1, 0, error.as_mut_ptr(), 256), 0);
        assert_eq!(st_engine_set_frozen_song_policy(e, 1, 7, error.as_mut_ptr(), 256), 1);
        // An extent shorter than the song is rejected at commit; the draft stays editable.
        assert_eq!(st_engine_set_frozen_song_extent(e, 7, 1000, error.as_mut_ptr(), 256), 1);
        assert_eq!(st_engine_commit_update_checked(e, error.as_mut_ptr(), 256), 0);
        assert_ne!(error[0], 0);
        assert_eq!(st_engine_duration(e, 1), duration);
        st_engine_cancel_update(e);
        let epoch = st_engine_pattern_epoch(e);
        st_engine_begin_update(e);
        st_engine_set_timing(e, 90.0, 4, 0.0, 1.0);
        commit(e);
        assert_eq!(st_engine_pattern_epoch(e), epoch, "a commit without pattern edits keeps the epoch");
        st_engine_begin_update(e);
        set_note(e, 1, 62);
        commit(e);
        assert_ne!(st_engine_pattern_epoch(e), epoch);
    }
}

/// The control thread republishes the project and replaces PCM while the render thread plays:
/// retired projects and samples are reclaimed by `collect` without disturbing the renderer.
#[test]
fn concurrent_publication_and_rendering() {
    struct Handle(*mut STEngine);
    // SAFETY: the engine's threading contract is exactly this split: one control thread, one
    // render thread.
    unsafe impl Send for Handle {}
    unsafe {
        let engine = Engine::new();
        let e = engine.0;
        begin_project(e, st_default_instrument());
        load_sine(e, 0);
        for row in 0..16 {
            set_note(e, row, 48 + row);
        }
        commit(e);
        st_engine_play(e, 0);
        let handle = Handle(e);
        let renderer = std::thread::spawn(move || {
            let handle = handle;
            let mut peak = 0.0f32;
            for _ in 0..400 {
                let (l, _) = render(handle.0, 256, 256);
                assert!(l.iter().all(|x| x.is_finite()));
                peak = l.iter().fold(peak, |m, x| m.max(x.abs()));
                while st_engine_pop_event(handle.0, &mut STEvent::default()) != 0 {}
            }
            peak
        });
        for round in 0..200 {
            st_engine_begin_update(e);
            st_engine_set_timing(e, 100.0 + round as f64, 4, 0.0, 1.0);
            set_note(e, round % 16, 50 + round % 20);
            commit(e);
            if round % 10 == 0 {
                load_sine(e, 0);
            }
            st_engine_note_on(e, 0, 0, 72, 100);
            st_engine_collect(e);
        }
        assert!(renderer.join().unwrap() > 0.0);
    }
}

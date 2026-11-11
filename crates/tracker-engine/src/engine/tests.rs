//! Unit tests of the engine through its handles (the acceptance tests are the integration tests
//! in `tests/`; these cover the same ground in miniature and run first).

use super::host::{ControlHalf, Engine, RenderHalf};
use super::*;

const RATE: f64 = 48000.0;

fn engine() -> Engine {
    Engine::new(RATE).expect("an engine at 48 kHz")
}

/// One track, one 16-row pattern at 120 BPM / 4 lines per beat: a row lasts 6000 frames.
fn begin_project(control: &mut ControlHalf, instrument: STInstrument) {
    let (edit, shared, _) = control.parts();
    edit.begin_update(shared);
    edit.clear_project();
    assert!(edit.set_track_count(1));
    edit.set_timing(120.0, 4, 0.0, 1.0);
    edit.set_pattern(0, 16);
    edit.set_sequence(&[0], false, 0);
    edit.set_instrument(0, instrument);
    edit.set_track(0, default_track());
}

fn commit(control: &mut ControlHalf) {
    control.commit().expect("the project is accepted");
}

fn load_sine(control: &mut ControlHalf, instrument: i32) {
    let wave: Vec<f32> = (0..4800).map(|n| (n as f32 * 0.05).sin() * 0.5).collect();
    let (edit, shared, _) = control.parts();
    assert!(edit.set_sample(shared, instrument, Some(edits::Pcm { left: &wave, right: None, rate: RATE })));
}

fn set_note(control: &mut ControlHalf, row: i32, note: i32) {
    let mut step = default_step();
    step.note = note;
    step.instrument = 0;
    control.parts().0.set_step(0, 0, row, step, None);
}

fn play(control: &mut ControlHalf) {
    let (edit, shared, _) = control.parts();
    edit.play(shared, 0);
}

fn render(render: &mut RenderHalf, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    for (left, right) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        render.render(left, right);
    }
    (l, r)
}

fn events(render: &mut RenderHalf) -> Vec<STEvent> {
    std::iter::from_fn(|| render.pop_event()).collect()
}

#[test]
fn defaults_match_the_header() {
    let track = default_track();
    assert_eq!((track.volume, track.pan, track.instrument), (0.8, 0.0, 0));
    let instrument = default_instrument();
    assert_eq!(instrument.type_, ST_INSTRUMENT_SAMPLE);
    assert_eq!((instrument.root_note, instrument.slice_count, instrument.bit_depth), (60, 16, 16));
    assert_eq!((instrument.volume, instrument.end, instrument.loop_end, instrument.cutoff), (0.7, 1.0, 1.0, 18000.0));
    assert_eq!((instrument.grain_density, instrument.lfo_beats, instrument.lfo2_beats), (2, 1.0, 1.0));
    let step = default_step();
    assert_eq!((step.note, step.instrument, step.velocity, step.gate), (60, -1, 100, 0.8));
    assert_eq!(return_bus(8), 16);
    assert_eq!(return_bus(40), 40);
    assert_eq!(output_bus_count(8), 17);
    assert_eq!((channel_count(8), channel_output_bus(8, 3), instrument_capacity(64)), (16, 3, 64));
}

#[test]
fn rejects_invalid_engines_and_foreign_shared_state() {
    assert!(Engine::new(100.0).is_none());
    assert!(Engine::new(f64::NAN).is_none());
    assert!(host::split(400_000.0).is_none());
    // The halves of one engine do nothing with the shared state of another.
    let (mut one, mut other) = (engine(), engine());
    let (control, render_half, status) = one.halves();
    let (other_control, other_render, other_status) = other.halves();
    let (_, foreign, _) = other_control.parts();
    let (edit, _, _) = control.parts();
    assert!(edit.set_track_count(4));
    assert_eq!(edit.commit(foreign), Err(control::FOREIGN_ENGINE.to_string()));
    edit.play(foreign, 1);
    assert!(!edit.input_note(foreign, 1, ST_INPUT_NOTE_ON, 0, 0, 60, 100, 1));
    assert!(!edit.set_instrument_parameter(foreign, 0, ST_TARGET_CUTOFF, 0.5));
    assert_eq!(other_status.status().transport_pending, 0, "the other engine received no command");
    assert_eq!(status.status().transport_pending, 0);
    let (render_state, _) = render_half.parts();
    let mut buffer = [1.0f32; 8];
    let mut second = [1.0f32; 8];
    render_state.render(foreign, Some(&mut buffer), Some(&mut second), 8);
    assert_eq!((buffer, second), ([0.0; 8], [0.0; 8]));
    assert_eq!(render_state.render_block(foreign, 0, 64), 0);
    // Both engines still work with their own.
    control.commit().expect("the first engine commits its own draft");
    other_render.advance(16);
    render_half.advance(16);
    assert_eq!((status.status().track_count, other_status.status().track_count), (4, 8));
}

#[test]
fn sampler_output_is_audible_and_independent_of_block_size() {
    let render_with = |block: usize| {
        let mut engine = engine();
        let (control, audio, _) = engine.halves();
        begin_project(control, default_instrument());
        load_sine(control, 0);
        set_note(control, 0, 60);
        set_note(control, 2, 67);
        commit(control);
        play(control);
        render(audio, 20000, block)
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
    let mut engine = engine();
    let (control, audio, status) = engine.halves();
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_AU;
    begin_project(control, instrument);
    set_note(control, 0, 60);
    set_note(control, 2, 64);
    commit(control);
    play(control);
    let mut onsets = Vec::new();
    let mut offs = Vec::new();
    for block in 0..4 {
        render(audio, 5000, 5000);
        for event in events(audio) {
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
    let status = status.status();
    assert_eq!((status.playing, status.row, status.track_count), (1, 3, 1));
}

#[test]
fn queued_transport_is_reported_until_rendered() {
    let mut engine = engine();
    let (control, audio, status) = engine.halves();
    play(control);
    let queued = status.status();
    assert_eq!((queued.playing, queued.transport_pending, queued.requested_playing), (0, 1, 1));
    assert!(queued.transport_playing(), "a queued Play already counts as playing");
    render(audio, 16, 16);
    let playing = status.status();
    assert_eq!((playing.playing, playing.transport_pending), (1, 0));
    assert!(playing.transport_playing());
    let (edit, shared, _) = control.parts();
    edit.stop(shared);
    assert_eq!(status.status().transport_pending, 1);
    assert!(!status.status().transport_playing());
    render(audio, 16, 16);
    assert_eq!(status.status().playing, 0);
    let (edit, shared, _) = control.parts();
    edit.play_count_in(shared, false, 1);
    assert_eq!(status.status().count_in_shown(), 0, "nothing to show while the count-in is only queued");
    render(audio, 16, 16);
    let counting = status.status();
    assert!(counting.count_in > 0 && counting.count_in_shown() == counting.count_in);
}

#[test]
fn a_commit_without_pattern_edits_keeps_the_pattern_epoch() {
    let mut engine = engine();
    let control = engine.control();
    begin_project(control, default_instrument());
    commit(control);
    let (edit, shared, _) = control.parts();
    assert_eq!(edit.duration(true), 2.0); // 16 rows at 8 rows per second
    let epoch = edit.pattern_epoch();
    edit.begin_update(shared);
    edit.set_timing(90.0, 4, 0.0, 1.0);
    commit(control);
    assert_eq!(control.parts().0.pattern_epoch(), epoch, "a commit without pattern edits keeps the epoch");
    let (edit, shared, _) = control.parts();
    edit.begin_update(shared);
    set_note(control, 1, 62);
    commit(control);
    assert_ne!(control.parts().0.pattern_epoch(), epoch);
}

/// Sample and asset PCM is validated before anything is stored; an asset slot opens the draft
/// first, as it always did.
#[test]
fn pcm_is_validated_before_it_is_stored() {
    let mut engine = engine();
    let control = engine.control();
    begin_project(control, default_instrument());
    commit(control);
    let (edit, shared, _) = control.parts();
    let pcm = |left: &'static [f32], rate: f64| Some(edits::Pcm { left, right: None, rate });
    assert!(!edit.set_sample(shared, 0, pcm(&[], RATE)), "no frames");
    assert!(!edit.set_sample(shared, 0, pcm(&[0.5], 10.0)), "an unusable rate");
    assert!(!edit.set_sample(shared, 0, Some(edits::Pcm { left: &[0.5, 0.5], right: Some(&[0.5]), rate: RATE })));
    assert!(!edit.set_sample(shared, 5000, pcm(&[0.5], RATE)), "no such instrument");
    assert!(edit.draft.is_none(), "a rejected sample leaves no draft behind");
    assert!(!edit.set_audio_asset(shared, 256, pcm(&[0.5], RATE)), "no such asset slot");
    assert!(edit.draft.is_none());
    assert!(!edit.set_audio_asset(shared, 0, pcm(&[], RATE)));
    assert!(edit.draft.is_some(), "a valid slot opens the draft before the PCM is looked at");
    edit.cancel_update();
    assert!(edit.set_audio_asset(shared, 0, pcm(&[0.5], RATE)));
    assert!(edit.set_sample(shared, 0, pcm(&[0.5], RATE)));
    assert!(edit.set_sample(shared, 0, None), "clearing a sample");
}

/// The control thread republishes the project and replaces PCM while the render thread plays:
/// retired projects and samples are reclaimed by `collect` without disturbing the renderer.
#[test]
fn concurrent_publication_and_rendering() {
    let mut engine = engine();
    let control = engine.control();
    begin_project(control, default_instrument());
    load_sine(control, 0);
    for row in 0..16 {
        set_note(control, row, 48 + row);
    }
    commit(control);
    play(control);
    let (mut control, mut audio, _) = engine.split();
    let renderer = std::thread::spawn(move || {
        let mut peak = 0.0f32;
        for _ in 0..400 {
            let (l, _) = render(&mut audio, 256, 256);
            assert!(l.iter().all(|x| x.is_finite()));
            peak = l.iter().fold(peak, |m, x| m.max(x.abs()));
            while audio.pop_event().is_some() {}
        }
        peak
    });
    for round in 0..200 {
        let (edit, shared, _) = control.parts();
        edit.begin_update(shared);
        edit.set_timing(100.0 + round as f64, 4, 0.0, 1.0);
        set_note(&mut control, round % 16, 50 + round % 20);
        commit(&mut control);
        if round % 10 == 0 {
            load_sine(&mut control, 0);
        }
        let (edit, shared, _) = control.parts();
        edit.note_on(shared, 0, 0, 72, 100);
        edit.collect(shared);
    }
    assert!(renderer.join().unwrap() > 0.0);
}

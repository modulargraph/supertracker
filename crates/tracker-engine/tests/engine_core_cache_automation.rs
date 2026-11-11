//! Engine core suites: long renders that must keep note-offs, exact stepped automation, timed
//! mix defaults and the automation refresh (also as a concurrent mailbox), live notes across
//! automatic playback changes, the explicit slice effect and step preview ownership.
//!
//! One `#[test]` per suite, calling the engine through `engine_support_a::Engine`, which renders
//! it block by block as a host does: the tests read its buses (a plain render gives their sum).
//! Every test ends with the "zero render-thread allocations" check.
// The tests index their buffers in plain loops.
#![allow(clippy::needless_range_loop)]

mod engine_support_a;

use engine_support_a::core_helpers::*;
use engine_support_a::tone::*;
use engine_support_a::{assert_zero_render_allocations, Engine, SetOnDrop};
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use tracker_engine::engine::*;

#[test]
fn test_large_pull_protects_note_offs() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_AU);
    e.begin_update();
    let mut step = default_step();
    step.gate = 10.0;
    e.set_step(0, 0, 0, step, &[]);
    for lane in 0..40 {
        e.add_host_automation_point(0, lane, lane as u64, 0, 0.0, 0.0);
        e.add_host_automation_point(0, lane, lane as u64, 0, 64.0, 1.0);
    }
    e.commit_update();
    e.play(0);
    // 16 blocks whose events nobody collects.
    let mut pull = Audio::new(65536);
    bus_render_many(&mut e, &[0], 0, &mut [&mut pull]);
    for n in 0..600 {
        e.note_on(0, 0, 96 + n % 12, 100);
    }
    let mut one_frame = Audio::new(1);
    bus_render(&mut e, 0, 65536, &mut one_frame);
    e.panic();
    bus_render(&mut e, 0, 65537, &mut one_frame);
    let (mut on, mut off, mut manual_on, mut manual_off) = (0, 0, 0, 0);
    while let Some(event) = e.pop_event() {
        if event.type_ == ST_EVENT_NOTE_ON {
            if event.note == 60 {
                on += 1;
                assert!(event.frame_offset == 0);
            } else {
                manual_on += 1;
            }
        }
        if event.type_ == ST_EVENT_NOTE_OFF {
            if event.note == 60 {
                off += 1;
                // Frame 60000, counted from the start of its block.
                assert!(event.frame_offset == 60000 % 4096);
            } else {
                manual_off += 1;
            }
        }
    }
    assert!(on == 1 && off == 1);
    assert!(manual_on > 0 && manual_on < 600 && manual_on == manual_off);
    assert!(e.status().dropped_events > 0);
    assert_zero_render_allocations();
}

#[test]
fn test_exact_stepped_automation() {
    let mut e = Engine::new();
    configure(&mut e, TONE);
    e.begin_update();
    // The mixer values of a track: the targets whose changes the engine reports.
    let targets = [ST_TARGET_VOLUME, ST_TARGET_PAN, ST_TARGET_DELAY, ST_TARGET_REVERB];
    for target in targets {
        e.add_automation_point(0, 0, target, 1, 0.0, 0.0);
        e.add_automation_point(0, 0, target, 1, 1.0, 1.0);
        e.add_automation_point(0, 0, target, 1, 1.5, 0.5);
    }
    e.add_host_automation_point(0, 9, 333, 1, 0.0, 0.0);
    e.add_host_automation_point(0, 9, 333, 1, 0.1, 0.75);
    e.add_host_automation_point(0, 9, 333, 1, 0.5, 0.75);
    e.add_host_automation_point(0, 9, 333, 1, 1.0, 1.0);
    e.commit_update();
    e.play(0);
    let mut events = render_events_block(&mut e, 12001, 257);
    for target in targets {
        let values = parameter_changes(&events, target);
        assert!(values.len() == 3);
        assert!(values[0].frame == 0 && values[0].e.value == 0.0);
        assert!(values[1].frame == 6000 && values[1].e.value == 1.0);
        assert!(values[2].frame == 9000 && values[2].e.value == 0.5);
    }
    let mut fractional: Vec<Timed> = Vec::new();
    for event in &events {
        if event.e.address == 333 {
            fractional.push(*event);
        }
    }
    assert!(fractional.len() == 3 && fractional[0].frame == 0 && fractional[1].frame == 600 && fractional[2].frame == 6000);
    e.seek(0, 2, 0);
    events = render_events(&mut e, 1);
    let mut restored = 0;
    for target in targets {
        let values = parameter_changes(&events, target);
        assert!(values.len() == 1 && values[0].frame == 0 && values[0].e.value == 0.5);
        restored += 1;
    }
    assert!(restored == 4);
    let mut locked = Engine::new();
    configure(&mut locked, TONE);
    locked.begin_update();
    locked.add_automation_point(0, 0, ST_TARGET_VOLUME, 1, 0.0, 0.8);
    let mut effect = default_step();
    effect.note = -3;
    effect.fx1 = fx(ST_FX_VOLUME, 0.3);
    locked.set_step(0, 0, 1, effect, &[]);
    locked.commit_update();
    locked.play(0);
    let changes = render_events(&mut locked, 15000);
    let volume = parameter_changes(&changes, ST_TARGET_VOLUME);
    assert!(volume.len() == 3);
    assert!(volume[0].frame == 0 && volume[1].frame == 6000 && volume[2].frame == 12000);
    assert!(
        (volume[0].e.value - 0.8).abs() < 1e-6
            && (volume[1].e.value - 0.3).abs() < 1e-6
            && (volume[2].e.value - 0.8).abs() < 1e-6
    );
    assert_zero_render_allocations();
}

#[test]
fn test_timed_mix_defaults_and_automation_refresh() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_AU);
    e.begin_update();
    e.add_automation_point(0, 0, ST_TARGET_VOLUME, 1, 0.0, 0.0);
    e.add_automation_point(0, 0, ST_TARGET_PAN, 1, 0.0, -0.6);
    e.commit_update();
    e.play(0);
    let mut first = Audio::new(512);
    bus_render(&mut e, 0, 0, &mut first);
    let mut initial: Vec<Timed> = Vec::new();
    while let Some(event) = e.pop_event() {
        initial.push(Timed { frame: event.frame_offset as u64, e: event });
    }
    let before = parameter_changes(&initial, ST_TARGET_VOLUME);
    assert!(before.len() == 1 && before[0].e.value == 0.0);
    // The project again, as a host publishes it: without the volume lane, with other mixer values.
    e.begin_update();
    e.clear_project();
    e.set_track_count(2);
    e.add_automation_point(0, 0, ST_TARGET_PAN, 1, 0.0, -0.6);
    let mut track = default_track();
    track.volume = 1.2;
    track.delay_send = 0.25;
    track.reverb_send = 0.1;
    e.set_track(0, track);
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_AU;
    instrument.volume = 0.6;
    instrument.pan = 0.2;
    e.set_instrument(0, instrument);
    e.commit_update();
    assert!(e.pop_event().is_none());
    bus_render(&mut e, 16, 0, &mut first);
    assert!(e.pop_event().is_none());
    let mut extended = Audio::new(512);
    bus_render(&mut e, 0, 512, &mut extended);
    let mut updated: Vec<Timed> = Vec::new();
    while let Some(event) = e.pop_event() {
        updated.push(Timed { frame: 512 + event.frame_offset as u64, e: event });
    }
    let targets = [ST_TARGET_VOLUME, ST_TARGET_DELAY, ST_TARGET_REVERB];
    let expected = [1.2f64, 0.25, 0.1];
    for n in 0..3 {
        let changes = parameter_changes(&updated, targets[n]);
        assert!(changes.len() == 1 && changes[0].frame == 512 && (changes[0].e.value - expected[n]).abs() < 1e-6);
    }
    let mut pan: Vec<Timed> = Vec::new();
    for value in &updated {
        if value.e.type_ == ST_EVENT_PARAMETER && value.e.track == 0 && value.e.target == ST_TARGET_PAN {
            pan.push(*value);
        }
    }
    assert!(
        pan.len() == 2
            && pan[0].frame == 512
            && pan[0].e.value == 0.0
            && pan[1].frame == 512
            && (pan[1].e.value + 0.6).abs() < 1e-6
    );
    bus_render(&mut e, 0, 1024, &mut first);
    assert!(e.pop_event().is_none());
    let mut song = Engine::new();
    configure(&mut song, ST_INSTRUMENT_AU);
    song.begin_update();
    song.set_pattern(0, 1);
    song.set_pattern(1, 1);
    let sequence = [0, 1];
    song.set_sequence(&sequence, 0, 0);
    song.add_automation_point(0, 0, ST_TARGET_VOLUME, 1, 0.0, 0.0);
    song.commit_update();
    song.play(1);
    let sequence_events = render_events(&mut song, 13000);
    let volume = parameter_changes(&sequence_events, ST_TARGET_VOLUME);
    assert!(
        volume.len() == 2
            && volume[0].frame == 0
            && volume[0].e.value == 0.0
            && volume[1].frame == 6000
            && (volume[1].e.value - 0.8).abs() < 1e-6
    );
    let mut locked = Engine::new();
    configure(&mut locked, ST_INSTRUMENT_AU);
    locked.begin_update();
    let mut effect = default_step();
    effect.note = -3;
    effect.fx1 = fx(ST_FX_VOLUME, 0.3);
    locked.set_step(0, 0, 0, effect, &[]);
    locked.add_automation_point(0, 0, ST_TARGET_VOLUME, 1, 0.0, 0.8);
    locked.add_automation_point(0, 0, ST_TARGET_PAN, 1, 0.0, -0.5);
    locked.commit_update();
    locked.play(0);
    let mut small = Audio::new(128);
    bus_render(&mut locked, 0, 0, &mut small);
    while locked.pop_event().is_some() {}
    locked.refresh_automation();
    bus_render(&mut locked, 16, 0, &mut small);
    assert!(locked.pop_event().is_none());
    bus_render(&mut locked, 0, 128, &mut small);
    let mut refreshed: Vec<Timed> = Vec::new();
    while let Some(event) = locked.pop_event() {
        refreshed.push(Timed { frame: event.frame_offset as u64, e: event });
    }
    let (locked_volume, held_pan) =
        (parameter_changes(&refreshed, ST_TARGET_VOLUME), parameter_changes(&refreshed, ST_TARGET_PAN));
    assert!(locked_volume.len() == 1 && locked_volume[0].frame == 0 && (locked_volume[0].e.value - 0.3).abs() < 1e-6);
    assert!(held_pan.len() == 1 && held_pan[0].frame == 0 && held_pan[0].e.value == -0.5);
    assert_zero_render_allocations();
}

#[test]
fn test_concurrent_automation_refresh_mailbox() {
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        for n in 0..1000 {
            e.note_on(0, 0, 60 + n % 12, 100);
            e.note_off(0, 60 + n % 12);
        }
        for _ in 0..100000 {
            e.refresh_automation();
        }
        let mut a = Audio::new(64);
        render(&mut e, &mut a);
        let (mut on, mut off, mut parameters) = (0, 0, 0);
        while let Some(event) = e.pop_event() {
            on += (event.type_ == ST_EVENT_NOTE_ON) as i32;
            off += (event.type_ == ST_EVENT_NOTE_OFF) as i32;
            parameters += (event.type_ == ST_EVENT_PARAMETER) as i32;
        }
        // Four mixer values for each of the two tracks, sent once for the project and once for
        // the refresh.
        assert!(on == 1000 && off == 1000 && parameters == 16);
        let status = e.status();
        assert!(status.dropped_commands == 0 && status.dropped_events == 0);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        let mut initial = Audio::new(64);
        render(&mut e, &mut initial);
        while e.pop_event().is_some() {}
        let (go, notes_done, refresh_done) = (AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false));
        let (on, off);
        {
            // One thread per half: audio renders and consumes events, "ui" is the control
            // thread, and a third thread posts refresh requests through the shared mailbox.
            let (mut control, mut audio_half, shared) = e.split();
            let (go, notes_done, refresh_done) = (&go, &notes_done, &refresh_done);
            (on, off) = std::thread::scope(|scope| {
                let audio = scope.spawn(move || {
                    while !go.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    let mut a = Audio::new(32);
                    let (mut on, mut off) = (0, 0);
                    let mut tick = || {
                        audio_half.render_into(&mut a.l, &mut a.r);
                        while let Some(value) = audio_half.pop_event() {
                            on += (value.type_ == ST_EVENT_NOTE_ON) as i32;
                            off += (value.type_ == ST_EVENT_NOTE_OFF) as i32;
                        }
                    };
                    while !notes_done.load(Ordering::Acquire) || !refresh_done.load(Ordering::Acquire) {
                        tick();
                    }
                    tick();
                    (on, off)
                });
                let ui = scope.spawn(move || {
                    let _finished = SetOnDrop(notes_done);
                    while !go.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    for n in 0..1000 {
                        control.note_on(0, 0, 48 + n % 24, 100);
                        control.note_off(0, 48 + n % 24);
                    }
                });
                let refresh = scope.spawn(move || {
                    let _finished = SetOnDrop(refresh_done);
                    while !go.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    for _ in 0..100000 {
                        shared.refresh_automation();
                    }
                });
                go.store(true, Ordering::Release);
                ui.join().unwrap();
                refresh.join().unwrap();
                audio.join().unwrap()
            });
        }
        assert!(on == 1000 && off == 1000);
        let status = e.status();
        assert!(status.dropped_commands == 0 && status.dropped_events == 0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_live_notes_survive_automatic_playback_changes() {
    for looping in [0, 1] {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.set_pattern(0, 1);
        let seq = [0];
        e.set_sequence(&seq, looping, 0);
        e.commit_update();
        e.play(1);
        e.note_on(0, 0, 69, 127);
        let mut a = Audio::new(14000);
        render(&mut e, &mut a);
        assert!(a.energy_in(8000, 11000) > 0.001);
        assert!(e.status().playing == looping);
        e.note_off(0, 69);
        let mut release = Audio::new(2048);
        render(&mut e, &mut release);
        assert!(release.energy_in(1024, 2048) == 0.0);
    }
    for type_ in [ST_INSTRUMENT_AU, ST_INSTRUMENT_MIDI] {
        for command in [-1, -2] {
            let mut e = Engine::new();
            configure(&mut e, type_);
            e.begin_update();
            e.set_pattern(0, 2);
            let seq = [0];
            e.set_sequence(&seq, 1, 0);
            let mut step = default_step();
            step.note = 60;
            step.gate = 32.0;
            e.set_step(0, 0, 0, step, &[]);
            step.note = command;
            e.set_step(0, 0, 1, step, &[]);
            e.commit_update();
            e.play(1);
            e.note_on(0, 0, 72, 127);
            let mut events = note_events(render_events(&mut e, 14000));
            let (mut live_on, mut live_off, mut playback_off) = (0, 0, 0);
            for event in &events {
                if event.e.note == 72 {
                    live_on += (event.e.type_ == ST_EVENT_NOTE_ON) as i32;
                    live_off += (event.e.type_ == ST_EVENT_NOTE_OFF) as i32;
                }
                if event.e.note == 60 && event.e.type_ == ST_EVENT_NOTE_OFF {
                    playback_off += 1;
                    assert!(event.frame == 6000);
                }
            }
            assert!(live_on == 1 && live_off == 0 && playback_off == 1);
            e.note_off(0, 60);
            assert!(note_events(render_events(&mut e, 1)).is_empty());
            e.note_off(0, 72);
            events = note_events(render_events(&mut e, 1));
            assert!(events.len() == 1 && events[0].e.type_ == ST_EVENT_NOTE_OFF && events[0].e.note == 72);
        }
    }
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        let mut p = test_tone_instrument(TEST_SINE);
        p.attack = 0.0;
        p.decay = 0.0;
        p.sustain = 1.0;
        p.release = 0.01;
        p.volume = 0.2;
        e.set_instrument(0, p);
        let mut step = default_step();
        step.note = 69;
        step.velocity = 127;
        step.gate = 32.0;
        e.set_step(0, 0, 0, step, &[]);
        e.commit_update();
        e.play(0);
        e.note_on(0, 0, 69, 127);
        let mut both = Audio::new(4096);
        render(&mut e, &mut both);
        e.note_off(0, 69);
        let mut playback = Audio::new(4096);
        render(&mut e, &mut playback);
        assert!(playback.energy_in(2048, 4096) > 0.001 && playback.energy_in(2048, 4096) < both.energy_in(2048, 4096) * 0.4);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_AU);
        e.begin_update();
        e.set_pattern(0, 1);
        let mut step = default_step();
        step.gate = 32.0;
        e.set_step(0, 0, 0, step, &[]);
        e.commit_update();
        e.play(1);
        e.note_on(0, 0, 72, 100);
        let mut events = note_events(render_events(&mut e, 7000));
        let mut playback_off = 0;
        for event in &events {
            if event.e.type_ == ST_EVENT_NOTE_OFF {
                assert!(event.e.note == 60 && event.frame == 6000);
                playback_off += 1;
            }
        }
        assert!(playback_off == 1);
        e.stop();
        events = note_events(render_events(&mut e, 1));
        assert!(events.len() == 1 && events[0].e.type_ == ST_EVENT_NOTE_OFF && events[0].e.note == 72);
    }
    assert_zero_render_allocations();
}

/// 5000 frames of a four-slice sample whose slices differ in level.
fn slice_fx_audio(note: i32, selector: i32, manual: bool, chord: &[i32]) -> Audio {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_SAMPLE);
    let mut sample = vec![0.0f32; 8192];
    for n in 0..sample.len() {
        sample[n] = (2.0 * PI * n as f64 / 64.0).sin() as f32
            * (if n < 2048 {
                0.03f32
            } else if n < 6144 {
                0.3f32
            } else {
                0.07f32
            });
    }
    e.begin_update();
    e.set_sample(0, &sample, 48000.0);
    let mut p = default_instrument();
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.mode = ST_MODE_SLICE;
    p.slice_count = 4;
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.volume = 0.2;
    e.set_instrument(0, p);
    if manual {
        let markers = [0.0f32, 0.25, 0.75];
        e.set_slice_points(0, &markers);
    }
    let mut step = default_step();
    step.note = note;
    step.gate = 8.0;
    if selector > 0 {
        step.fx1 = fx(ST_FX_SLICE, selector as f32);
    }
    e.set_step(0, 0, 0, step, chord);
    e.commit_update();
    e.play(0);
    let mut a = Audio::new(5000);
    render(&mut e, &mut a);
    a
}

#[test]
fn test_explicit_slice_fx() {
    let (mapped, explicit_slice, raised) =
        (slice_fx_audio(61, 0, false, &[]), slice_fx_audio(60, 2, false, &[]), slice_fx_audio(72, 2, false, &[]));
    assert!(mapped.l == explicit_slice.l && mapped.r == explicit_slice.r);
    assert!(explicit_slice.energy_in(1600, 1800) > 0.0001 && raised.energy_in(1600, 1800) < 1e-12);
    let (manual, manual_fx) = (slice_fx_audio(61, 0, true, &[]), slice_fx_audio(60, 2, true, &[]));
    assert!(manual.l == manual_fx.l);
    assert!(slice_fx_audio(60, 128, true, &[]).l == slice_fx_audio(60, 3, true, &[]).l);
    let chord = [0, 12];
    let combined = slice_fx_audio(60, 2, false, &chord);
    for n in 0..combined.l.len() {
        assert!(((combined.l[n] - explicit_slice.l[n] - raised.l[n]).abs() as f64) < 1e-6);
    }
    let empty = slice_fx_audio(-3, 2, false, &[]);
    assert!(empty.energy() == 0.0);
    assert_zero_render_allocations();
}

#[test]
fn test_step_preview_ownership() {
    {
        let (mut e, mut reference) = (Engine::new(), Engine::new());
        configure(&mut e, TONE);
        configure(&mut reference, TONE);
        for engine in [&mut e, &mut reference] {
            engine.note_on(0, 0, 72, 60);
        }
        let (mut warm, mut reference_warm) = (Audio::new(1024), Audio::new(1024));
        render(&mut e, &mut warm);
        render(&mut reference, &mut reference_warm);
        let mut step = default_step();
        step.note = 60;
        step.velocity = 35;
        step.gate = 256.0;
        let chord = [0, 7, 12];
        e.preview_step(0, step, &chord, 0.03);
        let (mut audio, mut expected) = (Audio::new(8000), Audio::new(8000));
        render(&mut e, &mut audio);
        render(&mut reference, &mut expected);
        assert!(audio.energy_in(0, 512) != expected.energy_in(0, 512));
        for n in 6000..8000 {
            assert!(audio.l[n] == expected.l[n] && audio.r[n] == expected.r[n]);
        }
        assert!(e.status().playing == 0 && e.status().seconds == 0.0);
    }
    {
        let (mut e, mut reference) = (Engine::new(), Engine::new());
        for engine in [&mut e, &mut reference] {
            configure(engine, TONE);
            engine.begin_update();
            let mut p = test_tone_instrument(TEST_SINE);
            p.attack = 0.0;
            p.sustain = 1.0;
            p.release = 0.01;
            engine.set_instrument(0, p);
            assert!(engine.set_track_voice(0, 0, 0, 2));
            engine.commit_update();
        }
        let mut step = default_step();
        e.preview_step(0, step, &[], 0.35);
        let (mut warm, mut silent) = (Audio::new(64), Audio::new(64));
        render(&mut e, &mut warm);
        render(&mut reference, &mut silent);
        e.note_on(0, 0, 72, 100);
        reference.note_on(0, 0, 72, 100);
        let (mut head, mut expected_head) = (Audio::new(128), Audio::new(128));
        render(&mut e, &mut head);
        render(&mut reference, &mut expected_head);
        step.note = -1;
        e.preview_step(0, step, &[], 0.03);
        let (mut held, mut expected) = (Audio::new(2048), Audio::new(2048));
        render(&mut e, &mut held);
        render(&mut reference, &mut expected);
        assert!(held.energy() > 0.001 && held.l == expected.l && held.r == expected.r);
        e.note_off(0, 72);
        let mut tail = Audio::new(2048);
        render(&mut e, &mut tail);
        assert!(tail.energy_in(1024, 2048) == 0.0);
    }
    for type_ in [ST_INSTRUMENT_AU, ST_INSTRUMENT_MIDI] {
        let mut e = Engine::new();
        configure(&mut e, type_);
        e.note_on(0, 0, 60, 100);
        render_events(&mut e, 1);
        let mut step = default_step();
        step.note = 60;
        // An empty slice of a one-note array: no chord.
        let ignored = [12];
        e.preview_step(0, step, &ignored[..0], 0.35);
        assert!(note_events(render_events(&mut e, 1)).is_empty());
        step.note = 72;
        step.velocity = 39;
        let chord = [0, 4];
        e.preview_step(0, step, &chord, 0.03);
        let mut notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 2 && notes[0].e.note == 72 && notes[1].e.note == 76 && notes[0].e.velocity == 39);
        e.note_on(0, 0, 72, 100);
        notes = note_events(render_events(&mut e, 1));
        assert!(
            notes.len() == 2
                && notes[0].e.type_ == ST_EVENT_NOTE_OFF
                && notes[0].e.note == 72
                && notes[1].e.type_ == ST_EVENT_NOTE_ON
        );
        step.note = -1;
        e.preview_step(0, step, &[], 0.35);
        notes = note_events(render_events(&mut e, 2000));
        assert!(notes.len() == 1 && notes[0].e.note == 76 && notes[0].e.type_ == ST_EVENT_NOTE_OFF);
        e.note_off(0, 60);
        e.note_off(0, 72);
        notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 2);
        assert!(e.status().playing == 0 && e.status().seconds == 0.0);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let mut sample = vec![0.1f32; 8192];
        sample[2048..4096].fill(0.5);
        e.begin_update();
        e.set_sample(0, &sample, 48000.0);
        let mut p = default_instrument();
        p.type_ = ST_INSTRUMENT_SAMPLE;
        p.mode = ST_MODE_ONESHOT;
        p.slice_count = 4;
        p.attack = 0.0;
        p.decay = 0.0;
        p.sustain = 1.0;
        p.release = 0.001;
        e.set_instrument(0, p);
        e.commit_update();
        let mut step = default_step();
        step.fx1 = fx(ST_FX_SLICE, 2.0);
        step.note = 60;
        e.preview_step(0, step, &[], 0.03);
        let mut audio = Audio::new(4000);
        render(&mut e, &mut audio);
        assert!(audio.energy_in(100, 500) > 0.03 && audio.energy_in(3000, 4000) == 0.0);
    }
    assert_zero_render_allocations();
}

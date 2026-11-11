//! Port of the former C++ test program `engine_tests.cpp`, part 1 of 5: tone and release, chords/ratchets/microtiming,
//! song swing and seek, probability, automation, sample modes, send buses, sample replacement
//! under a concurrent render, zones and slices, mono/legato/choke/mute, effects-only cells, the
//! long maximum-voice render, pitch bend/metronome/locks and the sampler/MIDI regressions.
//!
//! One `#[test]` per C++ suite (`testFooBar` is `test_foo_bar`), calling the engine's safe Rust
//! API through `engine_support_a::Engine`. Every C++ `assert` is one `assert!` here, with the
//! same inputs, expected values and tolerances; every test ends with the C++ program's final
//! "zero render-thread allocations" check.
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop, clippy::manual_range_contains)]

mod engine_support_a;

use engine_support_a::core_helpers::*;
use engine_support_a::tone::*;
use engine_support_a::{assert_zero_render_allocations, Engine, SetOnDrop};
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use tracker_engine::engine::*;

#[test]
fn test_tone_and_release() {
    let mut e = Engine::new();
    configure(&mut e, TONE);
    e.note_on(0, 0, 69, 100);
    let mut a = Audio::new(48000);
    render(&mut e, &mut a);
    assert!(a.energy() > 0.001);
    let mut crossings = 0i32;
    for i in 1..48000 {
        if a.l[i - 1] <= 0.0 && a.l[i] > 0.0 {
            crossings += 1;
        }
    }
    assert!((crossings - 440).abs() <= 1);
    e.note_off(0, 69);
    let mut tail = Audio::new(4096);
    render(&mut e, &mut tail);
    assert!(tail.energy_in(2048, 4096) == 0.0);
    assert_zero_render_allocations();
}

#[test]
fn test_chord_ratchet_microtiming() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_MIDI);
    e.begin_update();
    e.set_pattern(0, 4);
    let mut s = default_step();
    s.gate = 0.5;
    s.fx1 = fx(ST_FX_RATCHET, 2.0);
    s.fx2 = fx(ST_FX_MICROTIMING, 0.2);
    let chord = [0, 4, 7];
    e.set_step(0, 0, 0, s, &chord);
    e.commit_update();
    e.play(0);
    let ev = note_events(render_events(&mut e, 6000));
    let (mut on, mut off): (Vec<u64>, Vec<u64>) = (Vec::new(), Vec::new());
    for v in &ev {
        if v.e.type_ == ST_EVENT_NOTE_ON {
            on.push(v.frame);
        }
        if v.e.type_ == ST_EVENT_NOTE_OFF {
            off.push(v.frame);
        }
    }
    assert!(on.len() == 6 && off.len() == 6);
    for i in 0..3 {
        assert!((on[i] as i64 - 1200).abs() <= 1);
        assert!((on[i + 3] as i64 - 3600).abs() <= 1);
        assert!((off[i] as i64 - 2400).abs() <= 1);
        assert!((off[i + 3] as i64 - 4800).abs() <= 1);
    }
    assert!(ev[0].e.note == 60 && ev[1].e.note == 64 && ev[2].e.note == 67);
    assert_zero_render_allocations();
}

#[test]
fn test_song_swing_and_seek() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_AU);
    e.begin_update();
    e.set_timing(120.0, 4, 0.25, 0.7);
    e.set_pattern(0, 3);
    e.set_pattern(1, 2);
    let mut s = default_step();
    e.set_step(0, 0, 0, s, &[]);
    s.note = 72;
    e.set_step(1, 0, 0, s, &[]);
    let seq = [0, 1];
    e.set_sequence(&seq, 0, 0);
    e.commit_update();
    assert!((e.duration(1) - 0.65625).abs() < 1e-9);
    e.play(1);
    let ev = render_events(&mut e, 32000);
    let mut on: Vec<Timed> = Vec::new();
    for v in &ev {
        if v.e.type_ == ST_EVENT_NOTE_ON {
            on.push(*v);
        }
    }
    assert!(on.len() == 2);
    assert!(on[0].frame == 0 && on[1].frame == 19500);
    assert!(e.status().playing == 0);
    e.seek(1, 0, 1);
    e.play(1);
    let ev = note_events(render_events(&mut e, 64));
    assert!(!ev.is_empty());
    assert!(ev[0].e.type_ == ST_EVENT_NOTE_ON && ev[0].e.note == 72 && ev[0].frame == 0);
    assert_zero_render_allocations();
}

#[test]
fn test_probability() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_AU);
    e.begin_update();
    e.set_pattern(0, 2);
    let mut s = default_step();
    s.fx1 = fx(ST_FX_PROBABILITY, 0.0);
    e.set_step(0, 0, 0, s, &[]);
    s.fx1.value = 1.0;
    e.set_step(0, 0, 1, s, &[]);
    e.commit_update();
    e.play(0);
    let ev = render_events(&mut e, 12000);
    let mut n = 0;
    for v in &ev {
        if v.e.type_ == ST_EVENT_NOTE_ON {
            n += 1;
            assert!(v.frame == 6000);
        }
    }
    assert!(n == 1);
    assert_zero_render_allocations();
}

#[test]
fn test_automation() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_SAMPLE);
    let sample = vec![0.2f32; 2048];
    e.set_sample(0, &sample, 48000.0);
    e.begin_update();
    let mut i = default_instrument();
    i.type_ = ST_INSTRUMENT_SAMPLE;
    i.mode = ST_MODE_FORWARD;
    i.attack = 0.0;
    i.decay = 0.0;
    i.sustain = 1.0;
    i.volume = 0.5;
    e.set_instrument(0, i);
    e.add_automation_point(0, 0, ST_TARGET_VOLUME, 0, 0, 0, 0.0, 0.0);
    e.add_automation_point(0, 0, ST_TARGET_VOLUME, 0, 0, 0, 1.0, 1.0);
    e.add_automation_point(0, 0, ST_TARGET_PLUGIN, -1, 1234, 0, 0.0, 0.2);
    e.add_automation_point(0, 0, ST_TARGET_PLUGIN, -1, 1234, 0, 1.0, 0.8);
    e.commit_update();
    e.note_on(0, 0, 60, 127);
    e.play(0);
    e.note_on(0, 0, 60, 127);
    let mut a = Audio::new(6000);
    render(&mut e, &mut a);
    assert!(a.energy_in(4000, 5000) > a.energy_in(1000, 2000) * 6.0);
    let mut count = 0;
    let (mut first, mut last) = (-1.0f64, -1.0f64);
    while let Some(event) = e.pop_event() {
        if event.type_ == ST_EVENT_PARAMETER && event.target == ST_TARGET_PLUGIN {
            assert!(event.slot == -1 && event.address == 1234);
            if first < 0.0 {
                first = event.value;
            }
            last = event.value;
            count += 1;
        }
    }
    assert!(count == 188);
    assert!((first - 0.2).abs() < 1e-6 && last > 0.79);
    assert_zero_render_allocations();
}

#[test]
fn test_sample_modes() {
    for mode in 0..=ST_MODE_GRANULAR {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let mut sample = vec![0.0f32; 2048];
        for i in 0..sample.len() {
            sample[i] = (2.0 * PI * i as f64 / 64.0).sin() as f32 * 0.3;
        }
        assert!(e.set_sample(0, &sample, 48000.0));
        e.begin_update();
        let mut inst = default_instrument();
        inst.type_ = ST_INSTRUMENT_SAMPLE;
        inst.mode = mode;
        inst.attack = 0.0;
        inst.decay = 0.0;
        inst.sustain = 1.0;
        inst.release = 0.005;
        inst.slice_count = 4;
        e.set_instrument(0, inst);
        e.commit_update();
        e.note_on(0, 0, 60, 127);
        let mut a = Audio::new(8192);
        render(&mut e, &mut a);
        assert!(a.energy_in(0, 512) > 0.00001);
        if mode == ST_MODE_ONESHOT || mode == ST_MODE_SLICE {
            assert!(a.energy_in(4096, 8192) == 0.0);
        } else {
            // reverse is a backward loop
            assert!(a.energy_in(4096, 8192) > 0.00001);
        }
        e.note_off(0, 60);
        let mut b = Audio::new(1024);
        render(&mut e, &mut b);
        assert!(b.energy_in(512, 1024) == 0.0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_send_buses() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_SAMPLE);
    let mut sample = vec![0.0f32; 32];
    sample[0] = 0.5;
    e.set_sample(0, &sample, 48000.0);
    e.begin_update();
    let mut track = default_track();
    track.delay_send = 1.0;
    e.set_track(0, track);
    e.set_effects(0.01, 0.0, 1.0, 0.7, 0.4, 0.0);
    e.commit_update();
    e.note_on(0, 0, 60, 127);
    let (mut dry, mut wet) = (Audio::new(1024), Audio::new(1024));
    bus_render(&mut e, 0, 0, &mut dry);
    let before = e.status();
    bus_render(&mut e, 16, 0, &mut wet);
    let after = e.status();
    assert!(before.seconds == after.seconds);
    assert!(dry.energy_in(0, 32) > 0.00001);
    assert!(dry.energy_in(64, 1024) == 0.0);
    assert!(wet.energy_in(0, 470) == 0.0);
    assert!(wet.energy_in(480, 512) > 0.00001);
    assert_zero_render_allocations();
}

#[test]
fn test_sample_replacement_and_concurrency() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_SAMPLE);
    let mut sample = vec![0.25f32; 4096];
    e.set_sample(0, &sample, 48000.0);
    e.note_on(0, 0, 60, 127);
    let mut initial = Audio::new(64);
    render(&mut e, &mut initial);
    let done = AtomicBool::new(false);
    {
        // The render half goes to the audio thread; this thread stays the control thread.
        let (mut control, mut audio_half, _) = e.split();
        let done = &done;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let mut a = Audio::new(128);
                while !done.load(Ordering::SeqCst) {
                    audio_half.render_into(&mut a.l, &mut a.r);
                }
            });
            let _finish = SetOnDrop(done);
            for n in 0..100 {
                control.begin_update();
                let mut i = default_instrument();
                i.type_ = ST_INSTRUMENT_SAMPLE;
                i.mode = n % 7;
                control.set_instrument(0, i);
                sample[0] = (n % 3) as f32 * 0.1;
                control.set_sample(0, &sample, 48000.0);
                control.commit_update();
                control.note_on(0, 0, 60 + n % 12, 80);
                if n % 8 == 0 {
                    control.note_off(0, -1);
                }
            }
        });
    }
    e.panic();
    let mut b = Audio::new(2048);
    render(&mut e, &mut b);
    e.collect();
    assert!(b.energy() == 0.0);
    assert_zero_render_allocations();
}

#[test]
fn test_zones_slices_and_clearing() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_SAMPLE);
    let (quiet, loud) = (vec![0.1f32; 4096], vec![0.5f32; 4096]);
    e.begin_update();
    assert!(e.add_sample_zone(0, &quiet, 48000.0, 0, 127, 1, 63, 60, 0.0, 1.0));
    assert!(e.add_sample_zone(0, &loud, 48000.0, 0, 127, 64, 127, 60, 0.0, 1.0));
    e.commit_update();
    e.note_on(0, 0, 60, 60);
    let mut a = Audio::new(1024);
    render(&mut e, &mut a);
    e.panic();
    e.note_on(0, 0, 60, 120);
    let mut b = Audio::new(1024);
    render(&mut e, &mut b);
    assert!(b.energy() > a.energy() * 50.0);
    e.panic();
    e.begin_update();
    e.clear_zones(0);
    let mut p = default_instrument();
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.mode = ST_MODE_SLICE;
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    e.set_instrument(0, p);
    let mut sample = vec![0.1f32; 4096];
    sample[1024..].fill(0.5);
    e.set_sample(0, &sample, 48000.0);
    let slices = [0.0f32, 0.25, 0.9];
    e.set_slice_points(0, &slices);
    e.commit_update();
    e.note_on(0, 0, 61, 127);
    let mut c = Audio::new(512);
    render(&mut e, &mut c);
    assert!(c.energy() > 0.02);
    assert!(e.clear_sample(0));
    e.panic();
    e.note_on(0, 0, 60, 127);
    let mut d = Audio::new(512);
    render(&mut e, &mut d);
    assert!(d.energy() == 0.0);
    assert_zero_render_allocations();
}

#[test]
fn test_mono_legato_choke_and_mute() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_MIDI);
    e.begin_update();
    let mut p = default_instrument();
    p.type_ = ST_INSTRUMENT_MIDI;
    p.voice_mode = 2;
    e.set_instrument(0, p);
    e.commit_update();
    e.note_on(0, 0, 60, 100);
    let ev = note_events(render_events(&mut e, 128));
    assert!(ev.len() == 1 && ev[0].e.type_ == ST_EVENT_NOTE_ON && ev[0].frame == 0);
    e.note_on(0, 0, 64, 100);
    // legato overlaps: the new note-on precedes the old note-off
    let ev = note_events(render_events(&mut e, 128));
    assert!(
        ev.len() == 2
            && ev[0].e.type_ == ST_EVENT_NOTE_ON
            && ev[0].e.note == 64
            && ev[1].e.type_ == ST_EVENT_NOTE_OFF
            && ev[1].e.note == 60
            && ev[0].frame == 0
            && ev[1].frame == 0
    );
    e.note_on(0, 0, 64, 90);
    // the same pitch cannot overlap: off first
    let ev = note_events(render_events(&mut e, 128));
    assert!(ev.len() == 2 && ev[0].e.type_ == ST_EVENT_NOTE_OFF && ev[1].e.type_ == ST_EVENT_NOTE_ON && ev[1].e.note == 64);
    e.note_off(0, 64);
    let ev = note_events(render_events(&mut e, 128));
    assert!(ev.len() == 1 && ev[0].e.type_ == ST_EVENT_NOTE_OFF && ev[0].e.note == 64);
    e.panic();
    render_events(&mut e, 1);
    e.begin_update();
    p.voice_mode = 0;
    p.choke_group = 2;
    e.set_instrument(0, p);
    e.set_instrument(1, p);
    e.commit_update();
    e.note_on(0, 0, 60, 100);
    e.note_on(1, 1, 65, 100);
    let ev = note_events(render_events(&mut e, 128));
    assert!(ev.len() == 3 && ev[1].e.type_ == ST_EVENT_NOTE_OFF && ev[2].e.instrument == 1);
    e.panic();
    render_events(&mut e, 1);
    e.begin_update();
    let mut t = default_track();
    t.mute = 1;
    e.set_track(0, t);
    e.commit_update();
    e.note_on(0, 0, 60, 100);
    assert!(note_events(render_events(&mut e, 128)).is_empty());
    assert_zero_render_allocations();
}

#[test]
fn test_effects_only_cell() {
    let mut e = Engine::new();
    configure(&mut e, ST_INSTRUMENT_AU);
    e.begin_update();
    let mut s = default_step();
    s.note = 60;
    s.gate = 4.0;
    e.set_step(0, 0, 0, s, &[]);
    s.note = -3;
    s.fx1 = fx(ST_FX_VOLUME, 0.2);
    e.set_step(0, 0, 1, s, &[]);
    e.commit_update();
    e.play(0);
    let ev = render_events(&mut e, 12000);
    let (mut on, mut off, mut fx_count) = (0, 0, 0);
    for v in &ev {
        on += (v.e.type_ == ST_EVENT_NOTE_ON) as i32;
        off += (v.e.type_ == ST_EVENT_NOTE_OFF) as i32;
        if v.e.type_ == ST_EVENT_PARAMETER && v.e.target == ST_TARGET_VOLUME && v.e.track == 0 && v.frame == 6000 {
            fx_count += 1;
        }
    }
    assert!(on == 1 && off == 0 && fx_count == 1);
    assert_zero_render_allocations();
}

#[test]
fn test_long_render_maximum_voices() {
    for voice_count in [16, 32, 64, 256] {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.set_track_count(16);
        let mut p = test_tone_instrument(TEST_SAW);
        load_test_tone(&mut e, 0, TEST_SAW);
        p.volume = 0.01;
        p.attack = 0.0;
        p.sustain = 1.0;
        e.set_instrument(0, p);
        e.commit_update();
        for n in 0..voice_count {
            e.note_on(n % 16, 0, 36 + n / 16, 80);
        }
        let mut a = Audio::new(256);
        let start = std::time::Instant::now();
        for _ in 0..375 {
            render(&mut e, &mut a);
            assert!(a.energy() >= 0.0);
            for &x in &a.l {
                assert!(x >= -1.0 && x <= 1.0);
            }
        }
        let seconds = start.elapsed().as_secs_f64();
        println!("{voice_count:3} voices: 2 seconds stereo DSP, wall {seconds:.3}s ({:.2}x).", 2.0 / seconds);
        e.panic();
        render(&mut e, &mut a);
        assert!(a.energy() == 0.0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_pitch_bend_metronome_and_locks() {
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.note_on(0, 0, 69, 100);
        e.pitch_bend(0, 12.0);
        let mut a = Audio::new(48000);
        render(&mut e, &mut a);
        let mut crossings = 0i32;
        for i in 1..48000 {
            if a.l[i - 1] <= 0.0 && a.l[i] > 0.0 {
                crossings += 1;
            }
        }
        assert!((crossings - 880).abs() <= 1);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.set_metronome(1, 3, 4, 0.3);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(48000);
        render(&mut e, &mut a);
        assert!(a.energy_in(1, 512) > 0.00001 && a.energy_in(10000, 20000) == 0.0 && a.energy_in(24001, 24512) > 0.00001);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let sample = vec![0.2f32; 2048];
        e.set_sample(0, &sample, 48000.0);
        e.begin_update();
        let mut i = default_instrument();
        i.type_ = ST_INSTRUMENT_SAMPLE;
        i.mode = ST_MODE_FORWARD;
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        e.set_instrument(0, i);
        let mut step = default_step();
        step.gate = 4.0;
        step.fx1 = fx(ST_FX_VOLUME, 0.2);
        e.set_step(0, 0, 0, step, &[]);
        e.add_automation_point(0, 0, ST_TARGET_VOLUME, 0, 0, 0, 0.0, 0.9);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(12000);
        render(&mut e, &mut a);
        assert!(a.energy_in(9000, 10000) > a.energy_in(4000, 5000) * 15.0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_sampler_and_midi_regressions() {
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let sample = [0.1f32, 0.2, 0.3, 0.4];
        e.set_sample(0, &sample, 48000.0);
        e.begin_update();
        let mut i = default_instrument();
        i.type_ = ST_INSTRUMENT_SAMPLE;
        i.mode = ST_MODE_FORWARD;
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        e.set_instrument(0, i);
        let mut step = default_step();
        step.fx1 = fx(ST_FX_REVERSE, 1.0);
        step.gate = 4.0;
        e.set_step(0, 0, 0, step, &[]);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(2048);
        render(&mut e, &mut a);
        for n in 1000..1200 {
            assert!(((a.l[n] - a.l[n + 4]).abs() as f64) < 1e-5);
        }
        assert!((a.l[1000] - a.l[1003]).abs() as f64 > 1e-4);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let sample = [0.3f32];
        e.set_sample(0, &sample, 48000.0);
        e.begin_update();
        let mut i = default_instrument();
        i.type_ = ST_INSTRUMENT_SAMPLE;
        i.mode = ST_MODE_PINGPONG;
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        e.set_instrument(0, i);
        e.commit_update();
        e.note_on(0, 0, 60, 127);
        let mut a = Audio::new(2048);
        render(&mut e, &mut a);
        assert!(a.energy_in(1024, 2048) > 0.001);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let mut sample = vec![0.2f32; 4096];
        sample[2048..].fill(0.6);
        e.set_sample(0, &sample, 48000.0);
        e.begin_update();
        let mut i = default_instrument();
        i.type_ = ST_INSTRUMENT_SAMPLE;
        i.mode = ST_MODE_SLICE;
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        i.slice_count = 16;
        e.set_instrument(0, i);
        let mut step = default_step();
        step.note = 61;
        e.set_step(0, 0, 0, step, &[]);
        e.add_automation_point(0, 0, ST_TARGET_SLICE_COUNT, 0, 0, 0, 0.0, 2.0);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(512);
        render(&mut e, &mut a);
        assert!(a.energy_in(256, 512) > 0.03);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let sample = vec![0.3f32; 4096];
        e.set_sample(0, &sample, 48000.0);
        e.begin_update();
        let step = default_step();
        e.set_step(0, 0, 0, step, &[]);
        e.add_automation_point(0, 0, ST_TARGET_START, 0, 0, 0, 0.0, 0.25);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(512);
        render(&mut e, &mut a);
        assert!(a.energy() > 0.001);
        let mut tail = Audio::new(5000);
        render(&mut e, &mut tail);
        assert!(tail.energy_in(4096, 5000) == 0.0);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.note_on(0, 0, 60, 100);
        render_events(&mut e, 64);
        e.begin_update();
        let mut track = default_track();
        track.mute = 1;
        e.set_track(0, track);
        e.commit_update();
        let events = note_events(render_events(&mut e, 64));
        assert!(events.len() == 1 && events[0].e.type_ == ST_EVENT_NOTE_OFF && events[0].frame == 0);
    }
    assert_zero_render_allocations();
}

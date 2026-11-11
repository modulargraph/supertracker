//! Port of the former C++ test program `engine_tests.cpp`, part 4 of 5: raw host mixing and its
//! note mask, and host automation (destination map, timing, bounds and protected note-offs).
//!
//! One `#[test]` per C++ suite (`testFooBar` is `test_foo_bar`), calling the engine's safe Rust
//! API through `engine_support_a::Engine`. Every C++ `assert` is one `assert!` here, with the
//! same inputs, expected values and tolerances; every test ends with the C++ program's final
//! "zero render-thread allocations" check.
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod engine_support_a;

use engine_support_a::core_helpers::*;
use engine_support_a::tone::*;
use engine_support_a::{assert_zero_render_allocations, Engine};
use tracker_engine::engine::*;

/// C++ `configureHostMixVoice(e,raw)`: a filtered, driven saw voice on track 0. With `raw` the
/// track is muted, at volume 0, panned and sent to both effects, track 1 is soloed and the
/// master is 0: all of which raw host mixing must ignore.
fn configure_host_mix_voice(e: &mut Engine, raw: bool) {
    configure(e, TONE);
    e.begin_update();
    e.set_host_mixing(raw as i32, 0xffff);
    e.set_timing(120.0, 4, 0.0, if raw { 0.0 } else { 1.0 });
    let mut track = default_track();
    track.volume = if raw { 0.0 } else { 1.0 };
    track.pan = if raw { 1.0 } else { 0.0 };
    track.mute = raw as i32;
    track.reverb_send = if raw { 1.0 } else { 0.0 };
    track.delay_send = track.reverb_send;
    e.set_track(0, track);
    track = default_track();
    track.solo = raw as i32;
    e.set_track(1, track);
    let mut p = modulation_instrument();
    load_test_tone(e, 0, TEST_SAW);
    p.pan = -0.3;
    p.drive = 0.1;
    p.cutoff = 1200.0;
    p.filter_env_amount = 24.0;
    p.filter_decay = 0.05;
    p.lfo_rate = 2.0;
    p.lfo_depth = 0.1;
    e.set_instrument(0, p);
    e.commit_update();
    e.note_on(0, 0, 69, 100);
}

#[test]
fn test_raw_host_mixing() {
    // Host strips receive full instrument DSP despite track fader/mute/solo/master.
    {
        let (mut raw, mut reference) = (Engine::new(), Engine::new());
        configure_host_mix_voice(&mut raw, true);
        configure_host_mix_voice(&mut reference, false);
        let (mut actual, mut expected, mut returns) = (Audio::new(8192), Audio::new(8192), Audio::new(8192));
        bus_render_many(&mut raw, &[0, 16], 0, &mut [&mut actual, &mut returns]);
        bus_render_many(&mut reference, &[0], 0, &mut [&mut expected]);
        assert!(actual.l == expected.l && actual.r == expected.r && actual.energy() > 0.001 && returns.energy() == 0.0);
    }
    // Explicitly disabled host mixing ignores its mask and keeps legacy PCM exact.
    {
        let (mut a, mut b) = (Engine::new(), Engine::new());
        configure(&mut a, TONE);
        configure(&mut b, TONE);
        b.begin_update();
        b.set_host_mixing(0, 0);
        b.commit_update();
        a.note_on(0, 0, 69, 100);
        b.note_on(0, 0, 69, 100);
        let (mut first, mut second) = (Audio::new(4096), Audio::new(4096));
        render(&mut a, &mut first);
        render(&mut b, &mut second);
        assert!(first.l == second.l && first.r == second.r);
    }
    // A raw computational exclusion does not freeze a native voice's envelopes/LFO.
    {
        let (mut raw, mut reference) = (Engine::new(), Engine::new());
        configure_host_mix_voice(&mut raw, true);
        configure_host_mix_voice(&mut reference, true);
        let (mut a, mut b) = (Audio::new(512), Audio::new(512));
        render(&mut raw, &mut a);
        render(&mut reference, &mut b);
        raw.begin_update();
        raw.set_host_mixing(1, 0);
        raw.commit_update();
        render(&mut raw, &mut a);
        render(&mut reference, &mut b);
        assert!(a.energy() == 0.0);
        raw.begin_update();
        raw.set_host_mixing(1, 1);
        raw.commit_update();
        render(&mut raw, &mut a);
        render(&mut reference, &mut b);
        assert!(a.l == b.l && a.r == b.r);
    }
    // Unscaled metronome lives only on bus16, even when the raw master is zero.
    {
        let (mut raw, mut reference) = (Engine::new(), Engine::new());
        for (e, is_raw) in [(&mut raw, true), (&mut reference, false)] {
            configure(e, TONE);
            e.begin_update();
            e.set_timing(120.0, 4, 0.0, if is_raw { 0.0 } else { 1.0 });
            e.set_host_mixing(is_raw as i32, 0);
            e.set_metronome(1, 4, 4, 0.3);
            e.commit_update();
            e.play(0);
        }
        let (mut a, mut b) = (Audio::new(512), Audio::new(512));
        bus_render(&mut raw, 16, 0, &mut a);
        bus_render(&mut reference, 16, 0, &mut b);
        assert!(a.l == b.l && a.r == b.r && a.energy() > 0.001);
    }
    // Switching away and back cannot resurrect an old legacy delay tail.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_SAMPLE);
        let sample = [0.5f32];
        e.begin_update();
        e.set_sample(0, &sample, 48000.0);
        let mut track = default_track();
        track.delay_send = 1.0;
        e.set_track(0, track);
        e.set_effects(0.01, 0.8, 1.0, 0.7, 0.4, 0.0);
        e.commit_update();
        e.note_on(0, 0, 60, 127);
        let mut head = Audio::new(512);
        render(&mut e, &mut head);
        assert!(head.energy_in(480, 512) > 0.00001);
        e.begin_update();
        e.set_host_mixing(1, 0xffff);
        e.commit_update();
        let mut quiet = Audio::new(128);
        render(&mut e, &mut quiet);
        assert!(quiet.energy() == 0.0);
        e.begin_update();
        e.set_host_mixing(0, 0);
        e.commit_update();
        let mut tail = Audio::new(2048);
        render(&mut e, &mut tail);
        assert!(tail.energy() == 0.0);
    }
    // Timeline clips retain gain/source timing but bypass track mute/master scaling.
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let sample = vec![0.2f32; 48000];
        e.begin_update();
        e.set_audio_asset(0, &sample, 48000.0);
        e.add_audio_clip(0, 0, 0.0, 2.0, 0.0, 0.5, 0.0, 0.0, 0);
        let mut track = default_track();
        track.volume = 0.0;
        track.mute = 1;
        e.set_track(0, track);
        e.set_timing(120.0, 4, 0.0, 0.0);
        e.set_host_mixing(1, 1);
        e.commit_update();
        e.play(1);
        let mut a = Audio::new(512);
        render(&mut e, &mut a);
        for &value in &a.l {
            assert!(((value - 0.1f32).abs() as f64) < 1e-7);
        }
        e.begin_update();
        e.set_host_mixing(1, 0);
        e.commit_update();
        render(&mut e, &mut a);
        assert!(a.energy() == 0.0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_host_mix_note_mask() {
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_AU);
        e.begin_update();
        e.set_host_mixing(1, 1);
        let mut track = default_track();
        track.mute = 1;
        e.set_track(0, track);
        track = default_track();
        track.solo = 1;
        e.set_track(1, track);
        e.commit_update();
        e.note_on(0, 0, 60, 100);
        let mut notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 1 && notes[0].e.type_ == ST_EVENT_NOTE_ON);
        e.begin_update();
        e.set_host_mixing(1, 0);
        e.commit_update();
        notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 1 && notes[0].e.type_ == ST_EVENT_NOTE_OFF);
        e.note_on(0, 0, 64, 100);
        assert!(note_events(render_events(&mut e, 1)).is_empty());
        e.begin_update();
        e.set_host_mixing(1, 1);
        e.commit_update();
        e.note_on(0, 0, 64, 100);
        notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 1 && notes[0].e.type_ == ST_EVENT_NOTE_ON);
    }
    // Hardware output is audible outside the host graph, so mute/solo still gates it.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.begin_update();
        e.set_host_mixing(1, 0xffff);
        let mut track = default_track();
        track.mute = 1;
        e.set_track(0, track);
        e.commit_update();
        e.note_on(0, 0, 60, 100);
        assert!(note_events(render_events(&mut e, 1)).is_empty());
        e.begin_update();
        track.mute = 0;
        e.set_track(0, track);
        track.solo = 1;
        e.set_track(1, track);
        e.commit_update();
        e.note_on(0, 0, 60, 100);
        assert!(note_events(render_events(&mut e, 1)).is_empty());
        e.begin_update();
        track.solo = 0;
        e.set_track(1, track);
        e.set_host_mixing(1, 0);
        e.commit_update();
        e.note_on(0, 0, 60, 100);
        let mut notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 1 && notes[0].e.type_ == ST_EVENT_NOTE_ON);
        e.begin_update();
        e.set_host_mixing(1, 1);
        e.commit_update();
        assert!(note_events(render_events(&mut e, 1)).is_empty());
        e.begin_update();
        track.mute = 1;
        e.set_track(0, track);
        e.commit_update();
        notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 1 && notes[0].e.type_ == ST_EVENT_NOTE_OFF);
    }
    assert_zero_render_allocations();
}

/// Host events carry the destination map of the project that emitted them, so a host whose
/// destination table changed before this project was republished can drop them; republishing
/// re-sends every host lane (also an unchanged step lane).
#[test]
fn test_host_automation_map() {
    let mut e = Engine::new();
    configure(&mut e, TONE);
    e.begin_update();
    e.set_host_automation_map(0x51);
    e.add_host_automation_point(0, 3, 7, 1, 0.0, 0.25);
    e.add_automation_point(0, 0, ST_TARGET_VOLUME, 0, 0, 1, 0.0, 0.5);
    e.commit_update();
    e.play(0);
    let events = render_events(&mut e, 64);
    let mut host = host_events(events.clone());
    assert!(host.len() == 1 && host[0].e.host_map == 0x51 && host[0].e.target == 3);
    for event in &events {
        if event.e.type_ != ST_EVENT_HOST_PARAMETER {
            assert!(event.e.host_map == 0);
        }
    }
    assert!(host_events(render_events(&mut e, 64)).is_empty());
    e.begin_update();
    e.set_host_automation_map(0x52);
    e.commit_update();
    host = host_events(render_events(&mut e, 64));
    assert!(host.len() == 1 && host[0].e.host_map == 0x52 && host[0].e.target == 3 && host[0].e.value == 0.25);
    e.begin_update();
    e.clear_project();
    e.set_track_count(2);
    e.add_host_automation_point(0, 3, 7, 1, 0.0, 0.5);
    e.commit_update();
    host = host_events(render_events(&mut e, 64));
    assert!(host.len() == 1 && host[0].e.host_map == 0);
    assert_zero_render_allocations();
}

#[test]
fn test_host_automation_timing() {
    const ADDRESS: u64 = 0xfedcba9876543210;
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.set_host_mixing(1, 0);
        e.add_host_automation_point(0, 511, ADDRESS, 1, 1.0, 13.75);
        e.add_host_automation_point(0, 511, ADDRESS, 1, 0.0, -12.25);
        e.add_host_automation_point(0, 511, ADDRESS, 1, 1.0625, -3.125);
        e.add_host_automation_point(0, ST_MAX_HOST_DESTINATIONS, 1, 1, 0.0, 99.0);
        e.add_host_automation_point(0, -1, 1, 1, 0.0, 99.0);
        e.add_host_automation_point(0, 1, 1, 1, f64::NAN, 99.0);
        e.add_host_automation_point(0, 1, 1, 1, 0.0, f64::INFINITY);
        e.commit_update();
        e.play(0);
        let mut events = host_events(render_events_block(&mut e, 7000, 127));
        assert!(events.len() == 3);
        for event in &events {
            assert!(event.e.target == 511 && event.e.track == -1 && event.e.address == ADDRESS);
        }
        assert!(
            events[0].frame == 0
                && events[0].e.value == -12.25
                && events[1].frame == 6000
                && events[1].e.value == 13.75
                && events[2].frame == 6375
                && events[2].e.value == -3.125
        );
        assert!(host_events(render_events(&mut e, 128)).is_empty());
        e.seek(0, 1, 0);
        events = host_events(render_events(&mut e, 1));
        assert!(events.len() == 1 && events[0].frame == 0 && events[0].e.value == 13.75);
        e.refresh_automation();
        events = host_events(render_events(&mut e, 1));
        assert!(events.len() == 1 && events[0].frame == 0);
        e.begin_update();
        e.clear_automation(0);
        e.commit_update();
        assert!(host_events(render_events(&mut e, 1)).is_empty());
    }
    // Raw doubles retain precision and linear timing; host destinations do not act
    // like identically numbered native targets or native row locks.
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        let mut step = default_step();
        step.note = -3;
        step.fx1 = fx(ST_FX_VOLUME, 0.0);
        e.set_step(0, 0, 0, step, &[]);
        const START: f64 = 123456789.12345679;
        e.add_host_automation_point(0, 0, ADDRESS, 0, 0.0, START);
        e.add_host_automation_point(0, 0, ADDRESS, 0, 1.0, START + 10.0);
        e.commit_update();
        e.play(0);
        let events = host_events(render_events_block(&mut e, 100, 17));
        assert!(events.len() == 4 && events[0].e.value == START);
        for n in 0..events.len() {
            assert!(events[n].frame == n as u64 * 32);
            assert!((events[n].e.value - (START + 10.0 * (n * 32) as f64 / 6000.0)).abs() < 1e-7);
        }
    }
    {
        let (mut a, mut b) = (Engine::new(), Engine::new());
        configure(&mut a, TONE);
        configure(&mut b, TONE);
        b.begin_update();
        b.add_host_automation_point(0, 0, ADDRESS, 1, 0.0, 0.0);
        b.add_host_automation_point(0, 1, ADDRESS, 1, 0.0, 1.0);
        b.commit_update();
        for e in [&mut a, &mut b] {
            e.play(0);
            e.note_on(0, 0, 69, 100);
        }
        let (mut x, mut y) = (Audio::new(1024), Audio::new(1024));
        render(&mut a, &mut x);
        render(&mut b, &mut y);
        assert!(x.l == y.l && x.r == y.r);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.add_host_automation_point(0, 0, 0, 0, 0.0, -1e308);
        e.add_host_automation_point(0, 0, 0, 0, 1.0, 1e308);
        e.commit_update();
        e.play(0);
        let events = host_events(render_events(&mut e, 6100));
        for event in &events {
            assert!(event.e.value.is_finite());
        }
    }
    assert_zero_render_allocations();
}

#[test]
fn test_host_automation_bounds_and_note_offs() {
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        for n in 0..300 {
            e.add_host_automation_point(0, n, n as u64, 1, 0.0, n as f64);
        }
        e.commit_update();
        e.play(0);
        let events = host_events(render_events(&mut e, 1));
        assert!(events.len() == 256 && events.last().unwrap().e.target == 255);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_AU);
        e.begin_update();
        e.set_host_mixing(1, 1);
        let mut step = default_step();
        step.gate = 10.0;
        e.set_step(0, 0, 0, step, &[]);
        for lane in 0..40 {
            e.add_host_automation_point(0, lane, lane as u64, 0, 0.0, 0.0);
            e.add_host_automation_point(0, lane, lane as u64, 0, 64.0, 1.0);
        }
        e.commit_update();
        e.play(0);
        let mut pull = Audio::new(65536);
        bus_render_many(&mut e, &[0], 0, &mut [&mut pull]);
        let (mut on, mut off, mut controls) = (0, 0, 0);
        while let Some(event) = e.pop_event() {
            if event.type_ == ST_EVENT_NOTE_ON {
                on += 1;
                assert!(event.frame_offset == 0);
            }
            if event.type_ == ST_EVENT_NOTE_OFF {
                off += 1;
                // Frame 60000, counted from the start of its block.
                assert!(event.frame_offset == 60000 % 4096);
            }
            controls += (event.type_ == ST_EVENT_HOST_PARAMETER) as i32;
        }
        assert!(on == 1 && off == 1 && controls > 0 && e.status().dropped_events > 0);
    }
    assert_zero_render_allocations();
}


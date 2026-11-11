//! Engine core suites: what a host finds on the buses, the gating of external notes, and host
//! automation (destination map, timing, bounds and protected note-offs).
//!
//! One `#[test]` per suite, calling the engine through `engine_support_a::Engine`, which renders
//! it block by block as a host does: the tests read its buses (a plain render gives their sum).
//! Every test ends with the "zero render-thread allocations" check.
// The tests index their buffers in plain loops.
#![allow(clippy::needless_range_loop)]

mod engine_support_a;

use engine_support_a::core_helpers::*;
use engine_support_a::tone::*;
use engine_support_a::{assert_zero_render_allocations, Engine};
use tracker_engine::engine::*;

/// A filtered, driven saw voice on track 0. With `mixed` the track is muted, at volume 0, panned
/// and sent to both effects, and track 1 is soloed: mixer settings, which are the host's to
/// apply and leave the bus as it is.
fn configure_bus_voice(e: &mut Engine, mixed: bool) {
    configure(e, TONE);
    e.begin_update();
    let mut track = default_track();
    track.volume = if mixed { 0.0 } else { 1.0 };
    track.pan = if mixed { 1.0 } else { 0.0 };
    track.mute = mixed as i32;
    track.reverb_send = if mixed { 1.0 } else { 0.0 };
    track.delay_send = track.reverb_send;
    e.set_track(0, track);
    track = default_track();
    track.solo = mixed as i32;
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
fn test_buses_are_raw() {
    // A track's bus holds the full instrument DSP whatever the track's fader, pan, mute, solo
    // and sends say, and nothing of it reaches the return bus.
    {
        let (mut mixed, mut reference) = (Engine::new(), Engine::new());
        configure_bus_voice(&mut mixed, true);
        configure_bus_voice(&mut reference, false);
        let (mut actual, mut expected, mut returns) = (Audio::new(8192), Audio::new(8192), Audio::new(8192));
        bus_render_many(&mut mixed, &[0, 16], 0, &mut [&mut actual, &mut returns]);
        bus_render_many(&mut reference, &[0], 0, &mut [&mut expected]);
        assert!(actual.l == expected.l && actual.r == expected.r && actual.energy() > 0.001 && returns.energy() == 0.0);
    }
    // The metronome is on the return bus only, at its own volume.
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        e.begin_update();
        e.set_metronome(1, 4, 4, 0.3);
        e.commit_update();
        e.play(0);
        e.note_on(0, 0, 69, 100);
        let (mut click, mut track) = (Audio::new(512), Audio::new(512));
        bus_render_many(&mut e, &[16, 0], 0, &mut [&mut click, &mut track]);
        // The accent (1500 Hz) decays from the bar line: its first crest is close to the volume.
        let peak = click.l.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(click.l == click.r && peak > 0.28 && peak <= 0.3);
        // The same frames without the metronome: the track's bus holds the voice alone.
        let mut plain = Engine::new();
        configure(&mut plain, TONE);
        plain.play(0);
        plain.note_on(0, 0, 69, 100);
        let (mut silent, mut expected) = (Audio::new(512), Audio::new(512));
        bus_render_many(&mut plain, &[16, 0], 0, &mut [&mut silent, &mut expected]);
        assert!(silent.energy() == 0.0 && track.l == expected.l && track.r == expected.r && track.energy() > 0.001);
    }
    // A timeline clip is on its track's bus at its own gain, whatever the track's volume and mute.
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
        e.commit_update();
        e.play(1);
        let mut a = Audio::new(512);
        render(&mut e, &mut a);
        for &value in &a.l {
            assert!(((value - 0.1f32).abs() as f64) < 1e-7);
        }
    }
    assert_zero_render_allocations();
}

#[test]
fn test_external_note_gating() {
    // A plug-in instrument sounds in the host's graph: mute and solo do not gate its notes.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_AU);
        e.begin_update();
        let mut track = default_track();
        track.mute = 1;
        e.set_track(0, track);
        track = default_track();
        track.solo = 1;
        e.set_track(1, track);
        e.commit_update();
        e.note_on(0, 0, 60, 100);
        let notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 1 && notes[0].e.type_ == ST_EVENT_NOTE_ON);
    }
    // Hardware output is audible outside the host graph, so mute/solo still gates it.
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.begin_update();
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
        e.commit_update();
        e.note_on(0, 0, 60, 100);
        let mut notes = note_events(render_events(&mut e, 1));
        assert!(notes.len() == 1 && notes[0].e.type_ == ST_EVENT_NOTE_ON);
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
    e.add_automation_point(0, 0, ST_TARGET_VOLUME, 1, 0.0, 0.5);
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
        // The project published again without the lane, as a host publishes: nothing to send.
        e.begin_update();
        e.clear_project();
        e.set_track_count(2);
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


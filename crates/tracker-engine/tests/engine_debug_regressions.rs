//! Port of the former C++ test program `engine_debug_regressions.cpp`: targeted regressions from the 660cfb9 engine
//! audit. Renders and transport restores run as the render thread; every group ends with the C++
//! program's final "zero render allocations" check for its own thread.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations};
use engine_support_b::engine::Engine;
use tracker_engine::engine::*;

/// C++ `require`: fails the group with `message`.
#[track_caller]
fn require(ok: bool, message: &str) {
    assert!(ok, "{message}");
}

/// The C++ `run("zero render allocations", ...)` group, applied to each group's own thread.
fn zero_render_allocations() {
    require(render_allocations() == 0, "Render/restore allocated");
}

fn audio(e: &mut Engine, count: usize) -> Vec<f32> {
    let (mut l, mut r) = (vec![0.0f32; count], vec![0.0f32; count]);
    realtime(|| e.render(&mut l, &mut r));
    l
}

fn setup(e: &mut Engine, rows: i32) {
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0, 1.0);
    let mut track = default_track();
    track.volume = 1.0;
    e.set_track(0, track);
    e.set_pattern(0, rows);
    e.set_sequence(&[0], false, 0);
    let result = e.commit_update_checked();
    require(result.is_ok(), result.as_ref().err().map_or("", |error| error.as_str()));
}

fn print_pcm() -> Vec<f32> {
    (0..60000).map(|n| 0.1f32 + n as f32 * 0.000005f32).collect()
}

fn install_print(e: &mut Engine) {
    let mut pcm = print_pcm();
    e.begin_update();
    require(e.set_audio_asset(0, &pcm, None, 48000.0), "head asset");
    for v in &mut pcm {
        *v = -*v;
    }
    require(e.set_audio_asset(1, &pcm, None, 48000.0), "loop asset");
    e.add_pattern_clip(0, 1, 0, 0, 1.0);
    e.commit_update();
}

/// A pattern print plays from the frame its pass has reached: the head print on the first pass,
/// the loop print once the pattern has wrapped.
fn pattern_print(after_wrap: bool) {
    let mut e = Engine::new();
    setup(&mut e, 4);
    install_print(&mut e);
    e.play(0);
    audio(&mut e, if after_wrap { 25000 } else { 1000 });
    let out = audio(&mut e, 500);
    let pcm = print_pcm();
    let sign: f32 = if after_wrap { -1.0 } else { 1.0 };
    for n in 0..out.len() {
        require(out[n] == sign * pcm[1000 + n], "The print plays at the pass's frame, with the head or loop asset");
    }
    require(e.status().row == 0 && e.status().playing != 0, "The transport is on row 0 and playing");
}

/// The frame after the pattern's last row starts the loop print at its first frame; a stopped
/// transport plays no print.
fn pattern_print_wrap_and_stop() {
    let mut e = Engine::new();
    setup(&mut e, 4);
    install_print(&mut e);
    e.play(0);
    audio(&mut e, 24000);
    let out = audio(&mut e, 64);
    let pcm = print_pcm();
    for n in 0..out.len() {
        require(out[n] == -pcm[n], "A natural wrap selects the loop print from its start");
    }
    e.stop();
    let silent = audio(&mut e, 100);
    require(silent.iter().all(|&x| x == 0.0), "A stopped transport plays no print");
}

fn silent_sample_region() {
    for mode in
        [ST_MODE_ONESHOT, ST_MODE_FORWARD, ST_MODE_REVERSE, ST_MODE_PINGPONG, ST_MODE_GRANULAR, ST_MODE_WAVETABLE]
    {
        let mut e = Engine::new();
        setup(&mut e, 4);
        let mut pcm = vec![0.8f32; 1024];
        pcm[256..768].fill(0.0);
        e.begin_update();
        let mut instrument = default_instrument();
        instrument.type_ = ST_INSTRUMENT_SAMPLE;
        instrument.mode = mode;
        instrument.start = 0.25;
        instrument.end = 0.75;
        instrument.attack = 0.0;
        instrument.decay = 0.0;
        instrument.sustain = 1.0;
        instrument.volume = 1.0;
        instrument.tune = -12.0;
        instrument.grain_position = 0.01;
        e.set_instrument(0, instrument);
        require(e.set_sample(0, &pcm, None, 48000.0), "Silent selection asset");
        e.commit_update();
        e.note_on(0, 0, 60, 127);
        let out = audio(&mut e, 3000);
        let mut peak = 0.0f32;
        for value in &out {
            peak = peak.max(value.abs());
        }
        if peak > 0.0 {
            println!("silent selected region mode={mode} contaminated peak={peak}");
        }
        require(peak == 0.0, "Interpolation must not borrow excluded audio outside a silent playback region");
    }
}

fn silent_slice() {
    let mut e = Engine::new();
    setup(&mut e, 4);
    let mut pcm = vec![0.8f32; 1024];
    pcm[512..].fill(0.0);
    e.begin_update();
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_SAMPLE;
    instrument.mode = ST_MODE_SLICE;
    instrument.slice_count = 2;
    instrument.attack = 0.0;
    instrument.decay = 0.0;
    instrument.sustain = 1.0;
    instrument.volume = 1.0;
    instrument.tune = -12.0;
    e.set_instrument(0, instrument);
    require(e.set_sample(0, &pcm, None, 48000.0), "Silent slice asset");
    e.commit_update();
    e.note_on(0, 0, 61, 127);
    let out = audio(&mut e, 3000);
    require(out.iter().all(|&v| v == 0.0), "Pitched silent slice must not borrow preceding slice audio");
}

/// Transport frames are bpm-relative: a tempo change while playing keeps the row in both modes.
fn tempo_change() {
    for song in [0, 1] {
        let mut e = Engine::new();
        setup(&mut e, 16);
        e.begin_update();
        e.set_sequence(&[0, 0], false, 0);
        e.commit_update();
        e.play(song);
        audio(&mut e, 72100);
        require(e.status().row == 12, "Row 12 before the tempo change");
        e.begin_update();
        e.set_timing(60.0, 4, 0.0, 1.0);
        e.commit_update();
        audio(&mut e, 1);
        let s = e.status();
        if s.row != 12 {
            println!("tempo change song={} row={} expected=12", song, s.row);
        }
        require(s.row == 12 && s.sequence == 0, "Tempo change keeps the playing row");
        audio(&mut e, 3 * 12000);
        require(e.status().row == 15, "Rows then advance at the new tempo");
    }
}

/// MIDI has no ownership per pitch: an overlapping retrigger of the same AU pitch must end the
/// older note first, or its later note-off cuts the newer note short inside the plug-in.
fn external_retrigger() {
    let mut e = Engine::new();
    setup(&mut e, 8);
    e.begin_update();
    let mut instrument = default_instrument();
    instrument.type_ = ST_INSTRUMENT_AU;
    e.set_instrument(0, instrument);
    let mut step = default_step();
    step.instrument = 0;
    step.gate = 4.0;
    e.set_step(0, 0, 0, step, &[]);
    e.set_step(0, 0, 2, step, &[]);
    e.commit_update();
    e.play(0);
    let mut events: Vec<(i32, i64)> = Vec::new();
    let mut at: i64 = 0;
    for _block in 0..60 {
        audio(&mut e, 1000);
        while let Some(event) = e.pop_event() {
            if event.type_ == ST_EVENT_NOTE_ON || event.type_ == ST_EVENT_NOTE_OFF {
                events.push((event.type_, at + event.frame_offset as i64));
            }
        }
        at += 1000;
    }
    require(events.len() >= 4, "Two notes and their note-offs");
    let mut held = 0;
    for event in &events[..4] {
        held += if event.0 == ST_EVENT_NOTE_ON { 1 } else { -1 };
        require(held == 0 || held == 1, "Never two note-ons of one pitch at once");
    }
    require(
        events[1].0 == ST_EVENT_NOTE_OFF
            && events[1].1 == 12000
            && events[2].0 == ST_EVENT_NOTE_ON
            && events[2].1 == 12000,
        "Retrigger ends the older note at the new onset",
    );
    require(events[3].0 == ST_EVENT_NOTE_OFF && events[3].1 == 12000 + 24000, "The newer note keeps its full gate");
}

#[test]
fn pattern_print_head() {
    pattern_print(false);
    zero_render_allocations();
}

#[test]
fn pattern_print_loop() {
    pattern_print(true);
    zero_render_allocations();
}

#[test]
fn pattern_print_at_wrap_and_after_stop() {
    pattern_print_wrap_and_stop();
    zero_render_allocations();
}

#[test]
fn silent_sampler_playback_region() {
    silent_sample_region();
    zero_render_allocations();
}

#[test]
fn silent_pitched_slice() {
    silent_slice();
    zero_render_allocations();
}

#[test]
fn tempo_change_keeps_position() {
    tempo_change();
    zero_render_allocations();
}

#[test]
fn external_pitch_retrigger() {
    external_retrigger();
    zero_render_allocations();
}

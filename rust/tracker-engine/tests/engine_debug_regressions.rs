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

fn setup(e: &mut Engine, mapped: bool, rows: i32) {
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0, 1.0);
    let mut track = default_track();
    track.volume = 1.0;
    e.set_track(0, track);
    e.set_host_mixing(true, 1);
    e.set_pattern(0, rows);
    e.set_sequence(&[0], false, 0);
    if mapped {
        let tempo = [ClockTempoEvent { beat: 0.0, bpm: 120.0 }];
        let meter = [ClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 }];
        let result = e.set_timing_map(&tempo, &meter, 55);
        require(result.is_ok(), result.as_ref().err().map_or("", |error| error.as_str()));
    }
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

fn temporary_song_render(e: &mut Engine) {
    e.stop();
    e.seek(0, 0, 0);
    e.play(1);
    audio(e, 73);
}

fn checkpoint_print(mapped: bool, after_wrap: bool) {
    let mut e = Engine::new();
    setup(&mut e, mapped, 4);
    install_print(&mut e);
    e.play(0);
    audio(&mut e, if after_wrap { 25000 } else { 1000 });
    let state = e.capture_transport();
    temporary_song_render(&mut e);
    realtime(|| e.restore_transport(state, true));
    let out = audio(&mut e, 500);
    let pcm = print_pcm();
    let sign: f32 = if after_wrap { -1.0 } else { 1.0 };
    if (out[0] - sign * pcm[1000]).abs() > 1e-7f32 {
        println!(
            "checkpoint mapped={} loop={} actual={} expected={}",
            mapped as i32,
            after_wrap as i32,
            out[0],
            sign * pcm[1000]
        );
    }
    for n in 0..out.len() {
        require(
            out[n] == sign * pcm[1000 + n],
            "Pattern print checkpoint must restore exact cursor and head/loop asset",
        );
    }
    require(e.status().row == 0 && e.status().playing != 0, "Checkpoint musical row preserved");
}

fn section_clock(mapped: bool) {
    let mut e = Engine::new();
    setup(&mut e, mapped, 1);
    e.begin_update();
    e.set_pattern(1, 1);
    e.set_pattern(2, 1);
    e.set_sequence(&[0, 1, 2], true, 0);
    e.set_song_loop(1, 1);
    e.commit_update();
    e.play(1);
    let mut l = vec![0.0f32; 16000];
    let mut r = vec![0.0f32; l.len()];
    realtime(|| e.render_bus(0, 0, &mut l, &mut r));
    let mut span = ClockSpan::default();
    require(e.clock_span(6000, 1, 0, &mut span) == CLOCK_OK, "Retained section context");
    if span.context.next_boundary_frame != 12000 {
        println!("section mapped={} boundary={} expected=12000", mapped as i32, span.context.next_boundary_frame);
    }
    require(
        span.context.next_boundary_frame == 12000,
        "Section loop context predicts actual loop boundary, not song end",
    );
    require(span.context.next_boundary_kinds & CLOCK_BOUNDARY_LOOP != 0, "Section loop boundary kind");
    require(span.context.next_quarter_beat_frame == i64::MAX, "No integer beat is reachable in a .25-.5 beat loop");
    require(
        e.clock_span(12000, 1, 0, &mut span) == CLOCK_OK && span.context.song_frame == 6000,
        "Section wraps exact project frame",
    );
}

fn section_next_quarter(mapped: bool) {
    let mut e = Engine::new();
    setup(&mut e, mapped, 1);
    e.begin_update();
    e.set_pattern(1, 4);
    e.set_pattern(2, 1);
    e.set_sequence(&[0, 1, 2], true, 0);
    e.set_song_loop(1, 1);
    e.commit_update();
    e.play(1);
    let mut l = vec![0.0f32; 31000];
    let mut r = vec![0.0f32; l.len()];
    realtime(|| e.render_bus(0, 0, &mut l, &mut r));
    let mut span = ClockSpan::default();
    require(e.clock_span(27000, 1, 0, &mut span) == CLOCK_OK, "Section next-beat context");
    require(span.context.next_boundary_frame == 30000, "Quarter query loop boundary");
    require(span.context.next_quarter_beat_frame == 48000, "Next quarter after wrap must subtract nonzero loop origin");
    require(
        e.clock_span(27127, 1, 127, &mut span) == CLOCK_OK && span.context.next_quarter_beat_frame == 48127,
        "Upstream PDC shifts next quarter exactly once",
    );
}

fn checkpoint_bounds() {
    for mapped in [false, true] {
        let mut e = Engine::new();
        setup(&mut e, mapped, 4);
        install_print(&mut e);
        e.play(0);
        audio(&mut e, 7250);
        let good = e.capture_transport();
        for invalid in 0..5 {
            let mut state = good;
            if invalid == 0 {
                state.pattern_clip_valid = 0;
                state.pattern_clip_frame = 0;
                state.pattern_clip_pattern = 0;
                state.pattern_clip_looped = 0;
            }
            if invalid == 1 {
                state.pattern_clip_frame = i64::MAX;
            }
            if invalid == 2 {
                state.pattern_clip_frame = -1;
            }
            if invalid == 3 {
                state.pattern_clip_pattern = 99;
            }
            if invalid == 4 {
                state.pattern_clip_looped = 2;
            }
            temporary_song_render(&mut e);
            realtime(|| e.restore_transport(state, true));
            let out = audio(&mut e, 64);
            let pcm = print_pcm();
            for n in 0..out.len() {
                require(
                    out[n] == pcm[7250 + n],
                    "Invalid/default print checkpoint reconstructs row-local head position",
                );
            }
        }
        temporary_song_render(&mut e);
        e.restore_transport(good, false);
        let silent = audio(&mut e, 100);
        require(silent.iter().all(|&x| x == 0.0), "Canceled export must not resume frozen PCM");
    }
}

fn checkpoint_at_boundary() {
    for mapped in [false, true] {
        let mut e = Engine::new();
        setup(&mut e, mapped, 4);
        install_print(&mut e);
        e.play(0);
        audio(&mut e, 24000);
        let state = e.capture_transport();
        require(state.row_remaining == 0.0, "Pending wrap boundary");
        temporary_song_render(&mut e);
        e.restore_transport(state, true);
        let out = audio(&mut e, 64);
        let pcm = print_pcm();
        for n in 0..out.len() {
            require(out[n] == -pcm[n], "Pending natural wrap restores then selects steady loop print");
        }
    }
}

fn silent_sample_region() {
    for mode in
        [ST_MODE_ONESHOT, ST_MODE_FORWARD, ST_MODE_REVERSE, ST_MODE_PINGPONG, ST_MODE_GRANULAR, ST_MODE_WAVETABLE]
    {
        let mut e = Engine::new();
        setup(&mut e, false, 4);
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
    setup(&mut e, false, 4);
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

/// Legacy (unmapped) frames are bpm-relative: a tempo change while playing keeps the row in both
/// modes.
fn legacy_tempo_change() {
    for song in [0, 1] {
        let mut e = Engine::new();
        setup(&mut e, false, 16);
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
    setup(&mut e, false, 8);
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
fn fixed_pattern_head_checkpoint() {
    checkpoint_print(false, false);
    zero_render_allocations();
}

#[test]
fn mapped_pattern_head_checkpoint() {
    checkpoint_print(true, false);
    zero_render_allocations();
}

#[test]
fn fixed_pattern_loop_checkpoint() {
    checkpoint_print(false, true);
    zero_render_allocations();
}

#[test]
fn mapped_pattern_loop_checkpoint() {
    checkpoint_print(true, true);
    zero_render_allocations();
}

#[test]
fn fixed_section_retained_clock() {
    section_clock(false);
    zero_render_allocations();
}

#[test]
fn mapped_section_retained_clock() {
    section_clock(true);
    zero_render_allocations();
}

#[test]
fn fixed_section_next_quarter_and_pdc() {
    section_next_quarter(false);
    zero_render_allocations();
}

#[test]
fn mapped_section_next_quarter_and_pdc() {
    section_next_quarter(true);
    zero_render_allocations();
}

#[test]
fn checkpoint_bounds_and_cancellation() {
    checkpoint_bounds();
    zero_render_allocations();
}

#[test]
fn checkpoint_at_pending_wrap() {
    checkpoint_at_boundary();
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
fn legacy_tempo_change_keeps_position() {
    legacy_tempo_change();
    zero_render_allocations();
}

#[test]
fn external_pitch_retrigger() {
    external_retrigger();
    zero_render_allocations();
}

//! Port of `tests/engine_song_sends_tests.cpp`: song order/section-loop playback and shared
//! delay/reverb send return parameters.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::engine::Engine;
use tracker_engine::engine::*;

struct Audio {
    l: Vec<f32>,
    r: Vec<f32>,
}

impl Audio {
    fn new(n: usize) -> Audio {
        Audio { l: vec![0.0; n], r: vec![0.0; n] }
    }
}

struct Timed {
    frame: u64,
    e: STEvent,
}

fn render(e: &mut Engine, a: &mut Audio) {
    e.render(&mut a.l, &mut a.r);
}

fn note_ons(e: &mut Engine, frames: usize, block: usize) -> Vec<Timed> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < frames {
        let mut a = Audio::new(block.min(frames - at));
        render(e, &mut a);
        while let Some(v) = e.pop_event() {
            if v.type_ == ST_EVENT_NOTE_ON {
                out.push(Timed { frame: at as u64 + v.frame_offset as u64, e: v });
            }
        }
        at += block;
    }
    out
}

/// Patterns 0..3 have 4 rows (6000 frames per row at 120 BPM, 4 LPB, 48 kHz); row 0 plays note
/// 60+pattern.
fn configure_song(e: &mut Engine, sequence: &[i32], loop_song: bool) {
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, 0.0, 0.7);
    let mut i = default_instrument();
    i.type_ = ST_INSTRUMENT_MIDI;
    e.set_instrument(0, i);
    for p in 0..4 {
        e.set_pattern(p, 4);
        let mut s = default_step();
        s.note = 60 + p;
        s.gate = 1.0;
        e.set_step(p, 0, 0, s, &[]);
    }
    e.set_sequence(sequence, loop_song, 0);
    e.commit_update();
}

#[test]
fn rendered_order_matches_sequence() {
    let mut e = Engine::new();
    let seq = [2, 0, 0, 3, 1];
    configure_song(&mut e, &seq, false);
    e.play(1);
    let on = note_ons(&mut e, 24000 * 6, 256);
    assert!(on.len() == seq.len());
    for n in 0..seq.len() {
        assert!(on[n].e.note == 60 + seq[n]);
        assert!(on[n].frame == n as u64 * 24000);
    }
    assert!(e.status().playing == 0);
}

#[test]
fn section_loop() {
    let mut e = Engine::new();
    let seq = [0, 1, 2, 3];
    configure_song(&mut e, &seq, true);
    e.begin_update();
    e.set_song_loop(1, 2);
    e.commit_update();
    e.play(1);
    let on = note_ons(&mut e, 24000 * 8, 256);
    // 0, then 1 2 1 2 1 2 ... sample-accurately on 24000-frame boundaries.
    let expected = [0, 1, 2, 1, 2, 1, 2, 1];
    assert!(on.len() == 8);
    for n in 0..8 {
        assert!(on[n].e.note == 60 + expected[n]);
        assert!(on[n].frame == n as u64 * 24000);
    }
    let status = e.status();
    assert!(status.playing != 0 && status.sequence >= 1 && status.sequence <= 2);
    // Playback clock reflects the jump back to the section start (frame 24000 at 0.5 s).
    assert!(status.seconds >= 0.5 - 1e-6 && status.seconds < 1.5);
}

#[test]
fn section_loop_needs_loop_song_and_jumps_in_from_outside() {
    {
        let mut e = Engine::new();
        let seq = [0, 1, 2];
        configure_song(&mut e, &seq, false);
        e.begin_update();
        e.set_song_loop(0, 0);
        e.commit_update();
        e.play(1);
        let on = note_ons(&mut e, 24000 * 4, 256);
        assert!(on.len() == 3 && on[2].e.note == 62);
        assert!(e.status().playing == 0);
    }
    {
        // Start after the section: play to the end, then wrap into the section start (not slot 1).
        let mut e = Engine::new();
        let seq = [0, 1, 2, 3];
        configure_song(&mut e, &seq, true);
        e.begin_update();
        e.set_song_loop(1, 1);
        e.commit_update();
        e.seek(2, 0, 2);
        e.play(1);
        let on = note_ons(&mut e, 24000 * 4, 256);
        let expected = [2, 3, 1, 1];
        assert!(on.len() == 4);
        for n in 0..4 {
            assert!(on[n].e.note == 60 + expected[n]);
        }
    }
    {
        // Clearing the section returns to whole-song looping.
        let mut e = Engine::new();
        let seq = [0, 1];
        configure_song(&mut e, &seq, true);
        e.begin_update();
        e.set_song_loop(1, 1);
        e.commit_update();
        e.play(1);
        let mut on = note_ons(&mut e, 24000 * 3, 256);
        assert!(on.len() == 3 && on[2].e.note == 61);
        e.begin_update();
        e.set_song_loop(-1, -1);
        e.commit_update();
        on = note_ons(&mut e, 24000 * 2, 256);
        assert!(on.len() == 2 && on[0].e.note == 60 && on[1].e.note == 61);
    }
}

#[test]
fn section_loop_with_tempo_map() {
    let mut e = Engine::new();
    let seq = [0, 1, 2, 3];
    configure_song(&mut e, &seq, true);
    let tempos = [STClockTempoEvent { beat: 0.0, bpm: 120.0 }, STClockTempoEvent { beat: 2.0, bpm: 60.0 }];
    let meters = [STClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 }];
    e.begin_update();
    assert!(e.set_timing_map(&tempos, &meters, 7).is_ok());
    e.set_song_loop(1, 2);
    assert!(e.commit_update_checked().is_ok());
    e.play(1);
    let on = note_ons(&mut e, 200000, 256);
    let notes = [0, 1, 2, 1, 2, 1];
    let frames: [u64; 6] = [0, 24000, 48000, 96000, 120000, 168000];
    assert!(on.len() >= 6);
    for n in 0..6 {
        assert!(on[n].e.note == 60 + notes[n]);
        assert!(on[n].frame >= frames[n] && on[n].frame <= frames[n] + 1);
    }
}

/// An impulse through the delay send.
fn delay_impulse(beats: f64, time: f32, tone: f32, bpm: f64, frames: usize) -> Audio {
    let mut e = Engine::new();
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(bpm, 4, 0.0, 1.0);
    let mut i = default_instrument();
    i.type_ = ST_INSTRUMENT_SAMPLE;
    i.attack = 0.0;
    i.decay = 0.0;
    i.sustain = 1.0;
    i.release = 0.001;
    i.volume = 1.0;
    e.set_instrument(0, i);
    let mut click = vec![0.0f32; 16];
    click[0] = 0.8;
    e.set_sample(0, &click, None, 48000.0);
    let mut t = default_track();
    t.volume = 1.0;
    t.delay_send = 1.0;
    t.reverb_send = 0.0;
    e.set_track(0, t);
    e.set_effects(time, 0.0, 1.0, 0.7, 0.4, 0.0);
    e.set_send_effects(beats, tone, 0.0);
    e.commit_update();
    e.note_on(0, 0, 60, 127);
    let mut a = Audio::new(frames);
    render(&mut e, &mut a);
    a
}

/// The frame of the first sample above `level` on either channel.
fn first_above(a: &Audio, from: usize, level: f32) -> usize {
    for n in from..a.l.len() {
        if a.l[n].abs() > level || a.r[n].abs() > level {
            return n;
        }
    }
    a.l.len()
}

#[test]
fn synced_delay_and_tone() {
    let free = delay_impulse(0.0, 0.1, 1.0, 120.0, 60000);
    let mut echo = first_above(&free, 200, 1e-4);
    assert!((4795..=4805).contains(&echo));
    let synced = delay_impulse(0.5, 0.1, 1.0, 120.0, 60000);
    echo = first_above(&synced, 200, 1e-4);
    assert!((11995..=12005).contains(&echo)); // eighth note at 120 BPM = 0.25 s
    let faster = delay_impulse(0.5, 0.1, 1.0, 150.0, 60000);
    echo = first_above(&faster, 200, 1e-4);
    assert!((9595..=9605).contains(&echo)); // follows tempo
    // Tone 1 is bit-identical to the legacy open path; a dark tone removes high-frequency energy
    // from the echo.
    let open = delay_impulse(0.0, 0.1, 1.0, 120.0, 60000);
    let dark = delay_impulse(0.0, 0.1, 0.0, 120.0, 60000);
    let (mut open_peak, mut dark_peak) = (0.0f64, 0.0f64);
    for n in 4790..4900 {
        open_peak = open_peak.max((open.l[n].abs() + open.r[n].abs()) as f64);
        dark_peak = dark_peak.max((dark.l[n].abs() + dark.r[n].abs()) as f64);
    }
    assert!(dark_peak < open_peak * 0.25 && dark_peak > 0.0);
}

/// A synced delay whose length is one ULP above a whole frame count (0.75 beats at 140 BPM and
/// 44.1 kHz is 14175.000000000002 frames): once per pass of the delay line the read position
/// rounds up to exactly the line's size. That is its first sample; reading one past the end lost
/// (or corrupted) that echo frame.
#[test]
fn synced_delay_read_wraps_at_line_end() {
    let mut e = Engine::with_rate(44100.0);
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(140.0, 4, 0.0, 1.0);
    let mut i = default_instrument();
    i.type_ = ST_INSTRUMENT_SAMPLE;
    i.attack = 0.0;
    i.decay = 0.0;
    i.sustain = 1.0;
    i.release = 0.001;
    i.volume = 1.0;
    e.set_instrument(0, i);
    let mut click = vec![0.0f32; 16];
    click[0] = 0.8;
    e.set_sample(0, &click, None, 44100.0);
    let mut t = default_track();
    t.volume = 1.0;
    t.delay_send = 1.0;
    t.reverb_send = 0.0;
    e.set_track(0, t);
    e.set_effects(0.1, 0.0, 1.0, 0.7, 0.4, 0.0);
    e.set_send_effects(0.75, 1.0, 0.0);
    e.commit_update();
    e.note_on(0, 0, 60, 127);
    let mut a = Audio::new(15000);
    render(&mut e, &mut a);
    drop(e);
    // The click's first dry frame returns exactly one delay length later, followed by its second.
    assert!(a.l[0].abs() > 0.1f32 && a.l[1].abs() > 0.1f32);
    assert!((a.l[14175] - a.l[0]).abs() < 1e-6f32 && (a.r[14175] - a.r[0]).abs() < 1e-6f32);
    assert!((a.l[14176] - a.l[1]).abs() < 1e-6f32 && (a.r[14176] - a.r[1]).abs() < 1e-6f32);
}

#[test]
fn reverb_predelay() {
    let run = |predelay: f32| {
        let mut e = Engine::new();
        e.begin_update();
        e.set_track_count(1);
        e.set_timing(120.0, 4, 0.0, 1.0);
        let mut i = default_instrument();
        i.type_ = ST_INSTRUMENT_SAMPLE;
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        i.release = 0.001;
        i.volume = 1.0;
        e.set_instrument(0, i);
        let mut click = vec![0.0f32; 16];
        click[0] = 0.8;
        e.set_sample(0, &click, None, 48000.0);
        let mut t = default_track();
        t.volume = 1.0;
        t.delay_send = 0.0;
        t.reverb_send = 1.0;
        e.set_track(0, t);
        e.set_effects(0.3, 0.0, 0.0, 0.7, 0.4, 1.0);
        e.set_send_effects(0.0, 1.0, predelay);
        e.commit_update();
        e.note_on(0, 0, 60, 127);
        let mut a = Audio::new(24000);
        render(&mut e, &mut a);
        a
    };
    let (none, later) = (run(0.0), run(0.1));
    let (a, b) = (first_above(&none, 200, 1e-4), first_above(&later, 200, 1e-4));
    assert!(b >= a + 4795 && b <= a + 4805);
}

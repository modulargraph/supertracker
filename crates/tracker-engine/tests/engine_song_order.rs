//! Song playback: the order of the sequence and the section loop.
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
    e.set_timing(120.0, 4, 0.0);
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

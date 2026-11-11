//! Port of the former C++ test program `engine_step_effect_tests.cpp`: step effect playback regressions (sticky
//! ratchets, gateless note lifetime, probability, sample commands, live tempo changes).
#![cfg(feature = "engine")]
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod engine_support_b;

use engine_support_b::engine::Engine;
use tracker_engine::engine::*;

type Notes = Vec<(i32, i32)>;

struct Fixture {
    e: Engine,
}

impl Fixture {
    fn new(type_: i32) -> Fixture {
        let mut e = Engine::new();
        e.begin_update();
        e.set_track_count(1);
        e.set_timing(120.0, 4, 0.0, 0.7);
        e.set_pattern(0, 16);
        let mut i = default_instrument();
        i.type_ = type_;
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        i.release = 0.001;
        i.cutoff = 22000.0;
        i.volume = 0.5;
        e.set_instrument(0, i);
        Fixture { e }
    }

    /// C++ `step(row,note,fx=0,value=0,fx2=0,value2=0,gate=32)` with every argument given.
    #[allow(clippy::too_many_arguments)]
    fn step_full(&mut self, row: i32, note: i32, fx: i32, value: f32, fx2: i32, value2: f32, gate: f32) {
        let mut s = default_step();
        s.note = note;
        s.gate = gate;
        s.fx1 = STStepFX { type_: fx, value };
        s.fx2 = STStepFX { type_: fx2, value: value2 };
        self.e.set_step(0, 0, row, s, &[]);
    }

    /// `step(row, note, fx, value)`
    fn step_fx(&mut self, row: i32, note: i32, fx: i32, value: f32) {
        self.step_full(row, note, fx, value, 0, 0.0, 32.0);
    }

    /// `step(row, note)`
    fn step(&mut self, row: i32, note: i32) {
        self.step_full(row, note, 0, 0.0, 0, 0.0, 32.0);
    }

    fn play(&mut self) {
        self.e.commit_update();
        self.e.play(0);
    }

    fn notes(&mut self, frames: i32, block: i32) -> Notes {
        let mut out = Vec::new();
        let mut at = 0;
        while at < frames {
            let n = block.min(frames - at) as usize;
            let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
            self.e.render(&mut l, &mut r);
            while let Some(event) = self.e.pop_event() {
                if event.type_ == ST_EVENT_NOTE_ON {
                    out.push((at + event.frame_offset, event.note));
                }
            }
            at += block;
        }
        out
    }

    fn onsets(&mut self, frames: i32, block: i32) -> Vec<i32> {
        self.notes(frames, block).into_iter().map(|n| n.0).collect()
    }

    fn ramp(&mut self) {
        let sample: Vec<f32> = (0..48000).map(|i| 0.1f32 + 0.8f32 * i as f32 / 48000.0).collect();
        assert!(self.e.set_sample(0, &sample, None, 48000.0));
    }

    fn pcm(&mut self, frames: usize) -> Vec<f32> {
        let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
        self.e.render(&mut l, &mut r);
        l
    }
}

/// Note-ons and note-offs as (frame, note) / (frame, -note).
fn note_events(f: &mut Fixture, frames: i32, block: i32) -> Notes {
    let mut out = Vec::new();
    let mut at = 0;
    while at < frames {
        let n = block.min(frames - at) as usize;
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        f.e.render(&mut l, &mut r);
        while let Some(ev) = f.e.pop_event() {
            if ev.type_ == ST_EVENT_NOTE_ON || ev.type_ == ST_EVENT_NOTE_OFF {
                out.push((at + ev.frame_offset, if ev.type_ == ST_EVENT_NOTE_ON { ev.note } else { -ev.note }));
            }
        }
        at += block;
    }
    out
}

/// Row 2's plain note stops the sticky ratchet (it plays once); row 3's effect-only Ratchet then
/// repeats it.
fn sticky_ratchet_case(block: i32) {
    let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
    f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
    f.step(2, 64);
    f.step_fx(3, -3, ST_FX_RATCHET, 4.0);
    f.step_fx(4, -3, ST_FX_PAN, 0.5);
    f.step_fx(6, -3, ST_FX_RATCHET, 2.0);
    f.step(7, -1);
    f.play();
    let on = f.notes(54000, block);
    let expected: Notes = vec![
        (0, 60),
        (3000, 60),
        (6000, 60),
        (9000, 60),
        (12000, 64),
        (18000, 64),
        (19500, 64),
        (21000, 64),
        (22500, 64),
        (36000, 64),
        (39000, 64),
    ];
    assert!(on == expected);
}

/// Ratchet stop rules: a sticky ratchet continues through empty rows only. A new note on the lane
/// plays once (unless it has its own Ratchet); OFF and CUT stop the repeats. Each case is per
/// track, so kit pad tracks (ordinary tracks in a group) behave the same.
#[test]
fn ratchet_stop_rules() {
    // Ratchet then a plain note: the note plays once, the repeats stop.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
        f.step(2, 64);
        f.play();
        assert!(f.notes(30000, 256) == vec![(0, 60), (3000, 60), (6000, 60), (9000, 60), (12000, 64)]);
    }
    // A note with its own Ratchet takes over at its rate.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
        f.step_fx(1, 64, ST_FX_RATCHET, 3.0);
        f.step(2, -1);
        f.play();
        assert!(f.notes(18000, 256) == vec![(0, 60), (3000, 60), (6000, 64), (8000, 64), (10000, 64)]);
    }
    // Ratchet then OFF / CUT: stops.
    for note in [-1, -2] {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
        f.step(2, note);
        f.play();
        assert!(f.onsets(30000, 256) == vec![0, 3000, 6000, 9000]);
    }
    // Ratchet through empty rows continues.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
        f.step(4, -1);
        f.play();
        assert!(f.onsets(30000, 256) == vec![0, 3000, 6000, 9000, 12000, 15000, 18000, 21000]);
    }
    // Two lanes (two pads of a kit): a note on one lane never stops the other lane's ratchet.
    {
        let mut e = Engine::new();
        e.begin_update();
        e.set_track_count(2);
        e.set_timing(120.0, 4, 0.0, 0.7);
        e.set_pattern(0, 16);
        for t in 0..2 {
            let mut i = default_instrument();
            i.type_ = ST_INSTRUMENT_MIDI;
            e.set_instrument(t, i);
            let mut tr = default_track();
            tr.instrument = t;
            e.set_track(t, tr);
        }
        let mut put = |track: i32, row: i32, note: i32, fx: i32, v: f32| {
            let mut s = default_step();
            s.note = note;
            s.gate = 0.0;
            s.fx1 = STStepFX { type_: fx, value: v };
            e.set_step(0, track, row, s, &[]);
        };
        put(0, 0, 36, ST_FX_RATCHET, 2.0);
        put(1, 1, 38, 0, 0.0);
        put(0, 2, 36, 0, 0.0);
        put(1, 3, 38, ST_FX_RATCHET, 2.0);
        put(1, 4, -1, 0, 0.0);
        e.commit_update();
        e.play(0);
        let mut on: Notes = Vec::new();
        let mut at = 0;
        while at < 36000 {
            let (mut l, mut r) = ([0.0f32; 256], [0.0f32; 256]);
            e.render(&mut l, &mut r);
            while let Some(ev) = e.pop_event() {
                if ev.type_ == ST_EVENT_NOTE_ON {
                    on.push((at + ev.frame_offset, ev.note));
                }
            }
            at += 256;
        }
        on.sort();
        assert!(
            on == vec![(0, 36), (3000, 36), (6000, 36), (6000, 38), (9000, 36), (12000, 36), (18000, 38), (21000, 38)]
        );
    }
}

/// Note lifetime without a gate (gate 0): the note sounds until the next note on its lane (a
/// note-off with the release envelope), an OFF/CUT, or stop. Gated notes keep their explicit
/// length.
#[test]
fn gateless_lifetime() {
    // Next note releases: note-off for 60 at the frame the next note starts, before its note-on.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.0);
        f.step_full(3, 64, 0, 0.0, 0, 0.0, 0.0);
        f.step(6, -1);
        f.play();
        assert!(note_events(&mut f, 48000, 256) == vec![(0, 60), (18000, -60), (18000, 64), (36000, -64)]);
    }
    // CUT, and the default block size of one frame.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.0);
        f.step(5, -2);
        f.play();
        assert!(note_events(&mut f, 48000, 1) == vec![(0, 60), (30000, -60)]);
    }
    // A chord on one row: its notes leave each other sounding; the next row's note releases all
    // of them.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        let mut s = default_step();
        s.note = 60;
        s.gate = 0.0;
        let chord = [0, 4, 7];
        f.e.set_step(0, 0, 0, s, &chord);
        f.step_full(2, 62, 0, 0.0, 0, 0.0, 0.0);
        f.step(3, -1);
        f.play();
        let ev = note_events(&mut f, 24000, 256);
        assert!(ev.len() == 8);
        for n in 0..3 {
            assert!(ev[n].0 == 0 && ev[n].1 > 0);
        }
        for n in 3..6 {
            assert!(ev[n].0 == 12000 && ev[n].1 < 0);
        }
        assert!((ev[6] == (12000, 62)) && (ev[7] == (18000, -62)));
    }
    // A gated note keeps its own length; a later gateless note does not end before the next note.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.5);
        f.step_full(1, 62, 0, 0.0, 0, 0.0, 0.0);
        f.step(8, -1);
        f.play();
        assert!(note_events(&mut f, 60000, 256) == vec![(0, 60), (3000, -60), (6000, 62), (48000, -62)]);
    }
    // An explicit long gate is not shortened by the next note (old songs sound unchanged).
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 4.0);
        f.step_full(1, 62, 0, 0.0, 0, 0.0, 0.5);
        f.play();
        assert!(note_events(&mut f, 30000, 256) == vec![(0, 60), (6000, 62), (9000, -62), (24000, -60)]);
    }
    // Stop ends a gateless note.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.0);
        f.play();
        assert!(note_events(&mut f, 12000, 256) == vec![(0, 60)]);
        f.e.stop();
        let after = note_events(&mut f, 512, 256);
        assert!(!after.is_empty() && after[0].1 == -60);
    }
    // Microtiming delays the release to the next note's actual onset; a probability-dropped note
    // releases nothing.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.0);
        f.step_full(1, 62, ST_FX_MICROTIMING, 0.5, 0, 0.0, 0.0);
        f.step_full(2, 64, ST_FX_PROBABILITY, 0.0, 0, 0.0, 0.0);
        f.step(4, -1);
        f.play();
        assert!(note_events(&mut f, 30000, 256) == vec![(0, 60), (9000, -60), (9000, 62), (24000, -62)]);
    }
    // Ratchet without a gate: each repeat lasts one repeat spacing.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, ST_FX_RATCHET, 2.0, 0, 0.0, 0.0);
        f.step(1, -1);
        f.play();
        assert!(note_events(&mut f, 12000, 256) == vec![(0, 60), (3000, -60), (3000, 60), (6000, -60)]);
    }
    // A sampler note sounds until the next note releases it, then fades with the release
    // envelope.
    {
        let mut f = Fixture::new(ST_INSTRUMENT_SAMPLE);
        let sample = vec![0.5f32; 96000];
        assert!(f.e.set_sample(0, &sample, None, 48000.0));
        let mut i = default_instrument();
        i.type_ = ST_INSTRUMENT_SAMPLE;
        i.attack = 0.0;
        i.decay = 0.0;
        i.sustain = 1.0;
        i.release = 0.05;
        i.cutoff = 22000.0;
        i.volume = 0.5;
        i.mode = ST_MODE_ONESHOT;
        f.e.set_instrument(0, i);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.0);
        let mut quiet = default_step();
        quiet.note = 60;
        quiet.velocity = 1;
        quiet.gate = 0.0;
        f.e.set_step(0, 0, 4, quiet, &[]);
        f.play();
        let l = f.pcm(32000);
        assert!(l[20000].abs() > 0.09f32); // still sounding after three rows
        assert!(l[24000 + 600].abs() > l[31000].abs() * 4.0); // released by the next (near-silent) note: fading, not cut
        assert!(l[31000].abs() < l[20000].abs() * 0.05f32);
        for x in &l {
            assert!(x.is_finite());
        }
    }
}

#[test]
fn sticky_ratchet() {
    sticky_ratchet_case(1);
    sticky_ratchet_case(257);
    // The largest block a host renders.
    sticky_ratchet_case(4096);
}

#[test]
fn sample_commands() {
    let mut f = Fixture::new(ST_INSTRUMENT_SAMPLE);
    let sample: Vec<f32> = (0..48000).map(|i| 0.1f32 + 0.8f32 * i as f32 / 48000.0).collect();
    assert!(f.e.set_sample(0, &sample, None, 48000.0));
    f.step(0, 60);
    f.step_fx(1, -3, ST_FX_REVERSE, 1.0);
    f.step_fx(2, -3, ST_FX_OFFSET, 0.25);
    f.step_fx(3, -3, ST_FX_REVERSE, 0.0);
    f.play();
    let (mut l, mut r) = (vec![0.0f32; 24000], vec![0.0f32; 24000]);
    f.e.render(&mut l, &mut r);
    assert!(l[5000] > l[4000]);
    assert!(l[8000] < l[7000]);
    assert!(l[13000] > l[11000] * 2.0);
    assert!(l[15000] < l[14000]);
    assert!(l[21000] > l[20000]);
    for x in &l {
        assert!(x.is_finite());
    }
}

#[test]
fn ratchet_off() {
    for mode in 0..3 {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
        if mode == 0 {
            f.step_fx(1, -3, ST_FX_RATCHET, 1.0);
        } else if mode == 1 {
            f.step(1, -2);
        } else {
            let mut s = default_step();
            s.velocity = 0;
            f.e.set_step(0, 0, 1, s, &[]);
        }
        f.play();
        assert!(f.onsets(18000, 257) == vec![0, 3000]);
    }
}

#[test]
fn across_patterns_and_refresh() {
    let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
    f.e.set_pattern(0, 1);
    f.e.set_pattern(1, 3);
    f.e.set_sequence(&[0, 1], false, 0);
    f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
    f.e.commit_update();
    f.e.play(1);
    assert!(f.onsets(6000, 257) == vec![0, 3000]);
    f.e.begin_update();
    let mut mix = default_track();
    mix.pan = 0.25;
    f.e.set_track(0, mix);
    f.e.commit_update();
    assert!(f.onsets(12000, 257) == vec![0, 3000, 6000, 9000]);
}

/// A note dropped by its probability roll must leave the sticky ratchet exactly as it was: the
/// held note keeps its own rate.
#[test]
fn probability_drop_keeps_ratchet() {
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.5);
        f.step_full(1, 64, ST_FX_RATCHET, 4.0, ST_FX_PROBABILITY, 0.0, 0.5);
        f.play();
        assert!(f.notes(24000, 256) == vec![(0, 60)]);
    }
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, ST_FX_RATCHET, 2.0, 0, 0.0, 0.5);
        f.step_full(1, 64, ST_FX_RATCHET, 4.0, ST_FX_PROBABILITY, 0.0, 0.5);
        f.step(3, -1);
        f.play();
        assert!(f.notes(24000, 256) == vec![(0, 60), (3000, 60), (6000, 60), (9000, 60), (12000, 60), (15000, 60)]);
    }
}

/// The roll's draw (u+1)/2 is a float that can round up to exactly 1.0 (191 of the generator's
/// 2^32 outputs). A note without a Probability effect (probability 1) must still play then. These
/// chord sizes steer the engine's deterministic generator (seed, one step per rolled row, one
/// increment per started voice) so that row 9 draws 1.0.
#[test]
fn full_probability_never_drops() {
    let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
    let chords = [2usize, 1, 1, 5, 8, 8, 7, 1, 5];
    let offsets = [0, 1, 2, 3, 4, 5, 6, 7];
    for row in 0..9 {
        let mut s = default_step();
        s.note = 40;
        s.gate = 0.5;
        f.e.set_step(0, 0, row as i32, s, &offsets[..chords[row]]);
    }
    f.step_full(9, 100, 0, 0.0, 0, 0.0, 0.5);
    f.play();
    let mut started = 0;
    for n in f.notes(60000, 256) {
        if n.1 == 100 {
            assert!(n.0 == 54000);
            started += 1;
        }
    }
    assert!(started == 1);
}

/// A live tempo change (no tempo map, Pattern mode) keeps the current row's pending ratchet
/// repeats and its gates at their musical positions: they are frame counts scheduled at the old
/// tempo, like the row's own remaining length.
#[test]
fn tempo_change_rescales_pending_hits() {
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, ST_FX_RATCHET, 4.0, 0, 0.0, 0.5);
        f.step(1, -1);
        f.play();
        assert!(f.onsets(1000, 256) == vec![0]);
        f.e.begin_update();
        f.e.set_timing(240.0, 4, 0.0, 0.7);
        f.e.commit_update();
        // 5000 frames of the row were left; they become 2500, and the repeats at 1500/3000/4500
        // follow.
        assert!(f.onsets(6000, 256) == vec![250, 1000, 1750]);
    }
    {
        let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 1.0);
        f.play();
        let (mut l, mut r) = (vec![0.0f32; 8000], vec![0.0f32; 8000]);
        f.e.render(&mut l[..1000], &mut r[..1000]);
        f.e.begin_update();
        f.e.set_timing(240.0, 4, 0.0, 0.7);
        f.e.commit_update();
        f.e.render(&mut l, &mut r);
        // The one-row gate ends with its row, now 2500 frames after the change.
        let mut off = -1;
        while let Some(event) = f.e.pop_event() {
            if event.type_ == ST_EVENT_NOTE_OFF {
                off = event.frame_offset;
            }
        }
        assert!(off == 2500);
    }
}

/// An effect-only cell whose effects were all removed (note -3, no FX) is empty: the sticky
/// ratchet continues through it.
#[test]
fn empty_effect_cell_keeps_ratchet() {
    let mut f = Fixture::new(ST_INSTRUMENT_MIDI);
    f.step_fx(0, 60, ST_FX_RATCHET, 2.0);
    f.step(2, -3);
    f.play();
    assert!(f.onsets(30000, 256) == vec![0, 3000, 6000, 9000, 12000, 15000, 18000, 21000, 24000, 27000]);
}

/// Ratchet repeats replay the note's own playback effects: reverse, offset and effect-only
/// Reverse alongside Ratchet.
#[test]
fn ratchet_keeps_playback_effects() {
    {
        let mut f = Fixture::new(ST_INSTRUMENT_SAMPLE);
        f.ramp();
        f.step_full(0, 60, ST_FX_RATCHET, 2.0, ST_FX_REVERSE, 1.0, 0.9);
        f.play();
        let l = f.pcm(12000);
        for hit in 0..4 {
            let at = hit * 3000;
            assert!(l[at + 1500] < l[at + 1000]);
        }
    }
    {
        let mut f = Fixture::new(ST_INSTRUMENT_SAMPLE);
        f.ramp();
        f.step_full(0, 60, ST_FX_RATCHET, 2.0, ST_FX_OFFSET, 0.5, 0.9);
        f.play();
        let l = f.pcm(12000);
        for hit in 1..4 {
            let at = hit * 3000;
            assert!((l[at + 200] - l[200]).abs() < 0.01f32 * l[200]);
            assert!(l[at + 1500] > l[at + 1000]);
        }
    }
    {
        let mut f = Fixture::new(ST_INSTRUMENT_SAMPLE);
        f.ramp();
        f.step_full(0, 60, 0, 0.0, 0, 0.0, 0.9);
        f.step_full(1, -3, ST_FX_RATCHET, 4.0, ST_FX_REVERSE, 1.0, 32.0);
        f.step_fx(2, -3, ST_FX_RATCHET, 1.0);
        f.play();
        let l = f.pcm(12000);
        assert!(l[600] > l[300]);
        for hit in 0..4 {
            let at = 6000 + hit * 1500;
            assert!(l[at + 1000] < l[at + 300] && l[at + 1000] > 0.09f32);
        }
    }
}

//! Port of the former C++ test program `engine_track_choke_tests.cpp`: choke groups and voice modes as per-track
//! properties (`set_track_voice`): an open hat on one track chokes a closed hat on another track
//! of the same group, never a track of another group, and a track's voice mode overrides its
//! instrument's.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use tracker_engine::engine::*;

fn energy(e: &mut Engine, frames: usize) -> f64 {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    e.render(&mut l, &mut r);
    l.iter().map(|&x| x as f64 * x as f64).sum::<f64>() / frames as f64
}

/// Three tracks, one sustained tone instrument each. Voice settings: {choke, scope, mode} per
/// track.
fn setup(e: &mut Engine, voice: Option<&[[i32; 3]; 3]>) {
    e.begin_update();
    assert!(e.set_track_count(3) && e.set_instrument_count(3));
    e.set_timing(120.0, 4, 0.0, 1.0);
    for t in 0..3 {
        let mut track = default_track();
        track.instrument = t;
        track.volume = 1.0;
        e.set_track(t, track);
        let mut p = test_tone_instrument(TEST_SINE);
        p.attack = 0.0;
        p.sustain = 1.0;
        p.release = 0.001;
        p.volume = 0.5;
        p.mode = ST_MODE_FORWARD;
        e.set_instrument(t, p);
        load_test_tone(e, t, TEST_SINE);
        if let Some(voice) = voice {
            let v = voice[t as usize];
            assert!(e.set_track_voice(t, v[0], v[1], v[2]));
        }
    }
    e.commit_update();
}

#[test]
fn invalid_track_voice_settings_are_rejected() {
    let mut e = Engine::new();
    assert!(
        !e.set_track_voice(-1, 1, 0, -1)
            && !e.set_track_voice(0, 200, 0, -1)
            && !e.set_track_voice(0, 1, -1, -1)
            && !e.set_track_voice(0, 1, 0, 3)
            && !e.set_track_voice(ST_TRACK_SANITY_LIMIT, 1, 0, -1)
    );
}

#[test]
fn a_track_choke_group_stops_another_track_of_the_same_group_not_another_group() {
    // Tracks 0 and 1 share choke group 1 in group scope 1; track 2 has choke 1 in another group.
    let voice = [[1, 1, -1], [1, 1, -1], [1, 2, -1]];
    let mut e = Engine::new();
    setup(&mut e, Some(&voice));
    e.note_on(0, 0, 69, 100); // closed hat
    e.note_on(2, 2, 69, 100); // another group's hat
    let both = energy(&mut e, 4800);
    assert!(both > 0.01);
    e.note_on(1, 1, 69, 100); // open hat chokes the closed hat on track 0
    energy(&mut e, 2400);
    let after = energy(&mut e, 4800);
    // Two voices remain (track 1 and track 2): the level matches two tones, not three.
    let mut reference = Engine::new();
    setup(&mut reference, None);
    reference.note_on(1, 1, 69, 100);
    reference.note_on(2, 2, 69, 100);
    energy(&mut reference, 2400);
    let two = energy(&mut reference, 4800);
    assert!((after - two).abs() < two * 0.05);
    println!("  energy: two voices {two:.4}, after choke {after:.4}");
}

#[test]
fn mono_keeps_a_chord_whole_and_cuts_the_earlier_notes_on_the_next_note() {
    // Mono: a chord (two notes on the same frame) stays whole; a later note cuts both.
    let voice = [[0, 0, 1], [0, 0, -1], [0, 0, -1]];
    let mut e = Engine::new();
    setup(&mut e, Some(&voice));
    e.note_on(0, 0, 69, 100);
    e.note_on(0, 0, 76, 100);
    energy(&mut e, 2400);
    let chord = energy(&mut e, 4800);
    let mut one = Engine::new();
    setup(&mut one, Some(&voice));
    one.note_on(0, 0, 69, 100);
    energy(&mut one, 2400);
    let single = energy(&mut one, 4800);
    assert!(chord > single * 1.5);
    e.note_on(0, 0, 72, 100);
    energy(&mut e, 2400);
    let after = energy(&mut e, 4800);
    assert!((after - single).abs() < single * 0.1);
    println!("  energy: one {single:.4}, mono chord {chord:.4}, after the next note {after:.4}");
}

#[test]
fn a_mono_track_keeps_one_voice_without_settings_tracks_are_independent() {
    // No choke group: all three keep sounding (the reference for three voices).
    let mut e = Engine::new();
    setup(&mut e, None);
    e.note_on(0, 0, 69, 100);
    e.note_on(1, 1, 69, 100);
    e.note_on(2, 2, 69, 100);
    energy(&mut e, 2400);
    let three = energy(&mut e, 4800);
    let mut m = Engine::new();
    let mono = [[0, 0, 1], [0, 0, -1], [0, 0, -1]];
    setup(&mut m, Some(&mono));
    m.note_on(0, 0, 69, 100);
    m.note_on(0, 0, 69, 100);
    m.note_on(0, 0, 69, 100);
    energy(&mut m, 2400);
    let mono_energy = energy(&mut m, 4800);
    let mut one = Engine::new();
    setup(&mut one, None);
    one.note_on(0, 0, 69, 100);
    energy(&mut one, 2400);
    let single = energy(&mut one, 4800);
    assert!(three > single * 2.0);
    assert!((mono_energy - single).abs() < single * 0.05);
    println!("  energy: one {single:.4}, mono after three notes {mono_energy:.4}, three tracks {three:.4}");
}

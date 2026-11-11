//! Port of the former C++ test program `engine_many_tracks_tests.cpp`: tracks are sized from the project: 200-track
//! buses, return bus renumbering when the track count grows past sixteen,
//! copy-on-write commits, silent-bus history, and zero render allocations. The C++ `--bench`
//! mode (render CPU and commit latency) is the ignored test `bench`.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations, reset_render_allocations};
use engine_support_b::engine::Engine;
use engine_support_b::tone::*;
use std::time::Instant;
use tracker_engine::engine::*;

struct Pcm {
    l: Vec<f32>,
    r: Vec<f32>,
}

impl Pcm {
    fn new(n: usize) -> Pcm {
        Pcm { l: vec![0.0; n], r: vec![0.0; n] }
    }

    /// C++ `energy(from = 0, to = 0)`: `to == 0` means the end.
    fn energy(&self, from: usize, mut to: usize) -> f64 {
        if to == 0 {
            to = self.l.len();
        }
        let mut e = 0.0f64;
        for i in from..to {
            e += self.l[i] as f64 * self.l[i] as f64 + self.r[i] as f64 * self.r[i] as f64;
        }
        e / (to - from) as f64
    }
}

/// One bus of the block of `n` frames (at most 4096) at `at`.
fn bus(e: &mut Engine, b: i32, at: u64, n: usize) -> Pcm {
    let mut p = Pcm::new(n);
    realtime(|| e.render_bus(b, at, &mut p.l, &mut p.r));
    p
}

/// `n` frames from `at`, rendered block by block, of each of `buses`.
fn buses(e: &mut Engine, buses: &[i32], at: u64, n: usize) -> Vec<Pcm> {
    let mut out: Vec<Pcm> = buses.iter().map(|_| Pcm::new(n)).collect();
    let mut done = 0;
    while done < n {
        let count = 4096.min(n - done);
        for (b, p) in buses.iter().zip(out.iter_mut()) {
            realtime(|| e.render_bus(*b, at + done as u64, &mut p.l[done..done + count], &mut p.r[done..done + count]));
        }
        done += count;
    }
    out
}

fn commit(e: &mut Engine) {
    if let Err(error) = e.commit_update_checked() {
        panic!("commit: {error}");
    }
}

/// A sampler playing the sine test cycle.
fn sine() -> STInstrument {
    let mut p = test_tone_instrument(TEST_SINE);
    p.attack = 0.0;
    p.decay = 0.0;
    p.sustain = 1.0;
    p.release = 0.005;
    p.volume = 1.0;
    p.cutoff = 22000.0;
    p
}

fn volume_for(track: i32, count: i32) -> f32 {
    0.25f32 + 1.5f32 * track as f32 / count as f32
}

/// Every one of `count` tracks plays the same sine at its own volume.
fn all_tracks(e: &mut Engine, count: i32, pattern: i32, rows: i32) {
    e.begin_update();
    assert!(e.set_track_count(count));
    e.set_timing(120.0, 4, 0.0, 1.0);
    e.set_instrument(0, sine());
    load_test_tone(e, 0, TEST_SINE);
    e.set_pattern(pattern, rows);
    for t in 0..count {
        let mut track = default_track();
        track.volume = volume_for(t, count);
        track.instrument = 0;
        e.set_track(t, track);
        let mut s = default_step();
        s.note = 69;
        s.velocity = 127;
        s.gate = 64.0;
        e.set_step(pattern, t, 0, s, &[]);
    }
    commit(e);
}

#[test]
fn every_track_has_its_own_bus() {
    reset_render_allocations();
    let count = 200;
    {
        let mut e = Engine::new();
        all_tracks(&mut e, count, 0, 16);
        e.play(0);
        let mut buses = Vec::new();
        for t in 0..count {
            buses.push(bus(&mut e, t, 0, 2048));
        }
        let reference = &buses[0];
        assert!(reference.energy(256, 2048) > 1e-4);
        // Every track plays through its own bus, and the track's volume is not in it: the
        // same voice on every track gives the same samples on every bus.
        for t in 1..count {
            assert!(buses[t as usize].l == reference.l && buses[t as usize].r == reference.r);
        }
        // The return bus follows the tracks; buses beyond it render silence, not garbage.
        assert!(bus(&mut e, return_bus(count), 0, 2048).energy(0, 0) == 0.0);
        assert!(bus(&mut e, output_bus_count(count) + 3, 0, 2048).energy(0, 0) == 0.0);
    }
    {
        let mut e = Engine::new();
        all_tracks(&mut e, count, 0, 16);
        e.begin_update();
        for t in [7, 150, 199] {
            let mut track = default_track();
            track.volume = volume_for(t, count);
            track.instrument = 0;
            track.mute = (t == 7) as i32;
            e.set_track(t, track);
        }
        commit(&mut e);
        e.play(0);
        for t in 0..count {
            // Mute is the host's to apply: it silences no bus.
            let energy = bus(&mut e, t, 0, 2048).energy(256, 2048);
            assert!(energy > 0.0);
        }
    }
    {
        let mut e = Engine::new();
        all_tracks(&mut e, count, 0, 16);
        e.begin_update();
        for t in [150, 199] {
            let mut track = default_track();
            track.volume = volume_for(t, count);
            track.instrument = 0;
            track.solo = 1;
            e.set_track(t, track);
        }
        commit(&mut e);
        e.play(0);
        for t in 0..count {
            // So is solo.
            let energy = bus(&mut e, t, 0, 2048).energy(256, 2048);
            assert!(energy > 0.0);
        }
    }
    println!("200 tracks: every bus holds its track's voice, whatever the track's volume, mute and solo.");
    assert!(render_allocations() == 0);
}

#[test]
fn return_bus_follows_track_count() {
    reset_render_allocations();
    let mut e = Engine::new();
    e.begin_update();
    assert!(e.set_track_count(3));
    e.set_timing(120.0, 4, 0.0, 1.0);
    commit(&mut e);
    assert!(
        return_bus(3) == 16
            && channel_count(3) == 16
            && output_bus_count(3) == 17
    );
    e.begin_update();
    assert!(e.set_track_count(40));
    commit(&mut e);
    assert!(return_bus(40) == 40 && channel_count(40) == 40 && output_bus_count(40) == 41);
    assert!(!e.set_track_count(0) && !e.set_track_count(ST_TRACK_SANITY_LIMIT + 1));
    println!("The return bus follows the track count (never below 16).");
    assert!(render_allocations() == 0);
}

#[test]
fn grow_while_playing() {
    reset_render_allocations();
    let mut e = Engine::new();
    all_tracks(&mut e, 4, 0, 16);
    e.play(0);
    let mut at: u64 = 0;
    assert!(bus(&mut e, 0, at, 1024).energy(256, 0) > 0.0);
    at += 1024;
    for count in [4, 16, 17, 64, 200, 32, 8] {
        e.begin_update();
        assert!(e.set_track_count(count));
        for t in 0..count {
            let mut track = default_track();
            track.instrument = 0;
            track.volume = volume_for(t, count);
            e.set_track(t, track);
            let mut s = default_step();
            s.note = 69;
            s.velocity = 127;
            s.gate = 64.0;
            e.set_step(0, t, 0, s, &[]);
        }
        commit(&mut e);
        reset_render_allocations();
        bus(&mut e, 0, at, 1024);
        for t in (0..count).step_by(7) {
            bus(&mut e, t, at, 1024);
        }
        assert!(render_allocations() == 0);
        // Track 0 keeps sounding across every reshaping of the tables.
        assert!(bus(&mut e, 0, at, 1024).energy(0, 0) > 0.0);
        at += 1024;
    }
    // A full pattern pass retriggers every track of the final layout.
    e.seek(0, 0, 0);
    for _ in 0..8 {
        bus(&mut e, 0, at, 4096);
        at += 4096;
    }
    for t in 0..8 {
        assert!(bus(&mut e, t, at, 1024).energy(0, 0) > 0.0);
    }
    println!("Track count changes while playing (4..200) render without allocation and keep voices.");
    assert!(render_allocations() == 0);
}

#[test]
fn copy_on_write_commits() {
    reset_render_allocations();
    let mut e = Engine::new();
    let (count, patterns) = (64, 8);
    e.begin_update();
    assert!(e.set_track_count(count));
    e.set_instrument(0, sine());
    load_test_tone(&mut e, 0, TEST_SINE);
    for p in 0..patterns {
        e.set_pattern(p, 16);
        for t in 0..count {
            let mut s = default_step();
            s.note = 48 + p;
            s.velocity = 100;
            e.set_step(p, t, p, s, &[]);
        }
    }
    let seq: Vec<i32> = (0..patterns).collect();
    e.set_sequence(&seq, false, 0);
    commit(&mut e);
    // Rebuild with every pattern but one reused: the reused ones keep their steps.
    e.begin_update();
    e.clear_project();
    assert!(e.set_track_count(count));
    for p in 0..patterns {
        if p == 3 {
            e.set_pattern(p, 16);
            continue;
        }
        assert!(e.reuse_pattern(p));
    }
    assert!(!e.reuse_pattern(patterns + 5));
    e.set_sequence(&seq, false, 3);
    commit(&mut e);
    // Pattern 3 is now empty, pattern 5 kept its steps on every track.
    e.play(0);
    for audio in buses(&mut e, &[0, 31, 63], 0, 40000) {
        assert!(audio.energy(0, 0) == 0.0);
    }
    e.stop();
    e.begin_update();
    e.set_sequence(&seq, false, 5);
    commit(&mut e);
    e.seek(5, 0, 0);
    e.play(0);
    // Row 5 starts at 5*6000 frames; every track's note arrives there.
    for audio in buses(&mut e, &[0, 31, 63], 40000, 40000) {
        assert!(audio.energy(0, 29000) == 0.0 && audio.energy(31000, 40000) > 0.0);
    }
    println!("Copy-on-write: reused patterns keep their steps, edited ones are independent.");
    assert!(render_allocations() == 0);
}

/// C++ `--bench`: commit latency and render speed for 16, 64 and 200 tracks. Run with
/// `cargo test --release --test engine_many_tracks -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark (C++ --bench): prints commit latency and render speed"]
fn bench() {
    println!(
        "tracks | full rebuild | reuse+1 pattern | 1-step edit | render all active (x realtime) | render 8 active (x realtime)"
    );
    for count in [16, 64, 200] {
        let mut e = Engine::new();
        let (patterns, rows) = (32, 64);
        let build = |e: &mut Engine, changed: i32| {
            e.begin_update();
            e.clear_project();
            e.set_track_count(count);
            e.set_instrument(0, sine());
            load_test_tone(e, 0, TEST_SINE);
            for t in 0..count {
                let mut track = default_track();
                track.instrument = 0;
                track.volume = 0.01;
                e.set_track(t, track);
            }
            for p in 0..patterns {
                if changed >= 0 && p != changed {
                    e.reuse_pattern(p);
                    continue;
                }
                e.set_pattern(p, rows);
                for t in 0..count {
                    for r in (0..rows).step_by(2) {
                        let mut s = default_step();
                        s.note = 48 + (r + t) % 24;
                        s.velocity = 90;
                        s.gate = 1.5;
                        e.set_step(p, t, r, s, &[]);
                    }
                }
            }
            let seq: Vec<i32> = (0..patterns).collect();
            e.set_sequence(&seq, true, 0);
            commit(e);
        };
        let mut t0 = Instant::now();
        build(&mut e, -1);
        let full = t0.elapsed().as_secs_f64();
        t0 = Instant::now();
        for n in 0..10 {
            build(&mut e, n % patterns);
        }
        let reuse = t0.elapsed().as_secs_f64() / 10.0;
        t0 = Instant::now();
        for n in 0..100 {
            e.begin_update();
            let mut s = default_step();
            s.note = 60 + n % 12;
            e.set_step(n % patterns, n % count, 1, s, &[]);
            commit(&mut e);
        }
        let one = t0.elapsed().as_secs_f64() / 100.0;
        e.play(1);
        let block: usize = 512;
        let total: u64 = 48000 * 5;
        let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
        let mut at: u64 = 0;
        // The host pulls every track bus plus the return each cycle.
        let mut render = |e: &mut Engine, pulled: i32| {
            let start = Instant::now();
            let mut done = 0;
            while done < total {
                for b in 0..pulled {
                    e.render_bus(b, at, &mut l, &mut r);
                }
                done += block as u64;
                at += block as u64;
            }
            (total as f64 / 48000.0) / start.elapsed().as_secs_f64()
        };
        reset_render_allocations();
        let dense = realtime(|| render(&mut e, count));
        assert!(render_allocations() == 0);
        e.begin_update();
        e.clear_project();
        e.set_track_count(count);
        e.set_instrument(0, sine());
        load_test_tone(&mut e, 0, TEST_SINE);
        e.set_pattern(0, rows);
        for t in 0..8 {
            for r2 in (0..rows).step_by(2) {
                let mut s = default_step();
                s.note = 48 + r2 % 24;
                s.gate = 1.5;
                e.set_step(0, t, r2, s, &[]);
            }
        }
        commit(&mut e);
        let sparse = realtime(|| render(&mut e, count));
        assert!(render_allocations() == 0);
        println!(
            "{:6} | {:9.2} ms | {:12.2} ms | {:8.3} ms | {:10.0}x | {:10.0}x",
            count,
            full * 1e3,
            reuse * 1e3,
            one * 1e3,
            dense,
            sparse
        );
    }
}

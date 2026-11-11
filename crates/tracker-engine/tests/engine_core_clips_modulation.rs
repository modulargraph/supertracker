//! Port of the former C++ test program `engine_tests.cpp`, part 2 of 5: timeline audio clips, live arrangement
//! extension and long gates, the independent filter envelope, the LFO routing matrix,
//! independent LFOs with automation, modulation lifecycle and input sanitizing, and the
//! tempo-synced LFOs (periods, live edits, automation).
//!
//! One `#[test]` per suite, calling the engine through `engine_support_a::Engine`, which renders
//! it block by block as a host does: the tests read its buses (a plain render gives their sum).
//! Every test ends with the "zero render-thread allocations" check.
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod engine_support_a;

use engine_support_a::core_helpers::*;
use engine_support_a::{assert_zero_render_allocations, Engine};
use tracker_engine::engine::*;

#[test]
fn test_timeline_audio_clips() {
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let sample = vec![0.2f32; 48000];
        e.begin_update();
        e.set_pattern(0, 1);
        assert!(e.set_audio_asset(0, &sample, 48000.0));
        e.add_audio_clip(0, 0, 0.5, 1.0, 0.0, 1.0, 0.0, 0.0, 0);
        e.add_automation_point(0, 0, ST_TARGET_VOLUME, 0, 0, 0, 0.0, 0.0);
        e.add_automation_point(0, 0, ST_TARGET_VOLUME, 0, 0, 0, 1.0, 1.0);
        e.commit_update();
        assert!((e.duration(1) - 0.75).abs() < 1e-9);
        e.play(1);
        let mut a = Audio::new(40000);
        render(&mut e, &mut a);
        assert!(a.energy_in(0, 12000) == 0.0 && a.energy_in(12000, 35000) > 0.001 && a.energy_in(36000, 40000) == 0.0);
        assert!(e.status().playing == 0);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let mut sample = vec![0.1f32; 48000];
        sample[24000..].fill(0.5);
        e.begin_update();
        e.set_pattern(0, 8);
        e.set_audio_asset(0, &sample, 48000.0);
        e.add_audio_clip(0, 0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 0);
        e.commit_update();
        e.seek(0, 4, 0);
        e.play(1);
        let mut a = Audio::new(512);
        render(&mut e, &mut a);
        // The clip's own PCM (0.5 from frame 24000 on) at its gain of 1: no fader, no master.
        assert!(a.l[0] == 0.5 && a.r[0] == 0.5);
        assert!(e.status().row == 4);
        e.stop();
        render(&mut e, &mut a);
        assert!(a.energy() == 0.0);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let sample = vec![0.4f32; 48000];
        e.begin_update();
        e.set_pattern(0, 1);
        let seq = [0];
        e.set_sequence(&seq, 1, 0);
        e.set_audio_asset(0, &sample, 48000.0);
        e.add_audio_clip(0, 0, 0.0, 0.5, 0.0, 0.5, 0.01, 0.01, 0);
        e.commit_update();
        e.play(1);
        let mut a = Audio::new(12500);
        render(&mut e, &mut a);
        assert!(a.l[0] == 0.0 && a.l[12000] == 0.0 && a.l[12100] > 0.0);
        assert!(e.status().playing != 0);
        e.stop();
        render(&mut e, &mut a);
        e.begin_update();
        e.clear_project();
        e.set_pattern(0, 1);
        e.add_audio_clip(0, 0, 0.0, 0.25, 0.0, 1.0, 0.0, 0.0, 0);
        e.commit_update();
        e.seek(0, 0, 0);
        e.play(1);
        let mut b = Audio::new(128);
        render(&mut e, &mut b);
        assert!(b.energy() > 0.001);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let sample = vec![0.2f32; 48000];
        e.begin_update();
        e.set_audio_asset(0, &sample, 48000.0);
        e.add_audio_clip(0, 0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 0);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(1024);
        render(&mut e, &mut a);
        assert!(a.energy() == 0.0);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_live_clip_arrangement_extension_and_long_gates() {
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        let sample = vec![0.1f32; 48000];
        e.begin_update();
        e.set_pattern(0, 1);
        e.set_audio_asset(0, &sample, 48000.0);
        e.add_audio_clip(0, 1, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 0);
        e.commit_update();
        e.play(1);
        render_events(&mut e, 7000);
        e.begin_update();
        e.set_pattern(1, 4);
        let mut step = default_step();
        step.note = 72;
        e.set_step(1, 0, 1, step, &[]);
        let sequence = [0, 1];
        e.set_sequence(&sequence, 0, 0);
        e.commit_update();
        let events = render_events(&mut e, 7000);
        let mut count = 0;
        for v in &events {
            if v.e.type_ == ST_EVENT_NOTE_ON {
                count += 1;
                assert!(v.frame == 5000 && v.e.note == 72);
            }
        }
        assert!(count == 1);
    }
    {
        let mut e = Engine::new();
        configure(&mut e, ST_INSTRUMENT_MIDI);
        e.begin_update();
        e.set_pattern(0, 64);
        let mut step = default_step();
        step.gate = 32.0;
        e.set_step(0, 0, 0, step, &[]);
        e.commit_update();
        e.play(0);
        let events = render_events(&mut e, 193000);
        let mut count = 0;
        for v in &events {
            if v.e.type_ == ST_EVENT_NOTE_OFF {
                count += 1;
                assert!(v.frame == 192000);
            }
        }
        assert!(count == 1);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_independent_filter_envelope() {
    let mut p = modulation_instrument();
    p.cutoff = 100.0;
    p.filter_attack = 0.0;
    p.filter_decay = 0.08;
    p.filter_sustain = 0.0;
    p.filter_env_amount = 48.0;
    let a = modulation_audio(p, 12000, 512);
    assert!(a.energy_in(600, 1200) > a.energy_in(9000, 11000) * 30.0);
    p.filter_attack = 0.0;
    p.filter_decay = 0.0;
    p.filter_sustain = 1.0;
    p.cutoff = 200.0;
    p.filter_release = 0.01;
    // The C++ lambda captured `p` by reference and left the last duration in it.
    let release_energy = |p: &mut STInstrument, duration: f32| -> f64 {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        p.filter_release = duration;
        e.begin_update();
        e.set_instrument(0, *p);
        e.commit_update();
        e.note_on(0, 0, 69, 127);
        let mut warm = Audio::new(4096);
        render(&mut e, &mut warm);
        e.note_off(0, 69);
        let mut tail = Audio::new(6000);
        render(&mut e, &mut tail);
        tail.energy_in(4000, 5000)
    };
    assert!(release_energy(&mut p, 1.0) > release_energy(&mut p, 0.01) * 5.0);
    p.filter_env_amount = -48.0;
    p.cutoff = 3200.0;
    let closed = modulation_audio(p, 6000, 512);
    p.filter_env_amount = 0.0;
    let open = modulation_audio(p, 6000, 512);
    assert!(open.energy_in(4000, 5000) > closed.energy_in(4000, 5000) * 10.0);
    assert_zero_render_allocations();
}

#[test]
fn test_lfo_routing_matrix() {
    let mut p = modulation_instrument();
    p.lfo_shape = ST_LFO_SQUARE;
    p.lfo_depth = 1.0;
    p.lfo_target = ST_MOD_PITCH;
    let pitched = modulation_audio(p, 48000, 512);
    let mut crossings = 0i32;
    for n in 1..pitched.l.len() {
        if pitched.l[n - 1] <= 0.0 && pitched.l[n] > 0.0 {
            crossings += 1;
        }
    }
    assert!((crossings - 880).abs() <= 1);
    p.lfo_target = ST_MOD_VOLUME;
    p.lfo_rate = 2.0;
    let volume = modulation_audio(p, 48000, 512);
    assert!(volume.energy_in(1000, 10000) == 0.0 && volume.energy_in(14000, 22000) > 0.001);
    p.lfo_target = ST_MOD_PAN;
    p.lfo_rate = 1.0;
    let pan = modulation_audio(p, 48000, 512);
    let (mut left, mut right) = (0.0f64, 0.0f64);
    for n in 1000..20000 {
        left += (pan.l[n] * pan.l[n]) as f64;
        right += (pan.r[n] * pan.r[n]) as f64;
    }
    assert!(left == 0.0 && right > 3.0);
    left = 0.0;
    right = 0.0;
    for n in 28000..46000 {
        left += (pan.l[n] * pan.l[n]) as f64;
        right += (pan.r[n] * pan.r[n]) as f64;
    }
    assert!(right == 0.0 && left > 3.0);
    p.lfo_target = ST_MOD_CUTOFF;
    p.cutoff = 200.0;
    let cutoff = modulation_audio(p, 48000, 512);
    assert!(cutoff.energy_in(10000, 12000) > cutoff.energy_in(30000, 32000) * 100.0);
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.mode = ST_MODE_WAVETABLE;
    p.wavetable_position = 0.5;
    p.cutoff = 18000.0;
    p.lfo_target = ST_MOD_POSITION;
    let position = modulation_audio(p, 48000, 512);
    assert!(position.energy_in(1000, 20000) == 0.0 && position.energy_in(28000, 46000) > 0.001);
    assert_zero_render_allocations();
}

#[test]
fn test_independent_lfos_and_automation() {
    let mut p = modulation_instrument();
    p.lfo_target = ST_MOD_PITCH;
    p.lfo_rate = 5.0;
    p.lfo_depth = 1.0;
    let one = modulation_audio(p, 12000, 127);
    p.lfo_depth = 0.5;
    p.lfo2_target = ST_MOD_PITCH;
    p.lfo2_rate = 5.0;
    p.lfo2_depth = 0.5;
    let two = modulation_audio(p, 12000, 512);
    for n in 0..one.l.len() {
        assert!(((one.l[n] - two.l[n]).abs() as f64) < 1e-6);
    }
    p.lfo_shape = ST_LFO_RANDOM;
    p.lfo2_shape = ST_LFO_RANDOM;
    p.lfo2_rate = 7.0;
    p.lfo2_target = ST_MOD_PAN;
    let (random_a, random_b) = (modulation_audio(p, 12000, 1), modulation_audio(p, 12000, 127));
    for n in 0..random_a.l.len() {
        assert!(random_a.l[n] == random_b.l[n]);
        assert!(random_a.r[n] == random_b.r[n]);
    }
    let mut e = Engine::new();
    configure(&mut e, TONE);
    e.begin_update();
    p = modulation_instrument();
    p.lfo2_target = ST_MOD_VOLUME;
    p.lfo2_shape = ST_LFO_SQUARE;
    p.lfo2_rate = 0.0;
    e.set_instrument(0, p);
    let mut step = default_step();
    step.gate = 4.0;
    step.fx1 = fx(ST_FX_VOLUME, 0.8);
    e.set_step(0, 0, 0, step, &[]);
    e.add_automation_point(0, 0, ST_TARGET_LFO2_DEPTH, 0, 0, 0, 0.0, 1.0);
    e.add_automation_point(0, 0, ST_TARGET_VOLUME, 0, 0, 0, 0.0, 0.5);
    e.commit_update();
    e.play(0);
    let mut silent = Audio::new(12000);
    render(&mut e, &mut silent);
    assert!(silent.energy() == 0.0);
    assert_zero_render_allocations();
}

#[test]
fn test_modulation_lifecycle_and_compatibility() {
    let mut p = modulation_instrument();
    let baseline = modulation_audio(p, 8192, 512);
    p.filter_attack = 10.0;
    p.filter_decay = 0.0;
    p.filter_sustain = 1.0;
    p.filter_release = 0.0001;
    p.lfo_rate = 17.0;
    p.lfo_shape = ST_LFO_RANDOM;
    p.lfo2_rate = 29.0;
    p.lfo2_shape = ST_LFO_RANDOM;
    p.lfo2_target = ST_MOD_PAN;
    let inactive = modulation_audio(p, 8192, 127);
    assert!(baseline.l == inactive.l && baseline.r == inactive.r);
    let retrigger_energy = |mode: i32| -> f64 {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let mut i = modulation_instrument();
        i.cutoff = 100.0;
        i.filter_env_amount = 48.0;
        i.filter_attack = 0.0;
        i.filter_decay = 0.05;
        i.filter_sustain = 0.0;
        i.voice_mode = mode;
        e.begin_update();
        e.set_instrument(0, i);
        e.commit_update();
        e.note_on(0, 0, 69, 127);
        let mut warm = Audio::new(12000);
        render(&mut e, &mut warm);
        e.note_on(0, 0, 72, 127);
        let mut after = Audio::new(1800);
        render(&mut e, &mut after);
        after.energy_in(400, 1000)
    };
    assert!(retrigger_energy(1) > retrigger_energy(2) * 20.0);
    let released = |repeated: bool| -> Audio {
        let mut e = Engine::new();
        configure(&mut e, TONE);
        let mut i = modulation_instrument();
        i.cutoff = 200.0;
        i.filter_env_amount = 48.0;
        i.filter_attack = 0.0;
        i.filter_decay = 0.0;
        i.filter_sustain = 1.0;
        i.filter_release = 0.1;
        e.begin_update();
        e.set_instrument(0, i);
        e.commit_update();
        e.note_on(0, 0, 69, 127);
        let mut warm = Audio::new(4096);
        render(&mut e, &mut warm);
        e.note_off(0, 69);
        let mut head = Audio::new(1000);
        render(&mut e, &mut head);
        if repeated {
            e.note_off(0, 69);
        }
        let mut tail = Audio::new(6000);
        render(&mut e, &mut tail);
        tail
    };
    let (once, twice) = (released(false), released(true));
    assert!(once.l == twice.l && once.r == twice.r);
    p = modulation_instrument();
    p.filter_attack = f32::NAN;
    p.filter_decay = -100.0;
    p.filter_sustain = f32::INFINITY;
    p.filter_release = 0.0;
    p.filter_env_amount = f32::NAN;
    p.lfo2_rate = f32::INFINITY;
    p.lfo2_depth = f32::NAN;
    p.lfo_target = 1000;
    p.lfo_shape = -1;
    let safe = modulation_audio(p, 1024, 512);
    assert!(safe.energy() > 0.0001);
    assert_zero_render_allocations();
}

#[test]
fn test_tempo_synced_lfo_periods() {
    let defaults = default_instrument();
    assert!(defaults.lfo_sync == 0 && defaults.lfo2_sync == 0 && defaults.lfo_beats == 1.0 && defaults.lfo2_beats == 1.0);
    struct Division {
        bpm: f64,
        beats: f32,
        half_period: usize,
    }
    for slot in [0, 1] {
        for d in [
            Division { bpm: 120.0, beats: 1.0, half_period: 12000 },
            Division { bpm: 60.0, beats: 1.0, half_period: 24000 },
            Division { bpm: 120.0, beats: 0.75, half_period: 9000 },
            Division { bpm: 120.0, beats: 1.0f32 / 3.0, half_period: 4000 },
            Division { bpm: 60.0, beats: 0.5, half_period: 12000 },
        ] {
            let mut e = Engine::new();
            let mut p = synced_pan_instrument(slot);
            p.lfo2_beats = d.beats;
            p.lfo_beats = d.beats;
            configure_synced_lfo(&mut e, p, d.bpm);
            let mut a = Audio::new(d.half_period + 200);
            render(&mut e, &mut a);
            assert_pan_switch(&a, d.half_period);
        }
    }
    // Matching free-Hz patches render identically, and disabled sync ignores beat values.
    let mut p = modulation_instrument();
    p.lfo_rate = 2.0;
    p.lfo_depth = 0.6;
    p.lfo_target = ST_MOD_PITCH;
    p.lfo2_rate = 4.0;
    p.lfo2_depth = 0.3;
    p.lfo2_target = ST_MOD_PAN;
    p.lfo2_shape = ST_LFO_RANDOM;
    let free = modulation_audio(p, 25000, 127);
    p.lfo_beats = 64.0;
    p.lfo2_beats = 1.0f32 / 64.0;
    let unchanged = modulation_audio(p, 25000, 512);
    assert!(free.l == unchanged.l && free.r == unchanged.r);
    p.lfo2_sync = 1;
    p.lfo_sync = 1;
    p.lfo_beats = 1.0;
    p.lfo2_beats = 0.5;
    let synced = modulation_audio(p, 25000, 1);
    assert!(free.l == synced.l && free.r == synced.r);
    // Independent synced destinations retain their own periods.
    {
        let mut e = Engine::new();
        p = synced_pan_instrument(0);
        p.lfo2_sync = 1;
        p.lfo2_beats = 0.5;
        p.lfo2_depth = 1.0;
        p.lfo2_shape = ST_LFO_SQUARE;
        p.lfo2_target = ST_MOD_VOLUME;
        configure_synced_lfo(&mut e, p, 120.0);
        let mut a = Audio::new(24000);
        render(&mut e, &mut a);
        assert!(
            a.energy_in(1000, 5000) == 0.0
                && a.energy_in(7000, 11000) > 0.001
                && a.energy_in(13000, 17000) == 0.0
                && a.energy_in(19000, 23000) > 0.001
        );
        for n in 7000..11000 {
            assert!(a.l[n] == 0.0);
        }
        for n in 19000..23000 {
            assert!(a.r[n] == 0.0);
        }
    }
    // A fastest division at maximum tempo must not inherit the free-Hz100 cap.
    for slot in [0, 1] {
        let e = Engine::create(8000.0);
        assert!(e.is_some());
        let mut e = e.unwrap();
        p = synced_pan_instrument(slot);
        p.lfo2_beats = 1.0f32 / 64.0;
        p.lfo_beats = 1.0f32 / 64.0;
        configure_synced_lfo(&mut e, p, 999.0);
        let mut a = Audio::new(8000);
        render(&mut e, &mut a);
        let mut edges = 0i32;
        for n in 1..a.l.len() {
            if (a.l[n] == 0.0) != (a.l[n - 1] == 0.0) {
                edges += 1;
            }
        }
        assert!((edges - 2131).abs() <= 1);
        assert!(a.energy() > 0.001);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_tempo_synced_lfo_live_edits() {
    for slot in [0, 1] {
        for change in 0..3 {
            let mut e = Engine::new();
            let mut p = synced_pan_instrument(slot);
            configure_synced_lfo(&mut e, p, 120.0);
            let mut quarter = Audio::new(6000);
            render(&mut e, &mut quarter);
            e.begin_update();
            let remaining: usize;
            if change == 0 {
                e.set_timing(60.0, 4, 0.0);
                remaining = 12000;
            } else if change == 1 {
                if slot == 0 {
                    p.lfo_beats = 0.5;
                } else {
                    p.lfo2_beats = 0.5;
                }
                remaining = 3000;
                e.set_instrument(0, p);
            } else {
                if slot == 0 {
                    p.lfo_sync = 0;
                    p.lfo_rate = 4.0;
                } else {
                    p.lfo2_sync = 0;
                    p.lfo2_rate = 4.0;
                }
                remaining = 3000;
                e.set_instrument(0, p);
            }
            e.commit_update();
            let mut after = Audio::new(remaining + 200);
            render(&mut e, &mut after);
            assert_pan_switch(&after, remaining);
        }
    }
    for slot in [0, 1] {
        let mut e = Engine::new();
        let mut p = synced_pan_instrument(slot);
        if slot == 0 {
            p.lfo_sync = 0;
            p.lfo_rate = 2.0;
        } else {
            p.lfo2_sync = 0;
            p.lfo2_rate = 2.0;
        }
        configure_synced_lfo(&mut e, p, 120.0);
        let mut quarter = Audio::new(6000);
        render(&mut e, &mut quarter);
        if slot == 0 {
            p.lfo_sync = 1;
            p.lfo_beats = 0.5;
        } else {
            p.lfo2_sync = 1;
            p.lfo2_beats = 0.5;
        }
        e.begin_update();
        e.set_instrument(0, p);
        e.commit_update();
        let mut after = Audio::new(3200);
        render(&mut e, &mut after);
        assert_pan_switch(&after, 3000);
    }
    // Zero depth still advances the synced phase; a new mono voice retriggers it.
    for slot in [0, 1] {
        let mut e = Engine::new();
        let mut p = synced_pan_instrument(slot);
        p.voice_mode = 1;
        p.lfo2_depth = 0.0;
        p.lfo_depth = 0.0;
        configure_synced_lfo(&mut e, p, 120.0);
        let mut silent_mod = Audio::new(15000);
        render(&mut e, &mut silent_mod);
        if slot == 0 {
            p.lfo_depth = 1.0;
        } else {
            p.lfo2_depth = 1.0;
        }
        e.begin_update();
        e.set_instrument(0, p);
        e.commit_update();
        let mut left = Audio::new(128);
        render(&mut e, &mut left);
        for &value in &left.r {
            assert!(value == 0.0);
        }
        assert!(left.energy() > 0.001);
        e.note_on(0, 0, 60, 127);
        let mut right = Audio::new(128);
        render(&mut e, &mut right);
        for &value in &right.l {
            assert!(value == 0.0);
        }
        assert!(right.energy() > 0.001);
    }
    for slot in [0, 1] {
        let mut e = Engine::new();
        let mut p = synced_pan_instrument(slot);
        p.voice_mode = 2;
        configure_synced_lfo(&mut e, p, 120.0);
        let mut warm = Audio::new(9000);
        render(&mut e, &mut warm);
        e.note_on(0, 0, 64, 127);
        let mut legato = Audio::new(3200);
        render(&mut e, &mut legato);
        assert_pan_switch(&legato, 3000);
    }
    assert_zero_render_allocations();
}

#[test]
fn test_tempo_synced_lfo_automation() {
    for slot in [0, 1] {
        let mut e = Engine::new();
        let p = synced_pan_instrument(slot);
        configure_synced_lfo(&mut e, p, 120.0);
        e.begin_update();
        let mut step = default_step();
        step.gate = 8.0;
        e.set_step(0, 0, 0, step, &[]);
        let beats_target = if slot != 0 { ST_TARGET_LFO2_BEATS } else { ST_TARGET_LFO_BEATS };
        let rate_target = if slot != 0 { ST_TARGET_LFO2_RATE } else { ST_TARGET_LFO_RATE };
        e.add_automation_point(0, 0, beats_target, 0, 0, 1, 0.0, 1.0);
        e.add_automation_point(0, 0, beats_target, 0, 0, 1, 1.0, 0.5);
        e.add_automation_point(0, 0, rate_target, 0, 0, 1, 0.0, 83.0);
        e.commit_update();
        e.play(0);
        let mut a = Audio::new(9200);
        render(&mut e, &mut a);
        assert_pan_switch(&a, 9000);
    }
    // Nonfinite beat input falls back to one; bounds stay finite and deterministic.
    for slot in [0, 1] {
        let mut p = synced_pan_instrument(slot);
        p.lfo2_beats = f32::NAN;
        p.lfo_beats = f32::NAN;
        let mut e = Engine::new();
        configure_synced_lfo(&mut e, p, 120.0);
        let mut a = Audio::new(12200);
        render(&mut e, &mut a);
        assert_pan_switch(&a, 12000);
        p.lfo2_beats = -1.0;
        p.lfo_beats = -1.0;
        let low = modulation_audio(p, 2000, 512);
        p.lfo2_beats = 1.0f32 / 64.0;
        p.lfo_beats = 1.0f32 / 64.0;
        let mut clamped = modulation_audio(p, 2000, 512);
        assert!(low.l == clamped.l && low.r == clamped.r);
        p.lfo2_beats = 1000.0;
        p.lfo_beats = 1000.0;
        let high = modulation_audio(p, 2000, 512);
        p.lfo2_beats = 64.0;
        p.lfo_beats = 64.0;
        clamped = modulation_audio(p, 2000, 512);
        assert!(high.l == clamped.l && high.r == clamped.r);
    }
    assert_zero_render_allocations();
}

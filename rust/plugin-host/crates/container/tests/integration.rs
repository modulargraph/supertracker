//! Integration tests: the real container binary, the real supervisor, on this machine.

#![allow(clippy::bool_assert_comparison, clippy::needless_range_loop, clippy::cloned_ref_to_slice_refs, clippy::unnecessary_map_or)]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use stpc_container::test_backend::{self as tb, sine_sample};

// ---- audio ------------------------------------------------------------------------------

#[test]
fn audio_round_trip_is_bit_exact_for_any_layout_and_block_size() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let layouts: &[(&[u32], &[u32])] = &[
        (&[1], &[1]),
        (&[2], &[2]),
        (&[1], &[2]),
        (&[2, 2], &[2]),
        (&[6], &[6]),
        (&[8, 2, 1], &[8, 2]),
        (&[32, 32], &[64]),
    ];
    let mut rng = Rng(0x5EED_0001);
    for (in_buses, out_buses) in layouts {
        let instance = active(
            env.sup(),
            InstanceConfig { input_buses: in_buses.to_vec(), output_buses: out_buses.to_vec(), ..effect("copy") },
        );
        let inputs: usize = in_buses.iter().sum::<u32>() as usize;
        let outputs: usize = out_buses.iter().sum::<u32>() as usize;
        let mut sizes = vec![1, 2, 3, 7, 16, 63, 64, 65, 100, 128, 441, 512, 1000, 1024, 2048, 4095, 4096, 1];
        sizes.extend((0..40).map(|_| 1 + rng.below(4096) as usize));
        for frames in sizes {
            let input: Vec<Vec<f32>> = (0..inputs).map(|_| rng.raw_bits(frames)).collect();
            let block = run(&instance, &input, outputs, frames, &[]);
            assert_eq!(block.outcome.flags, PROCESS_OK, "layout {in_buses:?}->{out_buses:?} frames {frames}");
            for (channel, output) in block.outputs.iter().enumerate() {
                assert_bits_eq(output, &input[channel % inputs], &format!("{in_buses:?}->{out_buses:?} ch {channel} frames {frames}"));
            }
        }
        assert_eq!(instance.stats().fallback_blocks, 0);
    }
    env.finish();
}

#[test]
fn missing_inputs_are_silent_and_extra_outputs_are_zeroed() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), effect("copy"));
    let mut rng = Rng(7);
    let left = rng.signal(256);
    // Only one of two input channels supplied, three outputs requested from a stereo plug-in.
    let block = run(&instance, &[left.clone()], 3, 256, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &left, "left");
    assert!(block.outputs[1].iter().all(|&s| s == 0.0));
    assert!(block.outputs[2].iter().all(|&s| s == 0.0));
    // More frames than the instance was created for.
    let long_input = rng.signal(4097);
    let too_long = run(&instance, &[long_input.clone()], 2, 4097, &[]);
    assert_eq!(too_long.outcome.flags, PROCESS_INVALID | PROCESS_FALLBACK);
    assert_bits_eq(&too_long.outputs[0], &long_input, "an oversized block still gets the effect fallback");
    assert!(too_long.outputs[1].iter().all(|&s| s == 0.0));
    // Zero frames is a no-op.
    assert_eq!(run(&instance, &[], 2, 0, &[]).outcome.flags, PROCESS_OK);
    env.finish();
}

#[test]
fn inactive_instance_plays_the_fallback() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let effect_instance = env.sup().create_instance(effect("gain")).unwrap();
    let synth = env.sup().create_instance(instrument("sine")).unwrap();
    let mut rng = Rng(9);
    let input = vec![rng.signal(128), rng.signal(128)];
    let block = run(&effect_instance, &input, 2, 128, &[]);
    assert_eq!(block.outcome.flags, PROCESS_INACTIVE | PROCESS_FALLBACK);
    assert_bits_eq(&block.outputs[0], &input[0], "effect fallback is the dry signal");
    assert_bits_eq(&block.outputs[1], &input[1], "effect fallback is the dry signal");
    let block = run(&synth, &[], 2, 128, &[Event::note_on(0, 0, 69, 100)]);
    assert_eq!(block.outcome.flags, PROCESS_INACTIVE | PROCESS_FALLBACK);
    assert!(block.outputs.iter().flatten().all(|&s| s == 0.0), "instrument fallback is silence");
    effect_instance.activate().unwrap();
    assert_eq!(run(&effect_instance, &input, 2, 128, &[]).outcome.flags, PROCESS_OK);
    effect_instance.deactivate().unwrap();
    assert_eq!(run(&effect_instance, &input, 2, 128, &[]).outcome.flags, PROCESS_INACTIVE | PROCESS_FALLBACK);
    env.finish();
}

// ---- events -----------------------------------------------------------------------------

#[test]
fn midi_events_are_sample_accurate_and_echoed_with_their_offsets() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), instrument("sine"));
    let rate = 48_000.0;
    // (block size, events in the block)
    let script: Vec<(usize, Vec<Event>)> = vec![
        (64, vec![]),
        (128, vec![Event::note_on(37, 0, 69, 100)]),
        (1, vec![]),
        (333, vec![]),
        (256, vec![Event::note_off(200, 0, 69)]),
        (512, vec![Event::note_on(0, 0, 60, 90), Event::note_off(511, 0, 60)]),
        (100, vec![Event::note_on(99, 0, 72, 1)]),
        (4096, vec![Event::note_off(1, 0, 72), Event::note_on(1000, 0, 81, 127), Event::note_on(3000, 0, 57, 127)]),
    ];
    let mut note: Option<u8> = None;
    let mut age = 0u64;
    for (index, (frames, events)) in script.iter().enumerate() {
        let block = run(&instance, &[], 2, *frames, events);
        assert_eq!(block.outcome.flags, PROCESS_OK, "block {index}");
        let mut expected = vec![0.0f32; *frames];
        let mut next = 0;
        for (frame, slot) in expected.iter_mut().enumerate() {
            while next < events.len() && events[next].offset as usize <= frame {
                let bytes = events[next].midi_bytes();
                if bytes[0] & 0xF0 == 0x90 {
                    note = Some(bytes[1]);
                    age = 0;
                } else if note == Some(bytes[1]) {
                    note = None;
                }
                next += 1;
            }
            if let Some(n) = note {
                *slot = sine_sample(n, age, rate, 0.5);
                age += 1;
            }
        }
        assert_bits_eq(&block.outputs[0], &expected, &format!("block {index} left"));
        assert_bits_eq(&block.outputs[1], &expected, &format!("block {index} right"));
        assert_eq!(&block.out_events, events, "block {index}: MIDI out mirrors MIDI in, offsets included");
    }
    // A note-on really starts at its offset: silence before, sound from there.
    let fresh = active(env.sup(), instrument("sine"));
    let probe = run(&fresh, &[], 2, 64, &[Event::note_on(37, 0, 69, 100)]);
    assert!(probe.outputs[0][..=37].iter().all(|&s| s == 0.0)); // sin(0) = 0 at the onset itself
    assert!(probe.outputs[0][38] != 0.0);
    env.finish();
}

#[test]
fn parameter_events_are_sample_accurate() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), effect("gain"));
    let mut rng = Rng(11);
    let mut gain = 1.0f64;
    let script: Vec<(usize, Vec<(u32, f64)>)> = vec![
        (128, vec![]),
        (128, vec![(64, 0.5)]),
        (100, vec![(0, 2.0), (99, 0.25)]),
        (1, vec![(0, 3.0)]),
        (4096, vec![(1, 0.0), (2, 1.5), (4095, 0.75)]),
        (77, vec![]),
    ];
    for (index, (frames, changes)) in script.iter().enumerate() {
        let input = vec![rng.signal(*frames), rng.signal(*frames)];
        let events: Vec<Event> = changes.iter().map(|&(offset, value)| Event::param(offset, tb::gain::GAIN, value)).collect();
        let block = run(&instance, &input, 2, *frames, &events);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        let mut expected = vec![vec![0.0f32; *frames]; 2];
        let mut next = 0;
        for frame in 0..*frames {
            while next < changes.len() && changes[next].0 as usize <= frame {
                gain = changes[next].1;
                next += 1;
            }
            for channel in 0..2 {
                expected[channel][frame] = input[channel][frame] * gain as f32;
            }
        }
        for channel in 0..2 {
            assert_bits_eq(&block.outputs[channel], &expected[channel], &format!("block {index} ch {channel}"));
        }
    }
    // The last timed value is what the control side reads back.
    assert_eq!(instance.parameter(tb::gain::GAIN).unwrap(), Some(0.75));
    // Offsets beyond the block are clamped to its last frame, not dropped or misapplied.
    let input = vec![vec![1.0f32; 16], vec![1.0f32; 16]];
    let block = run(&instance, &input, 2, 16, &[Event::param(1000, tb::gain::GAIN, 2.0)]);
    assert_eq!(&block.outputs[0][..15], &[0.75f32; 15]);
    assert_eq!(block.outputs[0][15], 2.0);
    env.finish();
}

#[test]
fn event_overflow_is_reported() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), InstanceConfig { event_capacity: 8, out_event_capacity: 4, ..instrument("sine") });
    let events: Vec<Event> = (0..20).map(|i| Event::note_on(i, 0, 60, 100)).collect();
    let block = run(&instance, &[], 2, 64, &events);
    assert_eq!(block.outcome.flags, PROCESS_OK | PROCESS_EVENTS_DROPPED | PROCESS_OUT_EVENTS_DROPPED);
    assert_eq!(block.out_events, events[..4].to_vec());
    env.finish();
}

#[test]
fn sidechain_bus_reaches_the_plugin() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), InstanceConfig { input_buses: vec![2, 2], ..effect("ducker") });
    let info = instance.info();
    assert_eq!(info.inputs.len(), 2);
    assert!(!info.inputs[0].auxiliary && info.inputs[1].auxiliary);
    let mut rng = Rng(21);
    for frames in [64usize, 1, 500, 4096] {
        let input: Vec<Vec<f32>> = (0..4).map(|_| rng.signal(frames)).collect();
        let block = run(&instance, &input, 2, frames, &[]);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        for channel in 0..2 {
            let expected: Vec<f32> =
                (0..frames).map(|i| input[channel][i] * (1.0 - input[2 + channel][i].abs().min(1.0))).collect();
            assert_bits_eq(&block.outputs[channel], &expected, "ducked");
        }
        // Without a sidechain signal the main input passes unchanged.
        let block = run(&instance, &input[..2], 2, frames, &[]);
        assert_bits_eq(&block.outputs[0], &input[0], "no key");
    }
    env.finish();
}

#[test]
fn transport_and_block_flags_reach_the_plugin() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), InstanceConfig { sample_rate: 44_100.0, ..effect("probe") });
    let transport = Transport {
        flags: TRANSPORT_PLAYING | TRANSPORT_TEMPO_VALID | TRANSPORT_TIME_SIG_VALID | TRANSPORT_PPQ_VALID,
        time_sig_numerator: 7,
        time_sig_denominator: 8,
        tempo_bpm: 174.0,
        sample_position: 96_000,
        ppq_position: 12.5,
        ..Transport::default()
    };
    let silent = vec![0.0f32; 0];
    let input = vec![vec![0.5f32; 32], silent]; // second channel missing: flagged silent
    let block = run_with(&instance, &input, 2, 32, &[Event::param(3, 9, 1.0)], Some(&transport));
    assert_eq!(block.outcome.flags, PROCESS_OK);
    let report = &block.outputs[0];
    assert_eq!(report[..12], [32.0, 174.0, 96_000.0, transport.flags as f32, 0.0, 1.0, 0.0, 2.0, 12.5, 7.0, 8.0, 44_100.0]);
    // Offline mode is announced per block; no transport means "stopped".
    instance.set_offline(true).unwrap();
    let block = run(&instance, &input, 2, 32, &[]);
    assert_eq!(block.outputs[0][..7], [32.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    env.finish();
}

// ---- parameters, state, presets, editor ---------------------------------------------------

#[test]
fn parameter_metadata_values_and_text() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let gain = active(env.sup(), effect("gain"));
    let list = gain.parameters().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(gain.info().parameter_count, 2);
    let (g, mode) = (&list[0], &list[1]);
    assert_eq!((g.id, g.name.as_str(), g.unit_label.as_str(), g.identifier.as_str()), (tb::gain::GAIN, "Gain", "x", "gain"));
    assert_eq!((g.min, g.max, g.default, g.step_count), (0.0, 4.0, 1.0, 0));
    assert_eq!(g.flags, PARAM_AUTOMATABLE);
    assert_eq!(mode.flags, PARAM_AUTOMATABLE | PARAM_STEPPED);
    assert_eq!((mode.step_count, mode.group.as_str()), (2, "Character"));
    assert_eq!(mode.value_strings, ["Clean", "Warm", "Hot"]);

    assert_eq!(gain.parameter(tb::gain::GAIN).unwrap(), Some(1.0));
    gain.set_parameter(tb::gain::GAIN, 0.25).unwrap();
    gain.set_parameter(tb::gain::MODE, 2.0).unwrap();
    assert_eq!(gain.parameter(tb::gain::GAIN).unwrap(), Some(0.25));
    assert_eq!(gain.parameter(777).unwrap(), None);
    assert!(matches!(gain.set_parameter(777, 1.0), Err(Error::Plugin(_))));
    let values = gain.parameter_values().unwrap();
    assert_eq!(values, [ParameterValue { id: 1, value: 0.25 }, ParameterValue { id: 2, value: 2.0 }]);
    // An untimed change is audible from the next block.
    let block = run(&gain, &[vec![1.0; 8], vec![1.0; 8]], 2, 8, &[]);
    assert_eq!(block.outputs[0], [0.25f32; 8]);

    assert_eq!(gain.format_parameter(tb::gain::GAIN, &[1.0, 0.5, 2.0]).unwrap(), ["0.0 dB", "-6.0 dB", "6.0 dB"]);
    assert_eq!(gain.format_parameter(tb::gain::MODE, &[0.0, 1.0, 2.0]).unwrap(), ["Clean", "Warm", "Hot"]);
    let parsed = gain.parse_parameter(tb::gain::GAIN, "-6 dB").unwrap().unwrap();
    assert!((parsed - 0.501187).abs() < 1e-5);
    assert_eq!(gain.parse_parameter(tb::gain::MODE, "Warm").unwrap(), Some(1.0));
    assert_eq!(gain.parse_parameter(tb::gain::MODE, "nonsense").unwrap(), None);

    let big = env.sup().create_instance(InstanceConfig { options: "size=1024".into(), ..effect("bigstate") }).unwrap();
    let list = big.parameters().unwrap();
    assert_eq!(list.len(), 64);
    assert_eq!(list[20].name, "Param 21");
    assert_eq!(list[20].group, "Bank 2/Page 2");
    assert_eq!(list[20].default, 20.0 / 64.0);
    assert_eq!(list[7].flags, PARAM_AUTOMATABLE | PARAM_BOOLEAN | PARAM_STEPPED);
    assert_eq!(list[7].step_count, 1);
    for id in 1..=64u64 {
        big.set_parameter(id, id as f64 * 0.001).unwrap();
    }
    let values = big.parameter_values().unwrap();
    assert_eq!(values.len(), 64);
    assert!(values.iter().all(|v| v.value == v.id as f64 * 0.001));
    env.finish();
}

#[test]
fn presets_and_plugin_side_notifications() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let gain = active(env.sup(), effect("gain"));
    let id = gain.id();
    let presets = gain.presets().unwrap();
    assert_eq!(presets.iter().map(|p| (p.id, p.name.as_str())).collect::<Vec<_>>(), [(0, "Unity"), (1, "Half"), (2, "Double")]);
    gain.load_preset(2).unwrap();
    assert_eq!(gain.parameter(tb::gain::GAIN).unwrap(), Some(2.0));
    env.wait_event("rescan", LONG, |e| *e == HostEvent::ParametersRescan { instance: id });
    assert!(matches!(gain.load_preset(9), Err(Error::Plugin(_))));
    // The preset's state became the last known state.
    assert_eq!(gain.last_known_state().unwrap()[..8], 2.0f64.to_le_bytes());

    // Editor: opened in the container, edits come back as gestures.
    let editor = gain.open_editor("Track 3 – Gain").unwrap();
    assert_eq!((editor.width, editor.height, editor.resizable), (400, 300, false));
    assert_eq!(editor.window_id, "Track 3 – Gain".len() as u64);
    assert_eq!(gain.editor_title().as_deref(), Some("Track 3 – Gain"));
    gain.focus_editor().unwrap();
    gain.set_editor_title("Renamed").unwrap();
    assert_eq!(gain.editor_title().as_deref(), Some("Renamed"));
    env.clear_events();
    gain.set_parameter(tb::gain::SIMULATE_EDIT, 0.125).unwrap();
    env.wait_event("gesture end", LONG, |e| *e == HostEvent::ParameterGesture { instance: id, id: 1, begin: false });
    let edits: Vec<HostEvent> = env.events().into_iter().filter(|e| !matches!(e, HostEvent::Log { .. })).collect();
    assert_eq!(
        edits,
        [
            HostEvent::ParameterGesture { instance: id, id: 1, begin: true },
            HostEvent::ParameterChanged { instance: id, id: 1, value: 0.125 },
            HostEvent::ParameterGesture { instance: id, id: 1, begin: false },
        ]
    );
    assert_eq!(gain.parameter(tb::gain::GAIN).unwrap(), Some(0.125));
    gain.set_parameter(tb::gain::SIMULATE_CLOSE, 1.0).unwrap();
    env.wait_event("editor closed", LONG, |e| *e == HostEvent::EditorClosed { instance: id });
    let deadline = Instant::now() + LONG;
    while gain.editor_title().is_some() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    gain.close_editor().unwrap();

    // A plug-in without an editor says so.
    let delay = env.sup().create_instance(effect("delay")).unwrap();
    assert!(matches!(delay.open_editor("x"), Err(Error::Unsupported)));
    assert!(delay.presets().unwrap().is_empty());
    assert!(matches!(delay.load_preset(0), Err(Error::Unsupported)));
    env.finish();
}

#[test]
fn state_of_several_megabytes_round_trips() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let size = 12 << 20;
    let instance = env.sup().create_instance(InstanceConfig { options: format!("size={size}"), ..effect("bigstate") }).unwrap();
    let started = Instant::now();
    let first = instance.state().unwrap();
    let get_time = started.elapsed();
    assert_eq!(first.len(), 64 * 8 + size);
    assert!(first[64 * 8..] == tb::bigstate::blob(1, size)[..]);
    // Change parameters and blob, read back, then restore the first state.
    instance.set_parameter(5, 0.875).unwrap();
    instance.set_parameter(tb::bigstate::SEED, 99.0).unwrap();
    let second = instance.state().unwrap();
    assert!(second[64 * 8..] == tb::bigstate::blob(99, size)[..]);
    assert_eq!(second[4 * 8..5 * 8], 0.875f64.to_le_bytes());
    assert!(first != second);
    let started = Instant::now();
    instance.set_state(&first).unwrap();
    let set_time = started.elapsed();
    assert!(instance.state().unwrap() == first);
    assert_eq!(instance.parameter(5).unwrap(), Some(4.0 / 64.0));
    assert!(instance.last_known_state().unwrap().as_ref() == &first);
    // An instance can also be created with a state.
    let other = env
        .sup()
        .create_instance(InstanceConfig { options: "size=16".into(), initial_state: Some(second.clone()), ..effect("bigstate") })
        .unwrap();
    assert!(other.state().unwrap() == second);
    // A state the plug-in refuses is an error, not a crash.
    assert!(matches!(instance.set_state(&[1, 2, 3]), Err(Error::Plugin(_))));
    assert!(instance.last_known_state().unwrap().as_ref() == &first);
    println!("state of {} MiB: get {get_time:?}, set {set_time:?}", size >> 20);
    env.finish();
}

#[test]
fn latency_is_reported_and_the_transport_adds_none() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), InstanceConfig { options: "latency=64".into(), ..effect("delay") });
    assert_eq!(instance.latency(), 64);
    assert_eq!(instance.info().latency, 64);
    assert_eq!(instance.tail(), 64);
    let mut rng = Rng(31);
    // The output is the input delayed by exactly the reported latency: an impulse train
    // through blocks of varying size comes out 64 frames later, not 64 + a block.
    let mut sent: Vec<f32> = Vec::new();
    let mut received: Vec<f32> = Vec::new();
    for frames in [1usize, 5, 64, 100, 128, 31, 512, 4096, 3] {
        let input = rng.signal(frames);
        let block = run(&instance, &[input.clone(), input.clone()], 2, frames, &[]);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        sent.extend_from_slice(&input);
        received.extend_from_slice(&block.outputs[0]);
    }
    assert!(received[..64].iter().all(|&s| s == 0.0));
    assert_bits_eq(&received[64..], &sent[..sent.len() - 64], "delayed by the reported latency exactly");

    // A latency change made by the plug-in is announced and visible to the audio thread.
    let id = instance.id();
    instance.set_parameter(tb::delay::FRAMES, 256.0).unwrap();
    env.wait_event("latency", LONG, |e| *e == HostEvent::LatencyChanged { instance: id, frames: 256 });
    env.wait_event("tail", LONG, |e| *e == HostEvent::TailChanged { instance: id, frames: 256 });
    assert_eq!(instance.latency(), 256);
    assert_eq!(instance.refresh_info().unwrap().latency, 256);
    // A plug-in without latency reports 0; with `copy` the same block comes back at once
    // (see the bit-exact test), so the round trip itself contributes nothing.
    let copy = active(env.sup(), effect("copy"));
    assert_eq!(copy.latency(), 0);
    env.finish();
}

#[test]
fn bypass_uses_the_plugin_or_a_latency_matched_dry_path() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let mut rng = Rng(41);
    // `gain` has its own bypass.
    let gain = active(env.sup(), effect("gain"));
    gain.set_parameter(tb::gain::GAIN, 3.0).unwrap();
    assert_eq!(gain.set_bypass(true).unwrap(), true);
    let input = vec![rng.signal(64), rng.signal(64)];
    let block = run(&gain, &input, 2, 64, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &input[0], "plug-in bypass");
    assert_eq!(gain.set_bypass(false).unwrap(), true);
    assert_eq!(run(&gain, &[vec![1.0; 4], vec![1.0; 4]], 2, 4, &[]).outputs[0], [3.0f32; 4]);

    // `delay` has none: the client bypasses, delaying the dry signal by the plug-in's
    // latency so the host's delay compensation stays valid. For a pure delay that is
    // indistinguishable from the plug-in itself, block boundaries included.
    let delay = active(
        env.sup(),
        InstanceConfig { options: "latency=100".into(), max_dry_delay_frames: 1024, ..effect("delay") },
    );
    let mut sent = Vec::new();
    let mut received = Vec::new();
    for (index, frames) in [64usize, 7, 300, 1, 4096, 50, 129, 64, 64].into_iter().enumerate() {
        if index == 3 {
            assert_eq!(delay.set_bypass(true).unwrap(), false, "client-side bypass");
        }
        let input = rng.signal(frames);
        let block = run(&delay, &[input.clone(), input.clone()], 2, frames, &[]);
        let expected = if index >= 3 { PROCESS_BYPASSED | PROCESS_FALLBACK } else { PROCESS_OK };
        assert_eq!(block.outcome.flags, expected);
        assert_bits_eq(&block.outputs[0], &block.outputs[1], "both channels");
        sent.extend_from_slice(&input);
        received.extend_from_slice(&block.outputs[0]);
    }
    assert!(received[..100].iter().all(|&s| s == 0.0));
    assert_bits_eq(&received[100..], &sent[..sent.len() - 100], "bypassed signal keeps the plug-in's latency");
    env.finish();
}

#[test]
fn reconfigure_changes_block_size_and_sample_rate() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), InstanceConfig { max_frames: 256, ..effect("probe") });
    assert_eq!(run(&instance, &[vec![0.0; 256]], 2, 256, &[]).outcome.flags, PROCESS_OK);
    assert_eq!(run(&instance, &[vec![0.0; 257]], 2, 257, &[]).outcome.flags, PROCESS_INVALID | PROCESS_FALLBACK);
    instance.reconfigure(96_000.0, 2048).unwrap();
    assert!(instance.is_active());
    let block = run(&instance, &[vec![0.0; 2048]], 2, 2048, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_eq!((block.outputs[0][0], block.outputs[0][11]), (2048.0, 96_000.0));
    assert!(matches!(instance.reconfigure(0.0, 64), Err(Error::Invalid(_))));
    // Shrinking works too, and the plug-in keeps answering.
    instance.reconfigure(44_100.0, 32).unwrap();
    assert_eq!(run(&instance, &[vec![0.0; 32]], 2, 32, &[]).outputs[0][11], 44_100.0);
    env.finish();
}

// ---- policies -----------------------------------------------------------------------------

#[test]
fn each_policy_yields_the_expected_number_of_processes() {
    // (manufacturer, plug-in): 6 instances, 2 manufacturers, 4 distinct plug-ins.
    let plan = [("Acme", "gain"), ("Acme", "gain"), ("Acme", "copy"), ("Bolt", "delay"), ("Bolt", "delay"), ("Bolt", "sine")];
    let mut all_pids = Vec::new();
    for (policy, expected) in [
        (GroupingPolicy::Shared, 1),
        (GroupingPolicy::PerManufacturer, 2),
        (GroupingPolicy::PerPlugin, 4),
        (GroupingPolicy::PerInstance, 6),
    ] {
        let env = TestEnv::new(policy);
        assert_eq!(env.sup().policy(), policy);
        let instances: Vec<Instance> = plan
            .iter()
            .map(|(vendor, plugin)| {
                let mut config = if *plugin == "sine" { instrument(plugin) } else { effect(plugin) };
                config.manufacturer = vendor.to_string();
                active(env.sup(), config)
            })
            .collect();
        let containers = env.sup().containers();
        assert_eq!(containers.len(), expected, "{policy:?}");
        let mut pids = env.live_pids();
        pids.sort();
        pids.dedup();
        assert_eq!(pids.len(), expected, "{policy:?}: distinct processes");
        assert!(pids.iter().all(|&pid| {
            std::fs::read_link(format!("/proc/{pid}/exe")).unwrap() == std::path::Path::new(CONTAINER)
        }));
        assert_eq!(containers.iter().map(|c| c.instances.len()).sum::<usize>(), 6);
        // Same plug-in, same container, exactly when the policy says so.
        let same = instances[0].container() == instances[1].container();
        assert_eq!(same, policy != GroupingPolicy::PerInstance);
        let cross_vendor = instances[0].container() == instances[3].container();
        assert_eq!(cross_vendor, policy == GroupingPolicy::Shared);
        let same_vendor_other_plugin = instances[0].container() == instances[2].container();
        assert_eq!(same_vendor_other_plugin, matches!(policy, GroupingPolicy::Shared | GroupingPolicy::PerManufacturer));
        // All of them process audio.
        let mut rng = Rng(51);
        for (instance, (_, plugin)) in instances.iter().zip(plan) {
            let input = vec![rng.signal(128), rng.signal(128)];
            let block = run(instance, &input, 2, 128, &[]);
            assert_eq!(block.outcome.flags, PROCESS_OK, "{policy:?} {plugin}");
        }
        // A container goes away with its last instance.
        drop(instances);
        assert!(env.sup().containers().is_empty());
        assert!(env.live_pids().is_empty());
        assert_eq!(env.pids.lock().unwrap().len(), expected);
        // They are children of this process, and they are all gone (reaped) afterwards.
        all_pids.extend(env.pids.lock().unwrap().iter().copied());
        env.finish();
    }
    assert_eq!(all_pids.len(), 13);
    let children = container_children();
    assert!(all_pids.iter().all(|pid| !children.contains(pid)), "no container process (or zombie) survives");
}

#[test]
fn policy_change_applies_to_new_instances_only() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let a = active(env.sup(), effect("gain"));
    let b = active(env.sup(), effect("copy"));
    assert_eq!(a.container(), b.container());
    env.sup().set_policy(GroupingPolicy::PerInstance);
    let c = active(env.sup(), effect("gain"));
    let d = active(env.sup(), effect("gain"));
    assert_eq!(a.container(), b.container());
    assert!(c.container() != a.container() && c.container() != d.container());
    assert_eq!(env.sup().containers().len(), 3);
    env.finish();
}

// ---- crashes --------------------------------------------------------------------------------

fn wait_status(instance: &Instance, status: InstanceStatus) {
    let deadline = Instant::now() + LONG;
    while instance.status() != status {
        assert!(Instant::now() < deadline, "status is {:?}, expected {status:?}", instance.status());
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Runs blocks until the instance reports `Lost`; returns how many it took.
fn blocks_until_lost(instance: &Instance, input: &[Vec<f32>]) -> usize {
    for n in 1..=2000 {
        let block = run(instance, input, 2, input[0].len(), &[]);
        if block.outcome.flags & PROCESS_LOST != 0 {
            return n;
        }
        assert!(block.outcome.fell_back() || block.outcome.processed());
        std::thread::sleep(Duration::from_micros(200));
    }
    panic!("instance never became lost");
}

#[test]
fn crash_in_process_is_contained_and_flagged_within_the_block() {
    let env = TestEnv::new(GroupingPolicy::PerPlugin);
    let crasher = active(env.sup(), effect("crash"));
    let sibling = active(env.sup(), effect("crash")); // same plug-in: same container
    let gain = active(env.sup(), effect("gain"));
    let synth = active(env.sup(), instrument("sine"));
    assert_eq!(crasher.container(), sibling.container());
    assert!(gain.container() != crasher.container() && synth.container() != crasher.container());
    assert_eq!(env.sup().containers().len(), 3);
    let crashed_container = crasher.container();
    let crashed_pid = env.sup().containers().iter().find(|c| c.id == crashed_container).unwrap().pid;
    let mut rng = Rng(61);
    let input = vec![rng.signal(256), rng.signal(256)];
    assert_eq!(run(&crasher, &input, 2, 256, &[]).outcome.flags, PROCESS_OK);
    assert_eq!(run(&synth, &[], 2, 256, &[Event::note_on(0, 0, 69, 100)]).outcome.flags, PROCESS_OK);

    // The block that carries the fatal note: the container dies while the host waits.
    // The call returns the dry signal, flagged, without waiting for the 10 s deadline.
    let block = run(&crasher, &input, 2, 256, &[Event::note_on(10, 0, tb::crash::NOTE_ABORT, 100)]);
    assert!(block.outcome.fell_back() && block.outcome.flags & (PROCESS_LOST | PROCESS_LATE) != 0, "{:?}", block.outcome);
    assert!(block.elapsed < Duration::from_secs(5), "returned after {:?}", block.elapsed);
    assert_bits_eq(&block.outputs[0], &input[0], "dry pass-through");
    assert_bits_eq(&block.outputs[1], &input[1], "dry pass-through");

    // From the next block on the instance is flagged lost and never waits again.
    let mut slowest = Duration::ZERO;
    for _ in 0..500 {
        let block = run(&crasher, &input, 2, 256, &[]);
        assert_eq!(block.outcome.flags, PROCESS_LOST | PROCESS_FALLBACK);
        assert_bits_eq(&block.outputs[0], &input[0], "dry pass-through");
        slowest = slowest.max(block.elapsed);
    }
    assert!(slowest < Duration::from_millis(20), "a lost instance must not wait (slowest block {slowest:?})");
    wait_status(&crasher, InstanceStatus::Lost);
    // The sibling in the same container is lost too, from its very next block.
    let block = run(&sibling, &input, 2, 256, &[]);
    assert_eq!(block.outcome.flags, PROCESS_LOST | PROCESS_FALLBACK);
    assert_eq!(sibling.status(), InstanceStatus::Lost);

    // Other containers never noticed: audio keeps flowing, sample-exact.
    for _ in 0..50 {
        let block = run(&gain, &input, 2, 256, &[]);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        assert_bits_eq(&block.outputs[0], &input[0], "gain 1.0");
        assert_eq!(run(&synth, &[], 2, 256, &[]).outcome.flags, PROCESS_OK);
    }
    assert_eq!(gain.stats().fallback_blocks + synth.stats().fallback_blocks, 0);

    // The report: which container, how it died, which instances went with it.
    let exited = env.wait_event("container exited", LONG, |e| matches!(e, HostEvent::ContainerExited { .. }));
    let mut lost_ids = vec![crasher.id(), sibling.id()];
    lost_ids.sort();
    match exited {
        HostEvent::ContainerExited { container, pid, reason, mut lost } => {
            lost.sort();
            assert_eq!((container, pid, reason), (crashed_container, crashed_pid, ExitReason::Crashed { signal: libc::SIGABRT }));
            assert_eq!(lost, lost_ids);
        }
        _ => unreachable!(),
    }
    for id in lost_ids {
        env.wait_event("instance lost", LONG, |e| {
            *e == HostEvent::InstanceLost { instance: id, container: crashed_container, reason: ExitReason::Crashed { signal: libc::SIGABRT } }
        });
    }
    // The same events are available by polling.
    let mut polled = Vec::new();
    while let Some(event) = env.sup().poll_event() {
        polled.push(event);
    }
    assert!(polled.iter().any(|e| matches!(e, HostEvent::ContainerExited { .. })));
    assert_eq!(polled.iter().filter(|e| matches!(e, HostEvent::InstanceLost { .. })).count(), 2);
    // Control calls on a lost instance fail cleanly.
    assert!(matches!(crasher.parameters(), Err(Error::InstanceLost)));
    assert!(matches!(crasher.state(), Err(Error::InstanceLost)));
    let info = env.sup().containers().into_iter().find(|c| c.id == crashed_container).unwrap();
    assert_eq!((info.state, info.pid, info.instances.len()), (ContainerState::Dead, 0, 2));
    assert!(!process_exists(crashed_pid), "the crashed process was reaped");
    env.finish();
}

#[test]
fn instrument_in_a_crashed_container_falls_silent() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let crasher = active(env.sup(), effect("crash"));
    let synth = active(env.sup(), instrument("sine"));
    let block = run(&synth, &[], 2, 128, &[Event::note_on(0, 0, 69, 100)]);
    assert!(block.outputs[0].iter().any(|&s| s != 0.0));
    // Segfault on the real-time thread this time.
    let input = vec![vec![0.25f32; 128], vec![0.25f32; 128]];
    let block = run(&crasher, &input, 2, 128, &[Event::param(0, tb::crash::PROCESS, 2.0)]);
    assert!(block.outcome.fell_back());
    let n = blocks_until_lost(&synth, &input);
    assert!(n <= 1000);
    let block = run(&synth, &[], 2, 128, &[]);
    assert_eq!(block.outcome.flags, PROCESS_LOST | PROCESS_FALLBACK);
    assert!(block.outputs.iter().flatten().all(|&s| s == 0.0), "a lost instrument is silent");
    env.wait_event("segfault reported", LONG, |e| {
        matches!(e, HostEvent::ContainerExited { reason: ExitReason::Crashed { signal }, .. } if *signal == libc::SIGSEGV)
    });
    env.finish();
}

#[test]
fn crash_on_the_main_thread_fails_the_request_and_is_classified() {
    let env = TestEnv::new(GroupingPolicy::PerInstance);
    let cases = [
        (1.0, ExitReason::Crashed { signal: libc::SIGABRT }),
        (2.0, ExitReason::Crashed { signal: libc::SIGSEGV }),
        (3.0, ExitReason::Exited { code: 7 }),
    ];
    for (value, expected) in cases {
        let instance = active(env.sup(), effect("crash"));
        let container = instance.container();
        let started = Instant::now();
        let result = instance.set_parameter(tb::crash::MAIN, value);
        assert!(matches!(result, Err(Error::ContainerDied)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "the request must fail fast, not time out");
        env.wait_event("exit", LONG, |e| {
            matches!(e, HostEvent::ContainerExited { container: c, reason, .. } if *c == container && *reason == expected)
        });
        assert_eq!(instance.status(), InstanceStatus::Lost);
    }
    // Crash while loading a state, and a plug-in that crashes while being instantiated.
    let instance = active(env.sup(), effect("crash"));
    assert!(matches!(instance.set_state(b"crash"), Err(Error::ContainerDied)));
    let result = env.sup().create_instance(InstanceConfig { options: "crash_on_create".into(), ..effect("crash") });
    assert!(matches!(result, Err(Error::ContainerDied)), "{:?}", result.err());
    // A plug-in that merely fails to load is an ordinary error and leaves no container.
    let before = env.sup().containers().len();
    let result = env.sup().create_instance(InstanceConfig { options: "fail_on_create".into(), ..effect("gain") });
    assert!(matches!(&result, Err(Error::Plugin(message)) if message.contains("fail_on_create")), "{:?}", result.err());
    assert!(matches!(env.sup().create_instance(effect("nonexistent")), Err(Error::Plugin(_))));
    assert!(matches!(env.sup().create_instance(InstanceConfig::effect("vst9", "x")), Err(Error::UnknownFormat(_))));
    assert_eq!(env.sup().containers().len(), before);
    env.finish();
}

// ---- deadline, hang, restart ------------------------------------------------------------------

#[test]
fn late_block_is_dropped_and_the_instance_recovers_by_itself() {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::Shared;
        c.hang_timeout = Duration::from_secs(30);
    });
    let instance = active(env.sup(), InstanceConfig { deadline: Some(Duration::from_millis(40)), ..effect("hang") });
    let mut rng = Rng(71);
    let input = vec![rng.signal(128), rng.signal(128)];
    assert_eq!(run(&instance, &input, 2, 128, &[]).outcome.flags, PROCESS_OK);
    // The plug-in stalls for 400 ms inside this block: the host waits 40 ms, not 400.
    let block = run(&instance, &input, 2, 128, &[Event::param(0, tb::hang::PROCESS_SLEEP_MS, 400.0)]);
    assert_eq!(block.outcome.flags, PROCESS_LATE | PROCESS_FALLBACK);
    assert!(block.elapsed >= Duration::from_millis(40) && block.elapsed < Duration::from_millis(300), "{:?}", block.elapsed);
    assert_bits_eq(&block.outputs[0], &input[0], "dry signal while late");
    assert_eq!(instance.status(), InstanceStatus::Late);
    // While the container is still busy, blocks return at once.
    let mut late_blocks = 0;
    let started = Instant::now();
    loop {
        let block = run(&instance, &input, 2, 128, &[]);
        if block.outcome.flags == PROCESS_LATE | PROCESS_FALLBACK {
            assert!(block.elapsed < Duration::from_millis(20), "a late instance must not wait again: {:?}", block.elapsed);
            late_blocks += 1;
            std::thread::sleep(Duration::from_millis(1));
        } else {
            // Caught up: this block is processed again and marked as following a gap.
            assert_eq!(block.outcome.flags, PROCESS_OK | PROCESS_RESYNCED);
            assert_bits_eq(&block.outputs[0], &input[0], "processing resumed");
            break;
        }
        assert!(started.elapsed() < LONG);
    }
    assert!(late_blocks > 10, "only {late_blocks} late blocks");
    assert_eq!(instance.status(), InstanceStatus::Running);
    assert_eq!(run(&instance, &input, 2, 128, &[]).outcome.flags, PROCESS_OK);
    let stats = instance.stats();
    assert_eq!((stats.deadline_misses, stats.resyncs), (1, 1));
    assert_eq!(stats.fallback_blocks, late_blocks + 1);
    env.wait_event("deadline missed", LONG, |e| *e == HostEvent::DeadlineMissed { instance: instance.id() });
    assert!(!env.events().iter().any(|e| matches!(e, HostEvent::InstanceHung { .. } | HostEvent::InstanceLost { .. })));
    env.finish();
}

#[test]
fn hang_in_process_is_detected_killed_and_restorable() {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::PerPlugin;
        c.hang_timeout = Duration::from_millis(400);
    });
    let hanger = active(env.sup(), InstanceConfig { deadline: Some(Duration::from_millis(50)), ..effect("hang") });
    let gain = active(env.sup(), effect("gain"));
    let container = hanger.container();
    let old_pid = env.live_pids();
    let mut rng = Rng(81);
    let input = vec![rng.signal(64), rng.signal(64)];
    assert_eq!(run(&hanger, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);

    let started = Instant::now();
    let block = run(&hanger, &input, 2, 64, &[Event::note_on(0, 0, tb::hang::NOTE_FOREVER, 1)]);
    assert_eq!(block.outcome.flags, PROCESS_LATE | PROCESS_FALLBACK);
    assert!(block.elapsed >= Duration::from_millis(50) && block.elapsed < Duration::from_millis(350), "{:?}", block.elapsed);
    // The audio thread keeps running at full speed; the instance is flagged throughout.
    let mut slowest = Duration::ZERO;
    while hanger.status() != InstanceStatus::Lost {
        let block = run(&hanger, &input, 2, 64, &[]);
        assert!(block.outcome.fell_back() && block.outcome.flags & (PROCESS_LATE | PROCESS_LOST) != 0);
        assert_bits_eq(&block.outputs[1], &input[1], "dry signal");
        slowest = slowest.max(block.elapsed);
        assert_eq!(run(&gain, &input, 2, 64, &[]).outcome.flags, PROCESS_OK, "other containers are unaffected");
        assert!(started.elapsed() < LONG, "the hung container was never killed");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(slowest < Duration::from_millis(20), "slowest block while hung: {slowest:?}");
    let detected = started.elapsed();
    assert!(detected >= Duration::from_millis(400) && detected < Duration::from_secs(5), "hang detected after {detected:?}");
    let id = hanger.id();
    env.wait_event("hung", LONG, |e| *e == HostEvent::InstanceHung { instance: id, container });
    env.wait_event("lost", LONG, |e| *e == HostEvent::InstanceLost { instance: id, container, reason: ExitReason::Hung });
    env.wait_event("exited", LONG, |e| {
        matches!(e, HostEvent::ContainerExited { container: c, reason: ExitReason::Hung, .. } if *c == container)
    });
    assert_eq!(run(&hanger, &input, 2, 64, &[]).outcome.flags, PROCESS_LOST | PROCESS_FALLBACK);

    // Restart on request: a new process, the instance active again.
    let report = env.sup().restart_container(container).unwrap();
    assert_eq!(report, RestartReport { restored: vec![id], failed: vec![] });
    env.wait_event("restored", LONG, |e| *e == HostEvent::InstanceRestored { instance: id, container });
    assert_eq!(hanger.status(), InstanceStatus::Running);
    let block = run(&hanger, &input, 2, 64, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &input[0], "processing after restart");
    let new_pid = env.live_pids();
    assert_eq!(new_pid.len(), 2);
    assert!(new_pid.iter().filter(|p| !old_pid.contains(p)).count() == 1, "{old_pid:?} -> {new_pid:?}");
    env.finish();
}

#[test]
fn hung_main_thread_times_out_requests_but_audio_continues() {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::Shared;
        c.request_timeout = Duration::from_millis(300);
    });
    let hanger = active(env.sup(), effect("hang"));
    let gain = active(env.sup(), effect("gain"));
    let mut rng = Rng(91);
    let input = vec![rng.signal(64), rng.signal(64)];
    // A slow main thread delays requests without failing them.
    hanger.set_parameter(tb::hang::MAIN_SLEEP_MS, 100.0).unwrap();
    // A hung one: the request times out...
    let started = Instant::now();
    let result = hanger.set_parameter(tb::hang::MAIN_FOREVER, 1.0);
    assert!(matches!(result, Err(Error::Timeout(_))), "{result:?}");
    assert!(started.elapsed() >= Duration::from_millis(300) && started.elapsed() < Duration::from_secs(3));
    // ...the process is alive (its I/O thread answers) but its main thread is not...
    assert!(hanger.ping(false, Duration::from_secs(5)).is_ok());
    assert!(matches!(hanger.ping(true, Duration::from_millis(200)), Err(Error::Timeout(_))));
    assert!(matches!(gain.parameter(tb::gain::GAIN), Err(Error::Timeout(_))));
    // ...and audio of every instance in the container still runs: real-time threads do
    // not depend on the main thread.
    for _ in 0..50 {
        assert_eq!(run(&hanger, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);
        let block = run(&gain, &input, 2, 64, &[]);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        assert_bits_eq(&block.outputs[0], &input[0], "gain");
    }
    // The host decides: kill the container, then bring everything back.
    let container = hanger.container();
    env.sup().kill_container(container).unwrap();
    env.wait_event("killed", LONG, |e| {
        matches!(e, HostEvent::ContainerExited { reason: ExitReason::Killed, lost, .. } if lost.len() == 2)
    });
    let report = env.sup().restart_container(container).unwrap();
    assert_eq!(report.restored.len(), 2);
    assert_eq!(gain.parameter(tb::gain::GAIN).unwrap(), Some(1.0));
    assert!(hanger.ping(true, Duration::from_secs(5)).is_ok());
    assert_eq!(run(&gain, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);
    env.finish();
}

#[test]
fn offline_mode_has_no_deadline() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), InstanceConfig { deadline: Some(Duration::from_millis(20)), ..effect("hang") });
    let input = vec![vec![0.5f32; 64], vec![0.25f32; 64]];
    instance.set_offline(true).unwrap();
    // 300 ms inside the plug-in against a 20 ms deadline: offline, the block is waited for.
    let block = run(&instance, &input, 2, 64, &[Event::param(0, tb::hang::PROCESS_SLEEP_MS, 300.0)]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert!(block.elapsed >= Duration::from_millis(300));
    assert!(block.outcome.plugin_ns >= 300_000_000);
    assert_bits_eq(&block.outputs[0], &input[0], "processed");
    instance.set_offline(false).unwrap();
    let block = run(&instance, &input, 2, 64, &[Event::param(0, tb::hang::PROCESS_SLEEP_MS, 300.0)]);
    assert_eq!(block.outcome.flags, PROCESS_LATE | PROCESS_FALLBACK);
    // A container that dies during an offline block still releases the waiting thread.
    let crasher = active(env.sup(), effect("crash"));
    crasher.set_offline(true).unwrap();
    let started = Instant::now();
    let block = run(&crasher, &input, 2, 64, &[Event::note_on(0, 0, tb::crash::NOTE_SEGFAULT, 1)]);
    assert!(block.outcome.fell_back());
    assert!(started.elapsed() < Duration::from_secs(5));
    env.finish();
}

#[test]
fn restart_restores_state_parameters_and_activation() {
    let env = TestEnv::new(GroupingPolicy::PerPlugin);
    let size = 2 << 20;
    let a = active(env.sup(), InstanceConfig { options: format!("size={size}"), ..effect("bigstate") });
    let b = env.sup().create_instance(InstanceConfig { options: format!("size={size}"), ..effect("bigstate") }).unwrap();
    let gain = active(env.sup(), effect("gain"));
    assert_eq!(a.container(), b.container());
    let container = a.container();
    let pid_before = env.sup().containers().iter().find(|c| c.id == container).unwrap().pid;

    // a: a state the host fetched, then two parameter edits the state does not contain.
    a.set_parameter(tb::bigstate::SEED, 1234.0).unwrap();
    let a_state = a.state().unwrap();
    a.set_parameter(9, 0.5625).unwrap();
    a.set_parameter(10, 0.125).unwrap();
    // b: a state the host supplied.
    let mut b_state = vec![0u8; 64 * 8];
    b_state.extend_from_slice(&tb::bigstate::blob(77, 5000));
    b.set_state(&b_state).unwrap();
    gain.set_parameter(tb::gain::GAIN, 0.5).unwrap();

    env.sup().kill_container(container).unwrap();
    let input = vec![vec![1.0f32; 32], vec![1.0f32; 32]];
    blocks_until_lost(&a, &input);
    wait_status(&b, InstanceStatus::Lost);
    // The last known state survives the process; a state set while lost is kept for the restore.
    assert!(a.last_known_state().unwrap().as_ref() == &a_state);
    assert_eq!(run(&gain, &input, 2, 32, &[]).outputs[0], [0.5f32; 32]);

    let report = env.sup().restart_container(container).unwrap();
    let mut restored = report.restored.clone();
    restored.sort();
    assert_eq!((restored, report.failed.len()), (vec![a.id(), b.id()], 0));
    let pid_after = env.sup().containers().iter().find(|c| c.id == container).unwrap().pid;
    assert!(pid_after != 0 && pid_after != pid_before);

    // a: fetched state + replayed edits. b: supplied state.
    let now = a.state().unwrap();
    assert!(now[64 * 8..] == a_state[64 * 8..], "blob restored");
    assert_eq!(a.parameter(9).unwrap(), Some(0.5625));
    assert_eq!(a.parameter(10).unwrap(), Some(0.125));
    assert_eq!(a.parameter(11).unwrap(), Some(10.0 / 64.0));
    assert!(b.state().unwrap() == b_state);
    // Activation is restored too: a processes, b (never activated) does not.
    assert!(a.is_active() && !b.is_active());
    assert_eq!(run(&a, &input, 2, 32, &[]).outcome.flags, PROCESS_OK);
    assert_eq!(run(&b, &input, 2, 32, &[]).outcome.flags, PROCESS_INACTIVE | PROCESS_FALLBACK);
    // Restoring a healthy instance is a no-op; a single lost instance can be restored alone.
    a.restore().unwrap();
    env.sup().kill_container(container).unwrap();
    blocks_until_lost(&a, &input);
    b.set_state(&a_state).unwrap(); // while lost: applied by the restore
    b.restore().unwrap();
    assert!(b.state().unwrap() == a_state);
    assert_eq!(a.status(), InstanceStatus::Lost, "restoring b does not restore a");
    a.restore().unwrap();
    assert_eq!(a.parameter(9).unwrap(), Some(0.5625));
    env.finish();
}

#[test]
fn periodic_state_snapshots_make_unsaved_edits_survive_a_crash() {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::PerInstance;
        c.auto_state_interval = Some(Duration::from_millis(40));
    });
    let instance = active(env.sup(), InstanceConfig { options: "size=4096".into(), ..effect("bigstate") });
    // Changed inside the plug-in (think: the user turned knobs in its editor); the host
    // never asks for the state.
    instance.set_parameter(tb::bigstate::SEED, 4242.0).unwrap();
    let expected = tb::bigstate::blob(4242, 4096);
    let deadline = Instant::now() + LONG;
    while instance.last_known_state().map_or(true, |s| s[64 * 8..] != expected[..]) {
        assert!(Instant::now() < deadline, "no automatic snapshot");
        std::thread::sleep(Duration::from_millis(5));
    }
    env.sup().kill_container(instance.container()).unwrap();
    blocks_until_lost(&instance, &[vec![0.0; 16], vec![0.0; 16]]);
    instance.restore().unwrap();
    assert!(instance.state().unwrap()[64 * 8..] == expected[..]);
    env.finish();
}

#[test]
fn restore_failure_is_reported_and_leaves_the_instance_lost() {
    let env = TestEnv::new(GroupingPolicy::PerInstance);
    // Activation crashes: creating works, activating kills the container, restoring
    // (which re-activates... no: the instance never became active) works again.
    let instance = env.sup().create_instance(InstanceConfig { options: "crash_on_activate".into(), ..effect("gain") }).unwrap();
    assert!(matches!(instance.activate(), Err(Error::ContainerDied)));
    env.wait_event("lost", LONG, |e| matches!(e, HostEvent::InstanceLost { .. }));
    assert_eq!(instance.status(), InstanceStatus::Lost);
    assert!(!instance.is_active());
    instance.restore().unwrap();
    assert_eq!(instance.status(), InstanceStatus::Running);
    // A restore that cannot work: the saved state makes the plug-in crash on load.
    let bad = env.sup().create_instance(effect("crash")).unwrap();
    env.sup().kill_container(bad.container()).unwrap();
    env.wait_event("killed", LONG, |e| matches!(e, HostEvent::ContainerExited { reason: ExitReason::Killed, .. }));
    bad.set_state(b"crash").unwrap(); // accepted while lost, applied on restore
    let result = bad.restore();
    assert!(matches!(result, Err(Error::ContainerDied)), "{result:?}");
    env.wait_event("restore failed", LONG, |e| matches!(e, HostEvent::InstanceRestoreFailed { instance, .. } if *instance == bad.id()));
    assert_eq!(bad.status(), InstanceStatus::Lost);
    let report = env.sup().restart_container(bad.container()).unwrap();
    assert_eq!((report.restored.len(), report.failed.len()), (0, 1));
    env.finish();
}

// ---- start-up, handshake, shutdown ------------------------------------------------------------

#[test]
fn slow_instantiation_does_not_disturb_audio_and_can_time_out() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let gain = Arc::new(active(env.sup(), effect("gain")));
    // Audio of an instance in the same container while another one takes 1.2 s to load.
    let stop = Arc::new(AtomicBool::new(false));
    let audio = {
        let (gain, stop) = (gain.clone(), stop.clone());
        std::thread::spawn(move || {
            let input = vec![vec![0.5f32; 128], vec![0.5f32; 128]];
            let mut blocks = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let block = run(&gain, &input, 2, 128, &[]);
                assert_eq!(block.outcome.flags, PROCESS_OK);
                blocks += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
            blocks
        })
    };
    let started = Instant::now();
    let slow = env.sup().create_instance(InstanceConfig { options: "ms=1200".into(), ..effect("slow") }).unwrap();
    let took = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let blocks = audio.join().unwrap();
    assert!(took >= Duration::from_millis(1200));
    assert!(blocks > 200, "audio ran during the slow instantiation ({blocks} blocks)");
    slow.activate().unwrap();
    assert_eq!(run(&slow, &[vec![1.0; 8], vec![1.0; 8]], 2, 8, &[]).outcome.flags, PROCESS_OK);
    drop(slow);
    env.finish();

    // With a short creation timeout the caller gets an error; the container is not
    // blocked for good and the half-made instance is cleaned up behind the caller's back.
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::Shared;
        c.create_timeout = Duration::from_millis(200);
    });
    let gain = active(env.sup(), effect("gain"));
    let result = env.sup().create_instance(InstanceConfig { options: "ms=800".into(), ..effect("slow") });
    assert!(matches!(result, Err(Error::Timeout(_))), "{:?}", result.err());
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(gain.parameter(tb::gain::GAIN).unwrap(), Some(1.0));
    assert_eq!(env.sup().containers()[0].instances.len(), 1);
    env.finish();
}

#[test]
fn protocol_version_mismatch_is_rejected() {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::Shared;
        c.container_env = vec![("STPC_TEST_PROTOCOL_VERSION".into(), "999".into())];
    });
    let result = env.sup().create_instance(effect("gain"));
    match result {
        Err(Error::Rejected(reason)) => {
            assert!(reason.contains("protocol version mismatch") && reason.contains("999"), "{reason}");
        }
        other => panic!("expected a rejection, got {:?}", other.err()),
    }
    assert!(env.sup().containers().is_empty());
    assert!(container_children().iter().all(|pid| {
        std::fs::read_link(format!("/proc/{pid}/exe")).map_or(true, |exe| exe != std::path::Path::new(CONTAINER))
            || !env.pids.lock().unwrap().contains(pid)
    }));
    env.finish();
}

#[test]
fn container_binary_refuses_bad_invocations_and_exits_when_the_host_goes_away() {
    use std::process::{Command, Stdio};
    use stpc_transport::channel::{FrameKind, Listener};
    use stpc_transport::messages::{Hello, Welcome};
    use stpc_transport::wire::Wire;

    // No arguments: usage error.
    let status = Command::new(CONTAINER).stderr(Stdio::null()).status().unwrap();
    assert_eq!(status.code(), Some(2));
    // An endpoint nobody listens on.
    let status = Command::new(CONTAINER)
        .args(["--stpc-endpoint", "/nonexistent/stpc.sock", "--stpc-token", "t"])
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(4));

    let dir = std::env::temp_dir().join(format!("stpc-manual-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spawn = |name: &str| {
        let listener = Listener::bind(&dir, name).unwrap();
        let child = Command::new(CONTAINER)
            .args(["--stpc-endpoint", &listener.endpoint(), "--stpc-token", "secret"])
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stream = listener.accept(LONG, || true).unwrap();
        let frame = stream.recv().unwrap();
        assert_eq!(frame.kind, FrameKind::Handshake);
        let hello = Hello::decode(&frame.payload).unwrap();
        (child, stream, hello)
    };
    // The handshake carries what the host checks.
    let (mut child, stream, hello) = spawn("a.sock");
    assert_eq!((hello.magic, hello.protocol_version), (stpc_transport::HELLO_MAGIC, PROTOCOL_VERSION));
    assert_eq!((hello.pid, hello.token.as_str()), (child.id(), "secret"));
    assert_eq!(hello.formats, ["test"]);
    // Refused by the host: the container exits with its "rejected" code.
    let refusal = Welcome { accepted: false, protocol_version: PROTOCOL_VERSION, reason: "wrong connection token".into() };
    stream.send(FrameKind::Handshake, 0, &refusal.encode(), &[]).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(3));
    // A host that speaks another protocol version is refused by the container as well.
    let (mut child, stream, _) = spawn("b.sock");
    let other = Welcome { accepted: true, protocol_version: PROTOCOL_VERSION + 1, reason: String::new() };
    stream.send(FrameKind::Handshake, 0, &other.encode(), &[]).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(3));
    // Accepted, then the host disappears (crash): the container must not linger.
    let (mut child, stream, _) = spawn("c.sock");
    let welcome = Welcome { accepted: true, protocol_version: PROTOCOL_VERSION, reason: String::new() };
    stream.send(FrameKind::Handshake, 0, &welcome.encode(), &[]).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(child.try_wait().unwrap().is_none(), "the container runs while the host is connected");
    drop(stream);
    let started = Instant::now();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "listeners remove their socket paths");
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn shutdown_with_live_instances_is_clean() {
    let env = TestEnv::new(GroupingPolicy::PerInstance);
    let instances: Vec<Instance> = (0..5).map(|_| active(env.sup(), effect("gain"))).collect();
    let big = env.sup().create_instance(InstanceConfig { options: "size=1048576".into(), ..effect("bigstate") }).unwrap();
    assert_eq!(env.live_pids().len(), 6);
    assert_eq!(container_children().iter().filter(|p| env.pids.lock().unwrap().contains(p)).count(), 6);
    // While containers run, the socket directory holds no socket files (they are
    // unlinked as soon as the container has connected).
    assert_eq!(std::fs::read_dir(env.sup().socket_dir()).unwrap().count(), 0);
    env.sup().shutdown();
    let input = vec![vec![0.5f32; 16], vec![0.5f32; 16]];
    for instance in &instances {
        assert_eq!(instance.status(), InstanceStatus::Closed);
        let block = run(instance, &input, 2, 16, &[]);
        assert_eq!(block.outcome.flags, PROCESS_LOST | PROCESS_FALLBACK);
        assert!(matches!(instance.parameters(), Err(Error::Closed)));
    }
    assert!(matches!(env.sup().create_instance(effect("gain")), Err(Error::Closed)));
    assert!(matches!(big.state(), Err(Error::Closed)));
    // A regular shutdown is not reported as a loss.
    assert!(!env.events().iter().any(|e| matches!(e, HostEvent::InstanceLost { .. })));
    assert_eq!(
        env.events().iter().filter(|e| matches!(e, HostEvent::ContainerExited { reason: ExitReason::Shutdown, .. })).count(),
        6
    );
    let pids = env.pids.lock().unwrap().clone();
    assert_clean(&env.base, &pids);
    drop(instances);
    drop(big);
    env.finish();
}

#[test]
fn stale_socket_directories_of_dead_hosts_are_swept() {
    let base = std::env::temp_dir().join(format!("stpc-sweep-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    // A pid that certainly does not exist, and one that does (ours: must be left alone).
    let stale = base.join("stpc-999999999-dead");
    let own = base.join(format!("stpc-{}-other", std::process::id()));
    let unrelated = base.join("something-else");
    for dir in [&stale, &own, &unrelated] {
        std::fs::create_dir(dir).unwrap();
    }
    std::fs::write(stale.join("c1-1.sock"), b"").unwrap();
    let mut config = base_config();
    config.socket_dir = Some(base.clone());
    let sup = Supervisor::new(config).unwrap();
    assert!(!stale.exists(), "stale directory removed");
    assert!(own.exists() && unrelated.exists());
    assert!(sup.socket_dir().starts_with(&base));
    drop(sup);
    std::fs::remove_dir(&own).unwrap();
    std::fs::remove_dir(&unrelated).unwrap();
    std::fs::remove_dir(&base).unwrap();
}

#[test]
fn missing_container_binary_is_a_clear_error() {
    let mut config = base_config();
    config.container_path = Some("/nonexistent/supertracker-plugin-container".into());
    assert!(matches!(Supervisor::new(config), Err(Error::Spawn(message)) if message.contains("does not exist")));
    // A file that is not a container: it never connects, the spawn times out or aborts.
    let mut config = base_config();
    config.container_path = Some("/bin/true".into());
    config.spawn_timeout = Duration::from_secs(5);
    let sup = Supervisor::new(config).unwrap();
    let started = Instant::now();
    assert!(matches!(sup.create_instance(effect("gain")), Err(Error::Spawn(_))));
    assert!(started.elapsed() < Duration::from_secs(4), "an exited child aborts the wait");
    assert!(sup.containers().is_empty());
}

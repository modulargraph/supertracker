//! The VST3 backend, end to end: host process -> container process -> a real VST3 module
//! (`stpc-vst3-test-plugin`, built by cargo as a dependency of these tests and packaged
//! here as a `.vst3` bundle).
#![allow(clippy::bool_assert_comparison, clippy::needless_range_loop)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use stpc_container::vst3::module::{cid_words, hex_to_cid};
use stpc_container::vst3::state;
use stpc_vst3_test_plugin as plug;

/// A bundle of the test plug-in in a directory of its own.
struct Bundle {
    _scratch: Scratch,
    path: String,
}

fn bundle() -> Bundle {
    let scratch = Scratch::new("vst3");
    let path = vst3_bundle(&scratch.dir, "STTest").to_string_lossy().into_owned();
    Bundle { _scratch: scratch, path }
}

fn vst3(config: InstanceConfig, bundle: &Bundle, id: &str) -> InstanceConfig {
    InstanceConfig {
        format: "vst3".into(),
        plugin_id: id.into(),
        options: bundle.path.clone(),
        deadline: Some(LONG),
        manufacturer: plug::VENDOR.into(),
        ..config
    }
}

fn gain_config(bundle: &Bundle) -> InstanceConfig {
    vst3(InstanceConfig::effect("vst3", ""), bundle, plug::GAIN_CLASS_ID)
}

fn sine_config(bundle: &Bundle) -> InstanceConfig {
    vst3(InstanceConfig::instrument("vst3", ""), bundle, plug::SINE_CLASS_ID)
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + LONG;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

const GAIN: u64 = plug::PARAM_GAIN as u64;

fn scaled(input: &[f32], normalized: f64) -> Vec<f32> {
    input.iter().map(|s| s * plug::gain_factor(normalized)).collect()
}

/// The gain value inside a state blob (component state: magic, version, gain, ...).
fn gain_in_state(blob: &[u8]) -> f64 {
    let class = cid_words(&hex_to_cid(plug::GAIN_CLASS_ID).unwrap());
    let parts = state::decode(class, blob).expect("a state of the gain effect");
    f64::from_le_bytes(parts.component[8..16].try_into().unwrap())
}

#[test]
fn gain_effect_metadata_audio_parameters_and_state() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let bundle = bundle();
    let instance = env.sup().create_instance(gain_config(&bundle)).unwrap();
    let id = instance.id();

    let info = instance.info();
    // The class states no vendor: the factory's applies.
    assert_eq!(
        (info.name.as_str(), info.manufacturer.as_str(), info.version.as_str()),
        (plug::GAIN_NAME, plug::VENDOR, plug::VERSION)
    );
    assert_eq!(info.flags, INFO_HAS_BYPASS | if cfg!(target_os = "macos") { INFO_HAS_EDITOR } else { 0 });
    assert_eq!((info.inputs.len(), info.outputs.len()), (1, 1));
    assert_eq!(
        (info.inputs[0].name.as_str(), info.inputs[0].channels, info.inputs[0].auxiliary),
        ("Audio In", 2, false)
    );
    assert_eq!((info.outputs[0].name.as_str(), info.outputs[0].channels), ("Audio Out", 2));
    assert_eq!((info.latency, info.tail, info.parameter_count), (0, 0, 21));

    // Parameter ids are the plug-in's ParamIDs; values are normalized.
    let params = instance.parameters().unwrap();
    assert_eq!(
        params.iter().map(|p| p.id).collect::<Vec<_>>(),
        [100, 101, 102, 103, 104, 105, 110, 120, 121, 200, 201, 210, 211, 212, 213, 214, 215, 216, 217, 220, 221]
    );
    let by_id = |id: u32| params.iter().find(|p| p.id == id as u64).unwrap();
    let gain = by_id(plug::PARAM_GAIN);
    assert_eq!(
        (gain.name.as_str(), gain.short_name.as_str(), gain.unit_label.as_str(), gain.group.as_str()),
        ("Gain", "Gn", "x", "")
    );
    assert_eq!((gain.min, gain.max, gain.default, gain.flags, gain.step_count), (0.0, 1.0, 0.5, PARAM_AUTOMATABLE, 0));
    assert_eq!(by_id(plug::PARAM_BYPASS).flags, PARAM_AUTOMATABLE | PARAM_STEPPED | PARAM_BOOLEAN | PARAM_BYPASS);
    assert_eq!(by_id(plug::PARAM_LATENCY).flags, 0, "not automatable");
    let crash = by_id(plug::PARAM_CRASH);
    assert_eq!(crash.value_strings, ["No", "Abort", "Segfault"]);
    assert_eq!((crash.step_count, crash.flags, crash.group.as_str()), (2, PARAM_STEPPED, "Test"));
    assert_eq!(by_id(plug::PARAM_PROGRAM).value_strings, ["Unity", "Half", "Double"]);
    assert_eq!(by_id(plug::PARAM_UI_ZOOM).flags, PARAM_HIDDEN);
    assert_eq!(by_id(plug::PARAM_METER).flags, PARAM_READ_ONLY);
    let probe = by_id(plug::PARAM_OFFLINE);
    assert_eq!(
        (probe.group.as_str(), probe.flags),
        ("Test/Probe", PARAM_READ_ONLY | PARAM_HIDDEN | PARAM_STEPPED | PARAM_BOOLEAN)
    );
    // Text both ways goes through the edit controller.
    assert_eq!(instance.format_parameter(GAIN, &[0.25, 1.0]).unwrap(), ["0.50 x", "2.00 x"]);
    assert_eq!(instance.parse_parameter(GAIN, "1.25 x").unwrap(), Some(0.625));
    assert_eq!(instance.parse_parameter(GAIN, "loud").unwrap(), None);
    assert_eq!(instance.parameter(999).unwrap(), None);
    assert!(matches!(instance.set_parameter(999, 0.5), Err(Error::Plugin(_))));
    assert_eq!(instance.parameter(GAIN).unwrap(), Some(0.5));

    // Not active yet: the controller shows the change at once, the processor gets it with
    // its first block.
    instance.set_parameter(GAIN, 0.25).unwrap();
    assert_eq!(instance.parameter(GAIN).unwrap(), Some(0.25));
    instance.activate().unwrap();
    let mut rng = Rng(0x7357);
    let input = vec![rng.signal(256), rng.signal(256)];
    let block = run(&instance, &input, 2, 256, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    for channel in 0..2 {
        assert_bits_eq(&block.outputs[channel], &scaled(&input[channel], 0.25), "gain 0.5");
    }
    // The output parameter (peak meter) comes back as a change made by the plug-in, and
    // the controller is told too.
    let peak = scaled(&input[0], 0.25).iter().fold(0.0f32, |peak, s| peak.max(s.abs())) as f64;
    env.wait_event("meter", LONG, |e| {
        *e == HostEvent::ParameterChanged { instance: id, id: plug::PARAM_METER as u64, value: peak }
    });
    wait_until("meter in the controller", || instance.parameter(plug::PARAM_METER as u64).unwrap() == Some(peak));

    // Sample-accurate automation through the block's events; the controller follows.
    let events = [Event::param(100, GAIN, 1.0), Event::param(200, GAIN, 0.5)];
    let block = run(&instance, &input, 2, 256, &events);
    let expected: Vec<f32> = (0..256)
        .map(|i| {
            input[0][i]
                * plug::gain_factor(if i < 100 {
                    0.25
                } else if i < 200 {
                    1.0
                } else {
                    0.5
                })
        })
        .collect();
    assert_bits_eq(&block.outputs[0], &expected, "automation");
    wait_until("automation in the controller", || instance.parameter(GAIN).unwrap() == Some(0.5));
    assert!(
        !env.events().iter().any(|e| matches!(e, HostEvent::ParameterChanged { id: GAIN, .. })),
        "the host's own automation is not reported back to it"
    );
    // Active: an untimed change travels with the next block and applies from its start.
    instance.set_parameter(GAIN, 0.125).unwrap();
    assert_eq!(instance.parameter(GAIN).unwrap(), Some(0.125));
    let block = run(&instance, &input, 2, 256, &[]);
    assert_bits_eq(&block.outputs[1], &scaled(&input[1], 0.125), "queued change");
    // Block sizes from 1 to 4096, changing per call.
    instance.set_parameter(GAIN, 0.5).unwrap();
    for frames in [1usize, 4096, 17, 512, 3, 1024] {
        let input = vec![rng.signal(frames), rng.signal(frames)];
        let block = run(&instance, &input, 2, frames, &[]);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        assert_bits_eq(&block.outputs[0], &input[0], "unity gain");
        assert_bits_eq(&block.outputs[1], &input[1], "unity gain");
    }
    let values = instance.parameter_values().unwrap();
    assert_eq!(values.len(), 21);
    assert_eq!(values[0], ParameterValue { id: GAIN, value: 0.5 });

    // State: component and controller state in the backend's container.
    instance.set_parameter(GAIN, 0.375).unwrap();
    instance.set_parameter(plug::PARAM_UI_ZOOM as u64, 0.75).unwrap();
    run(&instance, &input, 2, 64, &[]);
    let saved = instance.state().unwrap();
    assert_eq!(saved.len(), state::HEADER_LEN + plug::GAIN_STATE_LEN + plug::GAIN_CONTROLLER_STATE_LEN);
    assert_eq!(&saved[..8], b"STV3\x01\x00\x01\x00");
    let parts = state::decode(cid_words(&hex_to_cid(plug::GAIN_CLASS_ID).unwrap()), &saved).unwrap();
    assert_eq!((&parts.component[..4], &parts.controller.unwrap()[..4]), (&b"STG3"[..], &b"STGC"[..]));
    assert_eq!(gain_in_state(&saved), 0.375);
    instance.set_parameter(GAIN, 0.875).unwrap();
    instance.set_parameter(plug::PARAM_UI_ZOOM as u64, 0.1).unwrap();
    run(&instance, &input, 2, 64, &[]);
    instance.set_state(&saved).unwrap();
    assert_eq!(instance.parameter(GAIN).unwrap(), Some(0.375), "the controller got the component state");
    assert_eq!(instance.parameter(plug::PARAM_UI_ZOOM as u64).unwrap(), Some(0.75), "and its own");
    let block = run(&instance, &input, 2, 64, &[]);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0][..64], 0.375), "restored gain");
    assert!(instance.state().unwrap() == saved);
    // Refused before the plug-in sees them: garbage, another class's state, a newer format.
    for (bad, needle) in [
        (b"not a state".to_vec(), "not a state saved"),
        (state::encode([1, 2, 3, 4], parts.component, None), "another plug-in"),
        (
            {
                let mut newer = saved.clone();
                newer[4] = 9;
                newer
            },
            "format version 9",
        ),
        // Refused by the plug-in itself.
        (state::encode(cid_words(&hex_to_cid(plug::GAIN_CLASS_ID).unwrap()), b"junk", None), "rejected the state"),
    ] {
        match instance.set_state(&bad) {
            Err(Error::Plugin(message)) => assert!(message.contains(needle), "\"{message}\" lacks \"{needle}\""),
            other => panic!("expected an error mentioning \"{needle}\", got {other:?}"),
        }
    }
    assert_eq!(instance.parameter(GAIN).unwrap(), Some(0.375));

    // A change made while no audio runs is still in the state read right after: the
    // backend flushes it to the processor itself (active: a zero-sample block with the
    // real-time thread excluded; inactive: a temporary activation).
    instance.set_parameter(GAIN, 0.9).unwrap();
    assert_eq!(gain_in_state(&instance.state().unwrap()), 0.9);
    instance.deactivate().unwrap();
    instance.set_parameter(GAIN, 0.1).unwrap();
    assert_eq!(gain_in_state(&instance.state().unwrap()), 0.1);
    instance.activate().unwrap();
    let block = run(&instance, &input, 2, 64, &[]);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0][..64], 0.1), "flushed while inactive");

    // Bypass maps to the parameter the plug-in flags as its bypass.
    assert_eq!(instance.set_bypass(true).unwrap(), true);
    let block = run(&instance, &input, 2, 64, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &input[0][..64], "bypassed by the plug-in");
    assert_eq!(instance.parameter(plug::PARAM_BYPASS as u64).unwrap(), Some(1.0));
    assert_eq!(instance.set_bypass(false).unwrap(), true);

    // Reset, deactivate / activate and a new sample rate and block size go through the
    // VST3 life cycle (the plug-in refuses a setup while active).
    instance.reset().unwrap();
    assert_eq!(run(&instance, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);
    instance.deactivate().unwrap();
    instance.activate().unwrap();
    instance.reconfigure(96_000.0, 128).unwrap();
    let block = run(&instance, &input, 2, 128, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0][..128], 0.1), "after reconfiguring");
    env.finish();
}

#[test]
fn bus_layouts_are_negotiated_and_mismatches_bridged() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let bundle = bundle();
    let mut rng = Rng(0xB05);
    // Mono: the plug-in accepts the arrangement.
    let mono =
        active(env.sup(), InstanceConfig { input_buses: vec![1], output_buses: vec![1], ..gain_config(&bundle) });
    assert_eq!((mono.info().inputs[0].channels, mono.info().outputs[0].channels), (1, 1));
    let input = vec![rng.signal(300)];
    let block = run(&mono, &input, 1, 300, &[Event::param(0, GAIN, 1.0)]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0], 1.0), "mono");

    // Sidechain: the second host input bus feeds the plug-in's auxiliary bus.
    let side = active(env.sup(), InstanceConfig { input_buses: vec![2, 2], ..gain_config(&bundle) });
    let info = side.info();
    assert_eq!(
        (info.inputs[1].name.as_str(), info.inputs[1].channels, info.inputs[1].auxiliary),
        ("Sidechain", 2, true)
    );
    let input: Vec<Vec<f32>> = (0..4).map(|_| rng.signal(200)).collect();
    let events = [Event::param(0, GAIN, 0.25), Event::param(50, plug::PARAM_SIDECHAIN as u64, 0.5)];
    let block = run(&side, &input, 2, 200, &events);
    for channel in 0..2 {
        let expected: Vec<f32> = (0..200)
            .map(|i| {
                let dry = input[channel][i] * plug::gain_factor(0.25);
                if i < 50 {
                    dry
                } else {
                    dry + input[2 + channel][i] * 0.5
                }
            })
            .collect();
        assert_bits_eq(&block.outputs[channel], &expected, "sidechain mix");
    }
    // A mono sidechain on a stereo effect.
    let mono_side = active(env.sup(), InstanceConfig { input_buses: vec![2, 1], ..gain_config(&bundle) });
    assert_eq!(mono_side.info().inputs[1].channels, 1);
    let input: Vec<Vec<f32>> = (0..3).map(|_| rng.signal(64)).collect();
    let events = [Event::param(0, plug::PARAM_SIDECHAIN as u64, 1.0)];
    let block = run(&mono_side, &input, 2, 64, &events);
    let expected: Vec<f32> = (0..64).map(|i| input[1][i] * plug::gain_factor(0.5) + input[2][i] * 1.0).collect();
    assert_bits_eq(&block.outputs[1], &expected, "mono sidechain on both channels");
    // Without a host bus for it the sidechain bus is switched off: the parameter does nothing.
    let plain = active(env.sup(), gain_config(&bundle));
    let input = vec![rng.signal(64), rng.signal(64)];
    let block = run(&plain, &input, 2, 64, &[Event::param(0, plug::PARAM_SIDECHAIN as u64, 1.0)]);
    assert_bits_eq(&block.outputs[0], &input[0], "no sidechain connected");

    // More host buses than the plug-in has: the extra ones are reported unused and stay silent.
    let wide = active(
        env.sup(),
        InstanceConfig { input_buses: vec![2, 2, 2], output_buses: vec![2, 2], ..gain_config(&bundle) },
    );
    let info = wide.info();
    assert_eq!((info.inputs[2].channels, info.inputs[2].name.as_str()), (0, "Unused input"));
    assert_eq!((info.outputs[1].channels, info.outputs[1].auxiliary), (0, true));
    let input: Vec<Vec<f32>> = (0..6).map(|_| rng.signal(64)).collect();
    let block = run(&wide, &input, 4, 64, &[]);
    assert_bits_eq(&block.outputs[0], &input[0], "main bus");
    assert!(block.outputs[2].iter().chain(&block.outputs[3]).all(|&s| s == 0.0));

    // A layout the plug-in refuses (quad): it stays stereo, and the backend bridges: the
    // first two host channels are processed, the others stay silent.
    let quad =
        active(env.sup(), InstanceConfig { input_buses: vec![4], output_buses: vec![4], ..gain_config(&bundle) });
    assert_eq!((quad.info().inputs[0].channels, quad.info().outputs[0].channels), (2, 2));
    let input: Vec<Vec<f32>> = (0..4).map(|_| rng.signal(64)).collect();
    let block = run(&quad, &input, 4, 64, &[Event::param(0, GAIN, 1.0)]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[1], &scaled(&input[1], 1.0), "second channel");
    assert!(block.outputs[2].iter().chain(&block.outputs[3]).all(|&s| s == 0.0));

    // An instrument asked for a mono output.
    let mono_sine = active(env.sup(), InstanceConfig { output_buses: vec![1], ..sine_config(&bundle) });
    assert_eq!(mono_sine.info().outputs[0].channels, 1);
    let block = run(&mono_sine, &[], 1, 32, &[Event::note_on(0, 0, 69, 100)]);
    let expected: Vec<f32> = (0..32).map(|k| plug::sine_sample(69, k, 48_000.0, 0.5)).collect();
    assert_bits_eq(&block.outputs[0], &expected, "mono instrument");
    env.finish();
}

#[test]
fn sine_instrument_notes_are_sample_accurate() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let bundle = bundle();
    let instance = active(env.sup(), sine_config(&bundle));
    let id = instance.id();
    let info = instance.info();
    assert_eq!(info.flags, INFO_INSTRUMENT | INFO_WANTS_MIDI | INFO_PRODUCES_MIDI | INFO_SUSTAIN_PEDAL);
    // The name comes from the Unicode class info.
    assert_eq!((info.name.as_str(), info.inputs.len(), info.outputs[0].channels), (plug::SINE_NAME, 0, 2));
    assert_eq!((info.tail, info.latency, info.parameter_count), (u64::MAX, 0, 8));
    let level = plug::PARAM_LEVEL as u64;
    let rate = 48_000.0;
    let script: Vec<(usize, Vec<Event>)> = vec![
        (64, vec![]),
        (128, vec![Event::note_on(41, 0, 69, 100)]),
        (1, vec![]),
        (500, vec![Event::param(250, level, 0.25)]),
        (256, vec![Event::note_off(13, 0, 69), Event::note_on(255, 0, 48, 1)]),
        (4096, vec![Event::note_off(4000, 0, 48)]),
    ];
    let (mut note, mut age, mut gain) = (None::<u8>, 0u64, 0.5f64);
    for (index, (frames, events)) in script.iter().enumerate() {
        let block = run(&instance, &[], 2, *frames, events);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        let mut expected = vec![0.0f32; *frames];
        let mut next = 0;
        for (frame, slot) in expected.iter_mut().enumerate() {
            while next < events.len() && events[next].offset as usize <= frame {
                let event = &events[next];
                if event.kind == EVENT_PARAM {
                    gain = event.value;
                } else if event.midi_bytes()[0] & 0xF0 == 0x90 {
                    note = Some(event.midi_bytes()[1]);
                    age = 0;
                } else if note == Some(event.midi_bytes()[1]) {
                    note = None;
                }
                next += 1;
            }
            if let Some(n) = note {
                *slot = plug::sine_sample(n, age, rate, gain);
                age += 1;
            }
        }
        assert_bits_eq(&block.outputs[0], &expected, &format!("block {index}"));
        assert_bits_eq(&block.outputs[1], &expected, &format!("block {index}"));
        assert!(block.out_events.is_empty());
        // The plug-in's silence flags arrive as the output silence mask.
        let silent = expected.iter().all(|&s| s == 0.0);
        assert_eq!(block.outcome.output_silence & 3 == 3, silent, "block {index}");
    }
    // The controller half of the single component was told about the automated level.
    wait_until("level in the controller", || instance.parameter(level).unwrap() == Some(0.25));

    // VST3 has no controller events: a MIDI controller reaches the plug-in as the
    // parameter it maps it to (IMidiMapping), at the event's sample offset, and the host
    // application hears of the change.
    let block = run(&instance, &[], 2, 64, &[Event::note_on(0, 0, 60, 127), Event::midi(32, &[0xB3, 7, 64])]);
    let expected: Vec<f32> =
        (0..64).map(|k| plug::sine_sample(60, k, rate, if k < 32 { 0.25 } else { 64.0 / 127.0 })).collect();
    assert_bits_eq(&block.outputs[0], &expected, "level from CC 7");
    env.wait_event("level from CC", LONG, |e| {
        *e == HostEvent::ParameterChanged { instance: id, id: level, value: 64.0 / 127.0 }
    });
    wait_until("CC in the controller", || instance.parameter(level).unwrap() == Some(64.0 / 127.0));
    assert_eq!(instance.format_parameter(level, &[0.5]).unwrap(), ["50 %"]);
    assert_eq!(instance.parse_parameter(level, "25 %").unwrap(), Some(0.25));
    // An unmapped controller is dropped, whatever the plug-in scribbled into the result.
    run(&instance, &[], 2, 64, &[Event::midi(0, &[0xB0, 1, 127])]);

    // Channel pressure and another controller arrive as the parameters the plug-in maps
    // them to, poly pressure as its own event; each at its sample offset.
    let events = [Event::midi(3, &[0xB0, 74, 99]), Event::midi(7, &[0xA0, 60, 45]), Event::midi(40, &[0xD0, 127])];
    let block = run(&instance, &[], 2, 64, &events);
    let set = 64.0 / 127.0;
    let expected: Vec<f32> = (128..192)
        .map(|k| plug::sine_sample(60, k, rate, if k < 168 { set } else { set * plug::pressure_gain(1.0) }))
        .collect();
    assert_bits_eq(&block.outputs[0], &expected, "channel pressure at its frame");
    wait_until("pressure in the controller", || instance.parameter(plug::PARAM_PRESSURE as u64).unwrap() == Some(1.0));
    wait_until("controller 74 in the controller", || {
        instance.parameter(plug::PARAM_BRIGHTNESS as u64).unwrap() == Some(99.0 / 127.0)
    });
    wait_until("poly pressure in the plug-in", || {
        instance.parameter(plug::PARAM_POLY_PRESSURE as u64).unwrap() == Some((60 * 128 + 45) as f64 / 16383.0)
    });
    run(&instance, &[], 2, 16, &[Event::midi(0, &[0xD0, 0])]);

    // "All notes off" releases the notes the plug-in holds, at its offset.
    let block = run(&instance, &[], 2, 64, &[Event::midi(20, &[0xB0, 123, 0])]);
    assert!(block.outputs[0][..20].iter().any(|&s| s != 0.0));
    assert!(block.outputs[0][20..].iter().all(|&s| s == 0.0), "note released by CC 123");
    // So does a reset: the held note gets its note-off before the component restarts.
    run(&instance, &[], 2, 64, &[Event::note_on(0, 0, 72, 100)]);
    instance.reset().unwrap();
    assert!(run(&instance, &[], 2, 64, &[]).outputs[0].iter().all(|&s| s == 0.0));

    // The sustain pedal (controller 64) is the parameter the plug-in maps it to: a note whose
    // key is released under it sounds until the pedal comes up, at the pedal's offset.
    let note_state = plug::PARAM_NOTE_STATE as u64;
    let events = [Event::note_on(0, 0, 65, 100), Event::midi(8, &[0xB0, 64, 127]), Event::note_off(16, 0, 65)];
    let block = run(&instance, &[], 2, 64, &events);
    let expected: Vec<f32> = (0..64).map(|k| plug::sine_sample(65, k, rate, set)).collect();
    assert_bits_eq(&block.outputs[0], &expected, "the pedal holds the released note");
    wait_until("the sustained note in the probe", || {
        instance.parameter(note_state).unwrap() == Some(plug::NOTE_SUSTAINED)
    });
    wait_until("the pedal in the controller", || instance.parameter(plug::PARAM_SUSTAIN as u64).unwrap() == Some(1.0));
    let block = run(&instance, &[], 2, 64, &[Event::midi(48, &[0xB0, 64, 40])]);
    let expected: Vec<f32> =
        (0..64).map(|k| if k < 48 { plug::sine_sample(65, 64 + k, rate, set) } else { 0.0 }).collect();
    assert_bits_eq(&block.outputs[0], &expected, "released when the pedal goes below half");
    wait_until("the silent note in the probe", || instance.parameter(note_state).unwrap() == Some(plug::NOTE_SILENT));
    // "All notes off" lets go of the pedal as well (the backend sets the mapped parameter).
    run(
        &instance,
        &[],
        2,
        64,
        &[Event::note_on(0, 0, 67, 100), Event::midi(1, &[0xB0, 64, 127]), Event::note_off(2, 0, 67)],
    );
    let block = run(&instance, &[], 2, 64, &[Event::midi(20, &[0xB0, 123, 0])]);
    assert!(block.outputs[0][..20].iter().any(|&s| s != 0.0));
    assert!(block.outputs[0][20..].iter().all(|&s| s == 0.0), "the pedal-held note released by CC 123");

    // Events out: with "thru" on the plug-in echoes notes (note id included) and turns
    // pitch bend, which arrived as a parameter, into a legacy MIDI event.
    instance.set_parameter(plug::PARAM_THRU as u64, 1.0).unwrap();
    let mut on = Event::note_on(5, 2, 64, 100);
    on.aux = 4242;
    let events = [on, Event::midi(9, &[0xE0, 0x7F, 0x7F]), Event::note_off(40, 2, 64)];
    let block = run(&instance, &[], 2, 64, &events);
    assert_eq!(block.out_events.len(), 3, "{:?}", block.out_events);
    let out = &block.out_events;
    assert_eq!((out[0].offset, out[0].midi_bytes(), out[0].aux), (5, &[0x92, 64, 100][..], 4242));
    assert_eq!((out[1].offset, out[1].midi_bytes()), (9, &[0xE0, 0x7F, 0x7F][..]));
    assert_eq!((out[2].offset, out[2].midi_bytes()), (40, &[0x82, 64, 0][..]));

    // State of a single component: its controller half has none of its own.
    let saved = instance.state().unwrap();
    assert_eq!(saved.len(), state::HEADER_LEN + plug::SINE_STATE_LEN);
    assert_eq!(&saved[..8], b"STV3\x01\x00\x00\x00");
    instance.set_parameter(level, 1.0).unwrap();
    instance.set_state(&saved).unwrap();
    assert_eq!(instance.parameter(level).unwrap(), Some(64.0 / 127.0));
    // No bypass parameter, no presets, and no editor without a window system binding.
    assert_eq!(instance.set_bypass(true).unwrap(), false);
    assert_eq!(instance.presets().unwrap(), []);
    assert!(matches!(instance.load_preset(0), Err(Error::Unsupported)));
    assert!(matches!(instance.open_editor("x"), Err(Error::Unsupported)));
    env.finish();
}

#[test]
fn latency_changes_follow_the_vst3_restart_protocol() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let bundle = bundle();
    let instance = env.sup().create_instance(gain_config(&bundle)).unwrap();
    let id = instance.id();
    let latency = plug::PARAM_LATENCY as u64;
    assert_eq!(instance.latency(), 0);
    // Changed while inactive (the controller tells the processor through the host's
    // message objects and calls restartComponent(kLatencyChanged)): reported at once.
    instance.set_parameter(latency, 0.125).unwrap();
    env.wait_event("latency 128", LONG, |e| *e == HostEvent::LatencyChanged { instance: id, frames: 128 });
    assert_eq!(instance.refresh_info().unwrap().latency, 128);
    instance.activate().unwrap();
    assert_eq!(instance.latency(), 128);
    // Changed while active: the new latency only becomes valid with setActive(true), so
    // the host is asked to restart the instance.
    env.clear_events();
    instance.set_parameter(latency, 0.25).unwrap();
    env.wait_event("restart request", LONG, |e| *e == HostEvent::RestartRequested { instance: id });
    let input = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    assert_eq!(run(&instance, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);
    assert_eq!(instance.refresh_info().unwrap().latency, 128, "unchanged until the restart");
    instance.deactivate().unwrap();
    instance.activate().unwrap();
    env.wait_event("latency 256", LONG, |e| *e == HostEvent::LatencyChanged { instance: id, frames: 256 });
    assert_eq!(instance.latency(), 256);
    assert_eq!(run(&instance, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);
    assert_eq!(
        env.events().iter().filter(|e| matches!(e, HostEvent::RestartRequested { .. })).count(),
        1,
        "one request per restart"
    );
    // A state with another latency, loaded while active: the same protocol.
    let with_256 = instance.state().unwrap();
    env.clear_events();
    instance.set_parameter(latency, 0.5).unwrap();
    // The request arrives from the container's main thread in its own time: restart only after it,
    // or it would arrive after the restart and stand for the next change.
    env.wait_event("restart request for 512", LONG, |e| *e == HostEvent::RestartRequested { instance: id });
    run(&instance, &input, 2, 64, &[]);
    instance.deactivate().unwrap();
    instance.activate().unwrap();
    env.wait_event("latency 512", LONG, |e| *e == HostEvent::LatencyChanged { instance: id, frames: 512 });
    env.clear_events();
    instance.set_state(&with_256).unwrap();
    env.wait_event("restart for the state", LONG, |e| *e == HostEvent::RestartRequested { instance: id });
    instance.deactivate().unwrap();
    instance.activate().unwrap();
    env.wait_event("latency 256 again", LONG, |e| *e == HostEvent::LatencyChanged { instance: id, frames: 256 });
    env.finish();
}

#[test]
fn program_lists_are_presets_and_editor_edits_reach_host_and_processor() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let bundle = bundle();
    let instance = active(env.sup(), gain_config(&bundle));
    let id = instance.id();
    let mut rng = Rng(0x9E5);
    let input = vec![rng.signal(64), rng.signal(64)];

    // The program list bound to the program change parameter is the preset list.
    let presets = instance.presets().unwrap();
    assert_eq!(
        presets.iter().map(|p| (p.id, p.name.as_str())).collect::<Vec<_>>(),
        [(0, "Unity"), (1, "Half"), (2, "Double")]
    );
    instance.load_preset(1).unwrap();
    // The plug-in announces restartComponent(kParamValuesChanged): read the values again.
    env.wait_event("values changed", LONG, |e| *e == HostEvent::ParametersRescan { instance: id });
    assert_eq!(instance.parameter(GAIN).unwrap(), Some(plug::PROGRAMS[1].1));
    assert_eq!(instance.parameter(plug::PARAM_PROGRAM as u64).unwrap(), Some(0.5));
    // The client snapshots the state after a preset load; the program is in it already.
    assert_eq!(gain_in_state(&instance.last_known_state().unwrap()), plug::PROGRAMS[1].1);
    let block = run(&instance, &input, 2, 64, &[]);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0], plug::PROGRAMS[1].1), "program Half");
    assert!(matches!(instance.load_preset(3), Err(Error::Plugin(_))));
    // A MIDI program change selects a program too, at its sample offset.
    let block = run(&instance, &input, 2, 64, &[Event::midi(16, &[0xC0, 2])]);
    let expected: Vec<f32> =
        (0..64).map(|i| input[0][i] * plug::gain_factor(plug::PROGRAMS[if i < 16 { 1 } else { 2 }].1)).collect();
    assert_bits_eq(&block.outputs[0], &expected, "program change");
    wait_until("program in the controller", || instance.parameter(plug::PARAM_PROGRAM as u64).unwrap() == Some(1.0));

    // The plug-in's editor moves a control: beginEdit / performEdit / endEdit reach the
    // host application, the host carries the value to the processor, and setDirty marks
    // the state as changed.
    env.clear_events();
    instance.set_parameter(plug::PARAM_EDIT_TRIGGER as u64, 0.3).unwrap();
    env.wait_event("dirty", LONG, |e| *e == HostEvent::StateDirty { instance: id });
    let gestures: Vec<HostEvent> = env
        .events()
        .into_iter()
        .filter(|e| {
            matches!(e, HostEvent::ParameterGesture { .. }) || matches!(e, HostEvent::ParameterChanged { id: GAIN, .. })
        })
        .collect();
    assert_eq!(
        gestures,
        [
            HostEvent::ParameterGesture { instance: id, id: GAIN, begin: true },
            HostEvent::ParameterChanged { instance: id, id: GAIN, value: 0.3 },
            HostEvent::ParameterGesture { instance: id, id: GAIN, begin: false },
        ]
    );
    let block = run(&instance, &input, 2, 64, &[]);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0], 0.3), "edit made in the editor");
    env.finish();
}

#[test]
fn transport_and_offline_mode_reach_the_plugin() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let bundle = bundle();
    let instance = active(env.sup(), gain_config(&bundle));
    let input = vec![vec![0.25f32; 32], vec![0.25f32; 32]];
    let probe = |id: u32| instance.parameter(id as u64).unwrap().unwrap();
    let transport = Transport {
        flags: TRANSPORT_PLAYING
            | TRANSPORT_RECORDING
            | TRANSPORT_LOOPING
            | TRANSPORT_TEMPO_VALID
            | TRANSPORT_TIME_SIG_VALID
            | TRANSPORT_PPQ_VALID,
        time_sig_numerator: 7,
        time_sig_denominator: 8,
        tempo_bpm: 140.5,
        sample_position: 123_456,
        ppq_position: 12.5,
        bar_start_ppq: 10.5,
        loop_start_ppq: 8.0,
        loop_end_ppq: 16.0,
    };
    let block = run_with(&instance, &input, 2, 32, &[], Some(&transport));
    assert_eq!(block.outcome.flags, PROCESS_OK);
    wait_until("the context probes", || probe(plug::PARAM_CTX_CYCLE_END) == 16.0 / 1e6);
    assert_eq!(probe(plug::PARAM_CTX_TEMPO), 140.5 / 1000.0);
    assert_eq!(probe(plug::PARAM_CTX_PPQ), 12.5 / 1e6);
    assert_eq!(probe(plug::PARAM_CTX_SAMPLES), 123_456.0 / (1u64 << 40) as f64);
    assert_eq!(probe(plug::PARAM_CTX_SIGNATURE), (7 * 256 + 8) as f64 / 65536.0);
    assert_eq!(probe(plug::PARAM_CTX_BAR), 10.5 / 1e6);
    assert_eq!(probe(plug::PARAM_CTX_CYCLE_START), 8.0 / 1e6);
    // kPlaying | kCycleActive | kRecording | kSystemTimeValid | kProjectTimeMusicValid |
    // kTempoValid | kBarPositionValid | kCycleValid | kTimeSigValid | kContTimeValid.
    let state = 2 | 4 | 8 | 256 | 512 | 1024 | 2048 | 4096 | 8192 | 131_072;
    assert_eq!(probe(plug::PARAM_CTX_STATE), state as f64 / (1u64 << 20) as f64);
    // A stopped transport without musical time: only what is known is flagged.
    let stopped = Transport { sample_position: 99, ..Transport::default() };
    run_with(&instance, &input, 2, 32, &[], Some(&stopped));
    wait_until("the stopped context", || probe(plug::PARAM_CTX_STATE) == (256 | 131_072) as f64 / (1u64 << 20) as f64);
    assert_eq!(probe(plug::PARAM_CTX_TEMPO), 120.0 / 1000.0, "the default tempo, not flagged valid");

    // Offline: the setup is redone with kOffline and blocks carry it.
    assert_eq!(probe(plug::PARAM_OFFLINE), 0.0);
    instance.set_offline(true).unwrap();
    assert_eq!(run(&instance, &input, 2, 32, &[]).outcome.flags, PROCESS_OK);
    wait_until("offline", || probe(plug::PARAM_OFFLINE) == 1.0);
    instance.set_offline(false).unwrap();
    assert_eq!(run(&instance, &input, 2, 32, &[]).outcome.flags, PROCESS_OK);
    wait_until("realtime", || probe(plug::PARAM_OFFLINE) == 0.0);
    env.finish();
}

#[test]
fn crash_and_hang_inside_a_vst3_plugin_are_contained_and_the_state_restored() {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::PerPlugin;
        c.hang_timeout = Duration::from_millis(400);
    });
    let bundle = bundle();
    let victim = active(env.sup(), gain_config(&bundle));
    let bystander = active(env.sup(), sine_config(&bundle));
    let builtin = active(env.sup(), effect("gain"));
    assert!(victim.container() != bystander.container());
    let container = victim.container();
    let mut rng = Rng(0xDEAD);
    let input = vec![rng.signal(128), rng.signal(128)];
    // Some state worth keeping, fetched by the host before the crash.
    run(&victim, &input, 2, 128, &[Event::param(0, GAIN, 0.25)]);
    victim.set_parameter(plug::PARAM_UI_ZOOM as u64, 0.625).unwrap();
    let saved = victim.state().unwrap();
    run(&bystander, &[], 2, 128, &[Event::note_on(0, 0, 69, 100)]);

    // Segfault inside the plug-in's process call.
    let block = run(&victim, &input, 2, 128, &[Event::param(64, plug::PARAM_CRASH as u64, 1.0)]);
    assert!(block.outcome.fell_back() && block.outcome.flags & (PROCESS_LOST | PROCESS_LATE) != 0);
    assert!(block.elapsed < Duration::from_secs(5));
    assert_bits_eq(&block.outputs[0], &input[0], "dry signal");
    for _ in 0..100 {
        let block = run(&victim, &input, 2, 128, &[]);
        assert_eq!(block.outcome.flags, PROCESS_LOST | PROCESS_FALLBACK);
        assert!(block.elapsed < Duration::from_millis(20));
        // The other containers keep playing: the other plug-in of the same module too.
        let other = run(&bystander, &[], 2, 128, &[]);
        assert_eq!(other.outcome.flags, PROCESS_OK);
        assert!(other.outputs[0].iter().any(|&s| s != 0.0));
        assert_eq!(run(&builtin, &input, 2, 128, &[]).outcome.flags, PROCESS_OK);
    }
    env.wait_event("crash report", LONG, |e| {
        matches!(e, HostEvent::ContainerExited { container: c, reason: ExitReason::Crashed { signal }, lost, .. }
            if *c == container && *signal == libc::SIGSEGV && lost == &vec![victim.id()])
    });

    // Restart: the plug-in comes back with the state the host last fetched, both halves.
    let report = env.sup().restart_container(container).unwrap();
    assert_eq!(report, RestartReport { restored: vec![victim.id()], failed: vec![] });
    assert_eq!(victim.parameter(GAIN).unwrap(), Some(0.25));
    assert_eq!(victim.parameter(plug::PARAM_UI_ZOOM as u64).unwrap(), Some(0.625));
    assert!(victim.state().unwrap() == saved);
    let block = run(&victim, &input, 2, 128, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0], 0.25), "restored gain");

    // An abort this time, to see the other signal classified.
    env.clear_events();
    let block = run(&victim, &input, 2, 128, &[Event::param(0, plug::PARAM_CRASH as u64, 0.5)]);
    assert!(block.outcome.fell_back());
    env.wait_event("abort report", LONG, |e| {
        matches!(e, HostEvent::ContainerExited { reason: ExitReason::Crashed { signal }, .. } if *signal == libc::SIGABRT)
    });
    victim.restore().unwrap();
    assert_eq!(run(&victim, &input, 2, 128, &[]).outcome.flags, PROCESS_OK);

    // A plug-in that never returns from process: detected, killed, restorable.
    env.clear_events();
    victim.set_deadline(Some(Duration::from_millis(50)));
    let block = run(&victim, &input, 2, 128, &[Event::param(0, plug::PARAM_HANG as u64, 0.5)]);
    assert_eq!(block.outcome.flags, PROCESS_LATE | PROCESS_FALLBACK);
    let started = Instant::now();
    while victim.status() != InstanceStatus::Lost {
        assert!(run(&victim, &input, 2, 128, &[]).outcome.fell_back());
        assert_eq!(run(&bystander, &[], 2, 128, &[]).outcome.flags, PROCESS_OK);
        assert!(started.elapsed() < LONG, "the hung container was never killed");
        std::thread::sleep(Duration::from_millis(1));
    }
    let id = victim.id();
    env.wait_event("hung", LONG, |e| *e == HostEvent::InstanceHung { instance: id, container });
    env.wait_event("lost", LONG, |e| {
        *e == HostEvent::InstanceLost { instance: id, container, reason: ExitReason::Hung }
    });
    victim.set_deadline(Some(LONG));
    victim.restore().unwrap();
    // The event the plug-in hung on went out with the block it never finished. No block went
    // by between the loss and the restore, so it was still on the client's books: it is given
    // up with the restored plug-in's first block (which says so), not sent to it.
    let block = run(&victim, &input, 2, 128, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK | PROCESS_EVENTS_LOST);
    assert_bits_eq(&block.outputs[0], &scaled(&input[0], 0.25), "restored after the hang");
    assert_eq!(run(&victim, &input, 2, 128, &[]).outcome.flags, PROCESS_OK);

    // A plug-in that hangs on its main thread: requests time out, audio continues.
    let hanger = active(env.sup(), InstanceConfig { name: "hanger".into(), ..gain_config(&bundle) });
    assert_eq!(hanger.container(), container);
    let stuck = std::thread::scope(|scope| {
        let request = scope.spawn(|| hanger.set_parameter(plug::PARAM_HANG as u64, 1.0));
        std::thread::sleep(Duration::from_millis(100));
        for _ in 0..20 {
            assert_eq!(run(&victim, &input, 2, 128, &[]).outcome.flags, PROCESS_OK, "audio continues");
        }
        env.sup().kill_container(container).unwrap();
        request.join().unwrap()
    });
    assert!(matches!(stuck, Err(Error::ContainerDied)), "{stuck:?}");
    env.finish();
}

#[test]
fn module_is_unloaded_with_its_last_instance() {
    let scratch = Scratch::new("vst3-marker");
    let marker = scratch.path("marker.txt");
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::Shared;
        c.container_env = vec![("STPC_VST3_TEST_MARKER".into(), marker.clone().into_os_string())];
    });
    let bundle = bundle();
    let lines = || -> Vec<String> {
        std::fs::read_to_string(&marker).unwrap_or_default().lines().map(str::to_string).collect()
    };
    // Something else keeps the container process alive throughout.
    let keeper = active(env.sup(), effect("gain"));
    let gain = active(env.sup(), gain_config(&bundle));
    let sine = active(env.sup(), sine_config(&bundle));
    assert_eq!(env.sup().containers().len(), 1);
    assert_eq!(lines(), ["entry"], "one module entry for two instances");
    run(&sine, &[], 2, 64, &[Event::note_on(0, 0, 60, 100)]);
    drop(gain);
    assert_eq!(lines(), ["entry"], "still in use");
    drop(sine);
    // Factory released, exit function called, and no plug-in object was left alive: the
    // teardown released every interface before the module went.
    assert_eq!(lines(), ["entry", "exit live=0"]);
    // Loading it again starts from scratch.
    let again = active(env.sup(), gain_config(&bundle));
    assert_eq!(lines(), ["entry", "exit live=0", "entry"]);
    let input = vec![vec![0.5f32; 16], vec![0.5f32; 16]];
    assert_eq!(run(&again, &input, 2, 16, &[]).outputs[0], [0.5f32; 16]);
    assert_eq!(run(&keeper, &input, 2, 16, &[]).outcome.flags, PROCESS_OK);
    // A container shutting down with live instances tears them down the same way.
    env.finish();
    assert_eq!(lines(), ["entry", "exit live=0", "entry", "exit live=0"]);
}

#[test]
fn loading_errors_are_reported() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let scratch = Scratch::new("vst3-errors");
    let good = bundle();
    let expect_error = |config: InstanceConfig, needle: &str| match env.sup().create_instance(config) {
        Err(Error::Plugin(message)) => assert!(message.contains(needle), "\"{message}\" does not mention \"{needle}\""),
        other => panic!("expected a plug-in error mentioning \"{needle}\", got {:?}", other.err()),
    };
    let at =
        |path: &std::path::Path| InstanceConfig { options: path.to_string_lossy().into_owned(), ..gain_config(&good) };
    expect_error(at(std::path::Path::new("/nonexistent/Plugin.vst3")), "cannot load");
    expect_error(InstanceConfig { options: String::new(), ..gain_config(&good) }, "path of the .vst3 bundle");
    expect_error(InstanceConfig { plugin_id: "gain".into(), ..gain_config(&good) }, "not a VST3 class id");
    expect_error(
        InstanceConfig { plugin_id: "00000000000000000000000000000001".into(), ..gain_config(&good) },
        "has no plug-in with the class id",
    );
    // The controller class is in the factory, but it is not a plug-in.
    expect_error(
        InstanceConfig { plugin_id: plug::GAIN_CONTROLLER_CLASS_ID.into(), ..gain_config(&good) },
        "has no plug-in with the class id",
    );
    // A bundle without a library for this machine.
    let empty = scratch.path("Empty.vst3");
    std::fs::create_dir_all(empty.join("Contents")).unwrap();
    expect_error(at(&empty), "has no library for this machine");
    // A shared library that is not a VST3 module.
    let libm = ["/lib/x86_64-linux-gnu/libm.so.6", "/usr/lib/libm.so.6", "/lib64/libm.so.6"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    if let Some(path) = libm {
        expect_error(at(std::path::Path::new(path)), "not a VST3 module");
    }
    // Modules whose entry fails or that hand out no factory.
    expect_error(at(&vst3_bundle(&scratch.dir, "EntryFails")), "module entry");
    expect_error(at(&vst3_bundle(&scratch.dir, "NoFactory")), "has no plug-in factory");
    // None of that hurt the container: a good plug-in still loads in it.
    let instance = active(env.sup(), gain_config(&good));
    assert_eq!(run(&instance, &[vec![1.0; 8], vec![1.0; 8]], 2, 8, &[]).outputs[0], [1.0f32; 8]);
    assert_eq!(env.sup().containers().len(), 1);
    env.finish();
}

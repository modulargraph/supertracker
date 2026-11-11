//! The CLAP backend, end to end: host process -> container process -> a real CLAP plug-in
//! library (`stpc-clap-test-plugin`, built by cargo as a dependency of these tests).
#![allow(clippy::bool_assert_comparison, clippy::needless_range_loop)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use stpc_clap_test_plugin as plug;

/// The plug-in library cargo built next to this test executable.
fn plugin_path() -> String {
    let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let name = if cfg!(target_os = "macos") { "libstpc_clap_test_plugin.dylib" } else { "libstpc_clap_test_plugin.so" };
    let path = dir.join(name);
    assert!(path.is_file(), "{} was not built", path.display());
    path.to_string_lossy().into_owned()
}

fn clap(config: InstanceConfig, id: &str) -> InstanceConfig {
    InstanceConfig {
        format: "clap".into(),
        plugin_id: id.into(),
        options: plugin_path(),
        deadline: Some(LONG),
        manufacturer: "SuperTracker Tests".into(),
        ..config
    }
}

fn gain_config() -> InstanceConfig {
    clap(InstanceConfig::effect("clap", ""), plug::GAIN_ID)
}

fn sine_config() -> InstanceConfig {
    clap(InstanceConfig::instrument("clap", ""), plug::SINE_ID)
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + LONG;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn gain_effect_metadata_audio_parameters_and_state() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = env.sup().create_instance(gain_config()).unwrap();

    let info = instance.info();
    assert_eq!(
        (info.name.as_str(), info.vendor.as_str(), info.version.as_str()),
        ("ST Test Gain", "SuperTracker Tests", "1.2.3")
    );
    assert_eq!(info.flags, INFO_HAS_BYPASS | INFO_HAS_EDITOR);
    assert_eq!((info.inputs.len(), info.outputs.len()), (1, 1));
    assert_eq!(
        (info.inputs[0].name.as_str(), info.inputs[0].channels, info.inputs[0].auxiliary),
        ("Audio In", 2, false)
    );
    assert_eq!((info.latency, info.tail, info.parameter_count), (0, 0, 7));

    let params = instance.parameters().unwrap();
    assert_eq!(params.iter().map(|p| p.id).collect::<Vec<_>>(), [1, 2, 3, 4, 10, 11, 12]);
    let gain = &params[0];
    assert_eq!(
        (gain.name.as_str(), gain.group.as_str(), gain.min, gain.max, gain.default),
        ("Gain", "Main", 0.0, 2.0, 1.0)
    );
    assert_eq!((gain.flags, gain.step_count), (PARAM_AUTOMATABLE, 0));
    assert_eq!(params[1].flags, PARAM_AUTOMATABLE | PARAM_STEPPED | PARAM_BYPASS | PARAM_BOOLEAN);
    assert_eq!(params[2].value_strings, ["No", "Abort", "Segfault"]);
    assert_eq!((params[2].step_count, params[2].flags), (2, PARAM_STEPPED));
    assert_eq!(params[4].flags, PARAM_READ_ONLY | PARAM_STEPPED);
    assert_eq!((params[5].group.as_str(), params[5].flags), ("Test/Probe", PARAM_READ_ONLY | PARAM_HIDDEN));
    assert_eq!(instance.format_parameter(1, &[0.5, 2.0]).unwrap(), ["0.50 x", "2.00 x"]);
    assert_eq!(instance.parse_parameter(1, "1.25 x").unwrap(), Some(1.25));
    assert_eq!(instance.parse_parameter(1, "loud").unwrap(), None);
    assert_eq!(instance.parameter(999).unwrap(), None);

    // Not active yet: a parameter change is flushed to the plug-in on its main thread.
    instance.set_parameter(plug::PARAM_GAIN as u64, 0.5).unwrap();
    assert_eq!(instance.parameter(plug::PARAM_GAIN as u64).unwrap(), Some(0.5));

    instance.activate().unwrap();
    let mut rng = Rng(0xC1A9);
    let input = vec![rng.signal(256), rng.signal(256)];
    let block = run(&instance, &input, 2, 256, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    for channel in 0..2 {
        let expected: Vec<f32> = input[channel].iter().map(|s| s * 0.5).collect();
        assert_bits_eq(&block.outputs[channel], &expected, "gain 0.5");
    }
    // Sample-accurate automation through the block's events.
    let events = [Event::param(100, plug::PARAM_GAIN as u64, 2.0), Event::param(200, plug::PARAM_GAIN as u64, 1.0)];
    let block = run(&instance, &input, 2, 256, &events);
    let expected: Vec<f32> = (0..256)
        .map(|i| {
            input[0][i]
                * if i < 100 {
                    0.5
                } else if i < 200 {
                    2.0
                } else {
                    1.0
                }
        })
        .collect();
    assert_bits_eq(&block.outputs[0], &expected, "automation");
    // Active: an untimed change travels with the next block and applies from its start.
    instance.set_parameter(plug::PARAM_GAIN as u64, 0.25).unwrap();
    let block = run(&instance, &input, 2, 256, &[]);
    let expected: Vec<f32> = input[1].iter().map(|s| s * 0.25).collect();
    assert_bits_eq(&block.outputs[1], &expected, "queued change");
    assert_eq!(instance.parameter(plug::PARAM_GAIN as u64).unwrap(), Some(0.25));
    // Block sizes from 1 to 4096, changing per call.
    instance.set_parameter(plug::PARAM_GAIN as u64, 1.0).unwrap();
    for frames in [1usize, 4096, 17, 512, 3, 1024] {
        let input = vec![rng.signal(frames), rng.signal(frames)];
        let block = run(&instance, &input, 2, frames, &[]);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        assert_bits_eq(&block.outputs[0], &input[0], "unity gain");
        assert_bits_eq(&block.outputs[1], &input[1], "unity gain");
    }

    // State: the plug-in's own bytes, there and back.
    instance.set_parameter(plug::PARAM_GAIN as u64, 0.75).unwrap();
    run(&instance, &input, 2, 64, &[]);
    let state = instance.state().unwrap();
    assert_eq!((state.len(), &state[..4]), (18, &b"STCT"[..]));
    instance.set_parameter(plug::PARAM_GAIN as u64, 1.5).unwrap();
    run(&instance, &input, 2, 64, &[]);
    assert_eq!(instance.parameter(plug::PARAM_GAIN as u64).unwrap(), Some(1.5));
    instance.set_state(&state).unwrap();
    assert_eq!(instance.parameter(plug::PARAM_GAIN as u64).unwrap(), Some(0.75));
    assert!(matches!(instance.set_state(b"not a state"), Err(Error::Plugin(_))));
    assert_eq!(instance.parameter(plug::PARAM_GAIN as u64).unwrap(), Some(0.75));
    // Loading made the plug-in ask for a main-thread callback: it gets one.
    wait_until("on_main_thread", || instance.parameter(plug::PARAM_MAIN_CALLS as u64).unwrap() >= Some(1.0));

    // Bypass maps to the parameter the plug-in flags as its bypass.
    assert_eq!(instance.set_bypass(true).unwrap(), true);
    let block = run(&instance, &input, 2, 64, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    assert_bits_eq(&block.outputs[0], &input[0][..64], "bypassed by the plug-in");
    assert_eq!(instance.parameter(plug::PARAM_BYPASS as u64).unwrap(), Some(1.0));
    assert_eq!(instance.set_bypass(false).unwrap(), true);

    // Reset and deactivate/activate go through the CLAP life cycle without complaint.
    instance.reset().unwrap();
    instance.deactivate().unwrap();
    instance.activate().unwrap();
    assert_eq!(run(&instance, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);
    env.finish();
}

#[test]
fn host_layout_may_differ_from_the_plugins_ports() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    // Mono host buses on a stereo plug-in: the second channel is scratch on both sides.
    let mono = active(env.sup(), InstanceConfig { input_buses: vec![1], output_buses: vec![1], ..gain_config() });
    assert_eq!(mono.info().inputs[0].channels, 1);
    let mut rng = Rng(5);
    let input = vec![rng.signal(300)];
    let block = run(&mono, &input, 1, 300, &[Event::param(0, plug::PARAM_GAIN as u64, 2.0)]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    let expected: Vec<f32> = input[0].iter().map(|s| s * 2.0).collect();
    assert_bits_eq(&block.outputs[0], &expected, "mono through a stereo plug-in");
    // More host buses than plug-in ports: the extra ones are reported unused and stay silent.
    let wide = active(env.sup(), InstanceConfig { input_buses: vec![2, 2], output_buses: vec![2, 2], ..gain_config() });
    let info = wide.info();
    assert_eq!((info.inputs[1].channels, info.outputs[1].channels), (0, 0));
    let input: Vec<Vec<f32>> = (0..4).map(|_| rng.signal(64)).collect();
    let block = run(&wide, &input, 4, 64, &[]);
    assert_bits_eq(&block.outputs[0], &input[0], "main bus");
    assert!(block.outputs[2].iter().chain(&block.outputs[3]).all(|&s| s == 0.0));
    env.finish();
}

#[test]
fn sine_instrument_notes_are_sample_accurate() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), sine_config());
    let info = instance.info();
    assert_eq!(info.flags, INFO_INSTRUMENT | INFO_WANTS_MIDI);
    assert_eq!((info.name.as_str(), info.inputs.len(), info.outputs[0].channels), ("ST Test Sine", 0, 2));
    let rate = 48_000.0;
    let script: Vec<(usize, Vec<Event>)> = vec![
        (64, vec![]),
        (128, vec![Event::note_on(41, 0, 69, 100)]),
        (1, vec![]),
        (500, vec![Event::param(250, plug::PARAM_LEVEL as u64, 0.25)]),
        (256, vec![Event::note_off(13, 0, 69), Event::note_on(255, 0, 48, 1)]),
        (4096, vec![Event::note_off(4000, 0, 48)]),
    ];
    let (mut note, mut age, mut level) = (None::<u8>, 0u64, 0.5f64);
    for (index, (frames, events)) in script.iter().enumerate() {
        let block = run(&instance, &[], 2, *frames, events);
        assert_eq!(block.outcome.flags, PROCESS_OK);
        let mut expected = vec![0.0f32; *frames];
        let mut next = 0;
        for (frame, slot) in expected.iter_mut().enumerate() {
            while next < events.len() && events[next].offset as usize <= frame {
                let event = &events[next];
                if event.kind == EVENT_PARAM {
                    level = event.value;
                } else if event.midi_bytes()[0] & 0xF0 == 0x90 {
                    note = Some(event.midi_bytes()[1]);
                    age = 0;
                } else if note == Some(event.midi_bytes()[1]) {
                    note = None;
                }
                next += 1;
            }
            if let Some(n) = note {
                *slot = plug::sine_sample(n, age, rate, level);
                age += 1;
            }
        }
        assert_bits_eq(&block.outputs[0], &expected, &format!("block {index}"));
        assert_bits_eq(&block.outputs[1], &expected, &format!("block {index}"));
        assert!(block.out_events.is_empty());
    }
    // A MIDI controller the plug-in maps to its Level parameter: the change it makes is
    // reported back to the host as a parameter change coming from the plug-in.
    let id = instance.id();
    run(&instance, &[], 2, 64, &[Event::midi(5, &[0xB0, 7, 64])]);
    env.wait_event("level from CC", LONG, |e| {
        *e == HostEvent::ParameterChanged { instance: id, id: 1, value: 64.0 / 127.0 }
    });
    assert_eq!(instance.parameter(1).unwrap(), Some(64.0 / 127.0));
    assert_eq!(instance.format_parameter(1, &[0.5]).unwrap(), ["50 %"]);
    // No bypass parameter, no editor: the client falls back / reports "unsupported".
    assert_eq!(instance.set_bypass(true).unwrap(), false);
    assert!(matches!(instance.open_editor("x"), Err(Error::Unsupported)));
    env.finish();
}

/// Controllers, pressure and pitch bend reach a CLAP plug-in at their sample offsets: as
/// MIDI when its note port takes MIDI, as note expressions when it takes CLAP events only.
#[test]
fn controllers_and_pressure_reach_both_note_dialects() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let rate = 48_000.0;
    let probes = |instance: &Instance| -> [f64; 4] {
        [
            plug::PARAM_PROBE_CONTROLLER,
            plug::PARAM_PROBE_PRESSURE,
            plug::PARAM_PROBE_POLY_PRESSURE,
            plug::PARAM_PROBE_BEND,
        ]
        .map(|id| instance.parameter(id as u64).unwrap().unwrap())
    };
    let events = [
        Event::note_on(0, 0, 60, 100),
        Event::midi(3, &[0xB0, 74, 99]),
        Event::midi(7, &[0xA0, 60, 45]),
        Event::midi(11, &[0xE0, 0x00, 0x60]),
        Event::midi(40, &[0xD0, 127]),
    ];
    // The sample after `k` frames, with channel pressure from frame 40 on.
    let expected: Vec<f32> = (0..64)
        .map(|k| plug::sine_sample(60, k, rate, if k < 40 { 0.5 } else { 0.5 * plug::pressure_gain(127) }))
        .collect();

    let midi = active(env.sup(), sine_config());
    assert_eq!(probes(&midi), [0.0, 0.0, 0.0, 8192.0]);
    let block = run(&midi, &[], 2, 64, &events);
    assert_bits_eq(&block.outputs[0], &expected, "pressure at its frame (MIDI dialect)");
    assert_eq!(probes(&midi), [(74 * 128 + 99) as f64, 127.0, (60 * 128 + 45) as f64, (0x60 << 7) as f64]);

    let notes = active(env.sup(), clap(InstanceConfig::instrument("clap", ""), plug::NOTE_SINE_ID));
    assert_eq!(notes.info().name, "ST Test Sine (CLAP notes)");
    let block = run(&notes, &[], 2, 64, &events);
    assert_bits_eq(&block.outputs[0], &expected, "pressure at its frame (note expression)");
    // Pressure and bend arrived as note expressions; a control change has no CLAP event.
    assert_eq!(probes(&notes), [0.0, 127.0, (60 * 128 + 45) as f64, (0x60 << 7) as f64]);
    // A bend back to the centre is no tuning at all.
    run(&notes, &[], 2, 16, &[Event::midi(0, &[0xE0, 0x00, 0x40]), Event::midi(1, &[0xD0, 0])]);
    assert_eq!(probes(&notes)[3], 8192.0);
    env.finish();
}

/// The sine instrument has a sustain pedal (MIDI controller 64), so that hosts can be tested
/// on passing the pedal and the note-offs on to an instrument that sustains by itself.
#[test]
fn the_sine_instruments_sustain_pedal_holds_a_released_note() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let rate = 48_000.0;
    let state = |instance: &Instance| instance.parameter(plug::PARAM_PROBE_NOTE_STATE as u64).unwrap().unwrap();
    let sine = active(env.sup(), sine_config());
    assert_eq!(state(&sine), plug::NOTE_SILENT as f64);
    // Key down, pedal down, key up: the note sounds on.
    let events = [Event::note_on(0, 0, 60, 100), Event::midi(8, &[0xB0, 64, 127]), Event::note_off(16, 0, 60)];
    let block = run(&sine, &[], 2, 64, &events);
    let expected: Vec<f32> = (0..64).map(|k| plug::sine_sample(60, k, rate, 0.5)).collect();
    assert_bits_eq(&block.outputs[0], &expected, "the pedal holds the released note");
    assert_eq!(state(&sine), plug::NOTE_SUSTAINED as f64);
    // "All notes off" releases keys, not what the pedal holds; half way down (64) is down;
    // pedal-up ends the note at its frame.
    let events = [Event::midi(4, &[0xB0, 123, 0]), Event::midi(20, &[0xB0, 64, 64]), Event::midi(48, &[0xB0, 64, 63])];
    let block = run(&sine, &[], 2, 64, &events);
    let expected: Vec<f32> =
        (0..64).map(|k| if k < 48 { plug::sine_sample(60, 64 + k, rate, 0.5) } else { 0.0 }).collect();
    assert_bits_eq(&block.outputs[0], &expected, "released when the pedal comes up");
    assert_eq!(state(&sine), plug::NOTE_SILENT as f64);
    // A key that is down is not released by the pedal coming up; then it ends with its key.
    let events = [Event::note_on(0, 0, 62, 100), Event::midi(8, &[0xB0, 64, 127]), Event::midi(20, &[0xB0, 64, 0])];
    let block = run(&sine, &[], 2, 64, &events);
    assert!(block.outputs[0][63] != 0.0);
    assert_eq!(state(&sine), plug::NOTE_KEY_DOWN as f64);
    let block = run(&sine, &[], 2, 64, &[Event::note_off(10, 0, 62)]);
    assert!(block.outputs[0][10..].iter().all(|&sample| sample == 0.0), "no pedal: the note ends with its key");
    // "All sound off" ends a note the pedal holds; a reset lifts the pedal.
    run(
        &sine,
        &[],
        2,
        64,
        &[Event::note_on(0, 0, 64, 100), Event::midi(1, &[0xB0, 64, 127]), Event::note_off(2, 0, 64)],
    );
    assert_eq!(state(&sine), plug::NOTE_SUSTAINED as f64);
    let block = run(&sine, &[], 2, 64, &[Event::midi(5, &[0xB0, 120, 0])]);
    assert!(block.outputs[0][5..].iter().all(|&sample| sample == 0.0));
    sine.reset().unwrap();
    let block = run(&sine, &[], 2, 64, &[Event::note_on(0, 0, 65, 100), Event::note_off(30, 0, 65)]);
    assert!(
        block.outputs[0][29] != 0.0 && block.outputs[0][30..].iter().all(|&sample| sample == 0.0),
        "the pedal is up after a reset"
    );
    env.finish();
}

#[test]
fn latency_changes_follow_the_clap_restart_protocol() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = env.sup().create_instance(gain_config()).unwrap();
    let id = instance.id();
    assert_eq!(instance.latency(), 0);
    // Changed while inactive: visible on request, announced when the plug-in is activated.
    instance.set_parameter(plug::PARAM_LATENCY as u64, 128.0).unwrap();
    assert_eq!(instance.refresh_info().unwrap().latency, 128);
    instance.activate().unwrap();
    env.wait_event("latency 128", LONG, |e| *e == HostEvent::LatencyChanged { instance: id, frames: 128 });
    assert_eq!(instance.latency(), 128);
    // Changed while active: the plug-in may not change latency now, it asks for a restart.
    instance.set_parameter(plug::PARAM_LATENCY as u64, 256.0).unwrap();
    let input = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    run(&instance, &input, 2, 64, &[]);
    env.wait_event("restart request", LONG, |e| *e == HostEvent::RestartRequested { instance: id });
    assert_eq!(instance.latency(), 128, "unchanged until the restart");
    instance.deactivate().unwrap();
    instance.activate().unwrap();
    env.wait_event("latency 256", LONG, |e| *e == HostEvent::LatencyChanged { instance: id, frames: 256 });
    assert_eq!(instance.latency(), 256);
    assert_eq!(run(&instance, &input, 2, 64, &[]).outcome.flags, PROCESS_OK);
    env.finish();
}

#[test]
fn floating_editor_is_opened_titled_and_closed() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = env.sup().create_instance(gain_config()).unwrap();
    let probe = |id: u32| instance.parameter(id as u64).unwrap().unwrap();
    assert_eq!(probe(plug::PARAM_EDITOR_STATE), 0.0);
    let editor = instance.open_editor("Track 2 – ST Test Gain").unwrap();
    assert_eq!((editor.width, editor.height, editor.resizable), (320, 200, false));
    assert_eq!(probe(plug::PARAM_EDITOR_STATE), 2.0, "created and shown");
    assert_eq!(probe(plug::PARAM_EDITOR_TITLE_LEN), "Track 2 – ST Test Gain".len() as f64);
    instance.set_editor_title("Other").unwrap();
    assert_eq!(probe(plug::PARAM_EDITOR_TITLE_LEN), 5.0);
    instance.focus_editor().unwrap();
    instance.close_editor().unwrap();
    assert_eq!(probe(plug::PARAM_EDITOR_STATE), 0.0, "hidden and destroyed");
    // Opening again works; destroying the instance with an open editor closes it first.
    instance.open_editor("Again").unwrap();
    assert_eq!(probe(plug::PARAM_EDITOR_STATE), 2.0);
    drop(instance);
    env.finish();
}

#[test]
fn crash_inside_a_clap_plugin_is_contained_and_the_state_restored() {
    let env = TestEnv::new(GroupingPolicy::PerPlugin);
    let victim = active(env.sup(), gain_config());
    let bystander = active(env.sup(), sine_config());
    let builtin = active(env.sup(), effect("gain"));
    assert!(victim.container() != bystander.container());
    let container = victim.container();
    let mut rng = Rng(0xDEAD);
    let input = vec![rng.signal(128), rng.signal(128)];
    // Some state worth keeping, fetched by the host before the crash.
    run(&victim, &input, 2, 128, &[Event::param(0, plug::PARAM_GAIN as u64, 0.5)]);
    let saved = victim.state().unwrap();
    run(&bystander, &[], 2, 128, &[Event::note_on(0, 0, 69, 100)]);

    // Segfault inside the plug-in's process call.
    let block = run(&victim, &input, 2, 128, &[Event::param(64, plug::PARAM_CRASH as u64, 2.0)]);
    assert!(block.outcome.fell_back() && block.outcome.flags & (PROCESS_LOST | PROCESS_LATE) != 0);
    assert!(block.elapsed < Duration::from_secs(5));
    assert_bits_eq(&block.outputs[0], &input[0], "dry signal");
    for _ in 0..100 {
        let block = run(&victim, &input, 2, 128, &[]);
        assert_eq!(block.outcome.flags, PROCESS_LOST | PROCESS_FALLBACK);
        assert!(block.elapsed < Duration::from_millis(20));
        // The other containers keep playing.
        let other = run(&bystander, &[], 2, 128, &[]);
        assert_eq!(other.outcome.flags, PROCESS_OK);
        assert!(other.outputs[0].iter().any(|&s| s != 0.0));
        assert_eq!(run(&builtin, &input, 2, 128, &[]).outcome.flags, PROCESS_OK);
    }
    env.wait_event("crash report", LONG, |e| {
        matches!(e, HostEvent::ContainerExited { container: c, reason: ExitReason::Crashed { signal }, lost, .. }
            if *c == container && *signal == libc::SIGSEGV && lost == &vec![victim.id()])
    });

    // Restart: the plug-in comes back with the state the host last fetched.
    let report = env.sup().restart_container(container).unwrap();
    assert_eq!(report, RestartReport { restored: vec![victim.id()], failed: vec![] });
    assert_eq!(victim.parameter(plug::PARAM_GAIN as u64).unwrap(), Some(0.5));
    assert!(victim.state().unwrap() == saved);
    let block = run(&victim, &input, 2, 128, &[]);
    assert_eq!(block.outcome.flags, PROCESS_OK);
    let expected: Vec<f32> = input[0].iter().map(|s| s * 0.5).collect();
    assert_bits_eq(&block.outputs[0], &expected, "restored gain");
    // An abort this time, to see the other signal classified.
    let block = run(&victim, &input, 2, 128, &[Event::param(0, plug::PARAM_CRASH as u64, 1.0)]);
    assert!(block.outcome.fell_back());
    env.wait_event("abort report", LONG, |e| {
        matches!(e, HostEvent::ContainerExited { reason: ExitReason::Crashed { signal }, .. } if *signal == libc::SIGABRT)
    });
    env.finish();
}

#[test]
fn loading_errors_are_reported() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let expect_error = |config: InstanceConfig, needle: &str| match env.sup().create_instance(config) {
        Err(Error::Plugin(message)) => assert!(message.contains(needle), "\"{message}\" does not mention \"{needle}\""),
        other => panic!("expected a plug-in error mentioning \"{needle}\", got {:?}", other.err()),
    };
    expect_error(InstanceConfig { options: "/nonexistent/plugin.clap".into(), ..gain_config() }, "cannot load");
    expect_error(InstanceConfig { options: String::new(), ..gain_config() }, "path of the .clap file");
    expect_error(InstanceConfig { plugin_id: "org.example.missing".into(), ..gain_config() }, "has no plug-in");
    // A shared library that is not a CLAP plug-in.
    let libc_path = ["/lib/x86_64-linux-gnu/libm.so.6", "/usr/lib/libm.so.6", "/lib64/libm.so.6"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    if let Some(path) = libc_path {
        expect_error(InstanceConfig { options: path.into(), ..gain_config() }, "not a CLAP plug-in");
    }
    // None of that hurt the container: a good plug-in still loads in it.
    let instance = active(env.sup(), gain_config());
    assert_eq!(run(&instance, &[vec![1.0; 8], vec![1.0; 8]], 2, 8, &[]).outputs[0], [1.0f32; 8]);
    assert_eq!(env.sup().containers().len(), 1);
    env.finish();
}

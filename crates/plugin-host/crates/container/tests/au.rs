//! The Audio Unit backend, end to end, against Audio Units that ship with macOS: host
//! process -> container process -> `AUAudioUnit`.
//!
//! macOS only, and **never run so far** (written on Linux, type-checked only). It is the
//! automated part of the validation checklist in docs/PLUGIN_CONTAINER.md ("Audio Units").
//! Where the exact behaviour of an Apple unit is an assumption rather than documented,
//! the test prints what it saw instead of asserting it; run with `--nocapture`:
//!
//! ```sh
//! cargo test -p stpc-container --test au -- --nocapture --test-threads=1
//! ```
#![cfg(target_os = "macos")]
#![allow(clippy::needless_range_loop)]

mod common;

use std::time::Duration;

use common::*;
use stpc_container::au::{list_components, ComponentId, ComponentKind};

const DELAY: &str = "aufx:dely:appl";
const DLS: &str = "aumu:dls%20:appl";
const SAMPLER: &str = "aumu:samp:appl";

// AUDelay's parameters (AudioUnitParameters.h, kDelayParam_*).
const DELAY_WET_DRY: u64 = 0; // percent, 0..100
const DELAY_TIME: u64 = 1; // seconds, 0..2
const DELAY_FEEDBACK: u64 = 2; // percent, -100..100
const DELAY_LOPASS: u64 = 3; // Hz

fn au_effect(id: &str) -> InstanceConfig {
    InstanceConfig {
        deadline: Some(LONG),
        manufacturer: "Apple".into(),
        max_frames: 1024,
        ..InstanceConfig::effect("au", id)
    }
}

fn au_instrument(id: &str) -> InstanceConfig {
    InstanceConfig {
        deadline: Some(LONG),
        manufacturer: "Apple".into(),
        max_frames: 1024,
        ..InstanceConfig::instrument("au", id)
    }
}

fn peak(samples: &[f32]) -> (usize, f32) {
    samples.iter().enumerate().fold((0, 0.0), |best, (i, s)| if s.abs() > best.1 { (i, s.abs()) } else { best })
}

/// Renders `blocks` blocks of `frames` and returns each output channel concatenated.
fn render(
    instance: &Instance,
    input: &[Vec<f32>],
    outputs: usize,
    frames: usize,
    blocks: usize,
    first: &[Event],
) -> Vec<Vec<f32>> {
    let mut all = vec![Vec::new(); outputs];
    for block in 0..blocks {
        let slice: Vec<Vec<f32>> =
            input.iter().map(|channel| channel[block * frames..(block + 1) * frames].to_vec()).collect();
        let result = run(instance, &slice, outputs, frames, if block == 0 { first } else { &[] });
        assert_eq!(result.outcome.flags & PROCESS_OK, PROCESS_OK, "block {block}: flags {:#x}", result.outcome.flags);
        for (channel, samples) in result.outputs.into_iter().enumerate() {
            all[channel].extend(samples);
        }
    }
    all
}

#[test]
fn the_listing_has_the_stock_units() {
    let list = list_components();
    assert!(list.len() > 20, "only {} components listed", list.len());
    let find = |id: &str| list.iter().find(|c| c.plugin_id == id).unwrap_or_else(|| panic!("{id} is not listed"));
    let delay = find(DELAY);
    assert_eq!(
        (delay.name.as_str(), delay.manufacturer.as_str(), delay.kind),
        ("AUDelay", "Apple", ComponentKind::Effect)
    );
    assert!(!delay.version_string.is_empty());
    let dls = find(DLS);
    assert_eq!(dls.kind, ComponentKind::Instrument);
    assert_eq!(find(SAMPLER).kind, ComponentKind::Instrument);
    for component in &list {
        // Every id is canonical and decodes to the listed kind.
        let id = ComponentId::parse(&component.plugin_id).unwrap();
        assert_eq!(id.to_string(), component.plugin_id);
        assert_eq!(id.component_kind(), Some(component.kind));
    }
    let mut ids: Vec<&str> = list.iter().map(|c| c.plugin_id.as_str()).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), before, "duplicate ids");
    println!("{} Audio Units; AUDelay: {delay:?}", list.len());
    println!("with a custom view: {}", list.iter().filter(|c| c.has_custom_view).count());
    println!("version 3: {}", list.iter().filter(|c| c.is_v3).count());
}

#[test]
fn audelay_metadata_parameters_and_audio() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = env.sup().create_instance(au_effect(DELAY)).unwrap();
    let info = instance.info();
    println!("{info:?}");
    assert_eq!((info.name.as_str(), info.vendor.as_str()), ("AUDelay", "Apple"));
    assert_eq!(info.flags & INFO_INSTRUMENT, 0);
    assert_ne!(info.flags & INFO_HAS_EDITOR, 0);
    assert_eq!((info.inputs.len(), info.outputs.len()), (1, 1));
    assert_eq!((info.inputs[0].channels, info.outputs[0].channels), (2, 2));
    assert_eq!(info.latency, 0);

    let params = instance.parameters().unwrap();
    for p in &params {
        println!(
            "{:>4} {:<24} [{} .. {}] default {} {:?} flags {:#x}",
            p.id, p.name, p.min, p.max, p.default, p.unit_label, p.flags
        );
    }
    assert_eq!(info.parameter_count as usize, params.len());
    let by_id = |id: u64| params.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no parameter {id}"));
    let mix = by_id(DELAY_WET_DRY);
    assert_eq!((mix.min, mix.max, mix.unit_label.as_str()), (0.0, 100.0, "%"));
    assert_ne!(mix.flags & PARAM_AUTOMATABLE, 0);
    let time = by_id(DELAY_TIME);
    assert_eq!((time.min, time.max, time.unit_label.as_str()), (0.0, 2.0, "s"));
    assert_eq!(time.default, 1.0);
    assert!(!instance.format_parameter(DELAY_TIME, &[0.5]).unwrap()[0].is_empty());

    // Fully wet, 10 ms, no feedback, filter wide open: an impulse comes back 480 frames later.
    for (id, value) in
        [(DELAY_WET_DRY, 100.0), (DELAY_TIME, 0.01), (DELAY_FEEDBACK, 0.0), (DELAY_LOPASS, by_id(DELAY_LOPASS).max)]
    {
        instance.set_parameter(id, value).unwrap();
    }
    instance.activate().unwrap();
    println!("latency after activation: {} frames, tail {}", instance.latency(), instance.tail());
    let mut input = vec![vec![0.0f32; 4096]; 2];
    input[0][0] = 1.0;
    input[1][0] = 1.0;
    let out = render(&instance, &input, 2, 512, 8, &[]);
    for channel in 0..2 {
        let (at, level) = peak(&out[channel]);
        println!("channel {channel}: peak {level} at frame {at}");
        assert!(level > 0.1, "no delayed impulse");
        assert!((440..=520).contains(&at), "impulse at {at}, expected about 480");
        assert!(peak(&out[channel][..400]).1 < 1e-3, "wet signal before the delay time");
    }
    // The values written are the values read (after rendering, see "next render" units).
    let time_now = instance.parameter(DELAY_TIME).unwrap().unwrap();
    assert!((time_now - 0.01).abs() < 1e-6, "delay time reads {time_now}");
    // Out-of-range writes are clamped to the range, not refused.
    instance.set_parameter(DELAY_WET_DRY, 1000.0).unwrap();
    assert_eq!(instance.parameter(DELAY_WET_DRY).unwrap(), Some(100.0));
    assert!(instance.set_parameter(987_654, 1.0).is_err());
    assert!(instance.set_parameter(DELAY_WET_DRY, f64::NAN).is_err());

    // Automation through the block's events: fully dry from frame 256 on. (Whether the
    // change lands on frame 256 or at the block start is the unit's business; either way
    // the impulse at frame 300 must come out dry and in place.)
    instance.reset().unwrap();
    let mut input = vec![vec![0.0f32; 512]; 2];
    input[0][300] = 1.0;
    let events = [Event::param(256, DELAY_WET_DRY, 0.0)];
    let result = run(&instance, &input, 2, 512, &events);
    let (at, level) = peak(&result.outputs[0]);
    println!("dry after automation: peak {level} at frame {at} (expected at 300)");
    assert_eq!(at, 300, "the dry impulse is not where it was sent");

    // Blocks larger and smaller than before, and one of a single frame.
    for frames in [1usize, 17, 1024] {
        let input = vec![vec![0.25f32; frames]; 2];
        let result = run(&instance, &input, 2, frames, &[]);
        assert_eq!(result.outcome.flags & PROCESS_OK, PROCESS_OK);
    }
    env.finish();
}

#[test]
fn audelay_state_presets_bypass_offline() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = env.sup().create_instance(au_effect(DELAY)).unwrap();
    instance.set_parameter(DELAY_TIME, 0.25).unwrap();
    instance.set_parameter(DELAY_FEEDBACK, -30.0).unwrap();
    let saved = instance.state().unwrap();
    println!("state: {} bytes, starts {:?}", saved.len(), &saved[..saved.len().min(28)]);
    assert_eq!(&saved[..4], b"STAU");
    // An unchanged plug-in gives byte-identical state.
    assert_eq!(instance.state().unwrap(), saved);

    instance.set_parameter(DELAY_TIME, 1.5).unwrap();
    assert_ne!(instance.state().unwrap(), saved);
    instance.set_state(&saved).unwrap();
    let time = instance.parameter(DELAY_TIME).unwrap().unwrap();
    let feedback = instance.parameter(DELAY_FEEDBACK).unwrap().unwrap();
    assert!((time - 0.25).abs() < 1e-6 && (feedback + 30.0).abs() < 1e-4, "restored {time} / {feedback}");
    println!("capture after restore is the restored bytes: {}", instance.state().unwrap() == saved);

    // A state of another unit, and garbage, are refused with a readable message.
    let other = env.sup().create_instance(au_effect("aufx:lpas:appl")).unwrap();
    let error = other.set_state(&saved).unwrap_err();
    println!("foreign state: {error}");
    assert!(instance.set_state(b"not a state").is_err());
    assert!(instance.set_state(&[]).is_err());

    let presets = instance.presets().unwrap();
    println!("{} presets: {:?}", presets.len(), presets.iter().take(5).collect::<Vec<_>>());
    if let Some(preset) = presets.first() {
        instance.load_preset(preset.id).unwrap();
    }
    assert!(instance.load_preset(0x7FFF_FFF0).is_err());

    instance.activate().unwrap();
    let bypassed_by_plugin = instance.set_bypass(true).unwrap();
    println!("bypass handled by the plug-in: {bypassed_by_plugin}");
    let mut input = vec![vec![0.0f32; 512]; 2];
    input[0][10] = 0.5;
    let result = run(&instance, &input, 2, 512, &[]);
    assert_eq!(peak(&result.outputs[0]).0, 10, "bypass does not pass the input through");
    instance.set_bypass(false).unwrap();

    instance.set_offline(true).unwrap();
    let result = run(&instance, &input, 2, 512, &[]);
    assert_eq!(result.outcome.flags & PROCESS_OK, PROCESS_OK);
    instance.set_offline(false).unwrap();
    instance.deactivate().unwrap();
    instance.reconfigure(44_100.0, 256).unwrap();
    instance.activate().unwrap();
    let result = run(&instance, &[vec![0.1; 256], vec![0.1; 256]], 2, 256, &[]);
    assert_eq!(result.outcome.flags & PROCESS_OK, PROCESS_OK);
    drop(other);
    env.finish();
}

#[test]
fn a_mono_slot_and_a_wide_slot() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    for (ins, outs) in [(1u32, 1u32), (1, 2), (2, 1)] {
        let config = InstanceConfig { input_buses: vec![ins], output_buses: vec![outs], ..au_effect(DELAY) };
        let instance = env.sup().create_instance(config).unwrap();
        let info = instance.info();
        println!("{ins} in / {outs} out: unit uses {} in, {} out", info.inputs[0].channels, info.outputs[0].channels);
        instance.set_parameter(DELAY_WET_DRY, 0.0).unwrap();
        instance.activate().unwrap();
        let mut input = vec![vec![0.0f32; 256]; ins as usize];
        for channel in &mut input {
            channel[5] = 0.5;
        }
        let result = run(&instance, &input, outs as usize, 256, &[]);
        assert_eq!(result.outcome.flags & PROCESS_OK, PROCESS_OK);
        for channel in 0..outs as usize {
            let (at, level) = peak(&result.outputs[channel]);
            assert!(level > 0.1 && at == 5, "{ins}/{outs} channel {channel}: peak {level} at {at}");
        }
    }
    env.finish();
}

fn plays_a_note(id: &str) {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = env.sup().create_instance(au_instrument(id)).unwrap();
    let info = instance.info();
    println!("{info:?}");
    assert_ne!(info.flags & INFO_INSTRUMENT, 0);
    assert_ne!(info.flags & INFO_WANTS_MIDI, 0);
    assert!(info.outputs[0].channels >= 1);
    instance.activate().unwrap();
    // Silence before any note.
    let quiet = run(&instance, &[], 2, 512, &[]);
    assert!(peak(&quiet.outputs[0]).1 < 1e-4, "sound without a note");
    let note = [Event::note_on(100, 0, 60, 110)];
    let empty: Vec<Vec<f32>> = Vec::new();
    let out = {
        let mut all = vec![Vec::new(); 2];
        for block in 0..40 {
            let result = run(&instance, &empty, 2, 512, if block == 0 { &note } else { &[] });
            assert_eq!(result.outcome.flags & PROCESS_OK, PROCESS_OK);
            for channel in 0..2 {
                all[channel].extend_from_slice(&result.outputs[channel]);
            }
        }
        all
    };
    let (at, level) = peak(&out[0]);
    let first = out[0].iter().position(|s| s.abs() > 1e-5);
    println!("{id}: peak {level} at frame {at}; first sound at frame {first:?} (note-on at 100)");
    assert!(level > 0.01, "the note made no sound");
    // Whether a unit honours the offset inside the block is the unit's business; the
    // line above shows it.
    let off = [Event::note_off(0, 0, 60)];
    run(&instance, &empty, 2, 512, &off);
    let state = instance.state().unwrap();
    instance.set_state(&state).unwrap();
    println!(
        "{id}: state {} bytes, {} parameters, {} presets",
        state.len(),
        instance.parameters().unwrap().len(),
        instance.presets().unwrap().len()
    );
    env.finish();
}

#[test]
fn dls_music_device_plays_a_note() {
    plays_a_note(DLS);
}

#[test]
fn ausampler_plays_a_note() {
    plays_a_note(SAMPLER);
}

#[test]
fn the_editor_opens_in_a_window() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = env.sup().create_instance(au_effect(DELAY)).unwrap();
    let editor = instance.open_editor("AUDelay – test").unwrap();
    println!("{editor:?}");
    assert!(editor.width >= 100 && editor.height >= 50);
    assert_ne!(editor.window_id, 0);
    std::thread::sleep(Duration::from_millis(500));
    instance.set_editor_title("AUDelay – renamed").unwrap();
    instance.focus_editor().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    // The main thread stays responsive with a window open.
    instance.ping(true, Duration::from_secs(2)).unwrap();
    instance.close_editor().unwrap();
    // Opening again after closing works.
    instance.open_editor("AUDelay – again").unwrap();
    instance.close_editor().unwrap();
    env.finish();
}

#[test]
fn errors_are_reported() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    for (id, needle) in [
        ("aufx:zzzz:zzzz", "not installed"),
        ("nonsense", "type:subtype:manufacturer"),
        ("auou:def%20:appl", "not an instrument"),
    ] {
        let error = env.sup().create_instance(au_effect(id)).err().unwrap_or_else(|| panic!("{id} was created"));
        println!("{id}: {error}");
        assert!(error.to_string().contains(needle), "{id}: {error}");
    }
    // An instrument in an effect configuration still loads (it ignores the input).
    let instance = env.sup().create_instance(au_effect(DLS)).unwrap();
    drop(instance);
    env.finish();
}

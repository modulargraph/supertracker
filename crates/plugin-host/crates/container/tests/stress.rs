//! Randomised stress: a control thread creates, edits, kills, restores and destroys
//! instances while an audio thread keeps processing whatever exists.

#![allow(clippy::needless_range_loop)]

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use common::*;
use stpc_container::test_backend as tb;

struct Entry {
    instance: Instance,
    plugin: &'static str,
    inputs: usize,
    outputs: usize,
    is_instrument: bool,
    /// False once anything may have altered the audio path (bypass, parameters).
    exact: bool,
}

const PLUGINS: [&str; 6] = ["copy", "gain", "delay", "sine", "bigstate", "crash"];

fn stress(seed: u64, operations: usize) {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::PerPlugin;
        c.hang_timeout = Duration::from_secs(20);
    });
    let entries: Arc<RwLock<Vec<Entry>>> = Arc::new(RwLock::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let processed = Arc::new(AtomicU64::new(0));
    let fallbacks = Arc::new(AtomicU64::new(0));

    let audio = {
        let (entries, stop, processed, fallbacks) =
            (entries.clone(), stop.clone(), processed.clone(), fallbacks.clone());
        std::thread::spawn(move || {
            let mut rng = Rng(seed ^ 0xA0D10);
            while !stop.load(Ordering::Relaxed) {
                let list = entries.read().unwrap();
                for entry in list.iter() {
                    let frames = 1 + rng.below(512) as usize;
                    let input: Vec<Vec<f32>> = (0..entry.inputs).map(|_| rng.raw_bits(frames)).collect();
                    let block = run(&entry.instance, &input, entry.outputs, frames, &[]);
                    let flags = block.outcome.flags;
                    assert!(block.outcome.processed() != block.outcome.fell_back(), "flags {flags:#x}");
                    if block.outcome.processed() {
                        processed.fetch_add(1, Ordering::Relaxed);
                        if entry.plugin == "copy" {
                            for (channel, output) in block.outputs.iter().enumerate() {
                                assert_bits_eq(output, &input[channel % entry.inputs], "copy under stress");
                            }
                        }
                    } else {
                        fallbacks.fetch_add(1, Ordering::Relaxed);
                        if entry.is_instrument {
                            assert!(block.outputs.iter().flatten().all(|&s| s == 0.0), "instrument fallback");
                        } else if entry.exact || flags & PROCESS_BYPASSED == 0 {
                            // Dry pass-through (these instances have no dry delay line).
                            for channel in 0..entry.outputs.min(entry.inputs) {
                                assert_bits_eq(&block.outputs[channel], &input[channel], "effect fallback");
                            }
                        }
                    }
                }
                drop(list);
                std::thread::sleep(Duration::from_micros(300));
            }
        })
    };

    let mut rng = Rng(seed);
    let mut counts = [0usize; 12];
    let policies = [
        GroupingPolicy::Shared,
        GroupingPolicy::PerManufacturer,
        GroupingPolicy::PerPlugin,
        GroupingPolicy::PerInstance,
    ];
    for _ in 0..operations {
        let len = entries.read().unwrap().len();
        let op = if len == 0 {
            0
        } else if len >= 12 {
            1
        } else {
            rng.below(12) as usize
        };
        counts[op] += 1;
        match op {
            0 | 10 => {
                let plugin = PLUGINS[rng.below(PLUGINS.len() as u64) as usize];
                let mut config = if plugin == "sine" { instrument(plugin) } else { effect(plugin) };
                config.manufacturer = ["Acme", "Bolt", "Cog"][rng.below(3) as usize].into();
                config.max_frames = 512;
                if plugin == "copy" {
                    let channels = 1 + rng.below(4) as u32;
                    config.input_buses = vec![channels];
                    config.output_buses = vec![channels];
                }
                if plugin == "bigstate" {
                    config.options = format!("size={}", 1000 + rng.below(200_000));
                }
                let (inputs, outputs) =
                    (config.input_buses.iter().sum::<u32>() as usize, config.output_buses.iter().sum::<u32>() as usize);
                // Creation can fail if the target container is killed concurrently: never here
                // (this thread is the only one that kills), so it must succeed.
                let instance = env.sup().create_instance(config).expect("create under stress");
                if rng.below(5) != 0 {
                    instance.activate().expect("activate under stress");
                }
                entries.write().unwrap().push(Entry {
                    instance,
                    plugin,
                    inputs,
                    outputs,
                    is_instrument: plugin == "sine",
                    exact: true,
                });
            }
            1 => {
                let index = rng.below(len as u64) as usize;
                let entry = entries.write().unwrap().swap_remove(index);
                drop(entry); // destroyed here, on the control thread
            }
            2 => {
                let list = entries.read().unwrap();
                let entry = &list[rng.below(len as u64) as usize];
                let result = match entry.plugin {
                    "gain" => entry.instance.set_parameter(tb::gain::MODE, rng.below(3) as f64),
                    "bigstate" => entry.instance.set_parameter(1 + rng.below(64), 0.5),
                    "delay" | "sine" | "copy" | "crash" => entry.instance.parameters().map(|_| ()),
                    _ => Ok(()),
                };
                assert!(matches!(result, Ok(()) | Err(Error::InstanceLost)), "{result:?}");
            }
            3 => {
                let list = entries.read().unwrap();
                let entry = &list[rng.below(len as u64) as usize];
                match entry.instance.state() {
                    Ok(state) => {
                        let result = entry.instance.set_state(&state);
                        assert!(matches!(result, Ok(()) | Err(Error::InstanceLost)), "{result:?}");
                    }
                    Err(Error::InstanceLost) => {}
                    Err(other) => panic!("state: {other}"),
                }
            }
            4 => {
                // Kill a random container; its instances become lost.
                let containers = env.sup().containers();
                if let Some(target) = containers.get(rng.below(containers.len().max(1) as u64) as usize) {
                    env.sup().kill_container(target.id).unwrap();
                }
            }
            5 => {
                // Crash from inside, on the plug-in's main thread.
                let list = entries.read().unwrap();
                if let Some(entry) =
                    list.iter().find(|e| e.plugin == "crash" && e.instance.status() == InstanceStatus::Running)
                {
                    let result = entry.instance.set_parameter(tb::crash::MAIN, 1.0 + rng.below(2) as f64);
                    assert!(matches!(result, Err(Error::ContainerDied)), "{result:?}");
                }
            }
            6 => {
                let list = entries.read().unwrap();
                let entry = &list[rng.below(len as u64) as usize];
                entry.instance.restore().expect("restore under stress");
                assert!(matches!(entry.instance.status(), InstanceStatus::Running | InstanceStatus::Late));
            }
            7 => {
                for container in env.sup().containers() {
                    if container.state == ContainerState::Dead {
                        let report = env.sup().restart_container(container.id).unwrap();
                        assert!(report.failed.is_empty(), "{report:?}");
                    }
                }
            }
            8 => env.sup().set_policy(policies[rng.below(4) as usize]),
            9 => {
                let list = entries.read().unwrap();
                let entry = &list[rng.below(len as u64) as usize];
                let result =
                    if entry.instance.is_active() { entry.instance.deactivate() } else { entry.instance.activate() };
                assert!(matches!(result, Ok(()) | Err(Error::InstanceLost)), "{result:?}");
            }
            11 => {
                let mut list = entries.write().unwrap();
                let index = rng.below(len as u64) as usize;
                let entry = &mut list[index];
                if entry.plugin != "copy" {
                    let rate = [44_100.0, 48_000.0, 96_000.0][rng.below(3) as usize];
                    let result = entry.instance.reconfigure(rate, 512);
                    assert!(matches!(result, Ok(()) | Err(Error::InstanceLost)), "{result:?}");
                    entry.exact = false;
                }
            }
            _ => unreachable!(),
        }
        if rng.below(4) == 0 {
            std::thread::sleep(Duration::from_millis(rng.below(4)));
        }
    }

    // Bring everything back and check that every instance works.
    for entry in entries.read().unwrap().iter() {
        entry.instance.restore().expect("final restore");
        if !entry.instance.is_active() {
            entry.instance.activate().expect("final activate");
        }
    }
    let before = processed.load(Ordering::Relaxed);
    let deadline = Instant::now() + LONG;
    while processed.load(Ordering::Relaxed) < before + 200 && !entries.read().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "audio stopped flowing");
        std::thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Ordering::Relaxed);
    audio.join().expect("audio thread");
    for entry in entries.read().unwrap().iter() {
        assert_eq!(entry.instance.status(), InstanceStatus::Running, "{}", entry.plugin);
        let input: Vec<Vec<f32>> = (0..entry.inputs).map(|_| vec![0.25; 64]).collect();
        assert_eq!(run(&entry.instance, &input, entry.outputs, 64, &[]).outcome.flags, PROCESS_OK, "{}", entry.plugin);
    }
    let started = env.pids.lock().unwrap().len();
    println!(
        "stress seed {seed:#x}: {operations} operations {counts:?}, {} container processes started, {} blocks processed, {} fallback blocks, {} instances at the end",
        started,
        processed.load(Ordering::Relaxed),
        fallbacks.load(Ordering::Relaxed),
        entries.read().unwrap().len()
    );
    entries.write().unwrap().clear();
    assert!(env.sup().containers().is_empty(), "containers end with their last instance");
    env.finish();
}

#[test]
fn randomized_operations() {
    let operations = std::env::var("STPC_STRESS_OPS").ok().and_then(|v| v.parse().ok()).unwrap_or(400);
    let seed =
        std::env::var("STPC_STRESS_SEED").ok().and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok());
    match seed {
        Some(seed) => stress(seed, operations),
        None => {
            for seed in [0x51DE_CAFE, 0x0BAD_5EED, 0x1234_5678_9ABC] {
                stress(seed, operations);
            }
        }
    }
}

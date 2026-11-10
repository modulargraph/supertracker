//! Real-time properties of the host-side process call: no allocation on the audio thread
//! in any path, and the measured cost of the round trip.

#![allow(clippy::bool_assert_comparison)]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

use common::*;
use stpc_container::test_backend as tb;

/// Counts allocator calls made by threads that asked to be watched.
struct CountingAllocator;

thread_local! {
    static WATCHED: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<u64> = const { Cell::new(0) };
}

fn note_call() {
    let _ = WATCHED.try_with(|watched| {
        if watched.get() {
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }
    });
}

// SAFETY: defers to the system allocator; the bookkeeping does not allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_call();
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_call();
        System.dealloc(ptr, layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_call();
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_call();
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Runs `f` and returns how many allocator calls this thread made meanwhile.
fn allocator_calls<R>(f: impl FnOnce() -> R) -> (u64, R) {
    CALLS.with(|calls| calls.set(0));
    WATCHED.with(|watched| watched.set(true));
    let result = f();
    WATCHED.with(|watched| watched.set(false));
    (CALLS.with(|calls| calls.get()), result)
}

/// Preallocated buffers for one stereo-in/stereo-out block of up to 4096 frames.
struct Rig {
    inputs: Vec<Vec<f32>>,
    outputs: Vec<Vec<f32>>,
    out_events: Vec<Event>,
}

impl Rig {
    fn new() -> Rig {
        let mut rng = Rng(0xA110C);
        Rig {
            inputs: vec![rng.signal(4096), rng.signal(4096)],
            outputs: vec![vec![0.0; 4096], vec![0.0; 4096]],
            out_events: vec![Event::default(); 64],
        }
    }

    /// No allocation in here: slices of preallocated buffers, arrays on the stack.
    fn process(
        &mut self,
        instance: &Instance,
        frames: usize,
        events: &[Event],
        transport: Option<&Transport>,
    ) -> ProcessOutcome {
        let (left, right) = self.outputs.split_at_mut(1);
        let inputs: [&[f32]; 2] = [&self.inputs[0][..frames], &self.inputs[1][..frames]];
        let mut outputs: [&mut [f32]; 2] = [&mut left[0][..frames], &mut right[0][..frames]];
        instance.process(&mut ProcessBuffers {
            frames,
            inputs: &inputs,
            outputs: &mut outputs,
            events,
            out_events: &mut self.out_events,
            transport,
            deadline: None,
        })
    }
}

#[test]
fn the_allocation_counter_works() {
    let (calls, _) = allocator_calls(|| drop(std::hint::black_box(vec![1u8; 100])));
    assert_eq!(calls, 2, "one allocation and one release");
    let (calls, _) = allocator_calls(|| std::hint::black_box(3 + 4));
    assert_eq!(calls, 0);
}

#[test]
fn process_never_allocates_in_any_path() {
    let env = TestEnv::with(|c| {
        c.policy = GroupingPolicy::PerPlugin;
        c.hang_timeout = Duration::from_secs(60);
    });
    let gain = active(env.sup(), effect("gain"));
    let synth = active(env.sup(), instrument("sine"));
    let delay = active(env.sup(), InstanceConfig { max_dry_delay_frames: 2048, ..effect("delay") });
    let inactive = env.sup().create_instance(effect("gain")).unwrap();
    let hang = active(env.sup(), InstanceConfig { deadline: Some(Duration::from_millis(5)), ..effect("hang") });
    let crash = active(env.sup(), effect("crash"));
    let mut rig = Rig::new();
    let transport =
        Transport { flags: TRANSPORT_PLAYING | TRANSPORT_TEMPO_VALID, tempo_bpm: 120.0, ..Transport::default() };
    let events = [
        Event::param(3, tb::gain::GAIN, 0.5),
        Event::note_on(10, 0, 64, 100),
        Event::param(20, tb::gain::GAIN, 1.0),
        Event::note_off(40, 0, 64),
    ];
    for instance in [&gain, &synth, &delay] {
        rig.process(instance, 64, &events, Some(&transport)); // warm-up outside the count
    }

    // 1. The normal path, with events, MIDI out, transport and varying block sizes.
    let (calls, blocks) = allocator_calls(|| {
        let mut ok = 0;
        for round in 0..1500usize {
            let frames = [64, 128, 1, 512, 4096, 77][round % 6];
            for instance in [&gain, &synth, &delay] {
                let outcome = rig.process(instance, frames, &events, Some(&transport));
                ok += (outcome.flags == PROCESS_OK) as usize;
            }
            // The real-time safe queries.
            std::hint::black_box((gain.status(), gain.latency(), gain.tail(), gain.stats()));
        }
        ok
    });
    assert_eq!(blocks, 4500, "every block was processed by the plug-in");
    assert_eq!(calls, 0, "allocator calls on the audio thread in the normal path");

    // 2. Fallback paths: inactive, client-side bypass with the dry delay line.
    assert_eq!(delay.set_bypass(true).unwrap(), false);
    let (calls, _) = allocator_calls(|| {
        for _ in 0..200 {
            assert_eq!(rig.process(&inactive, 128, &[], None).flags, PROCESS_INACTIVE | PROCESS_FALLBACK);
            assert_eq!(rig.process(&delay, 128, &[], None).flags, PROCESS_BYPASSED | PROCESS_FALLBACK);
        }
    });
    assert_eq!(calls, 0, "allocator calls in the inactive / bypass paths");

    // 3. Deadline miss, late blocks, and the resynchronisation afterwards.
    let stall = [Event::param(0, tb::hang::PROCESS_SLEEP_MS, 150.0)];
    let (calls, (late, resynced)) = allocator_calls(|| {
        let mut late = 0;
        let first = rig.process(&hang, 64, &stall, None);
        assert_eq!(first.flags, PROCESS_LATE | PROCESS_FALLBACK);
        let started = Instant::now();
        loop {
            let outcome = rig.process(&hang, 64, &[], None);
            if outcome.flags & PROCESS_LATE != 0 {
                late += 1;
                std::thread::sleep(Duration::from_millis(1));
            } else {
                return (late, outcome.flags);
            }
            assert!(started.elapsed() < LONG);
        }
    });
    assert!(late > 5);
    assert_eq!(resynced, PROCESS_OK | PROCESS_RESYNCED);
    assert_eq!(calls, 0, "allocator calls in the late / resync path");

    // 4. The container dies while the audio thread waits for it, and afterwards.
    let fatal = [Event::note_on(0, 0, tb::crash::NOTE_ABORT, 100)];
    let (calls, _) = allocator_calls(|| {
        let outcome = rig.process(&crash, 256, &fatal, None);
        assert!(outcome.fell_back());
        for _ in 0..300 {
            let outcome = rig.process(&crash, 256, &[], None);
            assert!(outcome.fell_back() && outcome.flags & (PROCESS_LOST | PROCESS_LATE) != 0);
        }
    });
    assert_eq!(calls, 0, "allocator calls while and after the container crashed");
    env.finish();
}

struct Timing {
    median: Duration,
    p99: Duration,
    p999: Duration,
    worst: Duration,
    plugin_median: Duration,
    fallbacks: usize,
}

fn measure(instance: &Instance, rig: &mut Rig, frames: usize, blocks: usize, pace: Option<Duration>) -> Timing {
    for _ in 0..200 {
        rig.process(instance, frames, &[], None);
    }
    let mut samples = Vec::with_capacity(blocks);
    let mut plugin = Vec::with_capacity(blocks);
    let mut fallbacks = 0;
    let mut next = Instant::now();
    for _ in 0..blocks {
        if let Some(period) = pace {
            next += period;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
        }
        let started = Instant::now();
        let outcome = rig.process(instance, frames, &[], None);
        samples.push(started.elapsed());
        plugin.push(Duration::from_nanos(outcome.plugin_ns));
        fallbacks += outcome.fell_back() as usize;
    }
    samples.sort();
    plugin.sort();
    Timing {
        median: samples[blocks / 2],
        p99: samples[blocks * 99 / 100],
        p999: samples[(blocks * 999 / 1000).min(blocks - 1)],
        worst: samples[blocks - 1],
        plugin_median: plugin[blocks / 2],
        fallbacks,
    }
}

/// Puts the calling thread into (or takes it out of) SCHED_FIFO, as a host's audio thread
/// would be. Needs privileges; returns whether it worked. Linux only.
fn set_realtime(on: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        let param = libc::sched_param { sched_priority: if on { 10 } else { 0 } };
        let policy = if on { libc::SCHED_FIFO } else { libc::SCHED_OTHER };
        // SAFETY: valid parameter block; 0 addresses the calling thread.
        unsafe { libc::sched_setscheduler(0, policy, &param) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = on;
        false
    }
}

/// A quick sanity bound that runs with the normal suite.
#[test]
fn round_trip_overhead_is_small() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let copy = active(env.sup(), effect("copy"));
    let mut rig = Rig::new();
    let timing = measure(&copy, &mut rig, 128, 3000, None);
    println!(
        "128 frames, 3000 blocks back to back: median {:?}, p99 {:?}, worst {:?}",
        timing.median, timing.p99, timing.worst
    );
    assert_eq!(timing.fallbacks, 0);
    assert!(timing.median < Duration::from_millis(2), "median round trip {:?}", timing.median);
    env.finish();
}

/// The measurement quoted in docs/PLUGIN_CONTAINER.md. Run alone:
/// `cargo test --release -p stpc-container --test realtime -- --ignored --nocapture --test-threads=1`
#[test]
#[ignore = "measurement; run alone with --ignored --nocapture"]
fn measure_round_trip_overhead() {
    let blocks: usize = std::env::var("STPC_BENCH_BLOCKS").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000);
    let paced_blocks = blocks / 10;
    println!("\nround trip of one stereo block through the `copy` plug-in (host process() call, wall clock)");
    println!(
        "{:<44} {:>6} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "configuration", "frames", "median", "p99", "p99.9", "worst", "plug-in"
    );
    // (name, wake-up, spin, measuring thread at real-time priority like an audio thread)
    let configurations: [(&str, WakePreference, Duration, bool); 5] = [
        ("futex", WakePreference::Address, Duration::ZERO, false),
        ("futex, RT host thread", WakePreference::Address, Duration::ZERO, true),
        ("futex + 100us spin, RT host", WakePreference::Address, Duration::from_micros(100), true),
        ("pipe", WakePreference::Pipe, Duration::ZERO, false),
        ("pipe, RT host thread", WakePreference::Pipe, Duration::ZERO, true),
    ];
    for (name, wake, spin, realtime) in configurations {
        let env = TestEnv::with(|c| {
            c.policy = GroupingPolicy::Shared;
            c.wake = wake;
            c.spin = spin;
        });
        let copy = active(env.sup(), effect("copy"));
        let mut rig = Rig::new();
        // Only now, so that neither the supervisor's threads nor the container inherit it.
        if realtime && !set_realtime(true) {
            println!("{name}: skipped (no permission for SCHED_FIFO)");
            env.finish();
            continue;
        }
        for frames in [64usize, 128, 512] {
            let period = Duration::from_secs_f64(frames as f64 / 48_000.0);
            for (label, count, pace) in
                [("back to back", blocks, None), ("paced at block rate", paced_blocks, Some(period))]
            {
                let t = measure(&copy, &mut rig, frames, count, pace);
                println!(
                    "{:<44} {:>6} {:>9.1?} {:>9.1?} {:>9.1?} {:>9.1?} {:>9.1?}{}",
                    format!("{name}; {label}"),
                    frames,
                    t.median,
                    t.p99,
                    t.p999,
                    t.worst,
                    t.plugin_median,
                    if t.fallbacks > 0 {
                        format!("  ({} of {count} blocks missed the {:?} deadline)", t.fallbacks, LONG)
                    } else {
                        String::new()
                    }
                );
            }
        }
        set_realtime(false);
        env.finish();
    }
    // The in-process baseline: what the same copy costs without a process boundary.
    let mut rig = Rig::new();
    for frames in [64usize, 128, 512] {
        let mut samples: Vec<Duration> = (0..blocks)
            .map(|_| {
                let started = Instant::now();
                for channel in 0..2 {
                    let (input, output) = (&rig.inputs[channel][..frames], &mut rig.outputs[channel][..frames]);
                    output.copy_from_slice(std::hint::black_box(input));
                }
                started.elapsed()
            })
            .collect();
        samples.sort();
        println!(
            "{:<44} {:>6} {:>9.1?} {:>9.1?}",
            "in-process copy (baseline)",
            frames,
            samples[blocks / 2],
            samples[blocks * 99 / 100]
        );
    }
}

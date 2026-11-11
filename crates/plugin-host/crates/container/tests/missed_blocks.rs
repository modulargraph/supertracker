//! What becomes of the events of a block the plug-in does not process: they reach it with
//! the next block it takes, at that block's first frame, in their order, before the block's
//! own events, and once. Each test makes the container miss blocks in one of the ways a
//! block can be missed, with the `events` test plug-in, which keeps a log of what it was
//! given:
//!
//! - the client does not reach the container at all (`Instance::hold_blocks`, standing in
//!   for a control operation that holds the instance);
//! - the container answers without processing (its main thread keeps the real-time thread
//!   out of `process`);
//! - the plug-in is late with a block: its events have arrived when it finishes, and are
//!   not sent again; what comes while it is busy waits;
//! - a plug-in that stays away past the bound: everything that waits is given up together;
//!   and nothing waits for a plug-in whose container is lost.

mod common;

use std::time::{Duration, Instant};

use common::*;
use stpc_container::test_backend::events as ev;

/// A key, its release, the sustain pedal and a parameter change: the events of one block.
fn phrase(offset: u32) -> Vec<Event> {
    vec![Event::note_on(offset, 0, 60, 100), Event::note_off(offset + 1, 0, 60), Event::midi(offset + 2, &[0xB0, 64, 127]), Event::param(offset + 3, 7, 42.0)]
}

/// The log entries of `events` as the plug-in records them, at `offset` or at their own.
fn entries(events: &[Event], offset: Option<u32>) -> Vec<f64> {
    events
        .iter()
        .map(|event| {
            let offset = offset.unwrap_or(event.offset);
            if event.kind == EVENT_PARAM {
                ev::param(offset, event.id, event.value)
            } else {
                ev::midi(offset, event.midi_bytes())
            }
        })
        .collect()
}

/// Everything the plug-in was given so far, in order.
fn log(instance: &Instance) -> Vec<f64> {
    let count = instance.parameter(ev::COUNT).unwrap().unwrap() as u64;
    (0..count).map(|index| instance.parameter(ev::LOG + index).unwrap().unwrap()).collect()
}

fn block(instance: &Instance, events: &[Event]) -> ProcessOutcome {
    run(instance, &[], 2, 64, events).outcome
}

#[test]
fn events_of_a_block_the_client_could_not_send_arrive_with_the_next_in_order() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), instrument("events"));
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    // Two blocks are missed; each carries events.
    instance.hold_blocks(2);
    let (first, second) = (phrase(10), vec![Event::note_on(5, 0, 72, 90), Event::midi(6, &[0xE0, 0, 96])]);
    assert_eq!(block(&instance, &first).flags, PROCESS_BUSY | PROCESS_FALLBACK);
    assert_eq!(block(&instance, &second).flags, PROCESS_BUSY | PROCESS_FALLBACK);
    assert!(log(&instance).is_empty(), "nothing reached the plug-in yet");
    // The next block has events of its own: they follow what waited, at their offsets.
    let own = vec![Event::note_off(0, 0, 72), Event::param(20, 3, 9.0)];
    assert_eq!(block(&instance, &own).flags, PROCESS_OK);
    let mut wanted = entries(&first, Some(0));
    wanted.extend(entries(&second, Some(0)));
    wanted.extend(entries(&own, None));
    assert_eq!(log(&instance), wanted, "what waited comes first, at frame 0, in its order");
    // Once: the block after that carries nothing over.
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    assert_eq!(log(&instance).len(), wanted.len());
    env.finish();
}

#[test]
fn events_of_a_block_the_container_answered_without_processing_arrive_with_the_next() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    let instance = active(env.sup(), instrument("events"));
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    // The plug-in's main thread keeps processing out for a while (as a parameter flush or
    // a reset does): blocks asked for meanwhile come back "busy", unprocessed.
    std::thread::scope(|scope| {
        scope.spawn(|| instance.set_parameter(ev::EXCLUDE_MS, 400.0).unwrap());
        let started = Instant::now();
        let mut sent: Vec<Event> = Vec::new();
        let mut number = 0u8;
        // One event per block until the container has refused some and processes again.
        let mut refused = 0;
        loop {
            let event = Event::note_on(u32::from(number % 60), 0, number % 128, 1 + number % 127);
            let outcome = block(&instance, &[event]);
            sent.push(event);
            number = number.wrapping_add(1);
            if outcome.flags == PROCESS_BUSY | PROCESS_FALLBACK {
                refused += 1;
            } else {
                assert_eq!(outcome.flags, PROCESS_OK);
                if refused > 0 {
                    break;
                }
            }
            assert!(started.elapsed() < LONG, "the container never refused a block");
            std::thread::sleep(Duration::from_millis(2));
        }
        // Every event arrived, in order, exactly once; those of refused blocks at frame 0
        // of the block that took them.
        let seen = log(&instance);
        assert_eq!(seen.len(), sent.len(), "{refused} blocks were refused");
        for (index, (seen, sent)) in seen.iter().zip(&sent).enumerate() {
            let (own, carried) = (ev::midi(sent.offset, sent.midi_bytes()), ev::midi(0, sent.midi_bytes()));
            assert!(*seen == own || *seen == carried, "event {index} arrived as {seen}, sent as {own}");
        }
        let carried = seen.iter().zip(&sent).filter(|(seen, sent)| sent.offset != 0 && **seen == ev::midi(0, sent.midi_bytes())).count();
        assert!(carried > 0, "some events waited for a block");
    });
    env.finish();
}

#[test]
fn events_of_a_late_block_arrive_once_and_what_follows_waits_for_the_container() {
    let env = TestEnv::with(|config| {
        config.policy = GroupingPolicy::Shared;
        config.hang_timeout = Duration::from_secs(30);
    });
    let instance = active(env.sup(), InstanceConfig { deadline: Some(Duration::from_millis(30)), ..instrument("events") });
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    // The plug-in stalls on this block: the client gives it up, the container finishes it.
    let mut late = vec![Event::param(0, ev::PROCESS_SLEEP_MS, 300.0)];
    late.extend(phrase(8));
    assert_eq!(block(&instance, &late).flags, PROCESS_LATE | PROCESS_FALLBACK);
    // While the container is busy, further blocks are not sent; their events wait.
    let mut waiting: Vec<Event> = Vec::new();
    let started = Instant::now();
    let mut number = 0u8;
    let resumed = loop {
        let event = Event::note_on(30, 0, 40 + number % 40, 100);
        let outcome = block(&instance, &[event]);
        number += 1;
        if outcome.processed() {
            break (outcome, event);
        }
        assert_eq!(outcome.flags, PROCESS_LATE | PROCESS_FALLBACK);
        waiting.push(event);
        assert!(started.elapsed() < LONG);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(resumed.0.flags, PROCESS_OK | PROCESS_RESYNCED);
    assert!(waiting.len() > 3, "only {} blocks went by while the container was busy", waiting.len());
    // The late block's events at their own offsets (the plug-in processed that block),
    // then what waited at frame 0 of the block that resumed, then that block's own event.
    let mut wanted = entries(&late, None);
    wanted.extend(entries(&waiting, Some(0)));
    wanted.extend(entries(&[resumed.1], None));
    assert_eq!(log(&instance), wanted);
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    assert_eq!(log(&instance).len(), wanted.len(), "nothing is sent twice");
    env.finish();
}

#[test]
fn a_plugin_that_stays_away_past_the_bound_loses_everything_that_waited_together() {
    let env = TestEnv::new(GroupingPolicy::Shared);
    // Room for eight events.
    let instance = active(env.sup(), InstanceConfig { event_capacity: 8, ..instrument("events") });
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    instance.hold_blocks(3);
    // Six wait; four more do not fit: all six are given up, and the caller is told. The
    // four are kept.
    let (first, second) = (vec![Event::note_on(0, 0, 60, 100); 6], phrase(0));
    assert_eq!(block(&instance, &first).flags, PROCESS_BUSY | PROCESS_FALLBACK);
    assert_eq!(block(&instance, &second).flags, PROCESS_BUSY | PROCESS_FALLBACK | PROCESS_EVENTS_LOST);
    assert_eq!(block(&instance, &[]).flags, PROCESS_BUSY | PROCESS_FALLBACK, "told once");
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    assert_eq!(log(&instance), entries(&second, Some(0)), "the note-ons that were given up never arrive, with or without their note-offs");

    // Time is the other bound: what has waited for more than two seconds of audio (at
    // 48 kHz, 24 blocks of 4096 frames) is given up when the next block comes.
    let patience = (48_000.0 * EVENT_PATIENCE_SECONDS) as usize / 4096 + 1;
    instance.hold_blocks(patience as u32 + 1);
    let before = log(&instance).len();
    assert_eq!(run(&instance, &[], 2, 4096, &[Event::note_off(0, 0, 60)]).outcome.flags, PROCESS_BUSY | PROCESS_FALLBACK);
    for _ in 1..patience {
        assert_eq!(run(&instance, &[], 2, 4096, &[]).outcome.flags, PROCESS_BUSY | PROCESS_FALLBACK);
    }
    assert_eq!(run(&instance, &[], 2, 4096, &[]).outcome.flags, PROCESS_BUSY | PROCESS_FALLBACK | PROCESS_EVENTS_LOST);
    assert_eq!(block(&instance, &[]).flags, PROCESS_OK);
    assert_eq!(log(&instance).len(), before);

    // A lost container takes what waited with it; the restored plug-in starts afresh.
    let crashing = active(env.sup(), InstanceConfig { event_capacity: 8, ..instrument("events") });
    crashing.hold_blocks(1);
    assert_eq!(block(&crashing, &phrase(0)).flags, PROCESS_BUSY | PROCESS_FALLBACK);
    let pid = env.live_pids()[0];
    // SAFETY: a signal to a process this test started.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let started = Instant::now();
    while crashing.status() != InstanceStatus::Lost {
        assert!(started.elapsed() < LONG);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(block(&crashing, &[]).flags, PROCESS_LOST | PROCESS_FALLBACK);
    assert_eq!(block(&crashing, &[Event::note_on(0, 0, 61, 1)]).flags, PROCESS_LOST | PROCESS_FALLBACK, "events for a lost plug-in are not kept");
    crashing.restore().expect("restore");
    assert_eq!(block(&crashing, &[]).flags, PROCESS_OK);
    assert!(log(&crashing).is_empty());
    // The same when no block went by between the loss and the restore: what waited is given
    // up with the restored plug-in's first block (the caller is told), and that block's own
    // events arrive.
    crashing.hold_blocks(1);
    assert_eq!(block(&crashing, &phrase(0)).flags, PROCESS_BUSY | PROCESS_FALLBACK);
    let pid = env.live_pids().into_iter().find(|live| *live != pid).expect("the restarted container");
    // SAFETY: a signal to a process this test started.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let started = Instant::now();
    while crashing.status() != InstanceStatus::Lost {
        assert!(started.elapsed() < LONG);
        std::thread::sleep(Duration::from_millis(2));
    }
    crashing.restore().expect("restore");
    let own = [Event::note_on(3, 0, 70, 9)];
    assert_eq!(block(&crashing, &own).flags, PROCESS_OK | PROCESS_EVENTS_LOST);
    assert_eq!(log(&crashing), entries(&own, None));
    drop(instance);
    drop(crashing);
    env.finish();
}

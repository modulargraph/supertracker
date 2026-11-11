//! Scheduled output end to end, against in-memory ports: the audio thread's handle, the sender
//! thread's timing, MIDI clock from a transport position stream, and panic.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use st_midi::memory::{MemoryPorts, Sent};
use st_midi::{ClockSettings, InputRouting, MidiOptions, MidiService, ReturnCompensation};

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Instruments 0 and 3 are external: 0 on device 500 channel 1, 3 on device 600 channel 10.
fn project() -> serde_json::Value {
    json!({
        "instruments": [
            {"type": "midi", "midiOutput": 500, "midiChannel": 1},
            {"type": "sample"},
            {"type": "midi", "midiOutput": 404, "midiChannel": 3},
            {"type": "midi", "midiOutput": 600, "midiChannel": 10},
        ],
        "tracks": [{"instrument": 0}, {"instrument": 1}, {"instrument": 2}, {"instrument": 3}],
    })
}

fn service(ports: &MemoryPorts) -> Arc<MidiService> {
    ports.add_output(500, "Synth A");
    ports.add_output(600, "Synth B");
    let service = MidiService::new(MidiOptions::in_memory(ports)).unwrap();
    service.set_project(&project());
    service
}

/// How late each message left, in microseconds, relative to `origin` plus its expected offset.
fn lateness(sent: &[Sent], origin: Instant, expected: &[Duration]) -> Vec<i64> {
    sent.iter().zip(expected).map(|(sent, offset)| sent.at.duration_since(origin).as_micros() as i64 - offset.as_micros() as i64).collect()
}

#[test]
fn notes_leave_at_the_time_of_their_frame() {
    let ports = MemoryPorts::new();
    let service = service(&ports);
    let mut midi = service.render_output();
    let clock = service.clock();

    // A 48 kHz block of 4800 frames (100 ms) that starts playing 40 ms from now.
    let origin = Instant::now() + Duration::from_millis(40);
    midi.begin_block(clock.at(origin), 4800, 48_000.0);
    // Handed over out of time order, as an engine may: the device still gets them in time order.
    assert!(midi.note(0, false, 60, 0, 3840), "an external instrument's note is taken");
    assert!(midi.note(0, true, 60, 100, 960));
    assert!(midi.note(3, true, 36, 127, 0));
    assert!(!midi.note(1, true, 60, 100, 0), "a built-in instrument's note is left to the engine");
    assert!(midi.note(2, true, 60, 100, 0), "an external instrument whose device is absent is still external");
    assert!(!midi.note(9999, true, 60, 100, 0), "an instrument beyond the project has no route");
    assert!(ports.sent().is_empty(), "nothing is sent before its time");

    wait_for("the three notes", || ports.sent().len() == 3);
    let sent = ports.sent();
    assert_eq!(sent.iter().map(|sent| (sent.output, sent.bytes.clone())).collect::<Vec<_>>(), [(600, vec![0x99, 36, 127]), (500, vec![0x90, 60, 100]), (500, vec![0x80, 60, 0])]);
    let late = lateness(&sent, origin, &[Duration::ZERO, Duration::from_millis(20), Duration::from_millis(80)]);
    // Never early; late by a thread wake-up at most (the bound is generous: test machines are busy).
    assert!(late.iter().all(|late| (0..100_000).contains(late)), "lateness in microseconds: {late:?}");
    println!("note lateness in microseconds: {late:?}");
    assert_eq!(service.output_stats().sent, 3);
    assert_eq!(service.output_stats().dropped, 0);

    // Without device time (offline, or a device that reports none) messages go out at once.
    midi.begin_block(0, 512, 48_000.0);
    let before = Instant::now();
    midi.note(0, true, 61, 90, 500);
    wait_for("the immediate note", || ports.sent().len() == 4);
    assert!(ports.sent()[3].at.duration_since(before) < Duration::from_millis(50));
}

#[test]
fn returned_instruments_are_sent_early_by_the_clock_lead() {
    let ports = MemoryPorts::new();
    let service = service(&ports);
    // Instrument 0 is heard back through a compensated line input; the input path takes 10 ms.
    service.set_return_compensation(ReturnCompensation { automatic_lead_milliseconds: 10.0, compensated: true, instruments: vec![0] });
    assert_eq!(service.host().route(0).lead, 10_000_000);
    assert_eq!(service.host().route(3).lead, 0);
    let mut midi = service.render_output();
    let origin = Instant::now() + Duration::from_millis(60);
    midi.begin_block(service.clock().at(origin), 4800, 48_000.0);
    midi.note(0, true, 60, 100, 0);
    midi.note(3, true, 60, 100, 0);
    wait_for("both notes", || ports.sent().len() == 2);
    let sent = ports.sent();
    assert_eq!((sent[0].output, sent[1].output), (500, 600), "the returned instrument's note goes first");
    let lead = sent[1].at.duration_since(sent[0].at).as_micros() as i64;
    assert!((5_000..60_000).contains(&lead), "about 10 ms early: {lead} µs");
    // Uncompensated returns need no lead.
    service.set_return_compensation(ReturnCompensation { automatic_lead_milliseconds: 10.0, compensated: false, instruments: vec![0] });
    assert_eq!(service.host().route(0).lead, 0);
}

#[test]
fn clock_follows_the_transport_position_stream() {
    let ports = MemoryPorts::new();
    let saved: Arc<Mutex<Vec<ClockSettings>>> = Arc::default();
    ports.add_output(500, "Synth A");
    ports.add_output(600, "Synth B");
    let service = MidiService::new(MidiOptions {
        save_clock_settings: Box::new({
            let saved = saved.clone();
            move |settings| {
                saved.lock().unwrap().push(settings.clone());
                Ok(())
            }
        }),
        ..MidiOptions::in_memory(&ports)
    })
    .unwrap();
    let status = service.clock_command(serde_json::from_value(json!({})).unwrap()).unwrap();
    assert!(!status.enabled && status.devices.len() == 2 && status.devices.iter().all(|device| device.clock && !device.transport));
    assert!(saved.lock().unwrap().is_empty(), "reading stores nothing");

    let mut midi = service.render_output();
    let clock = service.clock();
    // 120 BPM at 48 kHz: a pulse every 1000 frames (20.8 ms).
    let block = |midi: &mut st_midi::RenderOutput, start: Instant, beat: f64, playing: bool| {
        midi.begin_block(clock.at(start), 4800, 48_000.0);
        midi.clock(beat, playing, 120.0);
    };
    let origin = Instant::now() + Duration::from_millis(30);
    block(&mut midi, origin, 0.0, true);
    std::thread::sleep(Duration::from_millis(60));
    assert!(ports.sent().is_empty(), "no clock while it is off");

    // Clock to both outputs; Start/Stop only to Synth B.
    let status = service.clock_command(serde_json::from_value(json!({"enabled": true, "devices": [{"id": 600, "transport": true}]})).unwrap()).unwrap();
    assert!(status.enabled && status.devices[1].transport && !status.devices[0].transport);
    assert!(saved.lock().unwrap().last().unwrap().device(600).transport);
    assert!(service.host().clock_active());
    // The generator saw "playing" already: stopping sends Stop to B only, and the clock runs on.
    let origin = Instant::now() + Duration::from_millis(30);
    block(&mut midi, origin, 0.2, false);
    wait_for("a stopped block", || ports.sent().len() == 11);
    assert_eq!(ports.sent_messages(500), vec![vec![0xF8]; 5], "clock-only outputs never get Start/Stop");
    assert_eq!(ports.sent_messages(600), [vec![0xFC], vec![0xF8], vec![0xF8], vec![0xF8], vec![0xF8], vec![0xF8]]);
    let pulses: Vec<Sent> = ports.sent().into_iter().filter(|sent| sent.output == 500).collect();
    let expected: Vec<Duration> = (0..5).map(|pulse| Duration::from_micros(pulse * 20_833)).collect();
    let late = lateness(&pulses, origin, &expected);
    assert!(late.iter().all(|late| (0..100_000).contains(late)), "pulse lateness in microseconds: {late:?}");
    println!("pulse lateness in microseconds: {late:?}");

    // Play from the top: Start, then pulses. Mid-song: Song Position, then Continue.
    ports.clear_sent();
    block(&mut midi, Instant::now() + Duration::from_millis(5), 0.0, true);
    wait_for("a playing block", || ports.sent_messages(600).len() == 6);
    assert_eq!(ports.sent_messages(600)[..2], [vec![0xFA], vec![0xF8]]);
    block(&mut midi, Instant::now(), 0.2, false);
    wait_for("the stopped block to play out", || ports.sent_messages(600).len() == 12);
    assert_eq!(ports.sent_messages(600)[6], [0xFC]);
    ports.clear_sent();
    block(&mut midi, Instant::now() + Duration::from_millis(5), 2.5, true);
    wait_for("the continue", || ports.sent_messages(600).len() >= 2);
    assert_eq!(ports.sent_messages(600)[..2], [vec![0xF2, 10, 0], vec![0xFB]]);
    assert!(!ports.sent_messages(500).iter().any(|message| message != &[0xF8]));

    // An output that loses Start/Stop while the song plays gets a Stop, so its sequencer does not run on.
    wait_for("the block to play out", || ports.sent_messages(600).len() == 7);
    ports.clear_sent();
    service.clock_command(serde_json::from_value(json!({"devices": [{"id": 600, "transport": false}]})).unwrap()).unwrap();
    wait_for("the stop for the removed transport output", || ports.sent_messages(600) == [vec![0xFC]]);
    assert!(ports.sent_messages(500).is_empty());
    // Turning the clock off ends it; a manual lead is stored and reported.
    let status = service.clock_command(serde_json::from_value(json!({"enabled": false, "leadMilliseconds": 18})).unwrap()).unwrap();
    assert!(!status.enabled && status.lead_milliseconds == Some(18.0));
    assert!(!service.host().clock_active());
    ports.clear_sent();
    block(&mut midi, Instant::now(), 3.0, true);
    std::thread::sleep(Duration::from_millis(30));
    assert!(ports.sent().is_empty());
    let shown = serde_json::to_value(service.clock_status()).unwrap();
    assert_eq!(shown["leadMilliseconds"], 18.0);
    assert_eq!(shown["automaticLeadMilliseconds"], 0.0);
    assert_eq!(shown["running"], false);
    assert_eq!(shown["devices"][0], json!({"id": 500, "name": "Synth A", "offline": false, "clock": true, "transport": false}));
}

#[test]
fn clock_settings_survive_and_follow_hot_plug() {
    let ports = MemoryPorts::new();
    ports.add_output(500, "Synth A");
    let mut stored = ClockSettings { enabled: true, ..ClockSettings::default() };
    stored.apply(&serde_json::from_value(json!({"devices": [{"id": 700, "transport": true}, {"id": 500, "clock": false}]})).unwrap());
    let service = MidiService::new(MidiOptions { clock_settings: stored, ..MidiOptions::in_memory(&ports) }).unwrap();
    assert!(!service.host().clock_active(), "the only output present does not want clock");
    // The device the stored choice names is plugged in later: it gets clock and transport.
    ports.add_output(700, "Drum machine");
    assert!(service.refresh_devices());
    assert!(service.host().clock_active());
    let mut midi = service.render_output();
    midi.begin_block(service.clock().now(), 4800, 48_000.0);
    midi.clock(0.0, true, 120.0);
    wait_for("clock on the new device", || ports.sent_messages(700).len() == 6);
    assert_eq!(ports.sent_messages(700)[0], [0xFA]);
    assert!(ports.sent_messages(500).is_empty());
    // A setting that cannot be stored is an error and changes nothing.
    let failing = MidiService::new(MidiOptions { save_clock_settings: Box::new(|_| Err("the disk is full.".into())), ..MidiOptions::in_memory(&ports) }).unwrap();
    let error = failing.clock_command(serde_json::from_value(json!({"enabled": true})).unwrap()).unwrap_err();
    assert_eq!(error.to_string(), "The MIDI clock settings could not be saved: the disk is full.");
    assert!(!failing.clock_status().enabled && !failing.host().clock_active());
}

#[test]
fn all_notes_off_and_panic_reach_every_external_channel_once() {
    let ports = MemoryPorts::new();
    let service = service(&ports);
    // A second instrument on the same device and channel as instrument 0 must not double the messages.
    let mut project = project();
    project["instruments"].as_array_mut().unwrap().push(json!({"type": "midi", "midiOutput": 500, "midiChannel": 1}));
    service.set_project(&project);
    ports.clear_sent();
    let mut midi = service.render_output();
    midi.begin_block(service.clock().now() + 10_000_000_000, 512, 48_000.0);
    midi.all_notes_off();
    wait_for("the audio thread's all-notes-off", || ports.sent().len() == 8);
    assert_eq!(ports.sent_messages(500), [[0xB0, 64, 0], [0xB0, 66, 0], [0xB0, 67, 0], [0xB0, 123, 0]], "the pedals up, then all notes off; immediate, whatever the block's time");
    assert_eq!(ports.sent_messages(600), [[0xB9, 64, 0], [0xB9, 66, 0], [0xB9, 67, 0], [0xB9, 123, 0]]);

    ports.clear_sent();
    service.panic();
    wait_for("the panic", || ports.sent().len() == 10);
    assert_eq!(ports.sent_messages(500), [[0xB0, 64, 0], [0xB0, 66, 0], [0xB0, 67, 0], [0xB0, 123, 0], [0xB0, 120, 0]]);
    assert_eq!(ports.sent_messages(600), [[0xB9, 64, 0], [0xB9, 66, 0], [0xB9, 67, 0], [0xB9, 123, 0], [0xB9, 120, 0]]);

    // Channel messages for an external instrument go to its device on its channel, timed
    // like its notes and in the order they were handed over: a key comes up, the pedal goes
    // down, a key is struck, all at one frame.
    ports.clear_sent();
    midi.begin_block(service.clock().now() + 20_000_000, 512, 48_000.0);
    assert!(midi.note(3, true, 60, 100, 0));
    assert!(midi.note(3, false, 60, 0, 100));
    assert!(midi.message(3, 0xB5, 64, 127, 100), "the channel is the instrument's");
    assert!(midi.note(3, true, 62, 90, 100));
    assert!(midi.message(3, 0xE0, 0, 96, 100));
    assert!(midi.message(3, 0xB0, 64, 0, 99), "an earlier frame leaves earlier");
    assert!(!midi.message(1, 0xB0, 1, 64, 0), "not an external instrument");
    assert!(midi.message(2, 0xB0, 1, 64, 0), "external, its device absent: nothing is sent");
    wait_for("the scheduled messages", || ports.sent().len() == 6);
    assert_eq!(ports.sent_messages(600), [[0x99, 60, 100], [0xB9, 64, 0], [0x89, 60, 0], [0xB9, 64, 127], [0x99, 62, 90], [0xE9, 0, 96]]);
    assert!(ports.sent_messages(500).is_empty());
}

#[test]
fn route_changes_clear_the_old_channel_and_a_returning_device() {
    let ports = MemoryPorts::new();
    let service = service(&ports);
    ports.clear_sent();
    // Instrument 0 moves from channel 1 to channel 5: the pedals up and all-notes-off on the old
    // channel, and all-notes-off once more a moment later for notes that were already on their way.
    let generation = service.route_generation(0);
    let mut changed = project();
    changed["instruments"][0]["midiChannel"] = json!(5);
    service.set_project(&changed);
    wait_for("the old channel to be cleared", || ports.sent_messages(500).len() == 5);
    assert_eq!(ports.sent_messages(500), [[0xB0, 64, 0], [0xB0, 66, 0], [0xB0, 67, 0], [0xB0, 123, 0], [0xB0, 123, 0]]);
    assert_eq!(service.route_generation(0), generation + 1, "what the instrument was told on the old channel is void");
    service.set_project(&changed);
    assert_eq!(service.route_generation(0), generation + 1, "an unchanged route keeps its generation");
    let sent = ports.sent();
    let gap = sent[4].at.duration_since(sent[3].at);
    assert!(gap > Duration::from_millis(60) && gap < Duration::from_millis(400), "the repeat comes about 100 ms later: {gap:?}");
    assert!(ports.sent_messages(600).is_empty(), "an unchanged route is left alone");
    assert_eq!(service.host().route(0).channel, 4);

    // The device goes away and comes back: its channel is cleared before new notes.
    ports.clear_sent();
    let unplugged = service.route_generation(3);
    ports.remove_output(600);
    assert!(service.refresh_devices());
    assert_eq!(service.host().route(3).output, None);
    let mut midi = service.render_output();
    midi.begin_block(0, 64, 48_000.0);
    assert!(midi.note(3, true, 60, 100, 0), "still an external instrument; the note is dropped, not played by the engine");
    ports.add_output(600, "Synth B");
    assert!(service.refresh_devices());
    wait_for("the returning device to be cleared", || ports.sent_messages(600).len() == 4);
    assert_eq!(ports.sent_messages(600), [[0xB9, 64, 0], [0xB9, 66, 0], [0xB9, 67, 0], [0xB9, 123, 0]]);
    assert!(service.route_generation(3) > unplugged, "a device that is back knows nothing of a pedal it was sent before");
    assert!(service.host().route(3).output.is_some());
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(ports.sent_messages(600).len(), 4, "nothing was left scheduled for the device while it was away");
    assert_eq!(service.output_stats().dropped, 0, "and nothing was sent to it while it was away");

    // An instrument that stops being external is cleared as well.
    ports.clear_sent();
    changed["instruments"][3]["type"] = json!("sample");
    service.set_project(&changed);
    wait_for("the former external instrument", || ports.sent_messages(600).len() == 5);
    assert!(!service.host().route(3).external);
    service.set_routing(InputRouting::default());
    assert!(!service.host().route(0).external);
}

#[test]
fn a_flood_from_the_audio_thread_is_dropped_and_counted_not_blocked_on() {
    let ports = MemoryPorts::new();
    let service = service(&ports);
    let mut midi = service.render_output();
    // Later than the test runs, so nothing leaves while the queue fills.
    midi.begin_block(service.clock().now() + 60_000_000_000, 512, 48_000.0);
    let started = Instant::now();
    for index in 0..100_000 {
        midi.note(0, true, (index % 128) as u8, 100, 0);
    }
    assert!(started.elapsed() < Duration::from_secs(2), "pushing never waits");
    wait_for("the flood to be accounted for", || service.output_stats().dropped > 0);
    let stats = service.output_stats();
    // Everything beyond the queue (4096), the backlog (16 384) and the one message in the sender's hand.
    assert!(stats.dropped >= 100_000 - 4096 - 16_384 - 1 && stats.dropped < 100_000, "{stats:?}");
    assert_eq!(stats.sent, 0);
    // A second audio stream gets a queue of its own.
    drop(midi);
    let mut second = service.render_output();
    second.begin_block(0, 64, 48_000.0);
    second.note(3, true, 1, 1, 0);
    wait_for("the second stream's note", || ports.sent_messages(600).len() == 1);
    // Shutting down silences what was held and closes the devices without waiting for the backlog.
    let started = Instant::now();
    service.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(ports.output_connections(500) + ports.output_connections(600), 0);
}

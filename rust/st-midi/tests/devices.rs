//! The MIDI host against in-memory ports: selection by id, parsing, channel filter, output bytes
//! (tests/MIDILoopbackTests.swift) and hot-plug (tests/MIDIHotplugTests.swift).
//!
//! The Swift tests used private CoreMIDI endpoints. This machine has no MIDI service (no ALSA
//! sequencer device), so the same assertions run against `st_midi::memory::MemoryPorts`; the code
//! under test is everything above the device layer.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use st_midi::host::{HostEvent, HostOptions, MidiHost};
use st_midi::memory::MemoryPorts;
use st_midi::ports::OpenError;
use st_midi::{ConfigureRequest, Message};

type Captured = Arc<Mutex<Vec<[i64; 4]>>>;
type Events = Arc<Mutex<Vec<HostEvent>>>;

fn host_with(ports: &MemoryPorts, input_ids: Option<Vec<i32>>) -> Result<(MidiHost, Captured, Events), st_midi::MidiError> {
    let captured: Captured = Arc::default();
    let events: Events = Arc::default();
    let (sink, log) = (captured.clone(), events.clone());
    let host = MidiHost::new(HostOptions {
        backend: Ok(ports.backend()),
        input_ids,
        poll_interval: None,
        on_message: Arc::new(move |message, source| sink.lock().unwrap().push([i64::from(message.status), i64::from(message.data1), i64::from(message.data2), i64::from(source)])),
        on_event: Arc::new(move |event| log.lock().unwrap().push(event)),
    })?;
    Ok((host, captured, events))
}

fn request(value: serde_json::Value) -> ConfigureRequest {
    serde_json::from_value(value).expect("a configure request")
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

const SOURCE: i32 = -1_234_567; // CoreMIDI ids are signed
const DESTINATION: i32 = 7_654_321;

#[test]
fn loopback_ids_parser_channels_and_output_bytes() {
    let ports = MemoryPorts::new();
    ports.add_input(SOURCE, "SuperTracker Test Source");
    ports.add_input(99, "Unrelated");
    ports.add_output(DESTINATION, "SuperTracker Test Destination");
    let (host, captured, events) = host_with(&ports, Some(vec![SOURCE])).unwrap();
    assert_eq!(ports.input_connections(99), 0, "an initial selection avoids subscribing to unrelated sources");
    let devices = host.configure(&request(json!({"inputIDs": [SOURCE], "outputID": DESTINATION}))).unwrap();
    assert!(devices.sources.iter().any(|source| source.id == SOURCE && source.connected), "the source is discovered");
    assert_eq!(devices.output_id, Some(DESTINATION));
    assert!(devices.outputs[0].selected);
    assert!(matches!(events.lock().unwrap().last(), Some(HostEvent::Configuration(_))));

    // Running status, a clock byte, sustain, pitch bend and Stop keep their bytes and order.
    ports.feed(SOURCE, &[0x90, 60, 100, 61, 0, 0xF8, 0x90, 62, 90, 0xB0, 64, 127, 0xE0, 0, 64, 0xFC]);
    let expected = [[0x90, 60, 100], [0x90, 61, 0], [0xF8, 0, 0], [0x90, 62, 90], [0xB0, 64, 127], [0xE0, 0, 64], [0xFC, 0, 0]];
    {
        let received = captured.lock().unwrap();
        assert_eq!(received.iter().map(|message| [message[0], message[1], message[2]]).collect::<Vec<_>>(), expected);
        assert!(received.iter().all(|message| message[3] == i64::from(SOURCE)), "input keeps its source identity");
    }
    ports.feed(99, &[0x90, 1, 1]);
    assert_eq!(captured.lock().unwrap().len(), expected.len(), "an input that was not selected is not heard");

    // Channel filter: channel 3 only; real-time bytes always pass.
    host.configure(&request(json!({"inputChannel": 3}))).unwrap();
    ports.feed(SOURCE, &[0x90, 65, 80, 0x92, 66, 90, 0xF8]);
    {
        let received = captured.lock().unwrap();
        let filtered: Vec<[i64; 3]> = received[expected.len()..].iter().map(|message| [message[0], message[1], message[2]]).collect();
        assert_eq!(filtered, [[0x92, 66, 90], [0xF8, 0, 0]]);
    }

    // Output: status lengths, channels, all-notes-off, clock, transport and program change bytes.
    let route = host.prepare_destination(Some(DESTINATION)).expect("the output route");
    assert_eq!(route.id, DESTINATION);
    let messages: [&[u8]; 8] = [&[0x92, 60, 110], &[0x82, 60, 0], &[0xB2, 64, 127], &[0xE2, 0, 64], &[0xB2, 123, 0], &[0xF8], &[0xFA], &[0xC9, 64]];
    for bytes in messages {
        host.send_to(route, Message::new(bytes[0], *bytes.get(1).unwrap_or(&0), *bytes.get(2).unwrap_or(&0)), 0);
    }
    assert!(host.send(Message::new(0xF2, 2, 3), None), "a send to the default output");
    let expected_bytes: Vec<u8> = messages.iter().flat_map(|bytes| bytes.to_vec()).chain([0xF2, 2, 3]).collect();
    wait_for("the output bytes", || ports.sent_bytes(DESTINATION).len() >= expected_bytes.len());
    assert_eq!(ports.sent_bytes(DESTINATION), expected_bytes);
    assert_eq!(ports.sent_messages(DESTINATION).len(), 9, "one device message per MIDI message");
    assert!(!host.send(Message::new(0xF0, 0, 0), None) && !host.send(Message::new(0x40, 0, 0), None), "not short messages");
    assert!(!host.send(Message::new(0x90, 1, 1), Some(424_242)), "an unknown destination");

    assert_eq!(host.configure(&request(json!({"channel": 17}))).unwrap_err().to_string(), "MIDI output channel must be between 1 and 16.");
    assert_eq!(host.configure(&request(json!({"inputChannel": 17}))).unwrap_err().to_string(), "MIDI input channel must be 0 (all), or between 1 and 16.");
    assert_eq!(host.configure(&request(json!({"outputChannel": "9"}))).unwrap().channel, 9);
    host.configure(&request(json!({"inputChannel": 0}))).unwrap();
    let before = captured.lock().unwrap().len();
    ports.feed(SOURCE, &[0x80, 60, 0, 0x80, 62, 0, 0x82, 66, 0, 0xB0, 64, 0, 0xB0, 123, 0, 0xB2, 123, 0]);
    assert_eq!(captured.lock().unwrap().len(), before + 6);

    // Turning MIDI input off does not touch output.
    let off = host.configure(&request(json!({"enabled": false}))).unwrap();
    assert!(!off.enabled && off.input_ids.is_empty() && off.requested_input_ids == Some(vec![SOURCE.to_string()]));
    assert_eq!(ports.input_connections(SOURCE), 0);
    ports.feed(SOURCE, &[0x90, 1, 1]);
    assert_eq!(captured.lock().unwrap().len(), before + 6);
    assert!(host.prepare_destination(Some(DESTINATION)).is_some(), "disabling input must not disable hardware output");
    ports.clear_sent();
    assert!(host.send(Message::new(0x92, 64, 100), None) && host.send(Message::new(0x82, 64, 0), None));
    wait_for("the input-disabled output", || ports.sent_bytes(DESTINATION).len() == 6);
    assert_eq!(ports.sent_bytes(DESTINATION), [0x92, 64, 100, 0x82, 64, 0]);
    assert_eq!(host.output_stats().failed, 0);
    host.shutdown();
    assert_eq!(host.configure(&request(json!({"enabled": true}))).unwrap_err().to_string(), "The MIDI host is shut down.");
    assert_eq!(ports.output_connections(DESTINATION), 0, "shutdown closes the devices");
}

#[test]
fn selection_by_name_all_none_and_refusals() {
    let ports = MemoryPorts::new();
    ports.add_input(1, "Keys");
    ports.add_input(2, "Pads");
    ports.add_output(3, "Synth");
    let (host, _, _) = host_with(&ports, None).unwrap();
    let devices = host.devices();
    assert!(devices.all_inputs && devices.requested_input_ids.is_none() && devices.input_ids == [1, 2], "without a selection every input is connected");
    assert_eq!(devices.input_channel, 0);
    assert_eq!(devices.channel, 1);

    assert_eq!(host.configure(&request(json!({"input": "pads"}))).unwrap().input_ids, [2], "names match without regard to case");
    assert_eq!(host.configure(&request(json!({"inputID": "none"}))).unwrap().input_ids, Vec::<i32>::new());
    assert_eq!(host.configure(&request(json!({"inputID": "all"}))).unwrap().input_ids, [1, 2]);
    assert_eq!(host.configure(&request(json!({"inputID": null}))).unwrap().requested_input_ids, Some(vec![]), "null selects none");
    assert_eq!(host.configure(&request(json!({"inputs": ["1", 2.0]}))).unwrap().input_ids, [1, 2]);
    assert_eq!(host.configure(&request(json!({"output": "Synth"}))).unwrap().output_id, Some(3));
    assert_eq!(host.configure(&request(json!({"output": "off"}))).unwrap().output_id, None);
    assert!(host.prepare_destination(None).is_none() && !host.send(Message::new(0x90, 1, 1), None), "no default output: nothing is sent");

    // A request that names a missing device changes nothing.
    let error = host.configure(&request(json!({"inputIDs": [1, 404], "outputID": 3, "channel": 5}))).unwrap_err();
    assert_eq!(error.to_string(), "MIDI input '404' is not connected.");
    assert_eq!(host.configure(&request(json!({"outputID": 405}))).unwrap_err().to_string(), "MIDI output '405' is not connected.");
    let unchanged = host.devices();
    assert!(unchanged.input_ids == [1, 2] && unchanged.output_id.is_none() && unchanged.channel == 1);
    // A name made of digits is an id, never a name.
    ports.add_input(5, "2");
    host.refresh();
    assert_eq!(host.configure(&request(json!({"inputID": "2"}))).unwrap().input_ids, [2]);

    let listing = serde_json::to_value(host.devices()).unwrap();
    assert_eq!(listing["inputs"][1], json!({"id": 2, "name": "Pads", "manufacturer": "Memory", "offline": false, "connected": true}));
    assert_eq!(listing["outputs"][0], json!({"id": 3, "name": "Synth", "manufacturer": "Memory", "offline": false, "selected": false}));
    assert_eq!(listing["sources"], listing["inputs"]);
    assert_eq!(listing["destinations"], listing["outputs"]);
    for key in ["enabled", "inputIDs", "allInputs", "outputID", "requestedInputIDs", "requestedOutputID", "channel", "inputChannel"] {
        assert!(listing.get(key).is_some(), "the listing has {key}");
    }
    assert!(listing.get("error").is_none());
}

#[test]
fn hot_plug_keeps_saved_ids_and_releases_what_an_unplugged_device_held() {
    let ports = MemoryPorts::new();
    let (stable, removed) = (11, 22);
    ports.add_input(stable, "Private stable source");
    ports.add_input(removed, "Private disappearing source");
    // The device disappears between being listed and being opened.
    let mut triggered = false;
    ports.on_open_input(move |ports, id| {
        if id == removed && !triggered {
            triggered = true;
            ports.remove_input(id);
        }
        None
    });
    let (host, captured, events) = host_with(&ports, Some(vec![removed, stable])).unwrap();
    assert_eq!(host.devices().input_ids, [stable], "a vanished source neither stays connected nor keeps the surviving one from connecting");
    assert!(!host.devices().inputs.iter().any(|input| input.id == removed), "and is not listed as present");
    host.refresh(); // the one follow-up look
    host.refresh();
    assert_eq!(ports.input_open_attempts(), 2, "an absent device is not retried over and over");

    host.configure(&request(json!({"channel": 7}))).unwrap();
    assert!(host.devices().requested_input_ids.unwrap().contains(&removed.to_string()), "an unrelated setting must not drop the disconnected selection");

    // It returns with the same id: reconnected by the hot-plug check, and heard.
    ports.add_input(removed, "Private returned source");
    assert!(host.refresh());
    assert_eq!(host.devices().input_ids, [stable, removed]);
    assert!(matches!(events.lock().unwrap().last(), Some(HostEvent::DevicesChanged(devices)) if devices.input_ids == [stable, removed]));
    assert!(!host.refresh(), "nothing changed since");
    ports.feed(removed, &[0x90, 60, 100, 0x80, 60, 0, 0xB0, 123, 0]);
    let id = i64::from(removed);
    assert_eq!(*captured.lock().unwrap(), [[0x90, 60, 100, id], [0x80, 60, 0, id], [0xB0, 123, 0, id]]);

    // Unplugged with a key and the pedal down: exactly one note-off and one pedal-up in its name.
    ports.feed(removed, &[0x91, 62, 90, 0xB1, 64, 127, 0x92, 70, 80, 0x82, 70, 0]);
    assert_eq!(captured.lock().unwrap().len(), 7);
    // Settings resubmits every field; an unchanged route must not forget held keys.
    host.configure(&request(json!({"inputIDs": [removed, stable], "inputChannel": 0, "enabled": true}))).unwrap();
    ports.remove_input(removed);
    assert!(host.refresh());
    assert_eq!(captured.lock().unwrap()[7..], [[0x81, 62, 0, id], [0xB1, 64, 0, id]]);
    assert!(!host.refresh());
    assert_eq!(captured.lock().unwrap().len(), 9);

    // A changed route makes the receiver release everything itself: nothing is repeated on unplug.
    ports.feed(stable, &[0x90, 50, 100]);
    host.configure(&request(json!({"inputChannel": 5}))).unwrap();
    ports.remove_input(stable);
    host.refresh();
    assert_eq!(captured.lock().unwrap().len(), 10, "only the note-on itself");
    // A device that vanishes before its hot-plug check still gets its releases, at the next configure.
    ports.add_input(stable, "Private stable source");
    host.configure(&request(json!({"inputChannel": 0}))).unwrap();
    ports.feed(stable, &[0x93, 40, 1]);
    ports.remove_input(stable);
    host.configure(&request(json!({"channel": 2}))).unwrap();
    assert_eq!(captured.lock().unwrap().last(), Some(&[0x83, 40, 0, i64::from(stable)]));
}

#[test]
fn connection_failures_of_present_devices_stay_visible() {
    let ports = MemoryPorts::new();
    ports.add_input(11, "Stable");
    ports.on_open_input(|_, _| Some(OpenError::Failed("the port is busy (-50).".into())));
    let error = host_with(&ports, Some(vec![11])).err().expect("a refused connection of a present device is an error");
    assert_eq!(error.to_string(), "Connecting MIDI input 'Stable' failed: the port is busy (-50).");
    // Also when the device disappears at the same moment: the failure is not mistaken for an unplug.
    ports.add_input(12, "Port failure");
    ports.on_open_input(|ports, id| {
        ports.remove_input(id);
        Some(OpenError::Failed("invalid port.".into()))
    });
    assert!(host_with(&ports, Some(vec![12])).is_err());
    ports.fail_enumeration(Some("the MIDI server stopped."));
    let error = host_with(&ports, None).err().unwrap();
    assert_eq!(error.to_string(), "Listing MIDI devices failed: the MIDI server stopped.");
}

#[test]
fn orderly_teardown_cycles_and_output_ids_across_unplug() {
    let ports = MemoryPorts::new();
    ports.add_input(1, "Stable");
    for index in 0..32 {
        let id = 100 + index;
        ports.add_input(id, "Teardown");
        let (temporary, _, _) = host_with(&ports, Some(vec![id])).unwrap();
        temporary.shutdown();
        assert_eq!(ports.input_connections(id), 0);
        ports.remove_input(id);
        let (next, _, _) = host_with(&ports, Some(vec![1])).unwrap();
        assert_eq!(next.devices().input_ids, [1]);
        drop(next);
        assert_eq!(ports.input_connections(1), 0, "dropping the host closes its connections");
    }

    ports.add_output(500, "Private output");
    let (host, _, events) = host_with(&ports, Some(vec![1])).unwrap();
    host.configure(&request(json!({"outputID": 500}))).unwrap();
    let route = host.prepare_destination(None).expect("the default output resolves by its saved id");
    assert_eq!(route.id, 500);
    assert_eq!(host.prepare_destination(Some(route.slot.into())), None, "a slot number is not a device id");
    ports.remove_output(500);
    host.refresh();
    assert!(host.prepare_destination(None).is_none());
    wait_for("the unplugged output to be closed", || ports.output_connections(500) == 0);
    let devices = host.configure(&request(json!({"channel": 12, "inputChannel": 4}))).unwrap();
    assert_eq!(devices.requested_output_id.as_deref(), Some("500"), "the disconnected output stays requested");
    assert_eq!(devices.output_id, None);
    ports.add_output(500, "Private output returned");
    host.refresh();
    let restored = host.prepare_destination(None).expect("the replacement with the same id is found");
    assert_eq!((restored.id, restored.slot), (500, route.slot), "and keeps its slot");
    host.configure(&request(json!({"enabled": false}))).unwrap();
    assert!(host.prepare_destination(Some(500)).is_some(), "input enablement does not gate output");

    // A device that refuses messages is reported once per run of failures, not per message.
    ports.fail_output(500, true);
    for _ in 0..5 {
        host.send(Message::new(0x90, 1, 1), None);
    }
    wait_for("the failures", || host.output_stats().failed == 5);
    let errors: Vec<String> = events.lock().unwrap().iter().filter_map(|event| if let HostEvent::Error(message) = event { Some(message.clone()) } else { None }).collect();
    assert_eq!(errors, ["MIDI output failed (the device refused the message)."]);
}

#[test]
fn a_machine_without_a_midi_service_has_no_devices_and_says_why() {
    let host = MidiHost::new(HostOptions {
        backend: Err("The system's MIDI service is not available (no sequencer).".into()),
        input_ids: None,
        poll_interval: Some(Duration::from_millis(5)),
        on_message: Arc::new(|_, _| {}),
        on_event: Arc::new(|_| {}),
    })
    .unwrap();
    let devices = host.devices();
    assert!(devices.inputs.is_empty() && devices.outputs.is_empty());
    assert_eq!(devices.error.as_deref(), Some("The system's MIDI service is not available (no sequencer)."));
    assert!(!host.refresh());
    assert!(host.configure(&request(json!({"enabled": false, "inputChannel": 2}))).is_ok(), "settings that need no device still apply");
    assert_eq!(host.configure(&request(json!({"inputID": 5}))).unwrap_err().to_string(), "MIDI input '5' is not connected.");
    assert!(!host.send(Message::new(0x90, 1, 1), None));
}

#[test]
fn the_watcher_thread_follows_hot_plug_by_itself() {
    let ports = MemoryPorts::new();
    let events: Events = Arc::default();
    let log = events.clone();
    let host = MidiHost::new(HostOptions {
        backend: Ok(ports.backend()),
        input_ids: None,
        poll_interval: Some(Duration::from_millis(5)),
        on_message: Arc::new(|_, _| {}),
        on_event: Arc::new(move |event| log.lock().unwrap().push(event)),
    })
    .unwrap();
    ports.add_input(8, "Late arrival");
    wait_for("the new input", || host.devices().input_ids == [8]);
    ports.remove_input(8);
    wait_for("its removal", || host.devices().input_ids.is_empty());
    assert!(events.lock().unwrap().iter().filter(|event| matches!(event, HostEvent::DevicesChanged(_))).count() >= 2);
    let started = Instant::now();
    drop(host);
    assert!(started.elapsed() < Duration::from_secs(1), "shutdown does not wait for a poll interval");
}

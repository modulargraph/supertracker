//! The MIDI area against in-memory ports: output from the render thread (notes of external
//! instruments at their time, all-notes-off, clock that follows the tempo), and input through
//! the bridge (a key sounds, is reported, and is acknowledged for recording).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_audio::{AudioEngine, EngineOptions, PlayOptions};
use st_host::areas::audio::{AudioOptions, AudioOutput};
use st_host::areas::midi::{connect_output, MidiDevices, MidiOptions};
use st_host::areas::services::ServiceOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};
use st_midi::memory::{MemoryPorts, Sent};
use st_midi::MidiService;
use st_project::Project;

const RATE: f64 = 48_000.0;
const SYNTH: i32 = 700;
const KEYS: i32 = 300;

/// One track that plays an external MIDI instrument (device `SYNTH`, channel 3) on rows 0, 4
/// and 8 of a 16-row pattern, at `bpm` and four lines per beat; a second track with a
/// built-in wavetable instrument.
fn project(bpm: f64) -> Value {
    let note = |pitch: i64| json!({ "note": pitch, "instrument": 0, "velocity": 100 });
    let mut rows = vec![Value::Null; 16];
    (rows[0], rows[4], rows[8]) = (note(60), note(62), note(64));
    json!({
        "version": 2, "name": "MIDI", "bpm": bpm, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": [
            { "id": 0, "name": "Synth", "routingID": "synth", "volume": 1, "pan": 0, "instrument": 0 },
            { "id": 1, "name": "Sine", "routingID": "sine", "volume": 1, "pan": 0, "instrument": 1 }
        ],
        "instruments": [
            { "id": 0, "type": "midi", "name": "Synth", "midiOutput": SYNTH, "midiChannel": 3 },
            { "id": 1, "type": "sample", "name": "Sine", "samplePath": "builtin:wavetables/Basic/Sine.wav", "mode": "forward", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.05 }
        ],
        "patterns": [{ "id": 0, "length": 16, "steps": [rows, vec![Value::Null; 16]], "automation": [] }],
        "sequence": [0], "loopSong": true,
        "routing": { "buses": [], "outputs": {}, "sends": [], "sidechains": [] }
    })
}

/// An engine without a device whose render thread is this test, and a MIDI service on
/// in-memory ports connected to it as the host connects them.
fn offline(ports: &MemoryPorts) -> (Arc<AudioEngine>, Arc<MidiService>) {
    ports.add_output(SYNTH, "Synth");
    let service = MidiService::new(st_midi::MidiOptions::in_memory(ports)).unwrap();
    let engine = AudioEngine::new(EngineOptions::offline()).unwrap();
    connect_output(&engine, &service);
    (engine, service)
}

fn publish(engine: &AudioEngine, service: &MidiService, project: &Value) {
    engine.publish(&Project::from_value(project.clone()).unwrap()).unwrap();
    service.set_project(project);
}

/// Renders `seconds` of audio in 256-frame blocks at the pace of real time, as a device would
/// call the renderer: each block when its time has come. Returns how late the latest block was,
/// in milliseconds: on a busy machine this thread does not always run when it should, and what
/// leaves the outputs is then late by about as much.
fn render_paced(engine: &AudioEngine, seconds: f64) -> f64 {
    const FRAMES: usize = 256;
    let renderer = engine.renderer();
    let period = Duration::from_secs_f64(FRAMES as f64 / RATE);
    let (mut left, mut right) = ([0.0f32; FRAMES], [0.0f32; FRAMES]);
    let start = Instant::now();
    let mut latest = Duration::ZERO;
    for block in 0..(seconds * RATE / FRAMES as f64).ceil() as u32 {
        let due = start + period * block;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        latest = latest.max(Instant::now().saturating_duration_since(due));
        renderer.lock().unwrap().render(&mut left, &mut right);
    }
    latest.as_secs_f64() * 1000.0
}

/// A run in which a block was rendered more than this late is measured again (up to
/// [`ATTEMPTS`] times); the last run counts whatever happened, with the lateness allowed for.
const UNDISTURBED: f64 = 10.0;
const ATTEMPTS: usize = 5;

fn sent_to(ports: &MemoryPorts, output: i32) -> Vec<Sent> {
    ports.sent().into_iter().filter(|sent| sent.output == output).collect()
}

fn with_status(sent: &[Sent], status: u8) -> Vec<Sent> {
    sent.iter().filter(|sent| sent.bytes[0] == status).cloned().collect()
}

/// The sender thread hands messages over at their time; give it a moment to finish.
fn settle() {
    std::thread::sleep(Duration::from_millis(60));
}

#[test]
fn a_pattern_with_a_midi_instrument_plays_its_notes_at_their_times() {
    let ports = MemoryPorts::new();
    let (engine, service) = offline(&ports);
    publish(&engine, &service, &project(120.0));
    assert!(service.host().route(0).external && !service.host().route(1).external, "the project's routes are published");

    // Rows are 125 ms at 120 BPM and four lines per beat: notes at 0, 500 and 1000 ms.
    let (mut sent, mut late) = (Vec::new(), 0.0);
    for _ in 0..ATTEMPTS {
        engine.stop();
        render_paced(&engine, 0.05);
        settle();
        ports.clear_sent();
        engine.play(&PlayOptions { song: true, from_start: true, ..PlayOptions::default() });
        late = render_paced(&engine, 1.2);
        settle();
        sent = sent_to(&ports, SYNTH);
        if late < UNDISTURBED {
            break;
        }
    }
    let ons = with_status(&sent, 0x92);
    assert_eq!(ons.iter().map(|sent| sent.bytes.clone()).collect::<Vec<_>>(), [vec![0x92, 60, 100], vec![0x92, 62, 100], vec![0x92, 64, 100]], "note-ons on channel 3: {sent:?}");
    for pair in ons.windows(2) {
        let apart = pair[1].at.duration_since(pair[0].at).as_secs_f64() * 1000.0;
        assert!((apart - 500.0).abs() < 8.0 + 2.0 * late, "consecutive notes are 500 ms apart, measured {apart:.2} ms (blocks were up to {late:.1} ms late)");
    }
    // Each note is released at the end of its step (one row, 125 ms), before the next begins.
    let offs = with_status(&sent, 0x82);
    assert_eq!(offs.iter().map(|sent| sent.bytes[1]).collect::<Vec<_>>(), [60, 62, 64], "{sent:?}");
    for (on, off) in ons.iter().zip(&offs) {
        let held = off.at.duration_since(on.at).as_secs_f64() * 1000.0;
        assert!(held > 20.0 && held < 400.0, "a note-off follows its note-on within the step's gate, measured {held:.2} ms");
    }
    assert!(with_status(&sent, 0xB2).is_empty(), "no channel-mode messages while playing: {sent:?}");

    // Stop: the channel is cleared (pedal up, all notes off).
    ports.clear_sent();
    engine.stop();
    render_paced(&engine, 0.05);
    settle();
    let sent = sent_to(&ports, SYNTH);
    assert!(sent.iter().any(|sent| sent.bytes == [0xB2, 123, 0]) && sent.iter().any(|sent| sent.bytes == [0xB2, 64, 0]), "all notes off and pedal up on stop: {sent:?}");

    // Panic silences external instruments too, also while stopped.
    ports.clear_sent();
    engine.panic();
    render_paced(&engine, 0.03);
    settle();
    assert!(sent_to(&ports, SYNTH).iter().any(|sent| sent.bytes == [0xB2, 123, 0]), "all notes off on panic");

    // An offline render (an export) plays no external instrument.
    ports.clear_sent();
    engine.play(&PlayOptions { song: true, from_start: true, ..PlayOptions::default() });
    engine.render_offline(48_000, 512);
    settle();
    assert_eq!(sent_to(&ports, SYNTH), [], "render_offline sends no MIDI");
    engine.set_midi_output(None);
}

#[test]
fn midi_clock_follows_the_tempo_and_the_transport() {
    let ports = MemoryPorts::new();
    let (engine, service) = offline(&ports);
    publish(&engine, &service, &project(120.0));
    let status = service.clock_command(serde_json::from_value(json!({ "enabled": true, "devices": [{ "id": SYNTH, "clock": true, "transport": true }] })).unwrap()).unwrap();
    assert!(status.enabled && status.devices.iter().any(|device| device.id == SYNTH && device.clock && device.transport), "{status:?}");

    let pulses_in = |seconds: f64| -> Vec<Sent> {
        ports.clear_sent();
        render_paced(&engine, seconds);
        settle();
        sent_to(&ports, SYNTH)
    };
    let spacing = |pulses: &[Sent]| -> f64 {
        let pulses = with_status(pulses, 0xF8);
        assert!(pulses.len() > 8, "enough pulses to measure: {}", pulses.len());
        pulses.last().unwrap().at.duration_since(pulses[0].at).as_secs_f64() * 1000.0 / (pulses.len() - 1) as f64
    };
    // The mean spacing of the pulses of an undisturbed stretch of `seconds` (see `UNDISTURBED`),
    // and how far a late block can have moved it.
    let measured = |seconds: f64| -> (f64, f64) {
        let mut result = (0.0, 0.0);
        for _ in 0..ATTEMPTS {
            ports.clear_sent();
            let late = render_paced(&engine, seconds);
            settle();
            let pulses = with_status(&sent_to(&ports, SYNTH), 0xF8);
            assert!(pulses.len() > 8, "enough pulses to measure: {}", pulses.len());
            result = (pulses.last().unwrap().at.duration_since(pulses[0].at).as_secs_f64() * 1000.0 / (pulses.len() - 1) as f64, 2.0 * late / (pulses.len() - 1) as f64);
            if late < UNDISTURBED {
                break;
            }
        }
        result
    };

    // Stopped: the clock runs at the project's tempo, 24 pulses per quarter note.
    let (at_120, slack) = measured(0.5);
    assert!((at_120 - 60_000.0 / 120.0 / 24.0).abs() < 0.6 + slack, "20.83 ms per pulse at 120 BPM, measured {at_120:.3}");
    let stopped = pulses_in(0.2);
    assert!(!stopped.iter().any(|sent| [0xFA, 0xFB, 0xFC].contains(&sent.bytes[0])), "no transport message while nothing changes");

    // Play from the start: Start, then pulses at the same tempo.
    engine.play(&PlayOptions { song: true, from_start: true, ..PlayOptions::default() });
    let playing = pulses_in(0.5);
    assert_eq!(with_status(&playing, 0xFA).len(), 1, "one Start: {:?}", playing.iter().map(|sent| sent.bytes[0]).collect::<Vec<_>>());
    assert!(spacing(&playing) > 15.0 && spacing(&playing) < 27.0, "the clock keeps its tempo while playing: {:.3} ms per pulse", spacing(&playing));

    // The tempo changes with the project: twice as fast, half the spacing.
    publish(&engine, &service, &project(240.0));
    let _ = pulses_in(0.1);
    let (at_240, slack) = measured(0.4);
    assert!((at_240 - 60_000.0 / 240.0 / 24.0).abs() < 0.4 + slack, "10.42 ms per pulse at 240 BPM, measured {at_240:.3}");

    // Stop: Stop goes out, the pulses continue.
    engine.stop();
    let after = pulses_in(0.3);
    assert_eq!(with_status(&after, 0xFC).len(), 1);
    assert!(with_status(&after, 0xF8).len() > 8);

    // Clock off: silence.
    service.clock_command(serde_json::from_value(json!({ "enabled": false })).unwrap()).unwrap();
    let _ = pulses_in(0.05);
    assert_eq!(with_status(&pulses_in(0.2), 0xF8).len(), 0);
    engine.set_midi_output(None);
}

struct Fixture {
    host: TestHost,
    services: Host,
    ports: MemoryPorts,
    _folder: TempDir,
}

/// A host with the audio engine on the null device and MIDI on in-memory ports: a keyboard
/// (`KEYS`) and a synth (`SYNTH`).
fn start(label: &str) -> Fixture {
    let folder = TempDir::new(label);
    let ports = MemoryPorts::new();
    ports.add_input(KEYS, "Keys");
    ports.add_output(SYNTH, "Synth");
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(folder.path().join("support"), Arc::new(NoDialogs));
        options.autosave_every = None;
        options.audio = Some(AudioOptions { output: AudioOutput::Null });
        options.services = ServiceOptions { midi: Some(MidiOptions { devices: MidiDevices::Memory(ports.clone()) }), ..ServiceOptions::none() };
        services = Some(install(bridge, options).expect("install the host"));
    });
    host.request("ready", json!({})).expect("the handshake");
    Fixture { host, services: services.unwrap(), ports, _folder: folder }
}

/// Waits for a `midi.note` event that satisfies `wanted`.
fn note_event(host: &TestHost, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match host.wait_for_event("midi.note", Duration::from_millis(100)) {
            Some(event) if wanted(&event) => return event,
            _ => {}
        }
    }
    panic!("no midi.note event where {what}");
}

fn status_where(host: &TestHost, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        if let Some(status) = host.wait_for_event("status", Duration::from_millis(200)) {
            if wanted(&status) {
                return status;
            }
            last = status;
        }
    }
    panic!("no status event where {what}; the last one was {last}");
}

#[test]
fn a_key_on_a_midi_keyboard_sounds_is_reported_and_is_acknowledged_for_recording() {
    let fixture = start("midi-input");
    let (host, ports) = (&fixture.host, &fixture.ports);

    // The devices are part of the handshake and of midi.devices.
    let ready = host.request("ready", json!({})).unwrap();
    for command in ["midi.devices", "midi.configure", "midi.clock", "midi.performance"] {
        assert!(ready["commands"].as_array().unwrap().contains(&json!(command)), "{command} is implemented");
    }
    assert_eq!(ready["midi"]["inputs"][0], json!({ "id": KEYS, "name": "Keys", "manufacturer": "Memory", "offline": false, "connected": true }), "{}", ready["midi"]);
    assert_eq!(ready["midi"]["outputs"][0]["id"], SYNTH);
    let devices = host.request("midi.devices", json!({})).unwrap();
    assert_eq!((devices["enabled"].clone(), devices["inputIDs"].clone()), (json!(true), json!([KEYS])));

    host.request("engine.project", json!({ "project": project(120.0) })).unwrap();
    assert!(fixture.services.services.midi.as_ref().unwrap().service().host().route(0).external, "publication sets the routes");
    // The cursor is on the wavetable track: MIDI input plays it.
    let configured = host.request("midi.configure", json!({ "track": 1, "instrument": 1 })).unwrap();
    assert_eq!(configured["inputIDs"], json!([KEYS]));

    // A key: it sounds (the meters move) and the page hears about it, with the transport position.
    ports.feed(KEYS, &[0x90, 57, 110]);
    let pressed = note_event(host, "the key went down", |event| event["on"] == true);
    assert_eq!(
        (pressed["note"].clone(), pressed["velocity"].clone(), pressed["channel"].clone(), pressed["source"].clone(), pressed["track"].clone(), pressed["instrument"].clone()),
        (json!(57), json!(110), json!(1), json!(KEYS), json!(1), json!(1))
    );
    assert!(pressed["pattern"].is_number() && pressed["row"].is_number() && pressed["position"].is_number(), "{pressed}");
    assert!(pressed.get("inputOwner").is_none(), "not armed: no capture");
    status_where(host, "the note is heard", |status| status["peakL"].as_f64().unwrap() > 0.01);
    ports.feed(KEYS, &[0x80, 57, 0]);
    let released = note_event(host, "the key went up", |event| event["on"] == false);
    assert_eq!((released["note"].clone(), released["track"].clone()), (json!(57), json!(1)));
    status_where(host, "the note has faded", |status| status["peakL"].as_f64().unwrap() < 0.001);

    // Armed: the key is an input edge. The page gets it at once (pending) and again with the
    // audio thread's acknowledgement.
    assert_eq!(host.request("transport.recordArm", json!({ "armed": true })), Ok(json!({ "armed": true })));
    ports.feed(KEYS, &[0x90, 60, 96]);
    let pending = note_event(host, "the capture opens", |event| event["recordingPending"] == true && event["on"] == true);
    let owner = pending["inputOwner"].as_str().expect("the capture's name").to_string();
    assert!(owner.starts_with("midi:"), "{owner}");
    assert_eq!((pending["note"].clone(), pending["velocity"].clone(), pending["track"].clone(), pending["instrument"].clone(), pending["source"].clone()), (json!(60), json!(96), json!(1), json!(1), json!(KEYS)));
    let acknowledged = note_event(host, "the onset is acknowledged", |event| event["recordingOnly"] == true && event["on"] == true);
    assert_eq!(acknowledged["inputOwner"], json!(owner));
    assert!(acknowledged.get("recordingPending").is_none());
    let ack = &acknowledged["inputAcknowledgement"];
    assert_eq!((ack["status"].clone(), ack["applied"].clone(), ack["inputOwner"].clone(), ack["on"].clone(), ack["note"].clone()), (json!("applied"), json!(true), json!(owner), json!(true), json!(60)), "{ack}");
    assert!(ack["recordingContext"]["patterns"].is_array());
    status_where(host, "the recorded note is heard too", |status| status["peakL"].as_f64().unwrap() > 0.01);

    ports.feed(KEYS, &[0x80, 60, 40]);
    let pending = note_event(host, "the release is reported", |event| event["recordingPending"] == true && event["on"] == false);
    assert_eq!(pending["inputOwner"], json!(owner), "the release belongs to the same capture");
    let acknowledged = note_event(host, "the release is acknowledged", |event| event["recordingOnly"] == true && event["on"] == false);
    assert_eq!((acknowledged["inputOwner"].clone(), acknowledged["inputAcknowledgement"]["status"].clone(), acknowledged["inputAcknowledgement"]["on"].clone()), (json!(owner), json!("applied"), json!(false)));
    status_where(host, "the released note has faded", |status| status["peakL"].as_f64().unwrap() < 0.001);

    // A key struck again before its release: the open capture is cancelled, a new one opens.
    ports.feed(KEYS, &[0x90, 62, 90]);
    let first = note_event(host, "a capture opens", |event| event["recordingPending"] == true)["inputOwner"].clone();
    ports.feed(KEYS, &[0x90, 62, 90]);
    let cancelled = note_event(host, "the first capture is cancelled", |event| event["recordingCancelled"] == true);
    assert_eq!((cancelled["inputOwner"].clone(), cancelled["on"].clone(), cancelled["recordingOnly"].clone(), cancelled["reason"].clone()), (first, json!(false), json!(true), json!("retriggered")));
    ports.feed(KEYS, &[0x80, 62, 0]);
    host.request("transport.recordArm", json!({ "armed": false })).unwrap();

    // Transport from the keyboard: Start plays the song, Stop stops it.
    ports.feed(KEYS, &[0xFA]);
    status_where(host, "MIDI Start plays", |status| status["playing"] == true);
    ports.feed(KEYS, &[0xFC]);
    status_where(host, "MIDI Stop stops", |status| status["playing"] == false);

    // A keyboard that disappears is reported; quitting silences the synth and closes the devices.
    ports.remove_input(KEYS);
    fixture.services.services.midi.as_ref().unwrap().service().refresh_devices();
    let changed = loop {
        let status = host.wait_for_event("midi.status", Duration::from_secs(5)).expect("a midi.status event for the unplugged keyboard");
        if status["type"] == "midiDevicesChanged" {
            break status;
        }
    };
    assert!(changed["devices"]["inputs"].as_array().unwrap().iter().all(|input| input["id"] != KEYS || input["offline"] == true), "{changed}");
    ports.clear_sent();
    fixture.services.services.shutdown();
    assert!(ports.sent_messages(SYNTH).contains(&vec![0xB2, 123, 0]), "all notes off when the host quits: {:?}", ports.sent_messages(SYNTH));
    assert_eq!(ports.output_connections(SYNTH), 0, "the output is closed");
}

#[test]
fn notes_of_a_midi_instrument_played_from_the_page_leave_through_its_output() {
    let fixture = start("midi-output");
    let (host, ports) = (&fixture.host, &fixture.ports);
    host.request("engine.project", json!({ "project": project(120.0) })).unwrap();

    // The page's own keyboard (note.on / note.off) on the track of the external instrument.
    host.request("note.on", json!({ "note": 72, "velocity": 90, "track": 0, "instrument": 0 })).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ports.sent_messages(SYNTH).contains(&vec![0x92, 72, 90]) {
        assert!(Instant::now() < deadline, "the note-on never reached the synth: {:?}", ports.sent_messages(SYNTH));
        std::thread::sleep(Duration::from_millis(5));
    }
    host.request("note.off", json!({ "note": 72, "track": 0, "instrument": 0 })).unwrap();
    while !ports.sent_messages(SYNTH).iter().any(|message| message[0] == 0x82 && message[1] == 72) {
        assert!(Instant::now() < deadline, "the note-off never reached the synth: {:?}", ports.sent_messages(SYNTH));
        std::thread::sleep(Duration::from_millis(5));
    }

    // The clock settings are stored in the settings section midi.clock.
    let clock = host.request("midi.clock", json!({ "enabled": true, "devices": [{ "id": SYNTH, "clock": true }] })).unwrap();
    assert_eq!((clock["enabled"].clone(), clock["running"].clone(), clock["devices"][0]["clock"].clone()), (json!(true), json!(true), json!(true)), "{clock}");
    assert_eq!(fixture.services.settings.get_raw("midi.clock").unwrap()["enabled"], true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ports.sent_messages(SYNTH).contains(&vec![0xF8]) {
        assert!(Instant::now() < deadline, "no clock pulse reached the synth");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.request("midi.performance", json!({ "map": null })), Ok(json!({ "active": false })));
}

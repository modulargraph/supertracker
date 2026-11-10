//! The service as `st-host` uses it: the `midi.*` commands with the page's JSON, the events, and
//! input routed to a stand-in for the engine.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::IgnoredAny;
use serde_json::{json, Value};
use st_midi::memory::MemoryPorts;
use st_midi::{InputTarget, Message, MidiOptions, MidiService, NoteHandling, RoutedNote, TransportPosition};

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Records what the engine would be asked to do.
#[derive(Default)]
struct Engine {
    log: Mutex<Vec<String>>,
    capture: Mutex<bool>,
}

impl Engine {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.log.lock().unwrap())
    }
    fn push(&self, entry: String) {
        self.log.lock().unwrap().push(entry);
    }
}

impl InputTarget for Engine {
    fn note_on(&self, note: &RoutedNote) -> NoteHandling {
        self.push(format!("on t{} i{} n{} v{}", note.track, note.instrument, note.note, note.velocity));
        if *self.capture.lock().unwrap() {
            NoteHandling::Captured
        } else {
            NoteHandling::Played(Some(TransportPosition { pattern: 2, row: 17, position: 4.25 }))
        }
    }
    fn note_off(&self, note: &RoutedNote, sound: bool) -> NoteHandling {
        self.push(format!("off t{} n{}{}", note.track, note.note, if sound { "" } else { " (sustained)" }));
        if *self.capture.lock().unwrap() {
            NoteHandling::Captured
        } else {
            NoteHandling::Played(None)
        }
    }
    fn release(&self, track: usize, note: u8) {
        self.push(format!("release t{track} n{note}"));
    }
    fn message(&self, track: usize, message: Message) {
        self.push(format!("message t{track} {:#04x} {} {}", message.status, message.data1, message.data2));
    }
    fn pitch_bend(&self, track: usize, semitones: f32) {
        self.push(format!("bend t{track} {semitones}"));
    }
    fn panic(&self) {
        self.push("panic".into());
    }
    fn transport_start(&self) {
        self.push("start".into());
    }
    fn transport_stop(&self) {
        self.push("stop".into());
    }
    fn frozen(&self, track: usize) {
        self.push(format!("frozen t{track}"));
    }
    fn wake(&self) {
        self.push("wake".into());
    }
}

struct Fixture {
    ports: MemoryPorts,
    service: Arc<MidiService>,
    engine: Arc<Engine>,
    events: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Fixture {
    fn new() -> Self {
        let ports = MemoryPorts::new();
        ports.add_input(11, "Keys");
        ports.add_input(22, "Pads");
        ports.add_output(500, "Synth");
        let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let log = events.clone();
        let service = MidiService::new(MidiOptions { events: Box::new(move |name, payload| log.lock().unwrap().push((name.to_string(), payload))), ..MidiOptions::in_memory(&ports) }).unwrap();
        let engine = Arc::new(Engine::default());
        service.set_target(engine.clone());
        service.set_project(&json!({
            "instruments": [{"type": "sample"}, {"type": "midi", "midiInput": 22, "midiOutput": 500, "midiChannel": 4}, {"type": "sample"}],
            "tracks": [{"instrument": 0}, {"instrument": 1}, {"instrument": 2, "frozen": {"path": "print.wav"}}],
        }));
        Fixture { ports, service, engine, events }
    }

    fn play(&self, source: i32, bytes: &[u8]) {
        self.ports.feed(source, bytes);
        self.service.flush_input();
    }

    fn events(&self, name: &str) -> Vec<Value> {
        let mut events = self.events.lock().unwrap();
        let taken: Vec<Value> = events.iter().filter(|(event, _)| event == name).map(|(_, payload)| payload.clone()).collect();
        events.retain(|(event, _)| event != name);
        taken
    }

    fn configure(&self, payload: Value) -> Result<Value, String> {
        self.service.configure(serde_json::from_value(payload).unwrap()).map(|devices| serde_json::to_value(devices).unwrap()).map_err(|error| error.to_string())
    }
}

#[test]
fn notes_reach_the_engine_and_the_interface() {
    let f = Fixture::new();
    f.play(11, &[0x90, 60, 100, 0xF8, 0xFE]);
    assert_eq!(f.engine.take(), ["on t0 i0 n60 v100"], "clock and active sensing never reach the engine");
    assert_eq!(
        f.events("midi.note"),
        [json!({"on": true, "note": 60, "velocity": 100, "channel": 1, "source": 11, "track": 0, "instrument": 0, "pattern": 2, "row": 17, "position": 4.25})]
    );
    f.play(11, &[0x80, 60, 64]);
    assert_eq!(f.engine.take(), ["off t0 n60"]);
    assert_eq!(f.events("midi.note"), [json!({"on": false, "note": 60, "velocity": 64, "channel": 1, "source": 11, "track": 0, "instrument": 0})]);

    // The interface's cursor moves (midi.configure {track, instrument}); input follows it.
    let devices = f.configure(json!({"track": 0, "instrument": 2})).unwrap();
    assert_eq!(devices["inputIDs"], json!([11, 22]));
    assert_eq!(f.engine.take(), ["wake"], "configuring live input starts the audio engine");
    f.play(11, &[0x90, 62, 90, 0x80, 62, 0]);
    assert_eq!(f.engine.take(), ["on t0 i2 n62 v90", "off t0 n62"]);
    f.events("midi.note");
    f.service.select_destination(2, 2);
    f.play(11, &[0x90, 62, 90]);
    assert_eq!(f.engine.take(), ["frozen t2"], "a frozen track refuses notes");
    assert!(f.events("midi.note").is_empty());
    f.service.select_destination(0, 0);

    // When note recording captures input, the audio side reports the notes itself.
    *f.engine.capture.lock().unwrap() = true;
    f.play(11, &[0x90, 65, 80, 0x80, 65, 0]);
    assert_eq!(f.engine.take().len(), 2);
    assert!(f.events("midi.note").is_empty());
}

#[test]
fn controllers_sustain_and_transport() {
    let f = Fixture::new();
    f.play(11, &[0xB0, 7, 99, 0xE0, 0, 96, 0xD0, 30]);
    assert_eq!(f.engine.take(), ["message t0 0xb0 7 99", "message t0 0xe0 0 96", "bend t0 1", "message t0 0xd0 30 0"]);
    assert_eq!(f.events("midi.message"), [json!({"status": 0xB0, "data1": 7, "data2": 99, "source": 11})], "only control changes are reported");

    f.play(11, &[0xB0, 64, 127, 0x90, 60, 100, 0x80, 60, 0]);
    assert_eq!(f.engine.take(), ["on t0 i0 n60 v100", "off t0 n60 (sustained)"]);
    assert_eq!(f.events("midi.note").len(), 2, "the key release is reported although the sound goes on");
    f.play(11, &[0xB0, 64, 0]);
    assert_eq!(f.engine.take(), ["release t0 n60"]);
    assert!(f.events("midi.message").is_empty(), "the pedal is not reported as a control change");

    f.play(11, &[0xFA, 0xFC, 0xFB]);
    assert_eq!(f.engine.take(), ["start", "stop", "start"]);
    // While we send clock ourselves, an echoed Start must not restart our transport.
    f.service.clock_command(serde_json::from_value(json!({"enabled": true})).unwrap()).unwrap();
    assert_eq!(f.engine.take(), ["wake"], "the clock runs with the audio engine");
    f.play(11, &[0xFA, 0xFC]);
    assert!(f.engine.take().is_empty());
    f.service.clock_command(serde_json::from_value(json!({"enabled": false})).unwrap()).unwrap();

    f.ports.clear_sent();
    f.play(11, &[0xB0, 123, 0]);
    assert_eq!(f.engine.take(), ["panic"]);
    wait_for("external instruments to be silenced", || f.ports.sent_messages(500).len() == 3);
    assert_eq!(f.ports.sent_messages(500), [[0xB3, 64, 0], [0xB3, 123, 0], [0xB3, 120, 0]]);

    // The application's own keys share the pedal bookkeeping.
    assert!(!f.service.app_note_off(0, 60), "no pedal: the note stops");
    assert!(f.service.app_sustain(true).is_empty());
    assert!(f.service.app_note_off(0, 60) && f.service.app_note_off(1, 64));
    f.service.app_note_on(1, 64);
    assert_eq!(f.service.app_sustain(false), [st_midi::Destination { track: 0, note: 60 }]);
}

#[test]
fn an_external_instrument_plays_through_from_its_keyboard() {
    let f = Fixture::new();
    // Device 22 is instrument 1's input: its notes go to track 1, its controllers to the synth's channel.
    f.play(22, &[0x90, 48, 70]);
    assert_eq!(f.engine.take(), ["on t1 i1 n48 v70"]);
    f.ports.clear_sent();
    f.play(22, &[0xB0, 1, 50, 0xE0, 0, 64]);
    wait_for("the controllers on the device", || f.ports.sent_messages(500).len() == 2);
    assert_eq!(f.ports.sent_messages(500), [[0xB3, 1, 50], [0xE3, 0, 64]], "on the instrument's own channel");
    assert_eq!(f.engine.take(), ["bend t1 0"], "not sent to a plug-in as well");
    // With its device unplugged the messages are dropped, not handed to a plug-in.
    f.ports.remove_output(500);
    f.service.refresh_devices();
    f.play(22, &[0xB0, 1, 51]);
    assert!(f.engine.take().is_empty());
}

#[test]
fn held_notes_are_released_only_when_the_input_route_really_changes() {
    let f = Fixture::new();
    f.play(11, &[0xB0, 64, 127, 0x90, 60, 100, 0x90, 64, 100, 0x80, 64, 0]);
    f.engine.take();
    // Settings resubmits the current values: nothing is released.
    let all = f.configure(json!({"inputID": "all", "inputChannel": 0, "enabled": true})).unwrap();
    assert_eq!(all["allInputs"], true);
    assert_eq!(f.engine.take(), ["wake"]);
    // A new input channel is a new route: the held key and the pedalled note are released.
    let devices = f.configure(json!({"inputChannel": 2})).unwrap();
    assert_eq!(devices["inputChannel"], 2);
    assert_eq!(f.engine.take(), ["release t0 n60", "release t0 n64", "wake"]);
    f.play(11, &[0x80, 60, 0]);
    assert!(f.engine.take().is_empty(), "the released note is forgotten");
    f.play(11, &[0x90, 70, 1, 0x91, 71, 1]);
    assert_eq!(f.engine.take(), ["on t0 i0 n71 v1"], "the channel filter applies");
    // A refused request restores nothing and releases nothing.
    assert_eq!(f.configure(json!({"inputChannel": 99})).unwrap_err(), "MIDI input channel must be 0 (all), or between 1 and 16.");
    assert_eq!(f.configure(json!({"inputID": 404})).unwrap_err(), "MIDI input '404' is not connected.");
    assert!(f.engine.take().is_empty());
    f.play(11, &[0x81, 71, 0]);
    assert_eq!(f.engine.take(), ["off t0 n71"]);
    // Turning input off releases what is held and wakes nothing.
    f.play(11, &[0x91, 72, 1]);
    f.engine.take();
    let off = f.configure(json!({"enabled": false})).unwrap();
    assert_eq!(off["enabled"], false);
    assert_eq!(f.engine.take(), ["release t0 n72"]);
    f.play(11, &[0x91, 73, 1]);
    assert!(f.engine.take().is_empty());
}

#[test]
fn an_unplugged_keyboard_releases_its_notes_in_the_engine() {
    let f = Fixture::new();
    f.play(11, &[0x90, 60, 100, 0xB0, 64, 127, 0x90, 62, 100, 0x80, 62, 0]);
    f.engine.take();
    f.events("midi.note");
    f.events("midi.status");
    f.ports.remove_input(11);
    assert!(f.service.refresh_devices());
    f.service.flush_input();
    assert_eq!(f.engine.take(), ["off t0 n60 (sustained)", "release t0 n60", "release t0 n62"], "the key is released, then the pedal lets go of both notes");
    assert_eq!(f.events("midi.note"), [json!({"on": false, "note": 60, "velocity": 0, "channel": 1, "source": 11, "track": 0, "instrument": 0})]);
    let status = f.events("midi.status");
    assert_eq!(status.len(), 1);
    assert_eq!(status[0]["type"], "midiDevicesChanged");
    assert_eq!(status[0]["devices"]["inputIDs"], json!([22]));
}

#[test]
fn commands_speak_the_pages_json() {
    let f = Fixture::new();
    let devices = serde_json::to_value(f.service.devices(IgnoredAny).unwrap()).unwrap();
    assert_eq!(devices["inputs"].as_array().unwrap().len(), 2);
    assert_eq!(devices["outputs"][0]["name"], "Synth");
    // Studio settings: output by id, output channel, input by id.
    let applied = f.configure(json!({"inputChannel": 0, "channel": 3, "inputID": 22, "outputID": 500})).unwrap();
    assert_eq!(applied["inputIDs"], json!([22]));
    assert_eq!(applied["outputID"], 500);
    assert_eq!(applied["channel"], 3);
    assert_eq!(applied["requestedInputIDs"], json!(["22"]));
    let status = f.events("midi.status");
    assert_eq!(status.last().unwrap()["type"], "midiConfiguration");
    assert_eq!(status.last().unwrap()["devices"]["outputID"], 500);
    assert_eq!(f.configure(json!({"outputID": "none"})).unwrap()["outputID"], Value::Null);

    // midi.performance: a 128-row map turns the chord map on, null turns it off.
    let mut rows: Vec<Value> = (0..128).map(|key| json!([key])).collect();
    rows[60] = json!([60, 64, 67]);
    let active = f.service.performance(serde_json::from_value(json!({"map": rows})).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(active).unwrap(), json!({"active": true}));
    f.engine.take();
    f.play(22, &[0x91, 60, 100]);
    assert_eq!(f.engine.take(), ["on t1 i1 n60 v100", "on t1 i1 n64 v100", "on t1 i1 n67 v100"], "device 22 belongs to instrument 1");
    assert!(!f.service.performance(serde_json::from_value(json!({"map": null})).unwrap()).unwrap().active);
    assert!(!f.service.performance(serde_json::from_value(json!({})).unwrap()).unwrap().active);
    f.play(22, &[0x81, 60, 0]);
    assert_eq!(f.engine.take(), ["off t1 n60", "off t1 n64", "off t1 n67"], "the chord is released although the map is gone");

    // midi.clock as the settings dialog sends it.
    let clock = serde_json::to_value(f.service.clock_command(serde_json::from_value(json!({"enabled": true, "devices": [{"id": 500, "clock": true, "transport": true}], "leadMilliseconds": null})).unwrap()).unwrap()).unwrap();
    assert_eq!(clock, json!({"enabled": true, "running": false, "leadMilliseconds": null, "automaticLeadMilliseconds": 0.0, "devices": [{"id": 500, "name": "Synth", "offline": false, "clock": true, "transport": true}]}));
    f.service.set_audio_running(true);
    assert!(f.service.clock_status().running);

    // Blocked input (an export runs) is dropped entirely.
    f.service.set_input_blocked(true);
    f.play(22, &[0x91, 61, 100]);
    assert!(f.engine.take().is_empty());
    f.service.set_input_blocked(false);
    f.service.remap_tracks(&[(0, Some(1))]);
    f.service.shutdown();
    f.service.shutdown();
}

#[test]
fn a_service_without_a_midi_system_still_answers() {
    let service = MidiService::new(MidiOptions { backend: Err("The system's MIDI service is not available (test).".into()), ..MidiOptions::in_memory(&MemoryPorts::new()) }).unwrap();
    let devices = serde_json::to_value(service.devices(IgnoredAny).unwrap()).unwrap();
    assert_eq!(devices["error"], "The system's MIDI service is not available (test).");
    assert_eq!(devices["inputs"], json!([]));
    assert!(service.clock_command(serde_json::from_value(json!({"enabled": true})).unwrap()).unwrap().devices.is_empty());
    let mut midi = service.render_output();
    midi.begin_block(service.clock().now(), 256, 44_100.0);
    midi.clock(0.0, true, 120.0);
    assert!(!midi.note(0, true, 60, 100, 0));
    // The real system backend: present or not, the service starts.
    let system = MidiService::new(MidiOptions::system(Default::default(), Box::new(|_| Ok(())), Box::new(|_, _| {}))).unwrap();
    let listing = system.devices(IgnoredAny).unwrap();
    assert!(listing.error.is_some() || listing.inputs.iter().all(|input| input.id != 0));
}

//! The sustain pedal belongs to the instrument: through the bridge, with a MIDI keyboard and a
//! hardware synth on in-memory ports, the engine's own sampler, and the CLAP and VST3 test
//! instruments in containers. The test instruments have a sustain pedal and show the state of
//! their note in a read-only parameter (silent, key down, sustained: the note-off arrived and
//! the pedal keeps the note), so that "the pedal and the note-off reached the plug-in, and it
//! sustains" can be told from "the host held the note-off back".
//!
//! Plug-in and external instruments get the pedal with its value and their note-offs as
//! played; for the sampler the host holds the note-offs. Each test below is one of the edges:
//! the keyboard pointed at another track while the pedal is down; Stop and panic; a
//! publication, a replaced instrument, a plug-in restored after a crash; several keyboards and
//! channels with their own pedals; note recording under the pedal.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_audio::plugins::testing::{artifacts, clap_sine, vst3_sine, CLAP_NOTE_SINE, CLAP_SINE, VST3_SINE};
use st_host::areas::audio::{AudioOptions, AudioOutput};
use st_host::areas::midi::{MidiDevices, MidiOptions};
use st_host::areas::plugins::PluginAreaOptions;
use st_host::areas::services::ServiceOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};
use st_midi::memory::MemoryPorts;

const SYNTH: i32 = 700;
const KEYS: i32 = 300;
const PADS: i32 = 310;
/// The tracks of the project, by what plays them.
const EXTERNAL: i64 = 0;
const SAMPLER: i64 = 1;
const CLAP: i64 = 2;
const VST3: i64 = 3;
/// The CLAP sine whose note port takes CLAP note events only: no controller reaches it.
const NOTES_ONLY: i64 = 4;

/// The mixer channel of one of the five tracks above: what `midi.expression` and the plug-in
/// commands name a track by. It stays the track's when the tracks are reordered.
fn owner(track: i64) -> &'static str {
    ["track:hardware", "track:sampler", "track:clap", "track:vst3", "track:notes"][track as usize]
}

/// The CLAP sine's probe of the last control change, for a sustain pedal at `value`.
fn pedal_at(value: u8) -> f64 {
    64.0 * 128.0 + f64::from(value)
}

/// Who plays: the interface (`note.on`, `note.off`, `midi.expression`), or a MIDI device on a
/// channel (0 to 15).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Player {
    Page,
    Device(i32, u8),
}

const KEYBOARD: Player = Player::Device(KEYS, 0);

struct Fixture {
    host: TestHost,
    services: Host,
    ports: MemoryPorts,
    project: Value,
    /// `midi.note` events received and not yet asked for.
    notes: std::cell::RefCell<Vec<Value>>,
    _folder: TempDir,
}

/// A host on the null audio device with plug-in hosting, two MIDI keyboards (`KEYS`, `PADS`)
/// and a hardware synth (`SYNTH`) on in-memory ports, and a project of five tracks: an
/// external MIDI instrument on the synth's channel 3, the built-in sampler (a sine that sounds
/// for as long as its note is held), the CLAP sine, the VST3 sine, and the CLAP sine that
/// takes notes only.
fn start(label: &str) -> Fixture {
    let folder = TempDir::new(label);
    let installed = artifacts().install(&folder.path().join("plugins"));
    let ports = MemoryPorts::new();
    ports.add_input(KEYS, "Keys");
    ports.add_input(PADS, "Pads");
    ports.add_output(SYNTH, "Synth");
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(folder.path().join("support"), Arc::new(NoDialogs));
        options.autosave_every = None;
        options.audio = Some(AudioOptions { output: AudioOutput::Null });
        options.plugins = Some(PluginAreaOptions { container: Some(artifacts().container.clone()), folders: vec![installed.folder.clone()], only_these_folders: true });
        options.services = ServiceOptions { midi: Some(MidiOptions { devices: MidiDevices::Memory(ports.clone()) }), ..ServiceOptions::none() };
        services = Some(install(bridge, options).expect("install the host"));
    });
    host.request("ready", json!({})).expect("the handshake");
    let services = services.unwrap();
    let track = |index: i64, name: &str| json!({ "id": index, "name": name, "routingID": name, "volume": 1, "pan": 0, "instrument": index });
    let plugin = |name: &str, component: String, format: &str, path: &std::path::Path| {
        json!({ "instanceID": name, "owner": format!("track:{name}"), "kind": "instrument", "slot": 0, "component": component, "format": format, "bundlePath": path, "name": name })
    };
    let project = json!({
        "version": 2, "name": "Pedal", "bpm": 120, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": [track(EXTERNAL, "hardware"), track(SAMPLER, "sampler"), track(CLAP, "clap"), track(VST3, "vst3"), track(NOTES_ONLY, "notes")],
        "instruments": [
            { "id": 0, "type": "midi", "name": "Hardware", "midiOutput": SYNTH, "midiChannel": 3 },
            { "id": 1, "type": "sample", "name": "Sine", "samplePath": "builtin:wavetables/Basic/Sine.wav", "mode": "wavetable", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.05 },
            { "id": 2, "type": "au", "name": "CLAP sine", "volume": 1 },
            { "id": 3, "type": "au", "name": "VST3 sine", "volume": 1 },
            { "id": 4, "type": "au", "name": "CLAP sine, notes only", "volume": 1 }
        ],
        "patterns": [{ "id": 0, "length": 16, "steps": vec![vec![Value::Null; 16]; 5], "automation": [] }],
        "sequence": [0], "loopSong": true,
        "routing": { "buses": [], "outputs": {}, "sends": [], "sidechains": [] },
        "audioPlugins": [
            plugin("clap", format!("clap:{CLAP_SINE}"), "CLAP", &installed.clap), plugin("vst3", format!("vst3:{VST3_SINE}"), "VST3", &installed.vst3),
            plugin("notes", format!("clap:{CLAP_NOTE_SINE}"), "CLAP", &installed.clap),
        ],
    });
    let fixture = Fixture { host, services, ports, project, notes: Default::default(), _folder: folder };
    fixture.publish(&fixture.project);
    fixture.ports.clear_sent();
    fixture
}

impl Fixture {
    fn request(&self, command: &str, payload: Value) -> Value {
        self.host.request(command, payload.clone()).unwrap_or_else(|error| panic!("{command} {payload} failed: {error}"))
    }

    fn publish(&self, project: &Value) {
        let published = self.host.reply_within(self.host.send("engine.project", json!({ "project": project })), Duration::from_secs(120));
        published.expect("the project and its plug-ins are loaded");
    }

    /// Points the MIDI keyboards at `track` and its instrument, as the interface's cursor does.
    fn point_keys(&self, track: i64) {
        self.request("midi.configure", json!({ "track": track, "instrument": track }));
    }

    /// Plays bytes on a MIDI device and waits until they are routed and delivered.
    fn feed(&self, device: i32, bytes: &[u8]) {
        self.ports.feed(device, bytes);
        self.services.services.midi.as_ref().unwrap().service().flush_input();
    }

    fn note_on(&self, player: Player, track: i64, note: u8, velocity: u8) {
        match player {
            Player::Page => {
                self.request("note.on", json!({ "note": note, "velocity": velocity, "track": track, "instrument": track }));
            }
            Player::Device(device, channel) => {
                self.point_keys(track);
                self.feed(device, &[0x90 | channel, note, velocity]);
            }
        }
    }

    /// Releases the key. A device's release goes where its note went; the interface names the
    /// track again.
    fn note_off(&self, player: Player, track: i64, note: u8) {
        match player {
            Player::Page => {
                self.request("note.off", json!({ "note": note, "track": track, "instrument": track }));
            }
            Player::Device(device, channel) => self.feed(device, &[0x80 | channel, note, 0]),
        }
    }

    /// Moves a pedal (`controller` 64, 66 or 67) to `value`, played to `track`.
    fn pedal(&self, player: Player, track: i64, controller: u8, value: u8) {
        match player {
            Player::Page => {
                self.request("midi.expression", json!({ "kind": "cc", "controller": controller, "owner": owner(track), "value": f64::from(value) / 127.0 }));
            }
            Player::Device(device, channel) => {
                self.point_keys(track);
                self.feed(device, &[0xB0 | channel, controller, value]);
            }
        }
    }

    /// The value of one parameter of the plug-in instrument of `track` (one of the constants
    /// above: the plug-in is found by its channel, wherever the track is now), as it reports it.
    fn parameter(&self, track: i64, address: u64) -> f64 {
        let list = self.request("plugin.parameters", json!({ "owner": owner(track), "kind": "instrument", "slot": 0 }));
        let address = address.to_string();
        list["parameters"].as_array().unwrap().iter().find(|parameter| parameter["address"] == address.as_str()).unwrap_or_else(|| panic!("parameter {address} of track {track}"))["value"].as_f64().unwrap()
    }

    /// Waits until the plug-in on `track` shows `values` (pairs of address and value).
    fn shows(&self, what: &str, track: i64, values: &[(u64, f64)]) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let seen: Vec<f64> = values.iter().map(|(address, _)| self.parameter(track, *address)).collect();
            if seen.iter().zip(values).all(|(seen, (_, wanted))| (seen - wanted).abs() < 1e-9) {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: the plug-in shows {seen:?}, expected {values:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits until the CLAP sine's note is in `state` (`clap_sine::SILENT` and so on).
    fn clap_note(&self, what: &str, state: f64) {
        self.shows(what, CLAP, &[(clap_sine::NOTE_STATE, state)]);
    }

    fn vst3_note(&self, what: &str, state: f64) {
        self.shows(what, VST3, &[(vst3_sine::NOTE_STATE, state)]);
    }

    /// Waits until the synth has received `wanted` (and nothing else since the last clear).
    fn synth_gets(&self, what: &str, wanted: &[&[u8]]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.ports.sent_messages(SYNTH).len() < wanted.len() {
            assert!(Instant::now() < deadline, "{what}: the synth got {:?}", self.ports.sent_messages(SYNTH));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(self.ports.sent_messages(SYNTH), wanted, "{what}");
        self.ports.clear_sent();
    }

    /// The peak of a track's strip in the first status event made from now on.
    fn track_peak(&self, track: i64) -> f64 {
        self.host.events();
        let status = self.host.wait_for_event("status", Duration::from_secs(10)).expect("a status event");
        status["trackPeaks"][track as usize].as_f64().unwrap()
    }

    fn sampler_peak(&self) -> f64 {
        self.track_peak(SAMPLER)
    }

    fn track_fades(&self, what: &str, track: i64) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let peak = self.track_peak(track);
            if peak < 0.001 {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: the peak of track {track} is {peak}");
        }
    }

    fn sampler_where(&self, what: &str, wanted: impl Fn(f64) -> bool) -> f64 {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let peak = self.sampler_peak();
            if wanted(peak) {
                return peak;
            }
            assert!(Instant::now() < deadline, "{what}: the sampler's track peak is {peak}");
        }
    }

    /// Waits until the sampler's note is heard and returns its level.
    fn sampler_sounds(&self, what: &str) -> f64 {
        self.sampler_where(what, |peak| peak > 0.05);
        // Once more, when the attack is over and the meter has the note's full level.
        std::thread::sleep(Duration::from_millis(150));
        self.sampler_peak()
    }

    /// The sampler's note still sounds at `level` two seconds from now. A released note is
    /// gone after 50 ms and the meter falls 22 dB a second after it, so a level that has not
    /// halved is a note that is held.
    fn sampler_holds(&self, what: &str, level: f64) {
        std::thread::sleep(Duration::from_secs(2));
        let peak = self.sampler_peak();
        assert!(peak > level * 0.5, "{what}: the sampler's track peak fell from {level} to {peak}");
    }

    fn sampler_fades(&self, what: &str) {
        self.sampler_where(what, |peak| peak < 0.001);
    }

    /// Waits for a `midi.note` event that satisfies `wanted` and takes it; the others stay
    /// for later questions, in the order they arrived.
    fn note_event(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            {
                let mut notes = self.notes.borrow_mut();
                if let Some(index) = notes.iter().position(&wanted) {
                    return notes.remove(index);
                }
                assert!(Instant::now() < deadline, "no midi.note event where {what}; received: {notes:?}");
            }
            if let Some(event) = self.host.wait_for_event("midi.note", Duration::from_millis(100)) {
                self.notes.borrow_mut().push(event);
            }
        }
    }

    /// The `midi.note` events received so far and not asked for (they stay).
    fn note_events(&self) -> Vec<Value> {
        let mut notes = self.notes.borrow_mut();
        notes.extend(self.host.events().into_iter().filter(|(name, _)| name == "midi.note").map(|(_, event)| event));
        notes.clone()
    }

    fn forget_note_events(&self) {
        self.host.events();
        self.notes.borrow_mut().clear();
    }

    fn status_where(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last = Value::Null;
        while Instant::now() < deadline {
            if let Some(status) = self.host.wait_for_event("status", Duration::from_millis(200)) {
                if wanted(&status) {
                    return status;
                }
                last = status;
            }
        }
        panic!("no status event where {what}; the last one was {last}");
    }
}

#[test]
fn an_instrument_with_a_pedal_gets_it_and_sustains_and_the_host_holds_for_the_sampler() {
    let fixture = start("pedal-kinds");
    for player in [Player::Page, KEYBOARD] {
        // The CLAP instrument: the pedal arrives with its value, the note-off as played, and
        // the instrument sustains until the pedal comes up.
        fixture.note_on(player, CLAP, 60, 100);
        fixture.clap_note("the key is down", clap_sine::KEY_DOWN);
        fixture.pedal(player, CLAP, 64, 127);
        fixture.shows("the pedal reached the CLAP instrument", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127)), (clap_sine::NOTE_STATE, clap_sine::KEY_DOWN)]);
        if player == Player::Page {
            let released = fixture.request("note.off", json!({ "note": 60, "track": CLAP, "instrument": CLAP }));
            assert_eq!(released, json!({ "ok": true, "sustained": true }), "the reply says the note sounds on");
        } else {
            fixture.note_off(player, CLAP, 60);
        }
        fixture.clap_note("the note-off went through and the instrument sustains", clap_sine::SUSTAINED);
        fixture.pedal(player, CLAP, 64, 80);
        fixture.shows("half-pedalling is the instrument's: it gets the value", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(80)), (clap_sine::NOTE_STATE, clap_sine::SUSTAINED)]);
        fixture.pedal(player, CLAP, 64, 0);
        fixture.shows("pedal up", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);

        // The VST3 instrument maps controller 64 to its sustain parameter.
        fixture.note_on(player, VST3, 62, 100);
        fixture.vst3_note("the key is down", vst3_sine::KEY_DOWN);
        fixture.pedal(player, VST3, 64, 127);
        fixture.shows("the pedal reached the VST3 instrument", VST3, &[(vst3_sine::SUSTAIN, 1.0)]);
        fixture.note_off(player, VST3, 62);
        fixture.vst3_note("the note-off went through and the instrument sustains", vst3_sine::SUSTAINED);
        fixture.pedal(player, VST3, 64, 80);
        fixture.shows("the value of a half pedal", VST3, &[(vst3_sine::SUSTAIN, 80.0 / 127.0), (vst3_sine::NOTE_STATE, vst3_sine::SUSTAINED)]);
        fixture.pedal(player, VST3, 64, 0);
        fixture.shows("pedal up", VST3, &[(vst3_sine::SUSTAIN, 0.0), (vst3_sine::NOTE_STATE, vst3_sine::SILENT)]);

        // The external instrument: on its device and channel (3). Its note-off is not held.
        fixture.ports.clear_sent();
        fixture.note_on(player, EXTERNAL, 72, 90);
        fixture.synth_gets("the note", &[&[0x92, 72, 90]]);
        fixture.pedal(player, EXTERNAL, 64, 127);
        fixture.synth_gets("the pedal", &[&[0xB2, 64, 127]]);
        fixture.note_off(player, EXTERNAL, 72);
        fixture.synth_gets("the note-off as played", &[&[0x82, 72, 0]]);
        fixture.pedal(player, EXTERNAL, 64, 40);
        fixture.pedal(player, EXTERNAL, 64, 0);
        fixture.synth_gets("the pedal half way up and up", &[&[0xB2, 64, 40], &[0xB2, 64, 0]]);
        // Sostenuto and soft are passed on the same way.
        for controller in [66, 67] {
            fixture.pedal(player, EXTERNAL, controller, 127);
            fixture.pedal(player, CLAP, controller, 127);
            fixture.shows("the CLAP instrument got the pedal", CLAP, &[(clap_sine::LAST_CONTROLLER, f64::from(controller) * 128.0 + 127.0)]);
            fixture.pedal(player, EXTERNAL, controller, 0);
            fixture.pedal(player, CLAP, controller, 0);
            fixture.shows("and its release", CLAP, &[(clap_sine::LAST_CONTROLLER, f64::from(controller) * 128.0)]);
            fixture.synth_gets("the synth too", &[&[0xB2, controller, 127], &[0xB2, controller, 0]]);
        }

        // The sampler has no pedal: the host holds its note until the pedal comes up. Half
        // way up is up.
        fixture.note_on(player, SAMPLER, 57, 110);
        let level = fixture.sampler_sounds("the sampler's note");
        fixture.pedal(player, SAMPLER, 64, 127);
        if player == Player::Page {
            let released = fixture.request("note.off", json!({ "note": 57, "track": SAMPLER, "instrument": SAMPLER }));
            assert_eq!(released, json!({ "ok": true, "sustained": true }));
        } else {
            fixture.note_off(player, SAMPLER, 57);
        }
        fixture.sampler_holds("the host holds the released note", level);
        fixture.pedal(player, SAMPLER, 64, 63);
        fixture.sampler_fades("the pedal let go");
        fixture.pedal(player, SAMPLER, 64, 0);
        // Without the pedal a key release ends the note.
        fixture.note_on(player, SAMPLER, 57, 110);
        fixture.sampler_sounds("the sampler's note again");
        fixture.note_off(player, SAMPLER, 57);
        fixture.sampler_fades("released");

        // A plug-in the pedal cannot reach (its note port takes CLAP note events only, and
        // CLAP has no event for a control change) has no pedal of its own either: the host
        // holds its note-off, and the plug-in sees a key that stays down.
        fixture.note_on(player, NOTES_ONLY, 60, 100);
        fixture.shows("the key is down", NOTES_ONLY, &[(clap_sine::NOTE_STATE, clap_sine::KEY_DOWN)]);
        fixture.pedal(player, NOTES_ONLY, 64, 127);
        fixture.note_off(player, NOTES_ONLY, 60);
        std::thread::sleep(Duration::from_millis(300));
        fixture.shows("the note-off is held back by the host", NOTES_ONLY, &[(clap_sine::NOTE_STATE, clap_sine::KEY_DOWN), (clap_sine::LAST_CONTROLLER, 0.0)]);
        fixture.pedal(player, NOTES_ONLY, 64, 0);
        fixture.shows("and sent when the pedal comes up", NOTES_ONLY, &[(clap_sine::NOTE_STATE, clap_sine::SILENT)]);
        assert!(fixture.ports.sent_messages(SYNTH).is_empty(), "nothing of the sampler's pedal went to the synth: {:?}", fixture.ports.sent_messages(SYNTH));
        fixture.shows("or to a plug-in", CLAP, &[(clap_sine::LAST_CONTROLLER, 67.0 * 128.0)]);
    }
    // The interface's replies say where a pedal went: the sampler has no use for sostenuto.
    assert_eq!(fixture.request("midi.expression", json!({ "kind": "sustain", "owner": owner(SAMPLER), "value": 1 })), json!({ "sent": true, "owner": owner(SAMPLER), "instrument": SAMPLER }));
    assert_eq!(fixture.request("midi.expression", json!({ "kind": "sustain", "owner": owner(SAMPLER), "value": 0 }))["sent"], true);
    assert_eq!(fixture.request("midi.expression", json!({ "kind": "cc", "controller": 66, "owner": owner(SAMPLER), "value": 1 }))["sent"], false);
    assert_eq!(fixture.request("midi.expression", json!({ "kind": "cc", "controller": 66, "owner": owner(CLAP), "value": 1 }))["sent"], true);
    assert_eq!(fixture.request("midi.expression", json!({ "kind": "cc", "controller": 66, "owner": owner(CLAP), "value": 0 }))["sent"], true);
    // A pedal for a channel that is no track's is refused, like other expression.
    let refused = fixture.host.request("midi.expression", json!({ "kind": "sustain", "owner": "bus:drums", "value": 1 })).unwrap_err();
    assert!(refused.contains("not the channel of a track"), "{refused}");
    fixture.services.services.shutdown();
}

#[test]
fn a_pedal_follows_the_keyboard_to_another_track_and_lets_go_of_every_instrument_it_reached() {
    let fixture = start("pedal-follows");
    for player in [KEYBOARD, Player::Page] {
        // A chord on the CLAP instrument under the pedal.
        fixture.note_on(player, CLAP, 60, 100);
        fixture.pedal(player, CLAP, 64, 127);
        fixture.note_off(player, CLAP, 60);
        fixture.shows("the CLAP instrument sustains", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127)), (clap_sine::NOTE_STATE, clap_sine::SUSTAINED)]);

        // The keyboard is pointed at the synth: it gets the pedal with the first note, before
        // the note. The CLAP instrument goes on sustaining.
        fixture.ports.clear_sent();
        fixture.note_on(player, EXTERNAL, 72, 100);
        fixture.synth_gets("the pedal comes along with the first note", &[&[0xB2, 64, 127], &[0x92, 72, 100]]);
        fixture.note_on(player, EXTERNAL, 76, 100);
        fixture.synth_gets("once", &[&[0x92, 76, 100]]);
        fixture.note_off(player, EXTERNAL, 72);
        fixture.note_off(player, EXTERNAL, 76);
        fixture.synth_gets("its note-offs as played", &[&[0x82, 72, 0], &[0x82, 76, 0]]);

        // Then at the sampler: the host holds what is released there.
        fixture.note_on(player, SAMPLER, 57, 110);
        let level = fixture.sampler_sounds("the sampler's note");
        fixture.note_off(player, SAMPLER, 57);
        fixture.sampler_holds("the same pedal holds the sampler's note", level);
        fixture.clap_note("the first instrument still sustains", clap_sine::SUSTAINED);

        // The pedal comes up while the keyboard points at a track that never got it: every
        // instrument it reached is told, and nothing is left sounding.
        fixture.pedal(player, VST3, 64, 0);
        fixture.synth_gets("pedal-up on the synth", &[&[0xB2, 64, 0]]);
        fixture.shows("pedal-up at the CLAP instrument", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);
        fixture.sampler_fades("the sampler's note is released");
        fixture.shows("the VST3 instrument never had the pedal", VST3, &[(vst3_sine::SUSTAIN, 0.0), (vst3_sine::NOTE_STATE, vst3_sine::SILENT)]);
    }

    // A pedal pressed while keys hold a chord on another instrument than the one the keyboard
    // points at now reaches that instrument too.
    fixture.note_on(KEYBOARD, CLAP, 64, 100);
    fixture.clap_note("the key is down", clap_sine::KEY_DOWN);
    fixture.pedal(KEYBOARD, VST3, 64, 127);
    fixture.shows("the pedal reaches the instrument the key is on", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127))]);
    fixture.shows("and the one the keyboard points at", VST3, &[(vst3_sine::SUSTAIN, 1.0)]);
    fixture.note_off(KEYBOARD, CLAP, 64);
    fixture.clap_note("the held chord is sustained", clap_sine::SUSTAINED);
    fixture.pedal(KEYBOARD, VST3, 64, 0);
    fixture.clap_note("and released with the pedal", clap_sine::SILENT);
    fixture.shows("on both", VST3, &[(vst3_sine::SUSTAIN, 0.0)]);
    fixture.services.services.shutdown();
}

#[test]
fn stop_tells_every_instrument_pedal_up_and_a_panic_forgets_the_pedal() {
    let fixture = start("pedal-stop");
    let sustain_everywhere = |fixture: &Fixture| {
        fixture.note_on(KEYBOARD, CLAP, 60, 100);
        fixture.pedal(KEYBOARD, CLAP, 64, 127);
        fixture.note_off(KEYBOARD, CLAP, 60);
        fixture.note_on(KEYBOARD, VST3, 62, 100);
        fixture.note_off(KEYBOARD, VST3, 62);
        fixture.note_on(KEYBOARD, EXTERNAL, 72, 100);
        fixture.note_off(KEYBOARD, EXTERNAL, 72);
        fixture.note_on(KEYBOARD, SAMPLER, 57, 110);
        fixture.sampler_sounds("the sampler's note");
        fixture.note_off(KEYBOARD, SAMPLER, 57);
        fixture.clap_note("CLAP sustains", clap_sine::SUSTAINED);
        fixture.vst3_note("VST3 sustains", vst3_sine::SUSTAINED);
        fixture.synth_gets("the synth has the pedal and its note", &[&[0xB2, 64, 127], &[0x92, 72, 100], &[0x82, 72, 0]]);
    };

    // Stop while the song does not play: nothing would silence the synth but the pedal-up.
    sustain_everywhere(&fixture);
    fixture.request("transport.stop", json!({}));
    fixture.synth_gets("pedal-up on the synth", &[&[0xB2, 64, 0]]);
    fixture.shows("pedal-up at the CLAP instrument", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);
    fixture.shows("and at the VST3 instrument", VST3, &[(vst3_sine::SUSTAIN, 0.0), (vst3_sine::NOTE_STATE, vst3_sine::SILENT)]);
    fixture.sampler_fades("the host let go of the sampler's note");

    // The pedal is still down on the keyboard: the next notes are sustained again, each
    // instrument getting the pedal with its first note.
    fixture.note_on(KEYBOARD, CLAP, 60, 100);
    fixture.shows("the pedal again", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127))]);
    fixture.note_off(KEYBOARD, CLAP, 60);
    fixture.clap_note("sustained again", clap_sine::SUSTAINED);

    // Stop while the song plays: the synth gets pedal-up before all-notes-off.
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    fixture.status_where("the song plays", |status| status["playing"] == true && status["transportPending"] == false);
    fixture.note_on(KEYBOARD, EXTERNAL, 72, 100);
    fixture.note_off(KEYBOARD, EXTERNAL, 72);
    fixture.synth_gets("the synth has the pedal again", &[&[0xB2, 64, 127], &[0x92, 72, 100], &[0x82, 72, 0]]);
    fixture.request("transport.stop", json!({}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.ports.sent_messages(SYNTH).contains(&vec![0xB2, 123, 0]) {
        assert!(Instant::now() < deadline, "no all-notes-off on stop: {:?}", fixture.ports.sent_messages(SYNTH));
        std::thread::sleep(Duration::from_millis(5));
    }
    let sent = fixture.ports.sent_messages(SYNTH);
    assert_eq!(sent[0], [0xB2, 64, 0], "pedal-up first: {sent:?}");
    assert!(sent.contains(&vec![0xB2, 66, 0]) && sent.contains(&vec![0xB2, 67, 0]), "sostenuto and soft are let go as well: {sent:?}");
    fixture.clap_note("the CLAP instrument is silent again", clap_sine::SILENT);
    fixture.feed(KEYS, &[0xB0, 64, 0]);
    fixture.ports.clear_sent();

    // Panic: everything is silent at once, and the pedals are forgotten.
    sustain_everywhere(&fixture);
    fixture.request("engine.panic", json!({}));
    fixture.clap_note("the CLAP instrument is silent", clap_sine::SILENT);
    fixture.vst3_note("the VST3 instrument is silent", vst3_sine::SILENT);
    fixture.sampler_fades("the sampler is silent");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.ports.sent_messages(SYNTH).contains(&vec![0xB2, 123, 0]) {
        assert!(Instant::now() < deadline, "no all-notes-off on panic: {:?}", fixture.ports.sent_messages(SYNTH));
        std::thread::sleep(Duration::from_millis(5));
    }
    let sent = fixture.ports.sent_messages(SYNTH);
    let position = |message: &[u8]| sent.iter().position(|sent| sent == message).unwrap_or_else(|| panic!("{message:?} in {sent:?}"));
    assert!(position(&[0xB2, 64, 0]) < position(&[0xB2, 123, 0]), "pedal-up before all-notes-off: {sent:?}");
    // The plug-ins take a moment to come back from their reset. Then: the pedal is still
    // down on the keyboard, but counts as up until it is pressed again.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        fixture.note_on(KEYBOARD, CLAP, 65, 100);
        std::thread::sleep(Duration::from_millis(100));
        if fixture.parameter(CLAP, clap_sine::NOTE_STATE) == clap_sine::KEY_DOWN {
            break;
        }
        fixture.note_off(KEYBOARD, CLAP, 65);
        assert!(Instant::now() < deadline, "the CLAP instrument does not play after the panic");
    }
    fixture.note_off(KEYBOARD, CLAP, 65);
    fixture.clap_note("after a panic a key release ends the note", clap_sine::SILENT);
    fixture.note_on(KEYBOARD, SAMPLER, 57, 110);
    fixture.sampler_sounds("the sampler's note");
    fixture.note_off(KEYBOARD, SAMPLER, 57);
    fixture.sampler_fades("released: the host holds nothing");
    // Lifted and pressed again, the pedal is a pedal again.
    fixture.feed(KEYS, &[0xB0, 64, 0]);
    fixture.note_on(KEYBOARD, CLAP, 65, 100);
    fixture.pedal(KEYBOARD, CLAP, 64, 127);
    fixture.note_off(KEYBOARD, CLAP, 65);
    fixture.clap_note("sustained", clap_sine::SUSTAINED);
    fixture.pedal(KEYBOARD, CLAP, 64, 0);
    fixture.clap_note("released", clap_sine::SILENT);

    // A song that ends by itself silences the synth from the render thread (pedal-up, all
    // notes off), without a Stop. The pedal is still down on the keyboard: the next note
    // brings it to the synth again.
    let mut once = fixture.project.clone();
    once["loopSong"] = json!(false);
    fixture.publish(&once);
    fixture.ports.clear_sent();
    fixture.pedal(KEYBOARD, EXTERNAL, 64, 127);
    fixture.synth_gets("the pedal", &[&[0xB2, 64, 127]]);
    fixture.request("transport.play", json!({ "mode": "song", "fromStart": true }));
    fixture.status_where("the song plays", |status| status["playing"] == true && status["transportPending"] == false);
    fixture.status_where("the song has ended", |status| status["playing"] == false && status["transportPending"] == false);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.ports.sent_messages(SYNTH).contains(&vec![0xB2, 123, 0]) {
        assert!(Instant::now() < deadline, "no all-notes-off at the end of the song: {:?}", fixture.ports.sent_messages(SYNTH));
        std::thread::sleep(Duration::from_millis(5));
    }
    fixture.ports.clear_sent();
    fixture.note_on(KEYBOARD, EXTERNAL, 74, 100);
    fixture.synth_gets("the pedal comes with the next note", &[&[0xB2, 64, 127], &[0x92, 74, 100]]);
    fixture.note_off(KEYBOARD, EXTERNAL, 74);
    // (A note leaves with the audio block it belongs to, a pedal at once: the note-off is
    // awaited before the pedal moves.)
    fixture.synth_gets("the note-off", &[&[0x82, 74, 0]]);
    fixture.pedal(KEYBOARD, EXTERNAL, 64, 0);
    fixture.synth_gets("and the pedal's release", &[&[0xB2, 64, 0]]);
    fixture.services.services.shutdown();
}

#[test]
fn a_publication_leaves_the_pedal_alone_and_a_replaced_or_restored_instrument_gets_it_again() {
    let fixture = start("pedal-publication");
    let mut project = fixture.project.clone();

    // A publication that does not touch the instruments: the pedal stays down in them.
    fixture.note_on(KEYBOARD, CLAP, 60, 100);
    fixture.pedal(KEYBOARD, CLAP, 64, 127);
    fixture.note_off(KEYBOARD, CLAP, 60);
    fixture.note_on(KEYBOARD, EXTERNAL, 72, 100);
    fixture.note_off(KEYBOARD, EXTERNAL, 72);
    fixture.clap_note("the CLAP instrument sustains", clap_sine::SUSTAINED);
    fixture.synth_gets("the synth has the pedal", &[&[0xB2, 64, 127], &[0x92, 72, 100], &[0x82, 72, 0]]);
    project["bpm"] = json!(121);
    project["patterns"][0]["steps"][1][4] = json!({ "note": 50, "instrument": 1, "velocity": 90 });
    fixture.publish(&project);
    std::thread::sleep(Duration::from_millis(300));
    fixture.shows("the note is still sustained", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127)), (clap_sine::NOTE_STATE, clap_sine::SUSTAINED)]);
    assert!(fixture.ports.sent_messages(SYNTH).is_empty(), "and the synth heard nothing: {:?}", fixture.ports.sent_messages(SYNTH));

    // The external instrument moves to channel 5 while its pedal is down: the old channel is
    // told pedal-up (it would sustain for ever), the new one gets the pedal with the next note.
    project["instruments"][0]["midiChannel"] = json!(5);
    fixture.publish(&project);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.ports.sent_messages(SYNTH).contains(&vec![0xB2, 123, 0]) {
        assert!(Instant::now() < deadline, "the old channel was not cleared: {:?}", fixture.ports.sent_messages(SYNTH));
        std::thread::sleep(Duration::from_millis(5));
    }
    let sent = fixture.ports.sent_messages(SYNTH);
    assert_eq!(sent[0], [0xB2, 64, 0], "pedal-up on the old channel first: {sent:?}");
    // (All-notes-off is repeated a moment later for notes that were on their way.)
    std::thread::sleep(Duration::from_millis(300));
    fixture.ports.clear_sent();
    fixture.note_on(KEYBOARD, EXTERNAL, 74, 100);
    fixture.synth_gets("the new channel gets the pedal with the first note", &[&[0xB4, 64, 127], &[0x94, 74, 100]]);
    fixture.note_off(KEYBOARD, EXTERNAL, 74);
    fixture.synth_gets("and the note-off", &[&[0x84, 74, 0]]);

    // The CLAP instrument is replaced by another instance. It never heard of the pedal: the
    // next note brings it along.
    project["audioPlugins"][0]["instanceID"] = json!("another");
    fixture.publish(&project);
    fixture.shows("a new instance", CLAP, &[(clap_sine::LAST_CONTROLLER, 0.0), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);
    fixture.note_on(KEYBOARD, CLAP, 64, 100);
    fixture.shows("the pedal comes with the note", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127)), (clap_sine::NOTE_STATE, clap_sine::KEY_DOWN)]);
    fixture.note_off(KEYBOARD, CLAP, 64);
    fixture.clap_note("and the new instance sustains", clap_sine::SUSTAINED);

    // The plug-ins' container dies and the plug-ins are restored. The restored instruments
    // are silent, and get the pedal again with the next note.
    fixture.host.events();
    let manager = fixture.services.audio.as_ref().unwrap().engine().plugins().expect("plug-in hosting").clone();
    let instrument = manager.all().into_iter().find(|hosted| hosted.instrument && hosted.identity.format == "clap").expect("the CLAP instrument's instance");
    let pid = manager.container_pid(&instrument);
    assert!(std::process::Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap().success());
    let lost = fixture.host.wait_for_event("plugin.error", Duration::from_secs(20)).expect("the loss is announced");
    assert_eq!(lost["lost"], true, "{lost}");
    let retried = fixture.host.reply_within(fixture.host.send("plugins.recovery.retry", json!({})), Duration::from_secs(120)).expect("the plug-ins are restored");
    assert_eq!(retried["retryErrors"], json!([]), "{retried}");
    fixture.shows("restored and silent", CLAP, &[(clap_sine::LAST_CONTROLLER, 0.0), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);
    fixture.note_on(KEYBOARD, CLAP, 67, 100);
    fixture.shows("the pedal comes with the note", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127)), (clap_sine::NOTE_STATE, clap_sine::KEY_DOWN)]);
    fixture.note_off(KEYBOARD, CLAP, 67);
    fixture.clap_note("the restored instrument sustains", clap_sine::SUSTAINED);
    fixture.note_on(KEYBOARD, VST3, 62, 100);
    fixture.shows("the VST3 instrument gets it too", VST3, &[(vst3_sine::SUSTAIN, 1.0)]);
    fixture.note_off(KEYBOARD, VST3, 62);
    fixture.vst3_note("and sustains", vst3_sine::SUSTAINED);

    // The pedal comes up: every instrument, as it is now, lets go.
    fixture.ports.clear_sent();
    fixture.pedal(KEYBOARD, SAMPLER, 64, 0);
    fixture.synth_gets("pedal-up on the synth's new channel", &[&[0xB4, 64, 0]]);
    fixture.clap_note("the CLAP instrument is silent", clap_sine::SILENT);
    fixture.vst3_note("the VST3 instrument is silent", vst3_sine::SILENT);

    // Tracks are reordered while the pedal is down: the sampler's track and the CLAP
    // instrument's swap places. The pedals know these instruments by their track's place, so
    // before the publication the plug-in is told pedal-up and the sampler's held note is let
    // go: nothing is left sounding where the pedal no longer reaches it.
    fixture.note_on(KEYBOARD, CLAP, 60, 100);
    fixture.pedal(KEYBOARD, CLAP, 64, 127);
    fixture.note_off(KEYBOARD, CLAP, 60);
    fixture.clap_note("sustained", clap_sine::SUSTAINED);
    fixture.note_on(KEYBOARD, SAMPLER, 57, 110);
    fixture.sampler_sounds("the sampler's note");
    fixture.note_off(KEYBOARD, SAMPLER, 57);
    project["tracks"].as_array_mut().unwrap().swap(SAMPLER as usize, CLAP as usize);
    fixture.publish(&project);
    // (Where the two tracks are now. The plug-in is still found by its channel.)
    let (moved_clap, moved_sampler) = (SAMPLER, CLAP);
    fixture.shows("the moved plug-in was told pedal-up", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);
    fixture.track_fades("the sampler's held note was let go", moved_sampler);
    fixture.track_fades("and nothing sounds where it was", moved_clap);
    // The pedal is still down: the plug-in gets it again at its new place.
    fixture.request("midi.configure", json!({ "track": moved_clap, "instrument": CLAP }));
    fixture.feed(KEYS, &[0x90, 64, 100]);
    fixture.shows("the pedal comes with the note", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127)), (clap_sine::NOTE_STATE, clap_sine::KEY_DOWN)]);
    fixture.feed(KEYS, &[0x80, 64, 0]);
    fixture.shows("sustained at its new place", CLAP, &[(clap_sine::NOTE_STATE, clap_sine::SUSTAINED)]);
    fixture.feed(KEYS, &[0xB0, 64, 0]);
    fixture.shows("and released", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);
    fixture.services.services.shutdown();
}

#[test]
fn every_keyboard_and_channel_has_its_own_pedal() {
    let fixture = start("pedal-sources");
    let pads = Player::Device(PADS, 0);
    let second_channel = Player::Device(KEYS, 1);

    // Two keyboards sustain the same plug-in: it hears the higher pedal, and one pedal coming
    // up does not let go of what the other still holds.
    fixture.note_on(KEYBOARD, CLAP, 60, 100);
    fixture.pedal(KEYBOARD, CLAP, 64, 127);
    fixture.note_off(KEYBOARD, CLAP, 60);
    fixture.shows("the first keyboard's pedal", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(127)), (clap_sine::NOTE_STATE, clap_sine::SUSTAINED)]);
    fixture.pedal(pads, CLAP, 64, 100);
    fixture.pedal(KEYBOARD, CLAP, 64, 0);
    fixture.shows("the second keyboard's pedal still holds", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(100)), (clap_sine::NOTE_STATE, clap_sine::SUSTAINED)]);
    fixture.pedal(pads, CLAP, 64, 0);
    fixture.shows("both are up", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);

    // The same on the synth, with the interface's pedal as one of the two.
    fixture.ports.clear_sent();
    fixture.pedal(Player::Page, EXTERNAL, 64, 127);
    fixture.pedal(KEYBOARD, EXTERNAL, 64, 90);
    fixture.pedal(Player::Page, EXTERNAL, 64, 0);
    fixture.pedal(KEYBOARD, EXTERNAL, 64, 0);
    fixture.synth_gets("the synth hears the higher of the two pedals", &[&[0xB2, 64, 127], &[0xB2, 64, 90], &[0xB2, 64, 0]]);

    // For the sampler the host holds per keyboard and channel: the pedal of channel 1 does
    // not hold a key of channel 2, of another keyboard or of the interface.
    fixture.pedal(KEYBOARD, SAMPLER, 64, 127);
    for other in [second_channel, pads, Player::Page] {
        fixture.note_on(other, SAMPLER, 57, 110);
        fixture.sampler_sounds("another player's note");
        fixture.note_off(other, SAMPLER, 57);
        fixture.sampler_fades("ends with its key: the pedal is not its own");
    }
    // And another pedal coming up does not release what this one holds.
    fixture.note_on(KEYBOARD, SAMPLER, 57, 110);
    let level = fixture.sampler_sounds("the keyboard's note");
    fixture.note_off(KEYBOARD, SAMPLER, 57);
    for other in [second_channel, pads, Player::Page] {
        fixture.pedal(other, SAMPLER, 64, 127);
        fixture.pedal(other, SAMPLER, 64, 0);
    }
    fixture.sampler_holds("held by its own pedal", level);
    fixture.pedal(KEYBOARD, SAMPLER, 64, 0);
    fixture.sampler_fades("and released by it");

    // A keyboard that is unplugged with its pedal down: the pedal comes up in its name.
    fixture.note_on(KEYBOARD, CLAP, 60, 100);
    fixture.pedal(KEYBOARD, CLAP, 64, 127);
    fixture.note_off(KEYBOARD, CLAP, 60);
    fixture.clap_note("sustained", clap_sine::SUSTAINED);
    fixture.ports.remove_input(KEYS);
    let midi = fixture.services.services.midi.as_ref().unwrap().service().clone();
    assert!(midi.refresh_devices());
    midi.flush_input();
    fixture.shows("the unplugged keyboard's pedal is up", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);

    // A changed input route takes the devices' pedals down with it (no pedal-up would come
    // through), but not the interface's.
    fixture.note_on(pads, CLAP, 62, 100);
    fixture.pedal(pads, CLAP, 64, 127);
    fixture.note_off(pads, CLAP, 62);
    fixture.clap_note("sustained by the other keyboard", clap_sine::SUSTAINED);
    fixture.pedal(Player::Page, EXTERNAL, 64, 127);
    fixture.synth_gets("the interface's pedal on the synth", &[&[0xB2, 64, 127]]);
    fixture.request("midi.configure", json!({ "inputChannel": 5 }));
    fixture.shows("the device's pedal is lifted", CLAP, &[(clap_sine::LAST_CONTROLLER, pedal_at(0)), (clap_sine::NOTE_STATE, clap_sine::SILENT)]);
    assert!(fixture.ports.sent_messages(SYNTH).is_empty(), "the interface's pedal stays down: {:?}", fixture.ports.sent_messages(SYNTH));
    fixture.pedal(Player::Page, EXTERNAL, 64, 0);
    fixture.synth_gets("until it comes up", &[&[0xB2, 64, 0]]);
    fixture.services.services.shutdown();
}

#[test]
fn a_recorded_note_is_as_long_as_it_sounded_under_the_pedal() {
    let fixture = start("pedal-recording");
    assert_eq!(fixture.request("transport.recordArm", json!({ "armed": true })), json!({ "armed": true }));
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    fixture.status_where("the song plays", |status| status["playing"] == true && status["transportPending"] == false);
    let beats = |event: &Value| event["inputAcknowledgement"]["recordingClock"]["distanceBeats"].as_f64().unwrap_or_else(|| panic!("a recording clock in {event}"));
    let opened = |fixture: &Fixture, note: u8| -> (String, f64) {
        let pending = fixture.note_event("the capture opens", |event| event["recordingPending"] == true && event["on"] == true && event["note"] == note);
        let owner = pending["inputOwner"].as_str().expect("the capture's name").to_string();
        let acknowledged = fixture.note_event("the onset is acknowledged", |event| event["recordingOnly"] == true && event["on"] == true && event["inputOwner"] == owner.as_str());
        assert_eq!(acknowledged["inputAcknowledgement"]["status"], "applied", "{acknowledged}");
        (owner, beats(&acknowledged))
    };
    // The two events that close a capture: the release is reported, then acknowledged by the
    // audio thread (not cancelled). Returns the acknowledgement.
    let closed = |fixture: &Fixture, owner: &str| -> Value {
        fixture.note_event("the capture's release is reported", |event| event["recordingPending"] == true && event["on"] == false && event["inputOwner"] == owner);
        let acknowledged = fixture.note_event("and acknowledged", |event| event["recordingOnly"] == true && event["on"] == false && event["inputOwner"] == owner);
        assert_eq!(acknowledged["inputAcknowledgement"]["applyNote"], false, "the edge only marks the end: the sound is the pedal's business: {acknowledged}");
        assert!(acknowledged.get("recordingCancelled").is_none(), "{acknowledged}");
        acknowledged
    };

    // The same on an instrument the host holds for and on one that sustains by itself.
    for track in [SAMPLER, CLAP] {
        fixture.forget_note_events();
        fixture.note_on(KEYBOARD, track, 60, 100);
        let (owner, onset) = opened(&fixture, 60);
        fixture.pedal(KEYBOARD, track, 64, 127);
        fixture.note_off(KEYBOARD, track, 60);
        // The key is up for the interface; the capture stays open.
        let released = fixture.note_event("the key release", |event| event["on"] == false && event["note"] == 60);
        assert!(released.get("recordingPending").is_none() && released.get("recordingOnly").is_none() && released.get("inputOwner").is_none(), "{released}");
        if track == CLAP {
            fixture.clap_note("the plug-in got the note-off and sustains", clap_sine::SUSTAINED);
        }
        std::thread::sleep(Duration::from_millis(600));
        let early: Vec<Value> = fixture.note_events().into_iter().filter(|event| event["inputOwner"] == owner.as_str()).collect();
        assert_eq!(early, Vec::<Value>::new(), "the capture is not closed while the pedal is down");
        // The pedal comes up: the note ends, and that is the length it is recorded with.
        fixture.pedal(KEYBOARD, track, 64, 0);
        let release = closed(&fixture, &owner);
        assert_eq!(release["inputAcknowledgement"]["status"], "applied", "{release}");
        let release = beats(&release);
        // 600 ms under the pedal are 1.2 beats at 120 BPM.
        assert!(release - onset > 1.1, "track {track}: the note is recorded from {onset} to {release} beats");
    }
    fixture.clap_note("nothing is left sounding", clap_sine::SILENT);

    // A key struck again under the pedal ends the note it sustained there, and starts another.
    fixture.forget_note_events();
    fixture.note_on(KEYBOARD, CLAP, 62, 100);
    let (first, _) = opened(&fixture, 62);
    fixture.pedal(KEYBOARD, CLAP, 64, 127);
    fixture.note_off(KEYBOARD, CLAP, 62);
    fixture.note_on(KEYBOARD, CLAP, 62, 100);
    assert_eq!(closed(&fixture, &first)["inputAcknowledgement"]["status"], "applied");
    let (second, _) = opened(&fixture, 62);
    assert_ne!(first, second);
    // Half way up is up: the second note's key is released first, then the pedal eases off.
    fixture.note_off(KEYBOARD, CLAP, 62);
    fixture.pedal(KEYBOARD, CLAP, 64, 50);
    assert_eq!(closed(&fixture, &second)["inputAcknowledgement"]["status"], "applied");
    fixture.pedal(KEYBOARD, CLAP, 64, 0);

    // Stop ends a note the pedal holds, and closes its capture as the release of a key held
    // through Stop would (the audio thread says the transport changed under the edge).
    fixture.note_on(KEYBOARD, SAMPLER, 64, 100);
    let (third, _) = opened(&fixture, 64);
    fixture.pedal(KEYBOARD, SAMPLER, 64, 127);
    fixture.note_off(KEYBOARD, SAMPLER, 64);
    fixture.request("transport.stop", json!({}));
    closed(&fixture, &third);
    fixture.sampler_fades("the note Stop ended");
    fixture.pedal(KEYBOARD, SAMPLER, 64, 0);
    assert!(fixture.note_events().iter().all(|event| event["inputOwner"] != third.as_str()), "the capture was closed once");

    // The interface records its own keys: under its pedal it is told that the note sounds
    // on, and closes its capture when its pedal comes up.
    fixture.request("transport.play", json!({ "mode": "pattern", "fromStart": true }));
    fixture.status_where("the song plays again", |status| status["playing"] == true && status["transportPending"] == false);
    fixture.pedal(Player::Page, SAMPLER, 64, 127);
    let pressed = fixture.request("note.on", json!({ "note": 65, "velocity": 100, "track": SAMPLER, "instrument": SAMPLER, "captureInput": true, "inputOwner": "key-1" }));
    assert_eq!(pressed["inputAcknowledgement"]["status"], "applied", "{pressed}");
    let level = fixture.sampler_sounds("the interface's note");
    let released = fixture.request("note.off", json!({ "note": 65, "track": SAMPLER, "instrument": SAMPLER }));
    assert_eq!(released, json!({ "ok": true, "sustained": true }));
    fixture.sampler_holds("held by the interface's pedal", level);
    fixture.pedal(Player::Page, SAMPLER, 64, 0);
    let closing = fixture.request("note.off", json!({ "note": 65, "track": SAMPLER, "instrument": SAMPLER, "captureInput": true, "inputOwner": "key-1", "applyNote": false }));
    assert_eq!((closing["inputAcknowledgement"]["status"].clone(), closing["inputAcknowledgement"]["on"].clone()), (json!("applied"), json!(false)), "{closing}");
    assert!(beats(&closing) - beats(&pressed) > 4.0, "two seconds and more under the pedal: {pressed} to {closing}");
    fixture.sampler_fades("released with the pedal");
    fixture.request("transport.stop", json!({}));
    fixture.services.services.shutdown();
}

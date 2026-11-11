//! Controllers, channel and poly pressure and pitch bend played live, through the bridge: from
//! the interface (`midi.expression`) and from a MIDI keyboard (an in-memory port), to each
//! kind of instrument a track can have: an external MIDI instrument (an in-memory port), the
//! engine's own sampler, and plug-in instruments in containers (the CLAP and VST3 test
//! instruments of `crates/plugin-host`, which show what they received in parameters).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_audio::plugins::testing::{artifacts, clap_sine, vst3_sine, CLAP_SINE, VST3_SINE};
use st_host::areas::audio::{AudioOptions, AudioOutput};
use st_host::areas::midi::{MidiDevices, MidiOptions};
use st_host::areas::plugins::PluginAreaOptions;
use st_host::areas::services::ServiceOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};
use st_midi::memory::MemoryPorts;

const SYNTH: i32 = 700;
const KEYS: i32 = 300;
/// The tracks of the project, by what plays them.
const EXTERNAL: i64 = 0;
const SAMPLER: i64 = 1;
const CLAP: i64 = 2;
const VST3: i64 = 3;

/// The mixer channel of one of the four tracks above.
fn owner(track: i64) -> &'static str {
    ["track:hardware", "track:sampler", "track:clap", "track:vst3"][track as usize]
}

struct Fixture {
    host: TestHost,
    services: Host,
    ports: MemoryPorts,
    _folder: TempDir,
}

/// A host on the null audio device with plug-in hosting, a MIDI keyboard (`KEYS`) and a
/// hardware synth (`SYNTH`) on in-memory ports, and a project of four tracks: an external
/// MIDI instrument on the synth's channel 3, the built-in sampler, the CLAP sine and the VST3
/// sine.
fn start(label: &str) -> Fixture {
    let folder = TempDir::new(label);
    let installed = artifacts().install(&folder.path().join("plugins"));
    let ports = MemoryPorts::new();
    ports.add_input(KEYS, "Keys");
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
    // No block of a plug-in is given up, however busy the machine: a controller in a block the
    // plug-in missed would be lost.
    services.audio.as_ref().unwrap().engine().plugins().expect("plug-in hosting").set_min_deadline(Duration::from_secs(20));
    let track = |index: i64, name: &str| json!({ "id": index, "name": name, "routingID": name, "volume": 1, "pan": 0, "instrument": index });
    let plugin = |name: &str, component: String, format: &str, path: &std::path::Path| {
        json!({ "instanceID": name, "owner": format!("track:{name}"), "kind": "instrument", "slot": 0, "component": component, "format": format, "bundlePath": path, "name": name })
    };
    let project = json!({
        "version": 2, "name": "Expression", "bpm": 120, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": [track(EXTERNAL, "hardware"), track(SAMPLER, "sampler"), track(CLAP, "clap"), track(VST3, "vst3")],
        "instruments": [
            { "id": 0, "type": "midi", "name": "Hardware", "midiOutput": SYNTH, "midiChannel": 3 },
            { "id": 1, "type": "sample", "name": "Sine", "samplePath": "builtin:wavetables/Basic/Sine.wav", "mode": "wavetable", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.05 },
            { "id": 2, "type": "au", "name": "CLAP sine", "volume": 1 },
            { "id": 3, "type": "au", "name": "VST3 sine", "volume": 1 }
        ],
        "patterns": [{ "id": 0, "length": 16, "steps": [vec![Value::Null; 16], vec![Value::Null; 16], vec![Value::Null; 16], vec![Value::Null; 16]], "automation": [] }],
        "sequence": [0], "loopSong": true,
        "routing": { "buses": [], "outputs": {}, "sends": [], "sidechains": [] },
        "audioPlugins": [plugin("clap", format!("clap:{CLAP_SINE}"), "CLAP", &installed.clap), plugin("vst3", format!("vst3:{VST3_SINE}"), "VST3", &installed.vst3)],
    });
    let published = host.reply_within(host.send("engine.project", json!({ "project": project })), Duration::from_secs(120));
    published.expect("the project and its plug-ins are loaded");
    Fixture { host, services, ports, _folder: folder }
}

impl Fixture {
    fn express(&self, payload: Value) -> Value {
        self.host.request("midi.expression", payload.clone()).unwrap_or_else(|error| panic!("midi.expression {payload} failed: {error}"))
    }

    /// The value of one parameter of the plug-in instrument on `track`, as it reports it.
    fn parameter(&self, track: i64, address: u64) -> f64 {
        let list = self.host.request("plugin.parameters", json!({ "owner": owner(track), "kind": "instrument", "slot": 0 })).expect("the plug-in's parameters");
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
}

/// The CLAP sine's probes for the messages of [`MESSAGES`], and the VST3 sine's parameters.
fn clap_shows(controller: u8, value: u8, pressure: u8, key: u8, poly: u8, bend: u16) -> [(u64, f64); 4] {
    [
        (clap_sine::LAST_CONTROLLER, f64::from(controller) * 128.0 + f64::from(value)),
        (clap_sine::PRESSURE, f64::from(pressure)),
        (clap_sine::POLY_PRESSURE, f64::from(key) * 128.0 + f64::from(poly)),
        (clap_sine::BEND, f64::from(bend)),
    ]
}

fn vst3_shows(brightness: u8, pressure: u8, key: u8, poly: u8, bend: u16) -> [(u64, f64); 4] {
    [
        (vst3_sine::BRIGHTNESS, f64::from(brightness) / 127.0),
        (vst3_sine::PRESSURE, f64::from(pressure) / 127.0),
        (vst3_sine::POLY_PRESSURE, (f64::from(key) * 128.0 + f64::from(poly)) / 16383.0),
        (vst3_sine::BEND, f64::from(bend) / 16383.0),
    ]
}

#[test]
fn expression_from_the_interface_reaches_every_kind_of_instrument() {
    let fixture = start("expression-command");
    let ready = fixture.host.request("ready", json!({})).unwrap();
    assert!(ready["commands"].as_array().unwrap().contains(&json!("midi.expression")));
    // Controller 74 at 64, channel pressure 100, poly pressure 45 on key 60, a bend of +1/2.
    let messages = |track: i64| {
        [
            json!({ "owner": owner(track), "kind": "cc", "controller": 74, "value": 64.0 / 127.0 }),
            json!({ "owner": owner(track), "kind": "aftertouch", "value": 100.0 / 127.0 }),
            json!({ "owner": owner(track), "kind": "polyAftertouch", "note": 60, "value": 45.0 / 127.0 }),
            json!({ "owner": owner(track), "kind": "pitchBend", "value": 0.5 }),
        ]
    };
    let sent = |track: i64| -> Vec<Value> { messages(track).into_iter().map(|message| fixture.express(message)).collect() };
    let all = |replies: &[Value], sent: bool, track: i64| replies.iter().all(|reply| *reply == json!({ "sent": sent, "owner": owner(track), "instrument": track }));

    // The external instrument: on its device, on its channel (3), at once.
    fixture.ports.clear_sent();
    assert!(all(&sent(EXTERNAL), true, EXTERNAL));
    fixture.synth_gets("the external instrument", &[&[0xB2, 74, 64], &[0xD2, 100], &[0xA2, 60, 45], &[0xE2, 0x00, 0x60]]);

    // The plug-ins: in their format's terms. The CLAP sine takes MIDI; the VST3 sine maps
    // controller 74, channel pressure and pitch bend to parameters and gets poly pressure
    // as an event.
    assert!(all(&sent(CLAP), true, CLAP));
    fixture.shows("the CLAP instrument", CLAP, &clap_shows(74, 64, 100, 60, 45, 0x60 << 7));
    assert!(all(&sent(VST3), true, VST3));
    fixture.shows("the VST3 instrument", VST3, &vst3_shows(64, 100, 60, 45, 0x60 << 7));
    assert!(fixture.ports.sent_messages(SYNTH).is_empty(), "nothing of that went to the synth");

    // The engine's own instrument: pitch bend and the modulation wheel; it ignores the rest.
    let replies = sent(SAMPLER);
    assert!(all(&replies[..3], false, SAMPLER) && all(&replies[3..], true, SAMPLER), "{replies:?}");
    assert_eq!(fixture.express(json!({ "owner": owner(SAMPLER), "kind": "cc", "controller": 1, "value": 0.5 }))["sent"], true);
    // The sustain pedal is the instrument's as well: it gets the pedal, with its value, and its
    // note-offs as played (`tests/pedal.rs` has everything about the pedals).
    assert_eq!(fixture.express(json!({ "owner": owner(EXTERNAL), "kind": "sustain", "value": 1 }))["sent"], true);
    fixture.host.request("note.on", json!({ "note": 72, "velocity": 90, "track": EXTERNAL, "instrument": 0 })).unwrap();
    assert_eq!(fixture.host.request("note.off", json!({ "note": 72, "track": EXTERNAL, "instrument": 0 })).unwrap()["sustained"], true);
    fixture.synth_gets("the pedal, the note and its release", &[&[0xB2, 64, 127], &[0x92, 72, 90], &[0x82, 72, 0]]);
    assert_eq!(fixture.express(json!({ "owner": owner(EXTERNAL), "kind": "cc", "controller": 64, "value": 0 }))["sent"], true);
    fixture.synth_gets("pedal-up", &[&[0xB2, 64, 0]]);

    // What is not expression is refused with a reason.
    for (payload, reason) in [
        (json!({ "owner": owner(CLAP), "kind": "polyAftertouch", "value": 0.5 }), "needs a note"),
        (json!({ "owner": owner(CLAP), "kind": "cc", "controller": 123, "value": 0 }), "engine.panic"),
        (json!({ "owner": owner(CLAP), "kind": "program", "value": 0.5 }), "Unsupported expression kind"),
        (json!({ "owner": owner(CLAP), "kind": "aftertouch" }), "finite number"),
        (json!({ "owner": "bus:drums", "kind": "aftertouch", "value": 0.5 }), "not the channel of a track"),
    ] {
        let error = fixture.host.request("midi.expression", payload.clone()).unwrap_err();
        assert!(error.contains(reason), "{payload}: {error}");
    }
    // With the synth unplugged its messages go nowhere, and the reply says so.
    fixture.ports.remove_output(SYNTH);
    fixture.services.services.midi.as_ref().unwrap().service().refresh_devices();
    assert_eq!(fixture.express(json!({ "owner": owner(EXTERNAL), "kind": "aftertouch", "value": 1 })), json!({ "sent": false, "owner": owner(EXTERNAL), "instrument": EXTERNAL }));
    fixture.services.services.shutdown();
}

#[test]
fn controllers_and_pressure_from_a_midi_keyboard_reach_the_instrument_it_plays() {
    let fixture = start("expression-keyboard");
    let (host, ports) = (&fixture.host, &fixture.ports);
    // Controller 74 at 33, channel pressure 99, poly pressure 44 on key 61, a pitch bend.
    let played: &[u8] = &[0xB0, 74, 33, 0xD0, 99, 0xA0, 61, 44, 0xE0, 0x10, 0x50];
    let bend = 0x10 | 0x50 << 7;

    // The cursor is on the CLAP instrument's track: the keyboard plays it.
    host.request("midi.configure", json!({ "track": CLAP, "instrument": CLAP })).unwrap();
    ports.clear_sent();
    ports.feed(KEYS, played);
    fixture.shows("the CLAP instrument", CLAP, &clap_shows(74, 33, 99, 61, 44, bend));
    // The interface hears of the control change (MIDI learn), not of the streams.
    let message = host.wait_for_event("midi.message", Duration::from_secs(5)).expect("a midi.message event");
    assert_eq!(message, json!({ "status": 0xB0, "data1": 74, "data2": 33, "source": KEYS }));

    // Then on the VST3 instrument's track.
    host.request("midi.configure", json!({ "track": VST3, "instrument": VST3 })).unwrap();
    ports.feed(KEYS, played);
    fixture.shows("the VST3 instrument", VST3, &vst3_shows(33, 99, 61, 44, bend));
    assert!(ports.sent_messages(SYNTH).is_empty(), "nothing of that went to the synth");
    // The other plug-in heard nothing of it: a release of pressure there stays there.
    ports.feed(KEYS, &[0xD0, 0]);
    fixture.shows("pressure released on the VST3 instrument", VST3, &[(vst3_sine::PRESSURE, 0.0)]);
    fixture.shows("the CLAP instrument keeps what it got", CLAP, &[(clap_sine::PRESSURE, 99.0)]);

    // On the track of the external instrument the same keys go to its device, on its channel.
    host.request("midi.configure", json!({ "track": EXTERNAL, "instrument": EXTERNAL })).unwrap();
    ports.feed(KEYS, played);
    fixture.synth_gets("the external instrument", &[&[0xB2, 74, 33], &[0xD2, 99], &[0xA2, 61, 44], &[0xE2, 0x10, 0x50]]);
    fixture.shows("the plug-ins did not get them as well", CLAP, &[(clap_sine::PRESSURE, 99.0)]);
    fixture.services.services.shutdown();
}

//! Live input arrives, in order, at the instrument that should get it: through the bridge,
//! with a MIDI keyboard and a hardware synth on in-memory ports, the engine's own sampler, and
//! the CLAP and VST3 test instruments in containers.
//!
//! - What is played into a block a plug-in misses reaches it with the next block, in order.
//! - A pedal pressed right after a key came up, within one audio block, does not catch the
//!   note: for a plug-in instrument and for a device.
//! - A key held while its track is moved or removed, or while its instrument is replaced:
//!   the note it started ends, and its release ends nothing else.
//!
//! The first two drive the audio blocks themselves (the test holds the renderer, so the null
//! device's own thread finds it taken and renders nothing): what falls into one block is
//! what the test queued before rendering it.

use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_audio::graph::render::Renderer;
use st_audio::plugins::testing::{artifacts, clap_sine, vst3_sine, CLAP_SINE, VST3_SINE};
use st_audio::plugins::Hosted;
use st_audio::AudioEngine;
use st_host::areas::audio::{AudioOptions, AudioOutput};
use st_host::areas::midi::{MidiDevices, MidiOptions};
use st_host::areas::plugins::PluginAreaOptions;
use st_host::areas::services::ServiceOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};
use st_midi::memory::MemoryPorts;

const SYNTH: i32 = 700;
const KEYS: i32 = 300;
/// The tracks of the project, by what plays them (and their instruments' numbers).
const EXTERNAL: i64 = 0;
const SAMPLER: i64 = 1;
const CLAP: i64 = 2;
const VST3: i64 = 3;
/// A second sampled instrument that no track plays at first.
const OTHER_SAMPLE: i64 = 4;

const NAMES: [&str; 4] = ["hardware", "sampler", "clap", "vst3"];

/// The mixer channel of one of the four tracks: it stays the track's wherever the track goes.
fn owner(track: i64) -> String {
    format!("track:{}", NAMES[track as usize])
}

struct Fixture {
    host: TestHost,
    services: Host,
    ports: MemoryPorts,
    project: Value,
    _folder: TempDir,
}

/// A host on the null audio device with plug-in hosting, a MIDI keyboard (`KEYS`) and a
/// hardware synth (`SYNTH`) on in-memory ports, and a project of four tracks: an external MIDI
/// instrument on the synth's channel 3, the built-in sampler (a sine that sounds for as long
/// as its note is held), the CLAP sine and the VST3 sine.
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
    let track = |index: i64| json!({ "id": index, "name": NAMES[index as usize], "routingID": NAMES[index as usize], "volume": 1, "pan": 0, "instrument": index });
    let plugin = |name: &str, component: String, format: &str, path: &std::path::Path| {
        json!({ "instanceID": name, "owner": format!("track:{name}"), "kind": "instrument", "slot": 0, "component": component, "format": format, "bundlePath": path, "name": name })
    };
    let sine = |id: i64| json!({ "id": id, "type": "sample", "name": "Sine", "samplePath": "builtin:wavetables/Basic/Sine.wav", "mode": "wavetable", "volume": 1, "attack": 0, "decay": 0.01, "sustain": 1, "release": 0.05 });
    let project = json!({
        "version": 2, "name": "Live", "bpm": 120, "linesPerBeat": 4, "swing": 0, "masterVolume": 1, "metronome": false,
        "tracks": [track(EXTERNAL), track(SAMPLER), track(CLAP), track(VST3)],
        "instruments": [
            { "id": 0, "type": "midi", "name": "Hardware", "midiOutput": SYNTH, "midiChannel": 3 },
            sine(1),
            { "id": 2, "type": "plugin", "name": "CLAP sine", "volume": 1 },
            { "id": 3, "type": "plugin", "name": "VST3 sine", "volume": 1 },
            sine(OTHER_SAMPLE)
        ],
        "patterns": [{ "id": 0, "length": 16, "steps": vec![vec![Value::Null; 16]; 4], "automation": [] }],
        "sequence": [0], "loopSong": true,
        "routing": { "buses": [], "outputs": {}, "sends": [], "sidechains": [] },
        "audioPlugins": [plugin("clap", format!("clap:{CLAP_SINE}"), "CLAP", &installed.clap), plugin("vst3", format!("vst3:{VST3_SINE}"), "VST3", &installed.vst3)],
    });
    let fixture = Fixture { host, services, ports, project, _folder: folder };
    fixture.publish(&fixture.project);
    fixture.ports.clear_sent();
    fixture
}

/// The audio blocks in the test's hands: while this exists the null device renders nothing.
struct Blocks<'a> {
    renderer: MutexGuard<'a, Renderer>,
}

impl Blocks<'_> {
    /// Renders `count` blocks of 512 frames, at the pace of a device (MIDI for the synth is
    /// timed against the clock).
    fn render(&mut self, count: usize) {
        let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
        for _ in 0..count {
            std::thread::sleep(Duration::from_millis(11));
            self.renderer.render(&mut left, &mut right);
        }
    }

    /// Renders block after block until `done` (what a plug-in shows of itself arrives a
    /// moment after the block that changed it).
    fn until(&mut self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.render(1);
        }
    }
}

impl Fixture {
    fn engine(&self) -> &Arc<AudioEngine> {
        self.services.audio.as_ref().unwrap().engine()
    }

    /// The plug-in instance of the CLAP or the VST3 track.
    fn hosted(&self, track: i64) -> Arc<Hosted> {
        self.engine().plugins().expect("plug-in hosting").hosted(NAMES[track as usize]).expect("the instrument's instance")
    }

    fn request(&self, command: &str, payload: Value) -> Value {
        self.host.request(command, payload.clone()).unwrap_or_else(|error| panic!("{command} {payload} failed: {error}"))
    }

    fn publish(&self, project: &Value) {
        let published = self.host.reply_within(self.host.send("engine.project", json!({ "project": project })), Duration::from_secs(120));
        published.expect("the project and its plug-ins are loaded");
    }

    /// Plays bytes on the MIDI keyboard, pointed at the track at `place` and at `instrument`,
    /// and waits until they are routed and delivered.
    fn keys(&self, place: i64, instrument: i64, bytes: &[u8]) {
        self.request("midi.configure", json!({ "track": place, "instrument": instrument }));
        self.ports.feed(KEYS, bytes);
        self.services.services.midi.as_ref().unwrap().service().flush_input();
    }

    /// The interface plays: `note.on` and `note.off` name the track by its place.
    fn page_on(&self, place: i64, instrument: i64, note: u8) {
        self.request("note.on", json!({ "note": note, "velocity": 100, "track": place, "instrument": instrument }));
    }

    fn page_off(&self, place: i64, instrument: i64, note: u8) {
        self.request("note.off", json!({ "note": note, "track": place, "instrument": instrument }));
    }

    /// The interface's sustain pedal, for the track with this channel.
    fn page_pedal(&self, track: i64, value: f64) {
        self.request("midi.expression", json!({ "kind": "sustain", "owner": owner(track), "value": value }));
    }

    /// The state of the note of a plug-in instrument (silent, key down, sustained), found by
    /// its channel wherever its track is now.
    fn note_state(&self, track: i64) -> &'static str {
        let (address, states) = if track == CLAP {
            (clap_sine::NOTE_STATE, [clap_sine::SILENT, clap_sine::KEY_DOWN, clap_sine::SUSTAINED])
        } else {
            (vst3_sine::NOTE_STATE, [vst3_sine::SILENT, vst3_sine::KEY_DOWN, vst3_sine::SUSTAINED])
        };
        let list = self.request("plugin.parameters", json!({ "owner": owner(track), "kind": "instrument", "slot": 0 }));
        let address = address.to_string();
        let value = list["parameters"].as_array().unwrap().iter().find(|parameter| parameter["address"] == address.as_str()).expect("the note state")["value"].as_f64().unwrap();
        ["silent", "key down", "sustained"][states.iter().position(|state| (state - value).abs() < 1e-9).unwrap_or_else(|| panic!("an unknown note state {value}"))]
    }

    /// Waits (the device renders) until the plug-in's note is in `state`.
    fn note_becomes(&self, what: &str, track: i64, state: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let seen = self.note_state(track);
            if seen == state {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: the note of {} is {seen}, expected {state}", NAMES[track as usize]);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The plug-in's note is in `state` now and half a second from now.
    fn note_stays(&self, what: &str, track: i64, state: &str) {
        self.note_becomes(what, track, state);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(self.note_state(track), state, "{what}");
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

    /// The peak of the strip of the track at `place` in the first status event made from now on.
    fn peak(&self, place: i64) -> f64 {
        self.host.events();
        let status = self.host.wait_for_event("status", Duration::from_secs(10)).expect("a status event");
        status["trackPeaks"][place as usize].as_f64().unwrap()
    }

    fn peak_where(&self, what: &str, place: i64, wanted: impl Fn(f64) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let peak = self.peak(place);
            if wanted(peak) {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: the peak of the track at place {place} is {peak}");
        }
    }

    fn sounds(&self, what: &str, place: i64) {
        self.peak_where(what, place, |peak| peak > 0.05);
    }

    fn fades(&self, what: &str, place: i64) {
        self.peak_where(what, place, |peak| peak < 0.001);
    }

    /// The sampled note at `place` still sounds two seconds from now. A released note is gone
    /// after 50 ms and the meter falls 22 dB a second after it.
    fn holds(&self, what: &str, place: i64) {
        self.sounds(what, place);
        std::thread::sleep(Duration::from_millis(150));
        let level = self.peak(place);
        std::thread::sleep(Duration::from_secs(2));
        let peak = self.peak(place);
        assert!(peak > level * 0.5, "{what}: the peak of the track at place {place} fell from {level} to {peak}");
    }
}

#[test]
fn what_is_played_into_blocks_a_plug_in_misses_reaches_it_with_the_next_block_in_order() {
    let fixture = start("live-missed");
    let renderer = fixture.engine().renderer();
    let mut blocks = Blocks { renderer: renderer.lock().unwrap() };
    for track in [CLAP, VST3] {
        let hosted = fixture.hosted(track);
        blocks.render(2);
        let missed_before = hosted.instance.stats().fallback_blocks;
        // The plug-in misses the next two blocks. A key, the pedal and the key's release are
        // played into the first of them, from the interface.
        hosted.instance.hold_blocks(2);
        fixture.page_on(track, track, 60);
        fixture.page_pedal(track, 1.0);
        fixture.page_off(track, track, 60);
        blocks.render(2);
        assert_eq!(hosted.instance.stats().fallback_blocks, missed_before + 2, "two blocks were missed");
        assert_eq!(fixture.note_state(track), "silent", "nothing has reached the plug-in yet");
        // The next block carries all three, in the order they were played: the note is held
        // by the pedal. (Out of order the plug-in would be silent: a key released before the
        // pedal went down, or a release that came before its key.)
        blocks.until("the sustained note", || fixture.note_state(track) == "sustained");
        // Pedal-up into a missed block, from the keyboard this time, with a key it plays and
        // releases after it: the first note ends, the second is not held.
        hosted.instance.hold_blocks(1);
        fixture.page_pedal(track, 0.0);
        fixture.keys(track, track, &[0x90, 67, 100, 0x80, 67, 0]);
        blocks.render(1);
        assert_eq!(hosted.instance.stats().fallback_blocks, missed_before + 3);
        blocks.until("silence", || fixture.note_state(track) == "silent");
    }
    drop(blocks);
    fixture.services.services.shutdown();
}

#[test]
fn a_pedal_pressed_right_after_a_key_came_up_does_not_catch_the_note() {
    let fixture = start("live-order");
    let renderer = fixture.engine().renderer();
    let mut blocks = Blocks { renderer: renderer.lock().unwrap() };
    for track in [CLAP, VST3] {
        // From the interface: the key comes up and the pedal goes down within one block.
        fixture.page_on(track, track, 60);
        blocks.until("the key down", || fixture.note_state(track) == "key down");
        fixture.page_off(track, track, 60);
        fixture.page_pedal(track, 1.0);
        blocks.render(1);
        blocks.until("the note to end", || fixture.note_state(track) == "silent");
        // The other order, as close together: the pedal holds the note.
        fixture.page_on(track, track, 62);
        fixture.page_off(track, track, 62);
        blocks.render(1);
        blocks.until("the note the pedal holds", || fixture.note_state(track) == "sustained");
        fixture.page_pedal(track, 0.0);
        blocks.until("the pedal to let go", || fixture.note_state(track) == "silent");

        // From a MIDI keyboard: note-off and pedal arrive back to back.
        fixture.keys(track, track, &[0x90, 64, 100]);
        blocks.until("the key down", || fixture.note_state(track) == "key down");
        fixture.keys(track, track, &[0x80, 64, 0, 0xB0, 64, 127]);
        blocks.render(1);
        blocks.until("the note to end", || fixture.note_state(track) == "silent");
        fixture.keys(track, track, &[0x90, 65, 100, 0x80, 65, 0]);
        blocks.until("the note the pedal holds", || fixture.note_state(track) == "sustained");
        fixture.keys(track, track, &[0xB0, 64, 0]);
        blocks.until("the pedal to let go", || fixture.note_state(track) == "silent");
    }

    // The hardware synth (channel 3): its pedal is timed like its notes, so it gets all of
    // them in the order they were played, whatever falls into one block.
    fixture.ports.clear_sent();
    fixture.page_on(EXTERNAL, EXTERNAL, 72);
    blocks.render(1);
    fixture.page_off(EXTERNAL, EXTERNAL, 72);
    fixture.page_pedal(EXTERNAL, 1.0);
    fixture.page_on(EXTERNAL, EXTERNAL, 74);
    blocks.render(1);
    fixture.page_pedal(EXTERNAL, 0.0);
    fixture.page_off(EXTERNAL, EXTERNAL, 74);
    blocks.render(2);
    fixture.synth_gets("the page's notes and pedal", &[&[0x92, 72, 100], &[0x82, 72, 0], &[0xB2, 64, 127], &[0x92, 74, 100], &[0xB2, 64, 0], &[0x82, 74, 0]]);
    // From the keyboard, with a controller and a pitch bend between the notes.
    fixture.keys(EXTERNAL, EXTERNAL, &[0x90, 60, 90]);
    blocks.render(1);
    fixture.keys(EXTERNAL, EXTERNAL, &[0x80, 60, 0, 0xB0, 64, 127, 0xB0, 1, 50, 0x90, 62, 80, 0xE0, 0, 96, 0x80, 62, 0, 0xB0, 64, 0]);
    blocks.render(2);
    fixture.synth_gets(
        "the keyboard's notes, pedal and controllers",
        &[&[0x92, 60, 90], &[0x82, 60, 0], &[0xB2, 64, 127], &[0xB2, 1, 50], &[0x92, 62, 80], &[0xE2, 0, 96], &[0x82, 62, 0], &[0xB2, 64, 0]],
    );
    drop(blocks);
    fixture.services.services.shutdown();
}

#[test]
fn a_key_held_while_tracks_are_reordered_ends_its_own_note_and_no_other() {
    let fixture = start("live-reorder");
    // The interface holds a key on the CLAP instrument and one on the synth; the keyboard
    // holds one on the sampler, then one on the VST3 instrument.
    fixture.page_on(CLAP, CLAP, 60);
    fixture.page_on(EXTERNAL, EXTERNAL, 72);
    fixture.keys(SAMPLER, SAMPLER, &[0x90, 64, 100]);
    fixture.keys(VST3, VST3, &[0x91, 67, 100]);
    fixture.note_becomes("the CLAP key", CLAP, "key down");
    fixture.note_becomes("the VST3 key", VST3, "key down");
    fixture.sounds("the sampler's key", SAMPLER);
    fixture.synth_gets("the synth's key", &[&[0x92, 72, 100]]);

    // The sampler's and the CLAP instrument's tracks swap places, and so do the synth's and the
    // VST3 instrument's. Every held note is released where it sounds, before the tracks move.
    let mut project = fixture.project.clone();
    project["tracks"].as_array_mut().unwrap().swap(SAMPLER as usize, CLAP as usize);
    project["tracks"].as_array_mut().unwrap().swap(EXTERNAL as usize, VST3 as usize);
    fixture.publish(&project);
    // (Where the tracks are now.)
    let (sampler_place, clap_place, external_place, vst3_place) = (CLAP, SAMPLER, VST3, EXTERNAL);
    fixture.synth_gets("the synth's note was released", &[&[0x82, 72, 0]]);
    fixture.note_becomes("the CLAP instrument got its note-off", CLAP, "silent");
    fixture.note_becomes("the VST3 instrument got its note-off", VST3, "silent");
    fixture.fades("the sampler's note was released", sampler_place);
    fixture.fades("and nothing sounds where the sampler was", clap_place);

    // Now other players hold the same notes on the tracks that stand at the old places: the
    // keyboard plays note 60 on the sampler (where the CLAP instrument was) and note 72 on
    // the VST3 instrument (where the synth was); the interface plays note 64 on the CLAP
    // instrument (where the sampler was) and note 67 on the synth (where the VST3 was).
    fixture.keys(sampler_place, SAMPLER, &[0x92, 60, 100]);
    fixture.keys(vst3_place, VST3, &[0x93, 72, 100]);
    fixture.page_on(clap_place, CLAP, 64);
    fixture.page_on(external_place, EXTERNAL, 67);
    fixture.sounds("the sampler at its new place", sampler_place);
    fixture.note_becomes("the VST3 instrument at its new place", VST3, "key down");
    fixture.note_becomes("the CLAP instrument at its new place", CLAP, "key down");
    fixture.synth_gets("the synth at its new place", &[&[0x92, 67, 100]]);
    // The first keys come up, named by the places their key-downs named. Their notes ended
    // with the move: the releases end nothing, least of all the notes now held there.
    fixture.page_off(CLAP, CLAP, 60);
    fixture.page_off(EXTERNAL, EXTERNAL, 72);
    fixture.ports.feed(KEYS, &[0x80, 64, 0, 0x81, 67, 0]);
    fixture.services.services.midi.as_ref().unwrap().service().flush_input();
    fixture.holds("the sampler's new note is not cut by the interface's old key", sampler_place);
    fixture.note_stays("the VST3 instrument's new note is not cut by the interface's old key", VST3, "key down");
    fixture.note_stays("the CLAP instrument's new note is not cut by the keyboard's old key", CLAP, "key down");
    assert!(fixture.ports.sent_messages(SYNTH).is_empty(), "the synth's new note is not cut by the keyboard's old key: {:?}", fixture.ports.sent_messages(SYNTH));
    // The second keys come up: each ends its own note.
    fixture.ports.feed(KEYS, &[0x82, 60, 0, 0x83, 72, 0]);
    fixture.services.services.midi.as_ref().unwrap().service().flush_input();
    fixture.page_off(clap_place, CLAP, 64);
    fixture.page_off(external_place, EXTERNAL, 67);
    fixture.fades("the sampler", sampler_place);
    fixture.note_becomes("the VST3 instrument", VST3, "silent");
    fixture.note_becomes("the CLAP instrument", CLAP, "silent");
    fixture.synth_gets("the synth", &[&[0x82, 67, 0]]);
    fixture.services.services.shutdown();
}

#[test]
fn a_key_held_while_tracks_are_removed_is_released_and_its_release_ends_nothing_else() {
    let fixture = start("live-remove");
    fixture.page_on(EXTERNAL, EXTERNAL, 72);
    fixture.page_on(CLAP, CLAP, 60);
    fixture.keys(SAMPLER, SAMPLER, &[0x90, 64, 100]);
    fixture.keys(VST3, VST3, &[0x91, 67, 100]);
    fixture.note_becomes("the CLAP key", CLAP, "key down");
    fixture.note_becomes("the VST3 key", VST3, "key down");
    fixture.sounds("the sampler's key", SAMPLER);
    fixture.synth_gets("the synth's key", &[&[0x92, 72, 100]]);

    // The synth's track (the first) is removed: its note is released on the device. The
    // tracks behind it move up, and their held notes end with the move.
    let mut project = fixture.project.clone();
    project["tracks"].as_array_mut().unwrap().remove(EXTERNAL as usize);
    project["patterns"][0]["steps"].as_array_mut().unwrap().remove(EXTERNAL as usize);
    fixture.publish(&project);
    let (sampler_place, clap_place, vst3_place) = (0, 1, 2);
    fixture.synth_gets("the removed track's instrument was silenced", &[&[0x82, 72, 0]]);
    fixture.note_becomes("the CLAP instrument got its note-off", CLAP, "silent");
    fixture.note_becomes("the VST3 instrument got its note-off", VST3, "silent");
    fixture.fades("the sampler's note was released", sampler_place);
    // The keyboard holds note 72 on the sampler, which stands where the synth was, and note
    // 60 on the VST3 instrument, which stands where the CLAP instrument was.
    fixture.keys(sampler_place, SAMPLER, &[0x92, 72, 100]);
    fixture.keys(vst3_place, VST3, &[0x93, 60, 100]);
    fixture.sounds("the sampler where the synth was", sampler_place);
    fixture.note_becomes("the VST3 instrument where the CLAP instrument was", VST3, "key down");
    // The old keys come up: nothing they could end is left, and nothing else ends.
    fixture.page_off(EXTERNAL, EXTERNAL, 72);
    fixture.page_off(CLAP, CLAP, 60);
    fixture.ports.feed(KEYS, &[0x80, 64, 0, 0x81, 67, 0]);
    fixture.services.services.midi.as_ref().unwrap().service().flush_input();
    fixture.holds("the sampler's note goes on", sampler_place);
    fixture.note_stays("the VST3 instrument's note goes on", VST3, "key down");
    assert!(fixture.ports.sent_messages(SYNTH).is_empty(), "the synth hears nothing more: {:?}", fixture.ports.sent_messages(SYNTH));

    // The CLAP instrument's track is removed while the VST3 instrument's key is held (and
    // the sampler's, whose track stays where it is): the VST3 instrument's track moves up and
    // its note ends; the sampler's goes on until its key comes up.
    project["tracks"].as_array_mut().unwrap().remove(clap_place as usize);
    project["patterns"][0]["steps"].as_array_mut().unwrap().remove(clap_place as usize);
    project["audioPlugins"].as_array_mut().unwrap().remove(0);
    fixture.publish(&project);
    fixture.note_becomes("the VST3 instrument got its note-off", VST3, "silent");
    fixture.holds("the sampler's track did not move: its key still holds its note", sampler_place);
    fixture.ports.feed(KEYS, &[0x82, 72, 0, 0x83, 60, 0]);
    fixture.services.services.midi.as_ref().unwrap().service().flush_input();
    fixture.fades("the sampler's key came up", sampler_place);
    fixture.note_stays("the VST3 instrument stays silent", VST3, "silent");
    fixture.services.services.shutdown();
}

#[test]
fn a_key_held_while_its_instrument_is_replaced_still_ends_what_it_started() {
    let fixture = start("live-replace");
    let mut project = fixture.project.clone();

    // The sampler: the track is given another instrument while a key is down. The voice the
    // key started sounds on, and ends when the key comes up.
    fixture.page_on(SAMPLER, SAMPLER, 64);
    fixture.sounds("the sampler's key", SAMPLER);
    project["tracks"][SAMPLER as usize]["instrument"] = json!(OTHER_SAMPLE);
    fixture.publish(&project);
    fixture.holds("the held voice goes on", SAMPLER);
    fixture.page_off(SAMPLER, SAMPLER, 64);
    fixture.fades("the key's release ends it", SAMPLER);

    // The plug-in: the CLAP instrument is replaced by another instance while a key is down.
    // The instance that got the note-on is gone, and its note with it; the key's release
    // reaches the new instance as a note-off for a note it does not play, and a new key plays.
    fixture.keys(CLAP, CLAP, &[0x90, 60, 100]);
    fixture.note_becomes("the CLAP key", CLAP, "key down");
    project["audioPlugins"][0]["instanceID"] = json!("another");
    fixture.publish(&project);
    fixture.note_becomes("a new instance, silent", CLAP, "silent");
    fixture.keys(CLAP, CLAP, &[0x80, 60, 0]);
    fixture.note_stays("the release changes nothing", CLAP, "silent");
    fixture.keys(CLAP, CLAP, &[0x90, 62, 100]);
    fixture.note_becomes("the new instance plays", CLAP, "key down");
    fixture.keys(CLAP, CLAP, &[0x80, 62, 0]);
    fixture.note_becomes("and lets go", CLAP, "silent");

    // The device: the external instrument moves to channel 5 while a key is down. The old
    // channel is silenced as a whole (pedals up, all notes off); the key's release goes to
    // the instrument as it is now.
    fixture.ports.clear_sent();
    fixture.page_on(EXTERNAL, EXTERNAL, 72);
    fixture.synth_gets("the synth's key", &[&[0x92, 72, 100]]);
    project["instruments"][0]["midiChannel"] = json!(5);
    fixture.publish(&project);
    let deadline = Instant::now() + Duration::from_secs(10);
    while fixture.ports.sent_messages(SYNTH).iter().filter(|message| **message == [0xB2, 123, 0]).count() < 2 {
        assert!(Instant::now() < deadline, "the old channel was not cleared: {:?}", fixture.ports.sent_messages(SYNTH));
        std::thread::sleep(Duration::from_millis(5));
    }
    let sent = fixture.ports.sent_messages(SYNTH);
    assert!(sent.iter().all(|message| message[0] == 0xB2), "only the old channel was told: {sent:?}");
    fixture.ports.clear_sent();
    fixture.page_off(EXTERNAL, EXTERNAL, 72);
    fixture.synth_gets("the release, on the instrument's channel", &[&[0x84, 72, 0]]);
    fixture.services.services.shutdown();
}

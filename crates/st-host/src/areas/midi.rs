//! The MIDI area: devices, input, output to external instruments and MIDI clock (`st-midi`),
//! connected to the audio engine (`st-audio`).
//!
//! [`install`] creates the [`MidiService`] and registers `midi.devices`, `midi.configure`,
//! `midi.clock` and `midi.performance`. With an audio engine it also connects the two
//! directions between MIDI and audio:
//!
//! - **Input** ([`st_midi::InputTarget`], implemented here over the engine): a key played on a
//!   MIDI keyboard sounds on the track and instrument the service routed it to. While note
//!   recording is armed (`transport.recordArm`) the key is an *input edge*: the audio thread
//!   applies it and acknowledges the transport position it landed on. The interface then gets
//!   two `midi.note` events for it, as it does from its own keyboard path
//!   (`ui/app-input-capture.js`): at once `{on, note, velocity, channel, source, track,
//!   instrument, inputOwner, recordingPending: true}` (highlight the key, open a capture), and
//!   when the audio thread has answered the same with `recordingOnly: true` and
//!   `inputAcknowledgement` instead of `recordingPending`. A capture that ends without its key
//!   release (retriggered, input route changed, panic) is closed with `{on: false, inputOwner,
//!   recordingOnly: true, recordingCancelled: true, reason}`. Unarmed, the note is simply played
//!   and reported with the transport position (`pattern`, `row`, `position`).
//!   Controllers, channel and poly pressure, pitch bend and program changes played on a device
//!   go to the instrument the service routed them to, the way its notes go and behind them
//!   ([`crate::areas::audio::Audio::play_message`]): the engine's own voices take pitch bend
//!   and the modulation wheel, a plug-in instrument gets the message with its next block,
//!   and for an external MIDI instrument it leaves from the render thread for the
//!   instrument's device and channel, timed like its notes. `midi.expression`, the audio
//!   area's command for the same messages from the interface, goes the same way.
//!   The pedals (sustain, sostenuto, soft) and every key go through the audio area's record
//!   of the pedals ([`crate::areas::audio`], "The pedals"): an instrument with a pedal of its
//!   own gets the pedal and its note-offs as played, the engine's own instruments have their
//!   note-offs held back while the sustain pedal of the same device and channel is down.
//!
//!   **A recorded note is as long as it sounded.** A key released while the sustain pedal of
//!   its device and channel is down keeps its capture open: the interface hears of the key
//!   release at once as a plain `{on: false, note, …}` (the key is no longer highlighted),
//!   and the two events that close the capture follow when the note ends: when the pedal
//!   comes up (or goes below half), when the same key is struck again on the same instrument,
//!   and on Stop (where the acknowledgement is what a key held through Stop gets). The length
//!   a played-back note gets is so the length that was heard, whether the host held the note
//!   or the instrument did; patterns carry no pedal.
//! - **Output** ([`st_audio::midi::MidiOutput`], implemented here over
//!   [`st_midi::RenderOutput`]): on the render thread, the engine's note events for instruments
//!   of type `midi`, and the channel messages played live for them (the pedals among them),
//!   become timed messages for their device and channel in the order they were played, the
//!   transport position
//!   becomes MIDI clock and Start/Stop/Continue, and a stop or panic silences every external
//!   instrument. [`RenderHook`] gives each block its device time.
//!
//! Every project the session accepts sets the service's routes (`MidiService::set_project`).
//!
//! # Threads
//!
//! Create the service on the main thread ([`crate::install`] is called there): CoreMIDI
//! delivers to the run loop of the thread that created the client. Input arrives on the
//! service's input thread; nothing here blocks it (an input edge is queued for the audio
//! thread, its acknowledgement arrives on the engine's service thread).
//!
//! # Seams
//!
//! | Need | Seam |
//! |---|---|
//! | Line inputs: the latency of a synth heard back through its track | `MidiService::set_return_compensation` through [`Midi::service`] |
//! | The device's output latency | `st_audio::midi::MidiBlock::playback_delay` (from the device's callback timestamps) |

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use crate::areas::audio::{Audio, ExternalInstruments};
use std::time::Duration;

use serde_json::{json, Value};
use st_audio::midi::{MidiBlock, MidiOutput, MidiRender};
use st_audio::{AudioEngine, InputEdge, PlayOptions};
use st_midi::memory::MemoryPorts;
use st_midi::{InputTarget, Message, MidiClock, MidiService, NoteHandling, PedalTarget, RenderOutput, RoutedNote, RoutedPedal, TransportPosition, APP_SOURCE};

use crate::commands::Commands;
use crate::session::Session;
use crate::{Error, EventSink, Result, Settings};

/// Which MIDI devices the host uses.
#[derive(Clone)]
pub enum MidiDevices {
    /// The system's devices (CoreMIDI, the ALSA sequencer), watched for hot-plug.
    System,
    /// In-memory ports (tests). Hot-plug is checked when the test asks (`MidiService::refresh_devices`).
    Memory(MemoryPorts),
    /// No devices at all: the commands answer with empty lists.
    None,
}

/// How [`crate::install`] sets MIDI up.
#[derive(Clone)]
pub struct MidiOptions {
    pub devices: MidiDevices,
}

impl MidiOptions {
    /// The system's devices, unless the environment variable `SUPERTRACKER_MIDI` is `off`.
    pub fn from_environment() -> MidiOptions {
        let off = std::env::var("SUPERTRACKER_MIDI").is_ok_and(|value| value == "off");
        MidiOptions { devices: if off { MidiDevices::None } else { MidiDevices::System } }
    }
}

const SETTINGS_SECTION: &str = "midi.clock";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One key that is being recorded: the capture the interface opened for it.
struct Capture {
    owner: String,
    note: RoutedNote,
}

/// The engine as MIDI input plays it. See the module documentation.
struct Input {
    audio: Arc<Audio>,
    engine: Arc<AudioEngine>,
    events: EventSink,
    /// Recorded keys still held, by `(source, channel, note)`.
    captures: Mutex<HashMap<(i32, u8, u8), Capture>>,
    /// Recorded keys that were released under the sustain pedal and sound on, by `(source,
    /// channel, note)` and what the note sounds on: their captures end with the notes.
    sustained: Mutex<HashMap<(i32, u8, u8, PedalTarget), Capture>>,
    next_owner: AtomicU64,
}

impl Input {
    fn owner(&self) -> String {
        // Unique across restarts of the host as well: the interface keeps captures by this name.
        static RUN: OnceLock<u128> = OnceLock::new();
        let run = RUN.get_or_init(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_nanos()).unwrap_or(0));
        format!("midi:{run:x}-{}", self.next_owner.fetch_add(1, Ordering::SeqCst))
    }

    fn cancel(&self, capture: &Capture, reason: &str) {
        self.events.emit("midi.note", json!({ "on": false, "inputOwner": capture.owner, "recordingOnly": true, "recordingCancelled": true, "reason": reason }));
    }

    fn cancel_all(&self, reason: &str) {
        let mut captures: Vec<Capture> = lock(&self.captures).drain().map(|(_, capture)| capture).collect();
        captures.extend(lock(&self.sustained).drain().map(|(_, capture)| capture));
        for capture in &captures {
            self.cancel(capture, reason);
        }
    }

    /// The host released a held key's note because its track was moved or removed: a capture
    /// that waited for the key ends here.
    fn key_ended(&self, source: i32, channel: u8, note: u8) {
        let capture = lock(&self.captures).remove(&(source, channel, note));
        if let Some(capture) = capture {
            self.cancel(&capture, "the track was moved or removed");
        }
    }

    /// A note the sustain pedal kept sounding is over: the capture that waited for it gets its
    /// release, without touching the sound.
    fn sustained_note_ended(&self, source: i32, channel: u8, note: u8, target: PedalTarget) {
        let capture = lock(&self.sustained).remove(&(source, channel, note, target));
        if let Some(capture) = capture {
            self.capture(&capture.note, false, capture.owner, false);
        }
    }

    /// Reports the key to the interface and hands it to the audio thread as an input edge; the
    /// acknowledgement follows as a second `midi.note` event.
    fn capture(&self, note: &RoutedNote, on: bool, owner: String, apply_note: bool) {
        let mut event = json!({
            "on": on, "note": note.note, "velocity": note.velocity, "channel": note.channel, "source": note.source,
            "track": note.track, "instrument": note.instrument, "inputOwner": owner,
        });
        let mut pending = event.clone();
        pending["recordingPending"] = json!(true);
        self.events.emit("midi.note", pending);
        let events = self.events.clone();
        let edge = InputEdge { owner, on, track: note.track as i32, instrument: note.instrument as i32, note: i32::from(note.note), velocity: i32::from(note.velocity), apply_note };
        self.engine.input_edge(
            edge,
            Box::new(move |acknowledgement| {
                event["recordingOnly"] = json!(true);
                event["inputAcknowledgement"] = acknowledgement;
                events.emit("midi.note", event);
            }),
        );
    }
}

impl InputTarget for Input {
    fn note_on(&self, note: &RoutedNote) -> NoteHandling {
        let key = (note.source, note.channel, note.note);
        // The pedals first: one that is down reaches the instrument before the note.
        let target = self.audio.key_down(note.source, note.channel, note.track as i32, note.instrument as i32, note.note);
        // The key struck again ends what it was recording: a note the pedal kept sounding on
        // this instrument gets its length here; a key that never came up loses its capture.
        let sustained = lock(&self.sustained).remove(&(note.source, note.channel, note.note, target));
        if let Some(sustained) = sustained {
            self.capture(&sustained.note, false, sustained.owner, false);
        }
        let held = lock(&self.captures).remove(&key);
        if let Some(held) = held {
            self.cancel(&held, "retriggered");
        }
        if self.engine.record_armed() {
            let owner = self.owner();
            lock(&self.captures).insert(key, Capture { owner: owner.clone(), note: *note });
            self.capture(note, true, owner, true);
            return NoteHandling::Captured;
        }
        self.engine.note_on(note.track as i32, note.instrument as i32, i32::from(note.note), i32::from(note.velocity));
        let transport = self.engine.transport_position();
        NoteHandling::Played(Some(TransportPosition { pattern: i64::from(transport.pattern), row: i64::from(transport.row), position: transport.position }))
    }

    fn note_off(&self, note: &RoutedNote) -> NoteHandling {
        let up = self.audio.key_up(note.source, note.channel, note.track as i32, note.instrument as i32, note.note);
        // The release of a recorded key belongs to its capture, also when recording was disarmed
        // meanwhile (the acknowledgement then says so).
        let capture = lock(&self.captures).remove(&(note.source, note.channel, note.note));
        match capture {
            // The note goes on sounding under the pedal: its capture ends when the note does.
            // The interface hears of the key itself now.
            Some(capture) if up.sustained => {
                let replaced = lock(&self.sustained).insert((note.source, note.channel, note.note, up.target), capture);
                if let Some(replaced) = replaced {
                    self.cancel(&replaced, "retriggered");
                }
            }
            Some(capture) => {
                self.capture(note, false, capture.owner, up.send_note_off);
                return NoteHandling::Captured;
            }
            None => {}
        }
        if up.send_note_off {
            self.engine.note_off(note.track as i32, i32::from(note.note));
        }
        NoteHandling::Played(None)
    }

    fn release(&self, note: &RoutedNote) {
        // The input route changed with the key down: a capture that was waiting for the key
        // ends here, and the note is released like a key (its pedal is lifted next).
        let up = self.audio.key_up(note.source, note.channel, note.track as i32, note.instrument as i32, note.note);
        let capture = lock(&self.captures).remove(&(note.source, note.channel, note.note));
        if let Some(capture) = capture {
            self.cancel(&capture, "MIDI input route changed");
        }
        if up.send_note_off {
            self.engine.note_off(note.track as i32, i32::from(note.note));
        }
    }

    fn pedal(&self, pedal: &RoutedPedal) {
        self.audio.pedal(pedal.source, pedal.channel, pedal.controller, pedal.value, pedal.destination.map(|(track, instrument)| (track as i32, instrument as i32)));
    }

    fn lift_pedals(&self) {
        self.audio.lift_pedals(|source| source != APP_SOURCE);
    }

    fn message(&self, track: usize, instrument: usize, message: Message) {
        self.audio.play_message(track as i32, instrument as i32, [message.status, message.data1, message.data2]);
    }

    fn panic(&self) {
        self.cancel_all("all notes off");
        self.audio.forget_pedals();
        self.engine.panic();
    }

    fn transport_start(&self) {
        self.engine.play(&PlayOptions { song: true, ..PlayOptions::default() });
    }

    fn transport_stop(&self) {
        self.audio.silence_pedals();
        self.engine.stop();
    }

    fn frozen(&self, track: usize) {
        let name = self.engine.project().and_then(|project| project.track_list().get(track).and_then(|item| item.name.clone())).unwrap_or_else(|| format!("Track {}", track + 1));
        self.events.emit("error", json!({ "message": format!("{name} is frozen: its instrument is printed to audio and its plug-ins are released. Unfreeze it to audition or play notes.") }));
    }
}

/// The render thread's MIDI output: `st-audio`'s hook over [`RenderOutput`].
///
/// # The time of a block
///
/// MIDI is scheduled in device time ([`MidiClock`]); the renderer counts frames. The hook keeps
/// one anchor (a graph frame and its device time) and derives each block's time from it, so
/// that events inside a run of blocks keep their exact distance in frames whatever the
/// scheduling of the audio callback did. The anchor follows the clock slowly (the audio
/// device's crystal and the system's clock drift apart), and is set anew when a block arrives
/// more than 50 ms from where it was expected (the stream started, stalled or was replaced).
/// No allocation, no lock; reading the clock is the only system call.
pub struct RenderHook {
    output: RenderOutput,
    clock: MidiClock,
    /// `(graph frame, device time)` of the last block.
    anchor: Option<(u64, f64)>,
    /// Counts the times every external instrument was sent pedal-up and all-notes-off from
    /// here ([`Silenced`]).
    silenced: Silenced,
}

/// Counts the times the render thread silenced the external instruments (the transport
/// stopped, by Stop or at the end of the song; a panic). What a device was told about a pedal
/// before is void afterwards: the count is part of an external instrument's generation
/// ([`ExternalInstruments::generation`]), so a pedal that is still down is sent again with the
/// next note.
type Silenced = Arc<AtomicU64>;

/// A block further than this from its expected time starts a new anchor (nanoseconds).
const REANCHOR: f64 = 50_000_000.0;
/// How much of the difference between expected and observed time a block corrects.
const FOLLOW: f64 = 0.02;

impl RenderHook {
    fn block_time(&mut self, block: &MidiBlock) -> u64 {
        let now = self.clock.now() as f64;
        let ticks_per_frame = MidiClock::ticks_per_frame(block.sample_rate);
        let time = match self.anchor {
            Some((frame, ticks)) if block.time >= frame => {
                let expected = ticks + (block.time - frame) as f64 * ticks_per_frame;
                if (expected - now).abs() <= REANCHOR {
                    expected + (now - expected) * FOLLOW
                } else {
                    now
                }
            }
            _ => now,
        };
        self.anchor = Some((block.time, time));
        (time + block.playback_delay.as_nanos() as f64).max(1.0) as u64
    }
}

impl MidiRender for RenderHook {
    fn begin_block(&mut self, block: &MidiBlock) {
        let time = self.block_time(block);
        self.output.begin_block(time, block.frames, block.sample_rate);
        self.output.clock(block.beat, block.playing, block.tempo);
    }

    fn note(&mut self, instrument: usize, on: bool, note: u8, velocity: u8, frame_offset: i32) -> bool {
        self.output.note(instrument, on, note, velocity, frame_offset)
    }

    fn message(&mut self, instrument: usize, [status, data1, data2]: [u8; 3], frame_offset: i32) -> bool {
        self.output.message(instrument, status, data1, data2, frame_offset)
    }

    fn all_notes_off(&mut self) {
        self.output.all_notes_off();
        self.silenced.fetch_add(1, Ordering::Release);
    }
}

/// Gives the engine's renderers their hooks. It holds the service weakly: the service plays
/// through the engine (its input target), so a strong reference would keep both alive for ever.
struct Output {
    service: Weak<MidiService>,
    silenced: Silenced,
}

/// The hook of a renderer made after the MIDI service ended.
struct NoOutput;

impl MidiRender for NoOutput {
    fn begin_block(&mut self, _: &MidiBlock) {}
    fn note(&mut self, _: usize, _: bool, _: u8, _: u8, _: i32) -> bool {
        false
    }
    fn message(&mut self, _: usize, _: [u8; 3], _: i32) -> bool {
        false
    }
    fn all_notes_off(&mut self) {}
}

impl MidiOutput for Output {
    fn render_hook(&self) -> Box<dyn MidiRender> {
        match self.service.upgrade() {
            Some(service) => Box::new(RenderHook { output: service.render_output(), clock: service.clock(), anchor: None, silenced: self.silenced.clone() }),
            None => Box::new(NoOutput),
        }
    }
}

/// Puts `service`'s output on `engine`'s render thread: notes of external MIDI instruments,
/// MIDI clock and transport messages, all-notes-off on stop and panic. [`install`] does this for
/// the host's engine; it is public for tests that drive a renderer themselves.
pub fn connect_output(engine: &AudioEngine, service: &Arc<MidiService>) {
    connect(engine, service, Silenced::default());
}

fn connect(engine: &AudioEngine, service: &Arc<MidiService>, silenced: Silenced) {
    engine.set_midi_output(Some(Arc::new(Output { service: Arc::downgrade(service), silenced })));
}

/// The devices of external MIDI instruments, for the audio area.
struct Devices {
    service: Weak<MidiService>,
    silenced: Silenced,
}

impl ExternalInstruments for Devices {
    fn present(&self, instrument: usize) -> Option<bool> {
        let route = self.service.upgrade()?.host().route(instrument);
        route.external.then_some(route.output.is_some())
    }

    fn generation(&self, instrument: usize) -> u64 {
        // Both only ever count up, so their sum changes whenever one of them does.
        self.service.upgrade().map_or(0, |service| service.route_generation(instrument)) + self.silenced.load(Ordering::Acquire)
    }
}

/// The MIDI service of the host.
pub struct Midi {
    service: Arc<MidiService>,
    engine: Option<Arc<AudioEngine>>,
}

impl Midi {
    /// The service, for other areas (see the seams in the module documentation).
    pub fn service(&self) -> &Arc<MidiService> {
        &self.service
    }

    /// Silences external instruments, takes MIDI output off the render thread and closes every
    /// device (the host is quitting).
    pub fn shutdown(&self) {
        if let Some(engine) = &self.engine {
            engine.set_midi_output(None);
        }
        self.service.shutdown();
    }
}

impl Drop for Midi {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Creates the MIDI service, connects it to the audio engine (when there is one) and to
/// project publications, and registers the `midi.*` commands.
pub fn install(commands: &mut Commands, session: &Session, settings: &Arc<Settings>, events: EventSink, audio: Option<&Arc<Audio>>, options: &MidiOptions) -> Result<Arc<Midi>> {
    let engine = audio.map(|audio| audio.engine().clone());
    let (backend, poll_interval) = match &options.devices {
        MidiDevices::System => (st_midi::midir_backend::MidirBackend::new().map(|backend| Box::new(backend) as Box<dyn st_midi::ports::Backend>), Some(Duration::from_millis(500))),
        MidiDevices::Memory(ports) => (Ok(ports.backend()), None),
        MidiDevices::None => (Err("MIDI devices are turned off for this run (SUPERTRACKER_MIDI=off).".to_string()), None),
    };
    let service = MidiService::new(st_midi::MidiOptions {
        backend,
        clock_settings: settings.get(SETTINGS_SECTION),
        save_clock_settings: Box::new({
            let settings = settings.clone();
            move |value| settings.set(SETTINGS_SECTION, value).map_err(|error| error.to_string())
        }),
        events: Box::new({
            let events = events.clone();
            move |name, payload: Value| events.emit(name, payload)
        }),
        poll_interval,
        input_ids: None,
    })
    .map_err(|error| Error::new(error.to_string()))?;
    commands
        .with(&service)
        .register("midi.devices", MidiService::devices)
        .register("midi.configure", MidiService::configure)
        .register("midi.clock", MidiService::clock_command)
        .register("midi.performance", MidiService::performance);

    if let (Some(engine), Some(audio)) = (&engine, audio) {
        let input = Arc::new(Input { audio: audio.clone(), engine: engine.clone(), events, captures: Mutex::default(), sustained: Mutex::default(), next_owner: AtomicU64::new(1) });
        service.set_target(input.clone());
        service.set_audio_running(true);
        let silenced = Silenced::default();
        connect(engine, &service, silenced.clone());
        // Which instruments are external and have their device, for `midi.expression` and the
        // pedals; the end of recorded notes that a pedal kept sounding, and of recorded keys
        // whose track went away. Weak both ways: the service plays through the engine the
        // audio area owns, and its input target holds the audio area.
        audio.set_external_midi(Arc::new(Devices { service: Arc::downgrade(&service), silenced }));
        let input = Arc::downgrade(&input);
        audio.on_sustained_note_end(Box::new({
            let input = input.clone();
            move |source, channel, note, target| {
                if let Some(input) = input.upgrade() {
                    input.sustained_note_ended(source, channel, note, target);
                }
            }
        }));
        audio.on_key_ended(Box::new(move |source, channel, note| {
            if let Some(input) = input.upgrade() {
                input.key_ended(source, channel, note);
            }
        }));
    }
    {
        let service = service.clone();
        session.observe_projects(move |project| service.set_project(&project.to_value()));
    }
    // A session restored at launch is not published again until the interface is ready.
    if let Some(project) = session.project() {
        service.set_project(&project.to_value());
    }
    Ok(Arc::new(Midi { service, engine }))
}

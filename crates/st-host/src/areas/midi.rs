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
//! - **Output** ([`st_audio::midi::MidiOutput`], implemented here over
//!   [`st_midi::RenderOutput`]): on the render thread, the engine's note events for instruments
//!   of type `midi` become timed messages for their device and channel, the transport position
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
//! | Controllers, program changes and aftertouch from a keyboard for a track's plug-in | [`Midi::set_plugin_receiver`]; until the plug-in area sets one they go nowhere (pitch bend reaches the engine's own voices either way) |
//! | `midi.expression` (the audio area's command) for external MIDI instruments | `MidiService::send_to_instrument` through [`Midi::service`] |
//! | Line inputs: the latency of a synth heard back through its track | `MidiService::set_return_compensation` through [`Midi::service`] |
//! | The device's output latency | `st_audio::midi::MidiBlock::playback_delay` (the device layer does not report it yet) |

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use st_audio::midi::{MidiBlock, MidiOutput, MidiRender};
use st_audio::{AudioEngine, InputEdge, PlayOptions};
use st_midi::memory::MemoryPorts;
use st_midi::{InputTarget, Message, MidiClock, MidiService, NoteHandling, RenderOutput, RoutedNote, TransportPosition};

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

/// Receives a channel message for the plug-in of a track: `(track, status without channel, data1, data2)`.
pub type PluginReceiver = Box<dyn Fn(usize, u8, u8, u8) + Send + Sync>;

/// One key that is being recorded: the capture the interface opened for it.
struct Capture {
    owner: String,
    track: usize,
    note: u8,
}

/// The engine as MIDI input plays it. See the module documentation.
struct Input {
    engine: Arc<AudioEngine>,
    events: EventSink,
    /// Recorded keys still held, by `(source, channel, note)`.
    captures: Mutex<HashMap<(i32, u8, u8), Capture>>,
    next_owner: AtomicU64,
    plugin: OnceLock<PluginReceiver>,
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
        let captures: Vec<Capture> = lock(&self.captures).drain().map(|(_, capture)| capture).collect();
        for capture in &captures {
            self.cancel(capture, reason);
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
        if self.engine.record_armed() {
            let owner = self.owner();
            let previous = lock(&self.captures).insert((note.source, note.channel, note.note), Capture { owner: owner.clone(), track: note.track, note: note.note });
            if let Some(previous) = previous {
                self.cancel(&previous, "retriggered");
            }
            self.capture(note, true, owner, true);
            return NoteHandling::Captured;
        }
        self.engine.note_on(note.track as i32, note.instrument as i32, i32::from(note.note), i32::from(note.velocity));
        let transport = self.engine.transport_position();
        NoteHandling::Played(Some(TransportPosition { pattern: i64::from(transport.pattern), row: i64::from(transport.row), position: transport.position }))
    }

    fn note_off(&self, note: &RoutedNote, sound: bool) -> NoteHandling {
        // The release of a recorded key belongs to its capture, also when recording was disarmed
        // meanwhile (the acknowledgement then says so).
        let capture = lock(&self.captures).remove(&(note.source, note.channel, note.note));
        if let Some(capture) = capture {
            self.capture(note, false, capture.owner, sound);
            return NoteHandling::Captured;
        }
        if sound {
            self.engine.note_off(note.track as i32, i32::from(note.note));
        }
        NoteHandling::Played(None)
    }

    fn release(&self, track: usize, note: u8) {
        // A note released without its key (pedal up, the input route changed): a capture that was
        // waiting for that key ends here.
        let ended: Vec<Capture> = {
            let mut captures = lock(&self.captures);
            let keys: Vec<(i32, u8, u8)> = captures.iter().filter(|(_, capture)| capture.track == track && capture.note == note).map(|(key, _)| *key).collect();
            keys.iter().filter_map(|key| captures.remove(key)).collect()
        };
        for capture in &ended {
            self.cancel(capture, "MIDI input route changed");
        }
        self.engine.note_off(track as i32, i32::from(note));
    }

    fn message(&self, track: usize, message: Message) {
        if let Some(plugin) = self.plugin.get() {
            plugin(track, message.status & 0xF0, message.data1, message.data2);
        }
    }

    fn pitch_bend(&self, track: usize, semitones: f32) {
        self.engine.pitch_bend(track as i32, semitones);
    }

    fn panic(&self) {
        self.cancel_all("all notes off");
        self.engine.panic();
    }

    fn transport_start(&self) {
        self.engine.play(&PlayOptions { song: true, ..PlayOptions::default() });
    }

    fn transport_stop(&self) {
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
}

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

    fn all_notes_off(&mut self) {
        self.output.all_notes_off();
    }
}

/// Gives the engine's renderers their hooks. It holds the service weakly: the service plays
/// through the engine (its input target), so a strong reference would keep both alive for ever.
struct Output {
    service: Weak<MidiService>,
}

/// The hook of a renderer made after the MIDI service ended.
struct NoOutput;

impl MidiRender for NoOutput {
    fn begin_block(&mut self, _: &MidiBlock) {}
    fn note(&mut self, _: usize, _: bool, _: u8, _: u8, _: i32) -> bool {
        false
    }
    fn all_notes_off(&mut self) {}
}

impl MidiOutput for Output {
    fn render_hook(&self) -> Box<dyn MidiRender> {
        match self.service.upgrade() {
            Some(service) => Box::new(RenderHook { output: service.render_output(), clock: service.clock(), anchor: None }),
            None => Box::new(NoOutput),
        }
    }
}

/// Puts `service`'s output on `engine`'s render thread: notes of external MIDI instruments,
/// MIDI clock and transport messages, all-notes-off on stop and panic. [`install`] does this for
/// the host's engine; it is public for tests that drive a renderer themselves.
pub fn connect_output(engine: &AudioEngine, service: &Arc<MidiService>) {
    engine.set_midi_output(Some(Arc::new(Output { service: Arc::downgrade(service) })));
}

/// The MIDI service of the host.
pub struct Midi {
    service: Arc<MidiService>,
    engine: Option<Arc<AudioEngine>>,
    input: Option<Arc<Input>>,
}

impl Midi {
    /// The service, for other areas (see the seams in the module documentation).
    pub fn service(&self) -> &Arc<MidiService> {
        &self.service
    }

    /// Where controllers, program changes and aftertouch for a track's plug-in go. Only the
    /// first call has an effect.
    pub fn set_plugin_receiver(&self, receiver: PluginReceiver) {
        if let Some(input) = &self.input {
            let _ = input.plugin.set(receiver);
        }
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
pub fn install(commands: &mut Commands, session: &Session, settings: &Arc<Settings>, events: EventSink, engine: Option<Arc<AudioEngine>>, options: &MidiOptions) -> Result<Arc<Midi>> {
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
        destination_count: 1024,
        input_ids: None,
    })
    .map_err(|error| Error::new(error.to_string()))?;
    commands
        .with(&service)
        .register("midi.devices", MidiService::devices)
        .register("midi.configure", MidiService::configure)
        .register("midi.clock", MidiService::clock_command)
        .register("midi.performance", MidiService::performance);

    let input = engine.as_ref().map(|engine| Arc::new(Input { engine: engine.clone(), events, captures: Mutex::default(), next_owner: AtomicU64::new(1), plugin: OnceLock::new() }));
    if let (Some(engine), Some(input)) = (&engine, &input) {
        service.set_target(input.clone());
        service.set_audio_running(true);
        connect_output(engine, &service);
    }
    {
        let service = service.clone();
        session.observe_projects(move |project| service.set_project(&project.to_value()));
    }
    // A session restored at launch is not published again until the interface is ready.
    if let Some(project) = session.project() {
        service.set_project(&project.to_value());
    }
    Ok(Arc::new(Midi { service, engine, input }))
}

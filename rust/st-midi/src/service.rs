//! The service `st-host` registers: the `midi.*` commands, the events, and the calls the audio
//! side makes (routes from the project, the render handle, panic).

use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::clock::{ClockRequest, ClockSettings};
use crate::host::{ConfigureRequest, Devices, HostEvent, HostOptions, MidiError, MidiHost, OutputRoute};
use crate::memory::MemoryPorts;
use crate::message::Message;
use crate::output::{MidiClock, OutputStats, RenderOutput, Route, ROUTE_COUNT};
use crate::ports::Backend;
use crate::router::{InputAction, InputRouter, InputRouting, RoutedNote, APP_SOURCE};
use crate::sustain::Destination;

/// Where the transport was when a note was played, for the interface's note recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportPosition {
    /// The pattern playing.
    pub pattern: i64,
    /// The row playing.
    pub row: i64,
    /// The position within the song, as the engine reports it.
    pub position: f64,
}

/// What the receiver of a note did with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteHandling {
    /// It was played. The service reports it to the interface as a `midi.note` event, with the
    /// transport position when one is given.
    Played(Option<TransportPosition>),
    /// The receiver reports the note itself (input capture for note recording emits its own
    /// `midi.note` events with a recording acknowledgement).
    Captured,
}

/// The engine as MIDI input sees it. `st-host` implements this over `st-audio`. Every method is
/// called from the service's input thread, one at a time, in the order the messages arrived.
/// Implementations must not call back into [`MidiService`] methods that wait for that thread
/// ([`MidiService::flush_input`]); everything else is allowed.
pub trait InputTarget: Send + Sync {
    /// A key was pressed: start the note on `note.track` with `note.instrument`.
    fn note_on(&self, note: &RoutedNote) -> NoteHandling;
    /// A key was released. With `sound` false the sustain pedal holds the note: do not stop it.
    fn note_off(&self, note: &RoutedNote, sound: bool) -> NoteHandling;
    /// Stop a note the pedal was holding.
    fn release(&self, track: usize, note: u8);
    /// A controller, program change, aftertouch or pitch-bend message for the plug-in of a track
    /// (`message.status` is the kind, without a channel).
    fn message(&self, track: usize, message: Message);
    /// Bend the engine's own voices on a track, in semitones.
    fn pitch_bend(&self, track: usize, semitones: f32);
    /// Silence the engine (all notes off arrived). External instruments are silenced by the
    /// service.
    fn panic(&self);
    /// MIDI Start or Continue arrived.
    fn transport_start(&self);
    /// MIDI Stop arrived. External instruments are silenced by the service.
    fn transport_stop(&self);
    /// A note for a frozen track was refused.
    fn frozen(&self, _track: usize) {}
    /// MIDI input or clock was turned on: start the audio engine if it is not running (the clock
    /// and live input run with it).
    fn wake(&self) {}
}

/// Receives the service's events: `(name, payload)`. `st-host` passes `EventSink::emit`.
pub type EventCallback = Box<dyn Fn(&str, Value) + Send + Sync>;
/// Stores the settings section `midi.clock`. `st-host` passes `|s| settings.set("midi.clock", s)`.
pub type SaveClockSettings = Box<dyn Fn(&ClockSettings) -> Result<(), String> + Send + Sync>;

/// What the service is built from.
pub struct MidiOptions {
    /// The device layer; `Err` (the system has no MIDI service) gives a service without devices.
    pub backend: Result<Box<dyn Backend>, String>,
    /// The stored settings section `midi.clock`.
    pub clock_settings: ClockSettings,
    /// Called when the clock settings change.
    pub save_clock_settings: SaveClockSettings,
    /// Receives `midi.note`, `midi.message` and `midi.status`.
    pub events: EventCallback,
    /// How often devices are checked for hot-plug; `None` never (tests call
    /// [`MidiService::refresh_devices`]).
    pub poll_interval: Option<Duration>,
    /// Engine channels a sustained note can sound on (tracks and kit sounds).
    pub destination_count: usize,
}

impl MidiOptions {
    /// The system's devices, checked for hot-plug twice a second.
    pub fn system(clock_settings: ClockSettings, save_clock_settings: SaveClockSettings, events: EventCallback) -> Self {
        MidiOptions {
            backend: crate::midir_backend::MidirBackend::new().map(|backend| Box::new(backend) as Box<dyn Backend>),
            clock_settings,
            save_clock_settings,
            events,
            poll_interval: Some(Duration::from_millis(500)),
            destination_count: 1024,
        }
    }

    /// In-memory ports, no hot-plug thread, settings and events going nowhere: the base for tests.
    pub fn in_memory(ports: &MemoryPorts) -> Self {
        MidiOptions {
            backend: Ok(ports.backend()),
            clock_settings: ClockSettings::default(),
            save_clock_settings: Box::new(|_| Ok(())),
            events: Box::new(|_, _| {}),
            poll_interval: None,
            destination_count: 1024,
        }
    }
}

/// Payload of `midi.performance`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PerformanceRequest {
    /// For each of the 128 keys, the notes it plays; `null` turns the map off.
    pub map: Value,
}

/// Result of `midi.performance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PerformanceStatus {
    /// A chord or scale map is applied to incoming notes.
    pub active: bool,
}

/// One output in the result of `midi.clock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClockOutput {
    /// Device id.
    pub id: i32,
    /// Name to show.
    pub name: String,
    /// Listed but not reachable.
    pub offline: bool,
    /// Gets clock pulses.
    pub clock: bool,
    /// Gets Start, Stop and Continue.
    pub transport: bool,
}

/// Result of `midi.clock`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockStatus {
    /// Clock is sent.
    pub enabled: bool,
    /// The audio engine runs (the clock runs with it, also while the transport is stopped).
    pub running: bool,
    /// The manual lead, or `null` for automatic.
    pub lead_milliseconds: Option<f64>,
    /// The lead used when none is set: the audio input path's latency.
    pub automatic_lead_milliseconds: f64,
    /// The outputs present and what each gets.
    pub devices: Vec<ClockOutput>,
}

/// What the audio side knows about sound returning from external devices.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReturnCompensation {
    /// The clock lead to use when the user set none: what a clocked device's audio needs to reach
    /// the engine, in milliseconds.
    pub automatic_lead_milliseconds: f64,
    /// Returns through line inputs are latency-compensated: notes to the instruments below are
    /// sent early by the clock lead, or they would land one input path late in the mix.
    pub compensated: bool,
    /// Instruments heard back through a line input of their track.
    pub instruments: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Resolved {
    external: bool,
    destination: Option<i32>,
    channel: u8,
    output: Option<OutputRoute>,
}

struct Routes {
    resolved: Vec<Resolved>,
    compensation: ReturnCompensation,
    audio_running: bool,
}

enum InputItem {
    Message(Message, i32),
    Flush(Sender<()>),
    Stop,
}

struct Inner {
    host: OnceLock<MidiHost>,
    router: Mutex<InputRouter>,
    routes: Mutex<Routes>,
    clock_settings: Mutex<ClockSettings>,
    save_clock_settings: SaveClockSettings,
    events: EventCallback,
    target: RwLock<Option<Arc<dyn InputTarget>>>,
    input: Mutex<Sender<InputItem>>,
}

/// MIDI for the host: devices, input routing, output and clock.
///
/// Events it emits:
///
/// - `midi.note` `{on, note, velocity, channel, source, track, instrument, pattern?, row?, position?}`
///   for every note played or released by a device;
/// - `midi.message` `{status, data1, data2, source}` for control changes;
/// - `midi.status` `{type: "midiConfiguration" | "midiDevicesChanged", devices}` after a
///   configuration or hot-plug change, and `{type: "midiError", message}`.
pub struct MidiService {
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl MidiService {
    /// Starts the service: opens the device layer and connects every input present.
    pub fn new(options: MidiOptions) -> Result<Arc<Self>, MidiError> {
        let (sender, receiver) = mpsc::channel();
        let inner = Arc::new(Inner {
            host: OnceLock::new(),
            router: Mutex::new(InputRouter::new(options.destination_count)),
            routes: Mutex::new(Routes { resolved: vec![Resolved::default(); ROUTE_COUNT], compensation: ReturnCompensation::default(), audio_running: false }),
            clock_settings: Mutex::new(options.clock_settings),
            save_clock_settings: options.save_clock_settings,
            events: options.events,
            target: RwLock::new(None),
            input: Mutex::new(sender.clone()),
        });
        let messages = Mutex::new(sender);
        let for_events = Arc::downgrade(&inner);
        let host = MidiHost::new(HostOptions {
            backend: options.backend,
            input_ids: None,
            poll_interval: options.poll_interval,
            // Clock, tick and active sensing arrive dozens of times a second, often echoed by a
            // device we clock ourselves, and mean nothing here: they never leave the device thread.
            on_message: Arc::new(move |message, source| {
                if !matches!(message.status, 0xF8 | 0xF9 | 0xFE) {
                    let _ = lock(&messages).send(InputItem::Message(message, source));
                }
            }),
            on_event: Arc::new(move |event| {
                if let Some(inner) = for_events.upgrade() {
                    inner.host_event(event);
                }
            }),
        })?;
        let _ = inner.host.set(host);
        let for_thread = Arc::downgrade(&inner);
        let thread = thread::Builder::new()
            .name("midi-in".into())
            .spawn(move || input_loop(for_thread, receiver))
            .map_err(|error| MidiError::new(format!("Could not start the MIDI input thread: {error}.")))?;
        inner.refresh_clock();
        Ok(Arc::new(MidiService { inner, thread: Mutex::new(Some(thread)) }))
    }

    /// The device host underneath.
    pub fn host(&self) -> &MidiHost {
        self.inner.host()
    }

    /// Sets the receiver of routed input. Until one is set, input is routed but plays nothing.
    pub fn set_target(&self, target: Arc<dyn InputTarget>) {
        *self.inner.target.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(target);
    }

    // ---- commands -------------------------------------------------------------------------------

    /// Command `midi.devices`: the devices present and the configuration.
    pub fn devices(&self, _: IgnoredAny) -> Result<Devices, MidiError> {
        Ok(self.host().devices())
    }

    /// Command `midi.configure`: device selection (see [`ConfigureRequest`]) and the track and
    /// instrument input plays. Held input notes are released only when the input route really
    /// changes, not when a request repeats the current values.
    pub fn configure(&self, request: ConfigureRequest) -> Result<Devices, MidiError> {
        let touches_route = request.input_id.is_some() || request.input_ids.is_some() || request.input_channel.is_some() || request.enabled.is_some();
        let before = touches_route.then(|| input_route(&self.host().devices()));
        let previous_channel = {
            let mut router = lock(&self.inner.router);
            if request.track.is_some() || request.instrument.is_some() {
                router.set_destination(request.track.unwrap_or(0), request.instrument.unwrap_or(0));
            }
            router.input_channel()
        };
        let devices = self.host().configure(&request)?;
        lock(&self.inner.router).set_input_channel(devices.input_channel);
        if before.is_some_and(|before| previous_channel != devices.input_channel || before != input_route(&devices)) {
            self.release_input_notes();
        }
        if request.enabled != Some(false) {
            self.inner.wake();
        }
        Ok(devices)
    }

    /// Command `midi.clock`: an empty request reads the clock settings; `enabled`, `devices`
    /// (`[{id, clock?, transport?}]`) and `leadMilliseconds` change and store them.
    pub fn clock_command(&self, request: ClockRequest) -> Result<ClockStatus, MidiError> {
        if !request.is_read() {
            let settings = {
                let mut settings = lock(&self.inner.clock_settings);
                let mut next = settings.clone();
                next.apply(&request);
                (self.inner.save_clock_settings)(&next).map_err(|error| MidiError::new(format!("The MIDI clock settings could not be saved: {error}")))?;
                *settings = next.clone();
                next
            };
            self.inner.refresh_clock();
            if settings.enabled {
                // The clock runs with the audio engine, also while the transport is stopped.
                self.inner.wake();
            }
        }
        Ok(self.clock_status())
    }

    /// Command `midi.performance`: sets or clears the chord map applied to incoming notes.
    pub fn performance(&self, request: PerformanceRequest) -> Result<PerformanceStatus, MidiError> {
        Ok(PerformanceStatus { active: lock(&self.inner.router).set_performance_map(&request.map) })
    }

    // ---- for the audio side ---------------------------------------------------------------------

    /// The time base of scheduled output (see [`RenderOutput::begin_block`]).
    pub fn clock(&self) -> MidiClock {
        self.host().clock()
    }

    /// A handle for the audio thread. Create it when the audio stream starts, drop it when it ends.
    pub fn render_output(&self) -> RenderOutput {
        self.host().render_output()
    }

    /// The project changed: its instruments' devices and channels, and its tracks. An external
    /// instrument whose device or channel changed gets pedal-up and all-notes-off on the old one.
    pub fn set_routing(&self, routing: InputRouting) {
        lock(&self.inner.router).set_routing(routing);
        self.inner.refresh_routes();
    }

    /// [`set_routing`](Self::set_routing) from the project document the interface publishes.
    pub fn set_project(&self, project: &Value) {
        self.set_routing(InputRouting::from_project(project));
    }

    /// The audio side's latency facts changed (device, buffer size, line inputs).
    pub fn set_return_compensation(&self, compensation: ReturnCompensation) {
        lock(&self.inner.routes).compensation = compensation;
        self.inner.refresh_clock();
    }

    /// Tells the service whether the audio engine runs (reported by `midi.clock`).
    pub fn set_audio_running(&self, running: bool) {
        lock(&self.inner.routes).audio_running = running;
    }

    /// While blocked (an export runs, the project cannot be played) MIDI input is dropped.
    pub fn set_input_blocked(&self, blocked: bool) {
        lock(&self.inner.router).set_blocked(blocked);
    }

    /// The track and instrument MIDI input plays (the interface's cursor; a note played from the
    /// computer keyboard selects them too).
    pub fn select_destination(&self, track: i64, instrument: i64) {
        lock(&self.inner.router).set_destination(track, instrument);
    }

    /// Silences every external instrument: pedal up, all notes off and all sound off on each
    /// device and channel in use, now. Held input notes and pedals are forgotten (the engine was
    /// silenced by the caller).
    pub fn panic(&self) {
        self.inner.panic_external();
    }

    /// Sends a channel message to an external instrument's device on its channel
    /// (`message.status` is the kind; the channel is the instrument's). Returns false when the
    /// instrument is not external or its device is absent.
    pub fn send_to_instrument(&self, instrument: usize, message: Message) -> bool {
        self.inner.send_to_instrument(instrument, message)
    }

    /// The application's own note started on `track` (computer keyboard, pads): cancels the
    /// release its sustain pedal was holding for that note.
    pub fn app_note_on(&self, track: usize, note: u8) {
        lock(&self.inner.router).sustain().retrigger(APP_SOURCE, 1, track as i64, i64::from(note));
    }

    /// The application's own note was released. Returns true when the application's sustain pedal
    /// holds the sound: the caller must not stop the note.
    pub fn app_note_off(&self, track: usize, note: u8) -> bool {
        lock(&self.inner.router).sustain().defer_note_off(APP_SOURCE, 1, track as i64, i64::from(note))
    }

    /// The application's own sustain pedal (`midi.expression` with controller 64). Pedal-up
    /// returns the notes to stop now.
    pub fn app_sustain(&self, down: bool) -> Vec<Destination> {
        lock(&self.inner.router).sustain().set_pedal(APP_SOURCE, 1, down)
    }

    /// Tracks moved to other engine channels or were removed (see
    /// [`InputRouter::remap_tracks`]).
    pub fn remap_tracks(&self, moves: &[(usize, Option<usize>)]) {
        lock(&self.inner.router).remap_tracks(moves);
    }

    /// The clock settings and outputs, as `midi.clock` returns them.
    pub fn clock_status(&self) -> ClockStatus {
        let settings = lock(&self.inner.clock_settings).clone();
        let (running, automatic) = {
            let routes = lock(&self.inner.routes);
            (routes.audio_running, routes.compensation.automatic_lead_milliseconds)
        };
        ClockStatus {
            enabled: settings.enabled,
            running,
            lead_milliseconds: settings.lead_milliseconds,
            automatic_lead_milliseconds: automatic,
            devices: self
                .host()
                .devices()
                .outputs
                .into_iter()
                .map(|output| {
                    let device = settings.device(output.id);
                    ClockOutput { id: output.id, name: output.name, offline: output.offline, clock: device.clock, transport: device.transport }
                })
                .collect(),
        }
    }

    /// Checks the devices for hot-plug changes now (the watcher thread does this by itself).
    pub fn refresh_devices(&self) -> bool {
        self.host().refresh()
    }

    /// Waits until every input message received so far has been routed and delivered.
    pub fn flush_input(&self) {
        let (done, wait) = mpsc::channel();
        if lock(&self.inner.input).send(InputItem::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(5));
        }
    }

    /// Counters of the output path.
    pub fn output_stats(&self) -> OutputStats {
        self.host().output_stats()
    }

    /// Releases every note input still holds (the input route changed).
    pub fn release_input_notes(&self) {
        let actions = lock(&self.inner.router).release_all();
        self.inner.perform(actions);
    }

    /// Silences external instruments, closes every device and stops the threads. Stop the audio
    /// stream (and drop its [`RenderOutput`]) first.
    pub fn shutdown(&self) {
        self.inner.panic_external();
        let _ = lock(&self.inner.input).send(InputItem::Stop);
        if let Some(thread) = lock(&self.thread).take() {
            let _ = thread.join();
        }
        self.host().shutdown();
    }
}

impl Drop for MidiService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The input side of the configuration: when this text changes, held input notes are released.
fn input_route(devices: &Devices) -> String {
    let sorted = |ids: Vec<String>| ids.into_iter().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(",");
    let requested = devices.requested_input_ids.clone().map(sorted).unwrap_or_else(|| "*".into());
    let connected = sorted(devices.input_ids.iter().map(i32::to_string).collect());
    format!("{}|{requested}|{connected}|{}", devices.enabled, devices.input_channel)
}

fn input_loop(inner: Weak<Inner>, receiver: Receiver<InputItem>) {
    while let Ok(item) = receiver.recv() {
        match item {
            InputItem::Message(message, source) => {
                let Some(inner) = inner.upgrade() else { return };
                let clock_active = inner.host().clock_active();
                let actions = lock(&inner.router).handle(message, source, clock_active);
                inner.perform(actions);
            }
            InputItem::Flush(done) => {
                let _ = done.send(());
            }
            InputItem::Stop => return,
        }
    }
}

fn note_event(on: bool, note: &RoutedNote) -> Map<String, Value> {
    let Value::Object(event) = json!({
        "on": on, "note": note.note, "velocity": note.velocity, "channel": note.channel, "source": note.source,
        "track": note.track, "instrument": note.instrument,
    }) else {
        unreachable!("an object literal")
    };
    event
}

impl Inner {
    fn host(&self) -> &MidiHost {
        self.host.get().expect("the host is set before the service is used")
    }

    fn target(&self) -> Option<Arc<dyn InputTarget>> {
        self.target.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn wake(&self) {
        if let Some(target) = self.target() {
            target.wake();
        }
    }

    fn emit(&self, name: &str, payload: Value) {
        (self.events)(name, payload);
    }

    fn perform(&self, actions: Vec<InputAction>) {
        if actions.is_empty() {
            return;
        }
        let target = self.target();
        for action in actions {
            match action {
                InputAction::NoteOn(note) => {
                    let handling = target.as_ref().map_or(NoteHandling::Played(None), |target| target.note_on(&note));
                    if let NoteHandling::Played(position) = handling {
                        let mut event = note_event(true, &note);
                        if let Some(position) = position {
                            event.insert("pattern".into(), json!(position.pattern));
                            event.insert("row".into(), json!(position.row));
                            event.insert("position".into(), json!(position.position));
                        }
                        self.emit("midi.note", Value::Object(event));
                    }
                }
                InputAction::NoteOff { note, sound } => {
                    let handling = target.as_ref().map_or(NoteHandling::Played(None), |target| target.note_off(&note, sound));
                    if matches!(handling, NoteHandling::Played(_)) {
                        self.emit("midi.note", Value::Object(note_event(false, &note)));
                    }
                }
                InputAction::Release(destination) => {
                    if let Some(target) = &target {
                        target.release(destination.track, destination.note);
                    }
                }
                InputAction::Forward { track, instrument, message } => {
                    if !self.send_to_instrument(instrument, message) && !self.host().route(instrument).external {
                        if let Some(target) = &target {
                            target.message(track, message);
                        }
                    }
                }
                InputAction::PitchBend { track, semitones } => {
                    if let Some(target) = &target {
                        target.pitch_bend(track, semitones);
                    }
                }
                InputAction::Control { message, source } => {
                    self.emit("midi.message", json!({"status": message.status, "data1": message.data1, "data2": message.data2, "source": source}));
                }
                InputAction::Panic => {
                    if let Some(target) = &target {
                        target.panic();
                    }
                    self.panic_external();
                }
                InputAction::Start => {
                    if let Some(target) = &target {
                        target.transport_start();
                    }
                }
                InputAction::Stop => {
                    if let Some(target) = &target {
                        target.transport_stop();
                    }
                    self.panic_external();
                }
                InputAction::Frozen { track } => {
                    if let Some(target) = &target {
                        target.frozen(track);
                    }
                }
            }
        }
    }

    fn send_to_instrument(&self, instrument: usize, message: Message) -> bool {
        let resolved = lock(&self.routes).resolved.get(instrument).copied().unwrap_or_default();
        match (resolved.external, resolved.output) {
            (true, Some(route)) => {
                self.host().send_to(route, Message::new((message.status & 0xF0) | resolved.channel, message.data1, message.data2), 0);
                true
            }
            _ => false,
        }
    }

    fn panic_external(&self) {
        let resolved = lock(&self.routes).resolved.clone();
        let mut done = BTreeSet::new();
        for route in resolved.iter().filter(|route| route.external) {
            if !done.insert((route.destination, route.channel)) {
                continue;
            }
            if let Some(output) = route.output {
                for controller in [64, 123, 120] {
                    self.host().send_to(output, Message::new(0xB0 | route.channel, controller, 0), 0);
                }
            }
        }
        lock(&self.router).clear();
    }

    fn host_event(&self, event: HostEvent) {
        match event {
            HostEvent::Configuration(devices) => {
                self.refresh_routes();
                self.refresh_clock();
                self.emit("midi.status", json!({"type": "midiConfiguration", "devices": devices}));
            }
            HostEvent::DevicesChanged(devices) => {
                self.refresh_routes();
                self.refresh_clock();
                self.emit("midi.status", json!({"type": "midiDevicesChanged", "devices": devices}));
            }
            HostEvent::Error(message) => self.emit("midi.status", json!({"type": "midiError", "message": message})),
        }
    }

    /// Resolves every instrument's route against the devices present and publishes it to the
    /// audio thread.
    fn refresh_routes(&self) {
        let Some(host) = self.host.get() else { return };
        let routing = lock(&self.router).routing().clone();
        let lead = self.note_lead();
        let mut routes = lock(&self.routes);
        let returned: BTreeSet<usize> = routes.compensation.instruments.iter().copied().collect();
        let present: BTreeSet<i32> = host.devices().outputs.iter().map(|output| output.id).collect();
        let clock = host.clock();
        for index in 0..ROUTE_COUNT {
            let next = match routing.instruments.get(index) {
                Some(item) => Resolved {
                    external: item.external,
                    destination: item.output,
                    channel: item.channel,
                    output: if item.external { host.prepare_destination(item.output) } else { None },
                },
                None => Resolved::default(),
            };
            let old = std::mem::replace(&mut routes.resolved[index], next);
            if old.external {
                let clear = |output: OutputRoute, channel: u8, time: u64| {
                    host.send_to(output, Message::new(0xB0 | channel, 64, 0), time);
                    host.send_to(output, Message::new(0xB0 | channel, 123, 0), time);
                };
                if let (None, Some(resumed), true) = (old.output, next.output, next.external && old.destination == next.destination && old.channel == next.channel) {
                    // A returning interface can still drive a synth whose last release was lost
                    // while it was unplugged: clear that channel before new notes.
                    clear(resumed, next.channel, 0);
                }
                if old.channel != next.channel || old.output.map(|route| route.id) != next.output.map(|route| route.id) || !next.external {
                    // A device that is gone cannot be cleared; it is cleared when it returns.
                    if let Some(output) = old.output.filter(|output| present.contains(&output.id)) {
                        clear(output, old.channel, 0);
                        // Once more shortly after: notes already scheduled for the old route may
                        // still be on their way.
                        host.send_to(output, Message::new(0xB0 | old.channel, 123, 0), clock.after(Duration::from_millis(100)));
                    }
                }
            }
            host.set_route(
                index,
                Route {
                    external: next.external,
                    output: next.output.map(|route| route.slot),
                    channel: next.channel,
                    lead: if next.external && returned.contains(&index) { lead } else { 0 },
                },
            );
        }
    }

    fn lead_seconds(&self) -> f64 {
        let manual = lock(&self.clock_settings).lead_milliseconds;
        let automatic = lock(&self.routes).compensation.automatic_lead_milliseconds;
        (manual.unwrap_or(automatic) / 1000.0).clamp(0.0, crate::clock::MAXIMUM_LEAD_SECONDS)
    }

    /// Nanoseconds notes to returned instruments are sent early.
    fn note_lead(&self) -> u32 {
        if lock(&self.routes).compensation.compensated {
            (self.lead_seconds() * MidiClock::TICKS_PER_SECOND) as u32
        } else {
            0
        }
    }

    /// Rebuilds the clock outputs from the stored choice and the outputs present now.
    fn refresh_clock(&self) {
        let Some(host) = self.host.get() else { return };
        let settings = lock(&self.clock_settings).clone();
        let mut targets = Vec::new();
        if settings.enabled {
            for output in host.devices().outputs.iter().filter(|output| !output.offline) {
                let device = settings.device(output.id);
                if device.clock {
                    if let Some(route) = host.prepare_destination(Some(output.id)) {
                        targets.push((route, device.transport));
                    }
                }
            }
        }
        host.set_clock_targets(&targets, self.lead_seconds());
        // Notes to a returned instrument carry the same lead as the clock.
        self.refresh_routes();
    }
}

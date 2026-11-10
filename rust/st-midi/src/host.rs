//! The MIDI host: which devices exist, which inputs are connected, where output goes.
//!
//! Device ids are persistent ([`PortInfo::id`]), never an index into a list: a selection that was
//! saved keeps meaning the same device after it was unplugged and plugged in again, and stays
//! requested while the device is away.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::held::HeldInputs;
use crate::message::{Message, Parser};
use crate::output::{MidiClock, OutputEngine, OutputStats, RenderOutput, Route, MAX_OUTPUTS};
use crate::ports::{Backend, InputPort, OpenError, PortInfo};

/// An error of the MIDI host, in words for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiError(pub(crate) String);

impl MidiError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        MidiError(message.into())
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MidiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MidiError {}

/// A device named in a request: a device id (a number, or digits in a string) or a device name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier(pub String);

impl<'de> Deserialize<'de> for Identifier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Integer(i64),
            Number(f64),
            Text(String),
        }
        Ok(Identifier(match Raw::deserialize(deserializer)? {
            Raw::Integer(number) => number.to_string(),
            Raw::Number(number) if number.fract() == 0.0 && number.abs() < 1e15 => (number as i64).to_string(),
            Raw::Number(number) => number.to_string(),
            Raw::Text(text) => text,
        }))
    }
}

impl Identifier {
    fn is_none(&self) -> bool {
        ["", "none", "off", "null", "<null>"].contains(&self.0.to_lowercase().as_str())
    }
}

/// A field that distinguishes "absent" (`None`) from an explicit JSON `null` (`Some(None)`).
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// An integer sent as a number or as digits in a string.
fn lenient_integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Integer(i64),
        Number(f64),
        Text(String),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Option::<Raw>::deserialize(deserializer)? {
        Some(Raw::Integer(number)) => Some(number),
        Some(Raw::Number(number)) if number.is_finite() => Some(number as i64),
        Some(Raw::Text(text)) => text.trim().parse().ok(),
        _ => None,
    })
}

/// Payload of `midi.configure`. Fields left out keep their value.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConfigureRequest {
    /// Receive MIDI input at all. Output does not depend on it.
    pub enabled: Option<bool>,
    /// Connect exactly these inputs, by device id or name (an empty list: none).
    #[serde(rename = "inputIDs")]
    pub input_ids: Option<Vec<Identifier>>,
    /// One input: `"all"`, `"none"` (or `null`), a device id or a device name.
    #[serde(rename = "inputID", deserialize_with = "nullable")]
    pub input_id: Option<Option<Identifier>>,
    /// The default output: `"none"` (or `null`), a device id or a device name.
    #[serde(rename = "outputID", deserialize_with = "nullable")]
    pub output_id: Option<Option<Identifier>>,
    /// The default output channel, 1 to 16.
    #[serde(deserialize_with = "lenient_integer")]
    pub channel: Option<i64>,
    /// Receive only this channel (1 to 16), or all channels (0).
    #[serde(deserialize_with = "lenient_integer")]
    pub input_channel: Option<i64>,
    /// The track input plays ([`crate::MidiService`] uses it; the device host ignores it).
    #[serde(deserialize_with = "lenient_integer")]
    pub track: Option<i64>,
    /// The instrument input plays ([`crate::MidiService`] uses it; the device host ignores it).
    #[serde(deserialize_with = "lenient_integer")]
    pub instrument: Option<i64>,
}

/// An input device in a [`Devices`] listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputDevice {
    /// Device id.
    pub id: i32,
    /// Name to show.
    pub name: String,
    /// Manufacturer, if known.
    pub manufacturer: String,
    /// Listed but not reachable.
    pub offline: bool,
    /// The host receives from it.
    pub connected: bool,
}

/// An output device in a [`Devices`] listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputDevice {
    /// Device id.
    pub id: i32,
    /// Name to show.
    pub name: String,
    /// Manufacturer, if known.
    pub manufacturer: String,
    /// Listed but not reachable.
    pub offline: bool,
    /// It is the default output.
    pub selected: bool,
}

/// The devices and the current configuration: the result of `midi.devices` and `midi.configure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Devices {
    /// The inputs present.
    pub inputs: Vec<InputDevice>,
    /// The outputs present.
    pub outputs: Vec<OutputDevice>,
    /// MIDI input is on.
    pub enabled: bool,
    /// Ids of the connected inputs.
    #[serde(rename = "inputIDs")]
    pub input_ids: Vec<i32>,
    /// Every input is wanted (no explicit selection).
    pub all_inputs: bool,
    /// Id of the default output, when it is present.
    #[serde(rename = "outputID")]
    pub output_id: Option<i32>,
    /// The inputs that were asked for, present or not; `null` means all.
    #[serde(rename = "requestedInputIDs")]
    pub requested_input_ids: Option<Vec<String>>,
    /// The default output that was asked for, present or not.
    #[serde(rename = "requestedOutputID")]
    pub requested_output_id: Option<String>,
    /// Default output channel, 1 to 16.
    pub channel: u8,
    /// Input channel filter: 0 for all, or 1 to 16.
    pub input_channel: u8,
    /// Why no device can be used (the system has no MIDI service).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What the host reports besides messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// The configuration changed through [`MidiHost::configure`].
    Configuration(Devices),
    /// A device was plugged in or unplugged.
    DevicesChanged(Devices),
    /// Something failed in the background (an output refused a message, enumeration failed).
    Error(String),
}

/// Receives every input message with the id of the device it came from. Called on backend threads
/// (and on the configuring or hot-plug thread for releases sent in an unplugged device's name).
pub type MessageCallback = Arc<dyn Fn(Message, i32) + Send + Sync>;
/// Receives [`HostEvent`]s, on the thread that caused them.
pub type HostEventCallback = Arc<dyn Fn(HostEvent) + Send + Sync>;

/// An output the audio thread and control threads can send to: resolved once, on a control thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputRoute {
    /// The output's slot in the sender thread's table.
    pub slot: u8,
    /// The device id.
    pub id: i32,
}

struct InputState {
    enabled: bool,
    stopped: bool,
    channel: u8,
    parsers: HashMap<i32, Parser>,
    held: HeldInputs,
}

struct Slot {
    id: i32,
    open: bool,
}

struct Control {
    backend: Option<Box<dyn Backend>>,
    backend_error: Option<String>,
    stopped: bool,
    enabled: bool,
    /// `None` connects every input, an empty list none.
    requested_inputs: Option<Vec<String>>,
    requested_output: Option<String>,
    output_channel: u8,
    input_channel: u8,
    connected: Vec<(PortInfo, Box<dyn InputPort>)>,
    sources: Vec<PortInfo>,
    destinations: Vec<PortInfo>,
    output_id: Option<i32>,
    slots: Vec<Slot>,
    retry_pending: bool,
    last_listing: Option<(Vec<PortInfo>, Vec<PortInfo>)>,
}

struct Inner {
    control: Mutex<Control>,
    input: Arc<Mutex<InputState>>,
    on_message: MessageCallback,
    on_event: HostEventCallback,
    engine: OutputEngine,
    wake: (Mutex<bool>, Condvar),
}

/// MIDI devices: enumeration with hot-plug, input connections, and output.
pub struct MidiHost {
    inner: Arc<Inner>,
    watcher: Mutex<Option<JoinHandle<()>>>,
}

/// How a [`MidiHost`] starts.
pub struct HostOptions {
    /// The device layer; `Err` (no MIDI service) gives a host without devices that says why.
    pub backend: Result<Box<dyn Backend>, String>,
    /// Inputs to connect at once, by id; `None` connects all. Naming them avoids subscribing to
    /// unrelated devices before a saved configuration is applied.
    pub input_ids: Option<Vec<i32>>,
    /// How often the device lists are compared for hot-plug; `None` never (tests call
    /// [`MidiHost::refresh`] themselves).
    pub poll_interval: Option<Duration>,
    /// Receives input messages.
    pub on_message: MessageCallback,
    /// Receives events.
    pub on_event: HostEventCallback,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn resolve<'a>(identifier: &str, ports: &'a [PortInfo]) -> Option<&'a PortInfo> {
    if let Ok(number) = identifier.trim().parse::<i64>() {
        // Saved routes are device ids, never names that happen to be digits.
        return ports.iter().find(|port| i64::from(port.id) == number);
    }
    ports.iter().find(|port| port.name.to_lowercase() == identifier.to_lowercase())
}

impl MidiHost {
    /// Starts the host and connects the requested inputs. Fails when an input that is present
    /// cannot be connected; a missing MIDI service is not a failure (see [`Devices::error`]).
    pub fn new(options: HostOptions) -> Result<Self, MidiError> {
        let clock = MidiClock::new();
        let on_event = options.on_event.clone();
        let engine = OutputEngine::start(clock, Arc::new(move |_slot, error| on_event(HostEvent::Error(format!("MIDI output failed ({error}).")))))
            .map_err(MidiError::new)?;
        let (backend, backend_error) = match options.backend {
            Ok(backend) => (Some(backend), None),
            Err(error) => (None, Some(error)),
        };
        let inner = Arc::new(Inner {
            control: Mutex::new(Control {
                backend,
                backend_error,
                stopped: false,
                enabled: true,
                requested_inputs: options.input_ids.map(|ids| ids.iter().map(i32::to_string).collect()),
                requested_output: None,
                output_channel: 1,
                input_channel: 0,
                connected: Vec::new(),
                sources: Vec::new(),
                destinations: Vec::new(),
                output_id: None,
                slots: Vec::new(),
                retry_pending: false,
                last_listing: None,
            }),
            input: Arc::new(Mutex::new(InputState { enabled: true, stopped: false, channel: 0, parsers: HashMap::new(), held: HeldInputs::new() })),
            on_message: options.on_message,
            on_event: options.on_event,
            engine,
            wake: (Mutex::new(false), Condvar::new()),
        });
        {
            let mut control = lock(&inner.control);
            inner.reconnect(&mut control, true)?;
        }
        let watcher = match options.poll_interval {
            Some(interval) => {
                let weak = Arc::downgrade(&inner);
                Some(thread::Builder::new().name("midi-devices".into()).spawn(move || watch(weak, interval)).map_err(|error| MidiError::new(format!("Could not start the MIDI device watcher: {error}.")))?)
            }
            None => None,
        };
        Ok(MidiHost { inner, watcher: Mutex::new(watcher) })
    }

    /// The devices and the configuration, from the last enumeration.
    pub fn devices(&self) -> Devices {
        snapshot(&lock(&self.inner.control))
    }

    /// The time base of scheduled output.
    pub fn clock(&self) -> MidiClock {
        self.inner.engine.clock()
    }

    /// Applies a request. Every device it names is resolved before anything changes, so a request
    /// that fails leaves the working configuration alone. Emits [`HostEvent::Configuration`].
    pub fn configure(&self, request: &ConfigureRequest) -> Result<Devices, MidiError> {
        let (devices, releases) = {
            let mut control = lock(&self.inner.control);
            if control.stopped {
                return Err(MidiError::new("The MIDI host is shut down."));
            }
            let mut inputs = control.requested_inputs.clone();
            let mut output = control.requested_output.clone();
            let enabled = request.enabled.unwrap_or(control.enabled);
            let (mut output_channel, mut input_channel) = (control.output_channel, control.input_channel);
            if let Some(list) = &request.input_ids {
                inputs = Some(list.iter().map(|identifier| identifier.0.clone()).collect());
            } else if let Some(single) = &request.input_id {
                inputs = match single {
                    Some(identifier) if identifier.0.to_lowercase() == "all" => None,
                    Some(identifier) if !identifier.is_none() => Some(vec![identifier.0.clone()]),
                    _ => Some(Vec::new()),
                };
            }
            if let Some(single) = &request.output_id {
                output = single.as_ref().filter(|identifier| !identifier.is_none()).map(|identifier| identifier.0.clone());
            }
            if let Some(channel) = request.channel {
                if !(1..=16).contains(&channel) {
                    return Err(MidiError::new("MIDI output channel must be between 1 and 16."));
                }
                output_channel = channel as u8;
            }
            if let Some(channel) = request.input_channel {
                if !(0..=16).contains(&channel) {
                    return Err(MidiError::new("MIDI input channel must be 0 (all), or between 1 and 16."));
                }
                input_channel = channel as u8;
            }
            let (sources, destinations) = self.inner.enumerate(&mut control)?;
            if inputs != control.requested_inputs {
                for identifier in inputs.iter().flatten() {
                    if resolve(identifier, &sources).is_none() {
                        return Err(MidiError::new(format!("MIDI input '{identifier}' is not connected.")));
                    }
                }
            }
            if output != control.requested_output {
                if let Some(identifier) = &output {
                    if resolve(identifier, &destinations).is_none() {
                        return Err(MidiError::new(format!("MIDI output '{identifier}' is not connected.")));
                    }
                }
            }
            // The receiver releases every input note itself when the effective input route changes;
            // forget them then, so that a later unplug cannot repeat them. An unchanged route keeps
            // them for the releases an unplugged device owes.
            let as_set = |list: &Option<Vec<String>>| list.as_ref().map(|list| list.iter().cloned().collect::<std::collections::BTreeSet<_>>());
            let route_changed = as_set(&inputs) != as_set(&control.requested_inputs) || enabled != control.enabled || input_channel != control.input_channel;
            control.requested_inputs = inputs;
            control.requested_output = output;
            control.output_channel = output_channel;
            control.enabled = enabled;
            control.input_channel = input_channel;
            {
                let mut input = lock(&self.inner.input);
                input.enabled = enabled;
                input.channel = input_channel;
                input.parsers.clear();
                if route_changed {
                    input.held.clear();
                }
            }
            // A source that vanished before its hot-plug check still owes its releases.
            let removed = self.inner.reconnect(&mut control, true)?;
            let releases: Vec<(i32, Vec<Message>)> = removed.into_iter().map(|source| (source, self.inner.take_releases(source))).collect();
            (snapshot(&control), releases)
        };
        for (source, messages) in releases {
            for message in messages {
                (self.inner.on_message)(message, source);
            }
        }
        (self.inner.on_event)(HostEvent::Configuration(devices.clone()));
        Ok(devices)
    }

    /// Compares the device lists with the last ones and follows what changed: connects wanted
    /// inputs that appeared, delivers the releases of inputs that disappeared, closes outputs that
    /// are gone, and emits [`HostEvent::DevicesChanged`]. The watcher thread calls this; a test
    /// without one calls it itself. Returns whether anything changed.
    pub fn refresh(&self) -> bool {
        self.inner.refresh()
    }

    /// Resolves an output on a control thread: the device with this id, or the default output for
    /// `None`. The device is opened if it was not; `None` when it is absent or cannot be opened.
    /// Whether MIDI input is enabled does not matter.
    pub fn prepare_destination(&self, destination: Option<i32>) -> Option<OutputRoute> {
        let mut control = lock(&self.inner.control);
        self.inner.prepare(&mut control, destination)
    }

    /// Sends a message now from a control thread, to a device or to the default output. The
    /// channel is part of the status byte. Returns whether the message was handed to a device
    /// that is present (a refusal by the device arrives later as [`HostEvent::Error`]).
    pub fn send(&self, message: Message, destination: Option<i32>) -> bool {
        self.send_at(message, destination, 0)
    }

    /// Like [`send`](Self::send), at device time `time` (0: now).
    pub fn send_at(&self, message: Message, destination: Option<i32>, time: u64) -> bool {
        if !message.is_sendable() {
            return false;
        }
        match self.prepare_destination(destination) {
            Some(route) => {
                self.inner.engine.send(route.slot, message, time);
                true
            }
            None => false,
        }
    }

    /// Sends to a prepared route, at device time `time` (0: now).
    pub fn send_to(&self, route: OutputRoute, message: Message, time: u64) {
        if message.is_sendable() {
            self.inner.engine.send(route.slot, message, time);
        }
    }

    /// A handle for the audio thread.
    pub fn render_output(&self) -> RenderOutput {
        self.inner.engine.render_output()
    }

    /// Publishes an instrument's route for the audio thread.
    pub fn set_route(&self, instrument: usize, route: Route) {
        self.inner.engine.set_route(instrument, route);
    }

    /// The route published for an instrument.
    pub fn route(&self, instrument: usize) -> Route {
        self.inner.engine.route(instrument)
    }

    /// Publishes the clock outputs (`(route, transport)`) and the lead in seconds.
    pub fn set_clock_targets(&self, targets: &[(OutputRoute, bool)], lead_seconds: f64) {
        let targets: Vec<(u8, bool)> = targets.iter().map(|(route, transport)| (route.slot, *transport)).collect();
        self.inner.engine.set_clock_targets(&targets, lead_seconds);
    }

    /// True while any output gets clock.
    pub fn clock_active(&self) -> bool {
        self.inner.engine.clock_active()
    }

    /// Counters of the output path.
    pub fn output_stats(&self) -> OutputStats {
        self.inner.engine.stats()
    }

    /// Closes every connection and stops the threads. Stop rendering first.
    pub fn shutdown(&self) {
        {
            let mut control = lock(&self.inner.control);
            if control.stopped {
                return;
            }
            control.stopped = true;
            {
                let mut input = lock(&self.inner.input);
                input.stopped = true;
                input.enabled = false;
                input.parsers.clear();
                input.held.clear();
            }
            control.connected.clear();
            for slot in &mut control.slots {
                slot.open = false;
            }
            control.backend = None;
        }
        *lock(&self.inner.wake.0) = true;
        self.inner.wake.1.notify_all();
        if let Some(watcher) = lock(&self.watcher).take() {
            let _ = watcher.join();
        }
        self.inner.engine.shutdown();
    }
}

impl Drop for MidiHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn watch(inner: Weak<Inner>, interval: Duration) {
    loop {
        let Some(strong) = inner.upgrade() else { return };
        {
            let stopped = lock(&strong.wake.0);
            let (stopped, _) = strong.wake.1.wait_timeout(stopped, interval).unwrap_or_else(|poisoned| poisoned.into_inner());
            if *stopped {
                return;
            }
        }
        strong.refresh();
    }
}

fn snapshot(control: &Control) -> Devices {
    let inputs: Vec<InputDevice> = control
        .sources
        .iter()
        .map(|port| InputDevice {
            id: port.id,
            name: port.name.clone(),
            manufacturer: port.manufacturer.clone(),
            offline: port.offline,
            connected: control.connected.iter().any(|(connected, _)| connected.id == port.id),
        })
        .collect();
    let outputs: Vec<OutputDevice> = control
        .destinations
        .iter()
        .map(|port| OutputDevice {
            id: port.id,
            name: port.name.clone(),
            manufacturer: port.manufacturer.clone(),
            offline: port.offline,
            selected: control.output_id == Some(port.id),
        })
        .collect();
    Devices {
        inputs,
        outputs,
        enabled: control.enabled,
        input_ids: control.connected.iter().map(|(port, _)| port.id).collect(),
        all_inputs: control.requested_inputs.is_none(),
        output_id: control.output_id,
        requested_input_ids: control.requested_inputs.clone(),
        requested_output_id: control.requested_output.clone(),
        channel: control.output_channel,
        input_channel: control.input_channel,
        error: control.backend_error.clone(),
    }
}

impl Inner {
    fn enumerate(&self, control: &mut Control) -> Result<(Vec<PortInfo>, Vec<PortInfo>), MidiError> {
        let Some(backend) = control.backend.as_mut() else {
            return Ok((Vec::new(), Vec::new()));
        };
        let failed = |error: String| MidiError::new(format!("Listing MIDI devices failed: {error}"));
        Ok((backend.inputs().map_err(failed)?, backend.outputs().map_err(failed)?))
    }

    fn receiver(&self, source: i32) -> crate::ports::InputCallback {
        let (state, deliver) = (self.input.clone(), self.on_message.clone());
        Box::new(move |bytes: &[u8]| {
            let mut messages = Vec::new();
            {
                let mut input = lock(&state);
                if !input.enabled || input.stopped {
                    return;
                }
                let channel = input.channel;
                let mut parser = input.parsers.remove(&source).unwrap_or_default();
                parser.feed(bytes, |message| {
                    if message.status >= 0xF0 || channel == 0 || message.channel() == Some(channel) {
                        input.held.track(source, &message);
                        messages.push(message);
                    }
                });
                input.parsers.insert(source, parser);
            }
            // Delivered outside the lock: the receiver may take its time.
            for message in messages {
                deliver(message, source);
            }
        })
    }

    /// Releases a removed source still owes: key note-offs, then pedal-up.
    fn take_releases(&self, source: i32) -> Vec<Message> {
        let mut input = lock(&self.input);
        input.parsers.remove(&source);
        input.held.take_releases(source)
    }

    /// Brings the connections in line with the request and the devices present. Returns the ids
    /// of inputs that were disconnected.
    fn reconnect(&self, control: &mut Control, retry_vanished: bool) -> Result<Vec<i32>, MidiError> {
        let (sources, destinations) = self.enumerate(control)?;
        let wanted: Vec<PortInfo> = if !control.enabled {
            Vec::new()
        } else {
            match &control.requested_inputs {
                Some(identifiers) => {
                    let mut wanted: Vec<PortInfo> = Vec::new();
                    for port in identifiers.iter().filter_map(|identifier| resolve(identifier, &sources)) {
                        if !wanted.iter().any(|other| other.id == port.id) {
                            wanted.push(port.clone());
                        }
                    }
                    wanted
                }
                None => sources.clone(),
            }
        };
        let mut removed = Vec::new();
        control.connected.retain(|(port, _)| {
            let keep = wanted.iter().any(|other| other.id == port.id);
            if !keep {
                removed.push(port.id);
            }
            keep
        });
        for port in &wanted {
            if control.connected.iter().any(|(connected, _)| connected.id == port.id) {
                continue;
            }
            let receiver = self.receiver(port.id);
            let Some(backend) = control.backend.as_mut() else { break };
            match backend.open_input(port.id, receiver) {
                Ok(connection) => control.connected.push((port.clone(), connection)),
                Err(OpenError::Gone) => {
                    // Listing and opening are separate steps; a device may vanish between them.
                    // Its saved selection stays, and one more look follows instead of a failure.
                    if retry_vanished {
                        control.retry_pending = true;
                    }
                }
                Err(OpenError::Failed(error)) => {
                    control.sources = sources;
                    control.destinations = destinations;
                    return Err(MidiError::new(format!("Connecting MIDI input '{}' failed: {error}", port.name)));
                }
            }
        }
        // A device that vanished during the opening attempts is not listed as present.
        let (sources, destinations) = if control.retry_pending && retry_vanished { self.enumerate(control).unwrap_or((sources, destinations)) } else { (sources, destinations) };
        control.output_id = control.requested_output.as_ref().and_then(|identifier| resolve(identifier, &destinations)).map(|port| port.id);
        for slot in 0..control.slots.len() {
            if control.slots[slot].open && !destinations.iter().any(|port| port.id == control.slots[slot].id) {
                control.slots[slot].open = false;
                self.engine.close(slot as u8);
            }
        }
        control.last_listing = Some((sources.clone(), destinations.clone()));
        control.sources = sources;
        control.destinations = destinations;
        Ok(removed)
    }

    fn refresh(&self) -> bool {
        let outcome = {
            let mut control = lock(&self.control);
            if control.stopped || control.backend.is_none() {
                return false;
            }
            let listing = match self.enumerate(&mut control) {
                Ok(listing) => listing,
                Err(error) => {
                    drop(control);
                    (self.on_event)(HostEvent::Error(error.0));
                    return false;
                }
            };
            let retry = std::mem::take(&mut control.retry_pending);
            if !retry && control.last_listing.as_ref() == Some(&listing) {
                return false;
            }
            match self.reconnect(&mut control, false) {
                Ok(removed) => {
                    let releases: Vec<(i32, Vec<Message>)> = removed.into_iter().map(|source| (source, self.take_releases(source))).collect();
                    Ok((snapshot(&control), releases))
                }
                Err(error) => Err(error),
            }
        };
        match outcome {
            Ok((devices, releases)) => {
                // A device that disappeared while a key or the pedal was down never sends its
                // release; deliver one so that nothing keeps sounding.
                for (source, messages) in releases {
                    for message in messages {
                        (self.on_message)(message, source);
                    }
                }
                (self.on_event)(HostEvent::DevicesChanged(devices));
                true
            }
            Err(error) => {
                (self.on_event)(HostEvent::Error(error.0));
                false
            }
        }
    }

    fn prepare(&self, control: &mut Control, destination: Option<i32>) -> Option<OutputRoute> {
        if control.stopped {
            return None;
        }
        let id = match destination {
            Some(id) => control.destinations.iter().find(|port| port.id == id)?.id,
            None => control.output_id?,
        };
        let slot = match control.slots.iter().position(|slot| slot.id == id) {
            Some(slot) => slot,
            None if control.slots.len() < MAX_OUTPUTS => {
                control.slots.push(Slot { id, open: false });
                control.slots.len() - 1
            }
            None => {
                // Every slot belongs to some device: take one whose device is closed.
                let slot = control.slots.iter().position(|slot| !slot.open)?;
                control.slots[slot].id = id;
                slot
            }
        };
        if !control.slots[slot].open {
            match control.backend.as_mut()?.open_output(id) {
                Ok(port) => {
                    self.engine.open(slot as u8, port);
                    control.slots[slot].open = true;
                }
                Err(_) => return None,
            }
        }
        Some(OutputRoute { slot: slot as u8, id })
    }
}

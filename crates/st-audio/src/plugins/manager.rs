//! The plug-in manager: the supervisor of the container processes and one hosted instance per
//! plug-in of the project.
//!
//! # Life cycle of an instance
//!
//! [`PluginManager::reconcile`] runs on the control thread that publishes a project, before the
//! mixer is compiled. It reads the project's mixer ([`GraphSpec`]) and makes the set of hosted
//! instances match it: one instance per plug-in insert, **keyed by the insert's key** (the
//! descriptor's `instanceID`; `<instanceID>-on-<track>` for each track that plays an
//! instrument whose chain holds the insert; the instrument plug-in's own `instanceID`). An
//! instance whose key and plug-in are unchanged is kept, with everything it holds, across
//! publications and plan swaps; its state is loaded again only when the descriptor carries
//! another state than the one last loaded into or read from it (or when the publication
//! replaces the project). A new key creates an instance in a container, loads the descriptor's
//! state and activates it. A key that is gone moves its instance to the graveyard.
//!
//! The render plan reaches an instance through an [`Arc<Hosted>`] inside its processor
//! ([`super::insert::PluginInsert`]), so an instance outlives every plan that uses it. The
//! manager's own thread destroys a buried instance once nothing else refers to it: the
//! blocking destroy request never runs on the audio thread, nor under the engine's lock.
//!
//! # Threads
//!
//! | Thread | Does |
//! |---|---|
//! | control threads (bridge workers) | `reconcile`, and every [`Hosted`] request: parameters, state, presets, editor |
//! | audio thread | [`Hosted::instance`]`.process_raw` only, through the processor |
//! | the supervisor's threads | push [`HostEvent`]s into a channel, nothing else |
//! | the manager's thread | handles those events (losses, latency changes, restart requests, edits made in editors), coalesces parameter changes, buries instances |
//!
//! # What an event leads to
//!
//! * **Latency changed / restart requested.** A restart request is answered with
//!   `deactivate` + `activate` (the instance plays its dry fallback meanwhile). A new latency
//!   is announced as [`PluginEvent::Graph`]: the engine forgets the insert's processor and
//!   compiles the mixer again, so delay compensation follows.
//! * **Instance lost** (its container crashed, or hung and was killed). The audio thread
//!   already plays the fallback (the dry signal, delayed by the plug-in's latency; silence
//!   for an instrument). The loss is recorded in the [`RecoveryStore`] and announced as
//!   [`PluginEvent::Lost`]; `routing.status` shows the insert as unavailable.
//!   [`PluginManager::restore`] restarts the container and gives the instance its last state
//!   back (the client keeps it, with the parameter values set since).

use super::catalog::Catalog;
use super::ident::{parse_component, Identity};
use super::recovery::RecoveryStore;
use crate::graph::insert::{base64_decode, base64_encode};
use crate::graph::spec::{GraphSpec, InsertSpec};
use st_project::model::{PluginDescriptor, PluginKind, Project};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use stpc_client::{
    Error as ClientError, ExitReason, Fallback, GroupingPolicy, HostEvent, Instance, InstanceConfig, InstanceId, InstanceStatus, ParameterInfo, PluginKind as ClientKind, Supervisor,
    SupervisorConfig, INFO_HAS_EDITOR, INFO_SUSTAIN_PEDAL, PARAM_AUTOMATABLE, PARAM_BOOLEAN, PARAM_HIDDEN, PARAM_READ_ONLY, PARAM_STEPPED,
};
use tracker_engine::engine::util::Ring;

/// How the manager is set up.
#[derive(Debug, Clone)]
pub struct PluginOptions {
    /// The supervisor's configuration: container executable (default: next to the host's
    /// own, or `$STPC_CONTAINER_PATH`), grouping policy (default: one container per plug-in),
    /// socket directory, time limits.
    pub supervisor: SupervisorConfig,
    /// Where crashes at load are remembered; `None` forgets them with the process.
    pub recovery_path: Option<PathBuf>,
    /// Instances render offline: no deadline, so an export never drops a plug-in.
    pub offline: bool,
    /// The shortest wait a real-time block gets, however little of its render call is left.
    pub min_deadline: Duration,
}

impl Default for PluginOptions {
    fn default() -> PluginOptions {
        PluginOptions {
            supervisor: SupervisorConfig { socket_dir: default_socket_dir(), ..SupervisorConfig::default() },
            recovery_path: None,
            offline: false,
            min_deadline: Duration::from_micros(500),
        }
    }
}

/// A short directory for the supervisor's sockets: socket paths are limited to about a
/// hundred bytes, and the temporary directory of macOS is long. `None`: the client's choice
/// (`$XDG_RUNTIME_DIR`, else the temporary directory).
pub fn default_socket_dir() -> Option<PathBuf> {
    cfg!(target_os = "macos").then(|| PathBuf::from("/tmp"))
}

/// The grouping policy by its settings name: `shared`, `manufacturer`, `plugin` (the default)
/// or `instance`.
pub fn grouping_policy(name: &str) -> GroupingPolicy {
    match name {
        "shared" => GroupingPolicy::Shared,
        "manufacturer" => GroupingPolicy::PerManufacturer,
        "instance" => GroupingPolicy::PerInstance,
        _ => GroupingPolicy::PerPlugin,
    }
}

/// What the manager tells its listeners, from its own thread or from the control thread
/// that caused it. `key` is the instance's key ([`InsertSpec::key`]).
#[derive(Debug, Clone, PartialEq)]
pub enum PluginEvent {
    /// A parameter was changed in the plug-in's editor (or by the plug-in itself).
    Parameter { key: String, address: u64, normalized: f64, value: f64 },
    /// A control was grabbed or released in the plug-in's editor.
    Gesture { key: String, address: u64, begin: bool },
    /// Something the interface shows changed: `parameters` (read the list again), `latency`,
    /// `state` (the plug-in says its state is dirty), `editor` (its window was closed),
    /// `restored`.
    Changed { key: String, what: &'static str },
    /// The instance's container is gone; it plays its fallback until restored.
    Lost { key: String, name: String, reason: String },
    /// A plug-in of the published project could not be loaded.
    LoadFailed { key: String, name: String, error: String },
    /// The mixer must be compiled again for these inserts (latency or availability changed).
    Graph { keys: Vec<String> },
}

pub type Listener = Arc<dyn Fn(&PluginEvent) + Send + Sync>;

/// One parameter's range, for the audio thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamRange {
    pub id: u64,
    pub min: f64,
    pub max: f64,
    pub stepped: bool,
}

/// The parameters of an instance for the audio thread: their ranges, sorted by id, and their
/// base values.
///
/// The base value of a parameter is the value the user left it at: loaded with the state, set
/// from the interface, changed in the plug-in's editor. Lanes and step locks move a parameter
/// away from it for a while; the processor brings it back ([`super::insert::PluginInsert`]).
/// Control threads write base values in place (atomics), the audio thread reads them.
#[derive(Debug, Default)]
pub struct ParamTable {
    ranges: Vec<ParamRange>,
    /// Per range: the base value, normalized, as the bits of an `f64` (NaN: not known).
    base: Vec<AtomicU64>,
}

impl ParamTable {
    /// A table for these ranges; no base value is known yet.
    pub fn new(mut ranges: Vec<ParamRange>) -> ParamTable {
        ranges.sort_by_key(|range| range.id);
        ranges.dedup_by_key(|range| range.id);
        let base = ranges.iter().map(|_| AtomicU64::new(f64::NAN.to_bits())).collect();
        ParamTable { ranges, base }
    }

    #[inline]
    fn index(&self, id: u64) -> Option<usize> {
        self.ranges.binary_search_by_key(&id, |range| range.id).ok()
    }

    /// The parameter's own value for a normalized one; `None` for an unknown parameter.
    #[inline]
    pub fn plain(&self, id: u64, normalized: f64) -> Option<f64> {
        let range = &self.ranges[self.index(id)?];
        let value = range.min + normalized.clamp(0.0, 1.0) * (range.max - range.min);
        Some(if range.stepped { value.round() } else { value })
    }

    pub fn normalized(&self, id: u64, plain: f64) -> Option<f64> {
        let range = &self.ranges[self.index(id)?];
        Some(if range.max > range.min { ((plain - range.min) / (range.max - range.min)).clamp(0.0, 1.0) } else { 0.0 })
    }

    /// The base value of a parameter (normalized), if one is known.
    #[inline]
    pub fn base(&self, id: u64) -> Option<f64> {
        let value = f64::from_bits(self.base[self.index(id)?].load(Ordering::Relaxed));
        (!value.is_nan()).then_some(value)
    }

    fn set_base(&self, id: u64, normalized: f64) {
        if let Some(index) = self.index(id) {
            self.base[index].store(normalized.to_bits(), Ordering::Relaxed);
        }
    }

    /// Every known base value: `(id, normalized)`.
    pub fn base_values(&self) -> Vec<(u64, f64)> {
        self.ranges.iter().filter_map(|range| self.base(range.id).map(|base| (range.id, base))).collect()
    }
}

/// Where an instance sits in the project.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Placement {
    /// The descriptor's owner: `track:<id>`, `bus:<id>`, `master` or `instrument:<n>`.
    pub owner: String,
    /// The strip that renders it (differs from `owner` for an instrument's chain).
    pub strip: String,
    pub instrument: bool,
    pub slot: usize,
    /// The descriptor's `instanceID`.
    pub instance_id: String,
}

/// The parameters automation (lanes, step locks) holds away from their base values: from the
/// moment a lane or a lock moves one until the plug-in has received its return. Written by
/// the audio thread only; control threads read it to tell a value the user set from one a
/// lane holds for the moment.
///
/// `changes` counts every change of the set. A control thread that reads the plug-in's
/// values notes the count before it asks and takes nothing for a base value when the count
/// has moved by the time the answer is there: the answer may then be older than the change.
pub(crate) struct AutomatedSet {
    count: AtomicU32,
    addresses: Box<[AtomicU64]>,
    changes: AtomicU64,
}

/// Parameters remembered per instance as moved by automation; a parameter automated beyond
/// that is not returned to its base value by the host.
pub(crate) const AUTOMATED_CAPACITY: usize = 512;

impl AutomatedSet {
    fn new() -> AutomatedSet {
        AutomatedSet { count: AtomicU32::new(0), addresses: (0..AUTOMATED_CAPACITY).map(|_| AtomicU64::new(0)).collect(), changes: AtomicU64::new(0) }
    }

    #[inline]
    fn position(&self, address: u64) -> Option<usize> {
        let count = (self.count.load(Ordering::Acquire) as usize).min(self.addresses.len());
        self.addresses[..count].iter().position(|known| known.load(Ordering::Relaxed) == address)
    }

    pub(crate) fn contains(&self, address: u64) -> bool {
        self.position(address).is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }

    /// How often the set has changed.
    pub(crate) fn changes(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    /// The address at `index`, for the audio thread to walk the set.
    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<u64> {
        (index < self.count.load(Ordering::Relaxed) as usize).then(|| self.addresses[index].load(Ordering::Relaxed))
    }

    /// Audio thread: automation moved `address`.
    #[inline]
    pub(crate) fn insert(&self, address: u64) {
        let count = self.count.load(Ordering::Relaxed) as usize;
        if count < self.addresses.len() && self.position(address).is_none() {
            self.addresses[count].store(address, Ordering::Relaxed);
            self.count.store(count as u32 + 1, Ordering::Release);
            self.changes.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Audio thread: the plug-in has `address` at its base value again.
    #[inline]
    pub(crate) fn remove(&self, address: u64) {
        let Some(index) = self.position(address) else { return };
        let last = self.count.load(Ordering::Relaxed) as usize - 1;
        self.addresses[index].store(self.addresses[last].load(Ordering::Relaxed), Ordering::Relaxed);
        self.count.store(last as u32, Ordering::Release);
        self.changes.fetch_add(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct HostedControl {
    /// The state last loaded into or read from the instance, as the project stores it.
    known_state: Option<String>,
    /// The bytes of the last capture with their text: equal bytes give the identical text.
    captured: Option<(Vec<u8>, String)>,
    parameters: Option<Arc<Vec<ParameterInfo>>>,
    lost: Option<String>,
    /// Values set with [`Hosted::set_parameter`] the plug-in may not have applied yet (some
    /// formats deliver a set with the next block): `(id, value, blocks processed at the set)`.
    pending_sets: Vec<(u64, f64, u64)>,
    placement: Placement,
    /// Tables the audio thread may still be reading; freed with the instance.
    old_tables: Vec<*mut ParamTable>,
    editor_open: bool,
}

// SAFETY: the raw table pointers are only freed in `Drop`, when no other reference exists.
unsafe impl Send for HostedControl {}

/// One plug-in instance in a container, shared by the manager and the processor that renders it.
pub struct Hosted {
    pub key: String,
    pub identity: Identity,
    pub instrument: bool,
    /// The client's proxy. The audio thread calls `process_raw` and the real-time getters only.
    pub instance: Instance,
    /// Whether the plug-in uses the sidechain bus it was given.
    pub sidechain: bool,
    pub has_editor: bool,
    /// The sustain pedal (controller 64) can reach the plug-in. An instrument without this
    /// has no pedal of its own: whoever plays it holds its notes while the pedal is down.
    pub sustain_pedal: bool,
    table: AtomicPtr<ParamTable>,
    /// Channel messages from control threads for the audio thread (controllers, pressure, pitch
    /// bend): up to three MIDI bytes and their count, applied at the start of the next block.
    midi: Ring<[u8; 4]>,
    /// Serializes the senders of `midi`: the ring takes one producer at a time.
    midi_senders: Mutex<()>,
    /// See [`Hosted::pedal_generation`].
    pedal_generation: AtomicU64,
    pub(crate) automated: AutomatedSet,
    /// A control thread asks the audio thread to return what automation moved to its base
    /// values with the next block ([`Hosted::return_to_base`]).
    pub(crate) base_requested: AtomicBool,
    /// Returns to base values the audio thread has queued and the plug-in has not received
    /// yet (they go with the next block it answers).
    pub(crate) returns_pending: AtomicU32,
    /// The latency the mixer was last told about.
    pub(crate) planned_latency: AtomicU32,
    pub(crate) offline: bool,
    pub(crate) min_deadline_ns: u64,
    control: Mutex<HostedControl>,
}

impl Hosted {
    fn control(&self) -> MutexGuard<'_, HostedControl> {
        self.control.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The parameter ranges, for the audio thread: no lock, no allocation.
    #[inline]
    pub fn table(&self) -> &ParamTable {
        // SAFETY: the pointer is always a live `Box<ParamTable>`: a replaced table is kept
        // until the instance is dropped.
        unsafe { &*self.table.load(Ordering::Acquire) }
    }

    /// Audio thread: the next message queued by [`Hosted::send_midi`]: the bytes and how many
    /// of them count.
    #[inline]
    pub(crate) fn pop_midi(&self) -> Option<[u8; 4]> {
        self.midi.pop()
    }

    /// A number that changes whenever the plug-in may have lost what it was told about the
    /// pedals: it is another one for every instance, after a restore, and after the processor
    /// sent the pedals up with "all notes off" (panic, blocks the plug-in did not answer, the
    /// end of a bypass). Whoever holds a pedal down for the plug-in sends it again when the
    /// number is not the one it last sent the pedal under.
    pub fn pedal_generation(&self) -> u64 {
        self.pedal_generation.load(Ordering::Acquire)
    }

    /// The plug-in's pedals are up, whatever it was sent before. Any thread, the audio
    /// thread included.
    pub(crate) fn pedals_reset(&self) {
        self.pedal_generation.store(next_pedal_generation(), Ordering::Release);
    }

    /// Queues a channel message for the plug-in: a control change, channel or poly pressure,
    /// pitch bend or a program change (the two-byte messages ignore `bytes[2]`). It reaches
    /// the plug-in at the first frame of the next block. False for any other message and when
    /// the queue is full (the plug-in is not processing). Any control thread.
    pub fn send_midi(&self, bytes: [u8; 3]) -> bool {
        let length = match bytes[0] & 0xF0 {
            0xA0 | 0xB0 | 0xE0 => 3,
            0xC0 | 0xD0 => 2,
            _ => return false,
        };
        let _one_sender = self.midi_senders.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.midi.push(&[bytes[0], bytes[1] & 0x7F, if length == 3 { bytes[2] & 0x7F } else { 0 }, length])
    }

    /// The values just read from the plug-in as base values.
    ///
    /// `held` is `Some` for an ordinary read, with [`AutomatedSet::changes`] as it was before
    /// the plug-in was asked: what automation holds keeps the base value it had (the value
    /// read is the lane's or the lock's, not the user's), and when automation took or
    /// released a parameter while the answer was on its way, nothing is taken at all (the
    /// next read does it). `None`: every value counts (a state was just loaded).
    fn adopt_values(&self, read: &[(u64, f64)], held: Option<u64>) {
        let table = self.table();
        for (id, plain) in read {
            if held.is_some_and(|changes| table.base(*id).is_some() && (self.automated.changes() != changes || self.automated.contains(*id))) {
                continue;
            }
            if let Some(normalized) = table.normalized(*id, *plain) {
                table.set_base(*id, normalized);
            }
        }
    }

    pub fn placement(&self) -> Placement {
        self.control().placement.clone()
    }

    /// Why the instance is not processing, if it is not.
    pub fn lost(&self) -> Option<String> {
        if let Some(reason) = self.control().lost.clone() {
            return Some(reason);
        }
        matches!(self.instance.status(), InstanceStatus::Lost | InstanceStatus::Closed).then(|| format!("{} stopped working: its plug-in process ended.", self.identity.display_name()))
    }

    /// A new table for a parameter list read again. The base values known so far stay.
    fn install_table(&self, parameters: &[ParameterInfo]) {
        let ranges: Vec<ParamRange> = parameters.iter().map(|info| ParamRange { id: info.id, min: info.min, max: info.max, stepped: info.flags & (PARAM_STEPPED | PARAM_BOOLEAN) != 0 }).collect();
        let next = ParamTable::new(ranges);
        for (id, base) in self.table().base_values() {
            next.set_base(id, base);
        }
        let old = self.table.swap(Box::into_raw(Box::new(next)), Ordering::AcqRel);
        self.control().old_tables.push(old);
    }

    /// Reads the parameter list and the current values from the plug-in again.
    pub fn refresh_parameters(&self) -> Result<Arc<Vec<ParameterInfo>>, String> {
        self.read_parameters(true)
    }

    fn read_parameters(&self, keep_held: bool) -> Result<Arc<Vec<ParameterInfo>>, String> {
        let parameters = Arc::new(self.instance.parameters().map_err(|error| self.explain(error))?);
        self.install_table(&parameters);
        let changes = self.automated.changes();
        let values: Vec<(u64, f64)> = self.instance.parameter_values().map_err(|error| self.explain(error))?.into_iter().map(|value| (value.id, value.value)).collect();
        self.adopt_values(&values, keep_held.then_some(changes));
        self.control().parameters = Some(parameters.clone());
        Ok(parameters)
    }

    /// The parameter metadata (read once, then kept until the plug-in asks for a rescan).
    pub fn parameters(&self) -> Result<Arc<Vec<ParameterInfo>>, String> {
        if let Some(parameters) = self.control().parameters.clone() {
            return Ok(parameters);
        }
        self.refresh_parameters()
    }

    /// The current values in the parameters' own ranges, read from the plug-in. For a lost
    /// instance: the values last known.
    pub fn values(&self) -> Result<Vec<(u64, f64)>, String> {
        if self.lost().is_some() {
            let table = self.table();
            return Ok(table.base_values().into_iter().filter_map(|(id, normalized)| table.plain(id, normalized).map(|plain| (id, plain))).collect());
        }
        let changes = self.automated.changes();
        let mut values = self.instance.parameter_values().map_err(|error| self.explain(error))?;
        let mut control = self.control();
        // A value set a moment ago may not have reached the plug-in yet: it reads as set.
        let blocks = self.instance.stats().blocks;
        control.pending_sets.retain(|(_, _, at)| blocks < at + 2);
        for (id, plain, _) in &control.pending_sets {
            if let Some(value) = values.iter_mut().find(|value| value.id == *id) {
                value.value = *plain;
            }
        }
        let values: Vec<(u64, f64)> = values.into_iter().map(|value| (value.id, value.value)).collect();
        drop(control);
        self.adopt_values(&values, Some(changes));
        Ok(values)
    }

    /// Waits (briefly) until the values set last, and the returns to base values on their
    /// way, have reached the plug-in with a block, so that a state read next contains them.
    /// Gives up when no audio is running.
    fn settle(&self) {
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        loop {
            let blocks = self.instance.stats().blocks;
            let mut control = self.control();
            control.pending_sets.retain(|(_, _, at)| blocks < at + 2);
            let arrived = control.pending_sets.is_empty() && self.returns_pending.load(Ordering::Acquire) == 0;
            if arrived || std::time::Instant::now() >= deadline || self.instance.status() != InstanceStatus::Running {
                return;
            }
            drop(control);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The base values (normalized): what automation and step locks return to.
    pub fn base_values(&self) -> Vec<(u64, f64)> {
        self.table().base_values()
    }

    /// Brings every parameter a lane or a step lock moved back to its base value, and waits
    /// (briefly) until the plug-in has it: for a state that is the patch as the user made
    /// it. For when the song does not play; while it plays, the lanes would move the
    /// parameters again at once. Gives up when no audio is running.
    pub fn return_to_base(&self) {
        if (self.automated.is_empty() && self.returns_pending.load(Ordering::Acquire) == 0) || self.lost().is_some() {
            return;
        }
        self.base_requested.store(true, Ordering::Release);
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        // The audio thread takes the request with its next block and clears the count once
        // the plug-in has answered a block that carried the values.
        while self.base_requested.load(Ordering::Acquire) || self.returns_pending.load(Ordering::Acquire) > 0 {
            if std::time::Instant::now() >= deadline || self.instance.status() != InstanceStatus::Running {
                // Nobody renders: the request must not wait for a playback that starts later.
                self.base_requested.store(false, Ordering::Release);
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The base values of the parameters automation holds away from them at this moment.
    pub fn held_base_values(&self) -> Vec<(u64, f64)> {
        self.table().base_values().into_iter().filter(|(id, _)| self.automated.contains(*id)).collect()
    }

    fn remember_value(&self, id: u64, normalized: f64) {
        self.table().set_base(id, normalized);
    }

    /// Sets a parameter (in its own range) now. Returns the value sent (stepped parameters
    /// are rounded).
    pub fn set_parameter(&self, id: u64, plain: f64) -> Result<f64, String> {
        let parameters = self.parameters()?;
        let Some(info) = parameters.iter().find(|info| info.id == id) else { return Err("The plug-in has no such parameter.".to_string()) };
        if info.flags & PARAM_READ_ONLY != 0 {
            return Err(format!("{} cannot be changed: the plug-in only reports it.", info.name));
        }
        let mut value = plain.clamp(info.min.min(info.max), info.max.max(info.min));
        if info.flags & (PARAM_STEPPED | PARAM_BOOLEAN) != 0 {
            value = value.round();
        }
        let blocks = self.instance.stats().blocks;
        self.instance.set_parameter(id, value).map_err(|error| self.explain(error))?;
        if let Some(normalized) = self.table().normalized(id, value) {
            self.remember_value(id, normalized);
        }
        let mut control = self.control();
        control.pending_sets.retain(|(known, _, _)| *known != id);
        control.pending_sets.push((id, value, blocks));
        Ok(value)
    }

    /// The plug-in's text for values of one parameter.
    pub fn display(&self, id: u64, values: &[f64]) -> Option<Vec<String>> {
        self.instance.format_parameter(id, values).ok()
    }

    /// Reads the state from the plug-in: base64 of the container's state bytes. A plug-in that
    /// has not changed gives the identical text again.
    pub fn capture_state(&self) -> Result<String, String> {
        self.settle();
        let bytes = self.instance.state().map_err(|error| self.explain(error))?;
        let mut control = self.control();
        let text = match &control.captured {
            Some((known, text)) if *known == bytes => text.clone(),
            _ => base64_encode(&bytes),
        };
        control.captured = Some((bytes, text.clone()));
        control.known_state = Some(text.clone());
        Ok(text)
    }

    /// The state for the project file: read now, or the last one known while the instance is
    /// lost (its descriptor then keeps exactly what it had).
    pub fn snapshot_state(&self) -> Result<Option<String>, String> {
        if self.lost().is_some() {
            return Ok(self.control().known_state.clone());
        }
        self.capture_state().map(Some)
    }

    /// The state last loaded or read, without asking the plug-in.
    pub fn known_state(&self) -> Option<String> {
        self.control().known_state.clone()
    }

    /// Loads a state (base64, as [`Hosted::capture_state`] gives it).
    pub fn load_state(&self, state: &str) -> Result<(), String> {
        let bytes = base64_decode(state).filter(|bytes| !bytes.is_empty()).ok_or_else(|| format!("The saved state of {} is not readable.", self.identity.display_name()))?;
        self.load_state_bytes(&bytes)?;
        let mut control = self.control();
        control.known_state = Some(state.to_string());
        control.captured = Some((bytes, state.to_string()));
        Ok(())
    }

    /// Loads state bytes in the container's own framing.
    pub fn load_state_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        // A value set a moment ago and not yet delivered would land on top of the state.
        self.settle();
        self.instance.set_state(bytes).map_err(|error| self.explain(error))?;
        {
            let mut control = self.control();
            control.pending_sets.clear();
            control.captured = None;
            control.known_state = None;
        }
        let _ = self.instance.refresh_info();
        let _ = self.read_parameters(false);
        Ok(())
    }

    pub fn presets(&self) -> Vec<(u32, String)> {
        self.instance.presets().map(|presets| presets.into_iter().map(|preset| (preset.id, preset.name)).collect()).unwrap_or_default()
    }

    pub fn load_preset(&self, id: u32) -> Result<(), String> {
        self.settle();
        self.instance.load_preset(id).map_err(|error| self.explain(error))?;
        {
            let mut control = self.control();
            control.pending_sets.clear();
            control.captured = None;
            control.known_state = None;
        }
        let _ = self.instance.refresh_info();
        let _ = self.read_parameters(false);
        Ok(())
    }

    /// Opens the plug-in's window in its container (or brings the open one to the front).
    /// Returns `(width, height, window id)`.
    pub fn open_editor(&self, title: &str) -> Result<(u32, u32, u64), String> {
        if self.control().editor_open {
            self.focus_editor()?;
            return Ok((0, 0, 0));
        }
        let info = self.instance.open_editor(title).map_err(|error| match error {
            ClientError::Unsupported => format!("{} has no editor window on this system. Its parameters are on the plug-in's pages.", self.identity.display_name()),
            other => self.explain(other),
        })?;
        self.control().editor_open = true;
        Ok((info.width, info.height, info.window_id))
    }

    pub fn close_editor(&self) -> Result<(), String> {
        let was_open = std::mem::take(&mut self.control().editor_open);
        if was_open {
            self.instance.close_editor().map_err(|error| self.explain(error))?;
        }
        Ok(())
    }

    /// Brings the editor window to the front. On macOS the host yields activation to the
    /// container's process first (see [`yield_activation`]).
    pub fn focus_editor(&self) -> Result<(), String> {
        self.instance.focus_editor().map_err(|error| self.explain(error))
    }

    pub fn editor_open(&self) -> bool {
        self.control().editor_open
    }

    /// Latency in frames as the plug-in reports it now.
    pub fn latency(&self) -> u32 {
        self.instance.latency()
    }

    /// Tail in seconds (an infinite tail reads as 60 s).
    pub fn tail_seconds(&self, sample_rate: f64) -> f64 {
        match self.instance.tail() {
            u64::MAX => 60.0,
            frames => (frames as f64 / sample_rate.max(1.0)).min(600.0),
        }
    }

    fn explain(&self, error: ClientError) -> String {
        let name = self.identity.display_name();
        match error {
            ClientError::InstanceLost | ClientError::ContainerDied | ClientError::Closed => {
                format!("{name} is not running: its plug-in process ended. Choose Review interrupted plug-ins to restore it.")
            }
            ClientError::Timeout(_) => format!("{name} did not answer in time. It may be busy; try again."),
            ClientError::Unsupported => format!("{name} does not support this."),
            other => format!("{name}: {other}"),
        }
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        let control = self.control.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner());
        for table in control.old_tables.drain(..).chain([*self.table.get_mut()]) {
            if !table.is_null() {
                // SAFETY: each pointer came from `Box::into_raw` and is dropped exactly once.
                drop(unsafe { Box::from_raw(table) });
            }
        }
    }
}

/// What [`PluginManager::lookup`] knows about an insert.
pub enum Lookup {
    Hosted(Arc<Hosted>),
    /// The project wants this plug-in, but it could not be loaded: why.
    Unavailable(String),
    /// Not a plug-in of the reconciled project.
    Unknown,
}

struct Failure {
    signature: String,
    reason: String,
    name: String,
}

#[derive(Default)]
struct Inner {
    instances: HashMap<String, Arc<Hosted>>,
    by_id: HashMap<InstanceId, String>,
    failures: HashMap<String, Failure>,
    graveyard: Vec<Arc<Hosted>>,
    sample_rate: f64,
    max_frames: usize,
    wanted_format: Option<(f64, usize)>,
}

/// See the module documentation.
pub struct PluginManager {
    /// `Err`: why no container can be started (the container executable is missing). The
    /// manager then hosts nothing and says so for every plug-in a project wants.
    supervisor: Result<Supervisor, String>,
    options: PluginOptions,
    /// [`PluginOptions::min_deadline`] for the instances created next, in nanoseconds
    /// ([`PluginManager::set_min_deadline`]).
    min_deadline_ns: AtomicU64,
    recovery: Arc<RecoveryStore>,
    catalog: Mutex<Option<Arc<Catalog>>>,
    inner: Mutex<Inner>,
    /// Serializes reconciliations and loads (they wait for plug-ins; `inner` is never held
    /// across such a wait).
    loading: Mutex<()>,
    listeners: Mutex<Vec<Listener>>,
    /// The manager whose instances an offline copy takes its states from.
    live: Option<Weak<PluginManager>>,
    stop: Arc<AtomicBool>,
    panic: AtomicBool,
}

/// One plug-in the project wants.
struct Desired<'a> {
    insert: &'a InsertSpec,
    strip: &'a str,
    descriptor: Option<&'a PluginDescriptor>,
}

fn crash_text(reason: &ExitReason) -> String {
    match reason {
        ExitReason::Crashed { signal } => format!("its plug-in process crashed (signal {signal})"),
        ExitReason::Exited { code } => format!("its plug-in process exited (code {code})"),
        ExitReason::Hung => "it stopped answering and its plug-in process was ended".to_string(),
        ExitReason::Killed => "its plug-in process was ended".to_string(),
        ExitReason::Shutdown => "its plug-in process was shut down".to_string(),
    }
}

impl PluginManager {
    /// Starts the supervisor (no container runs until a plug-in is loaded) and the manager's
    /// thread. Without a container executable the manager still exists: see
    /// [`PluginManager::unavailable`].
    pub fn new(options: PluginOptions) -> Result<Arc<PluginManager>, String> {
        Self::build(options, None)
    }

    fn build(options: PluginOptions, live: Option<Weak<PluginManager>>) -> Result<Arc<PluginManager>, String> {
        let supervisor = Supervisor::new(options.supervisor.clone()).map_err(|error| format!("plug-in hosting is unavailable ({error})"));
        let (sender, receiver): (Sender<HostEvent>, Receiver<HostEvent>) = std::sync::mpsc::channel();
        if let Ok(supervisor) = &supervisor {
            let sender = Mutex::new(sender);
            supervisor.set_event_callback(Some(Box::new(move |event| {
                let _ = sender.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send(event.clone());
            })));
        }
        // An offline copy shares the store: what is held back stays held back.
        let recovery = match live.as_ref().and_then(Weak::upgrade) {
            Some(live) => live.recovery.clone(),
            None => Arc::new(RecoveryStore::open(options.recovery_path.clone())),
        };
        let manager = Arc::new(PluginManager {
            supervisor,
            min_deadline_ns: AtomicU64::new(options.min_deadline.as_nanos().min(u128::from(u64::MAX)) as u64),
            options,
            recovery,
            catalog: Mutex::new(None),
            inner: Mutex::new(Inner::default()),
            loading: Mutex::new(()),
            listeners: Mutex::new(Vec::new()),
            live,
            stop: Arc::new(AtomicBool::new(false)),
            panic: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&manager);
        let stop = manager.stop.clone();
        std::thread::Builder::new()
            .name("plugin-manager".to_string())
            .spawn(move || Self::run(weak, receiver, stop))
            .map_err(|error| format!("Plug-in hosting could not start its thread: {error}"))?;
        Ok(manager)
    }

    /// A manager for an offline render (export, freeze) of the same project: its own
    /// containers, instances that render without deadline, and every instance starting from
    /// the state its live counterpart has now.
    pub fn offline(self: &Arc<Self>) -> Result<Arc<PluginManager>, String> {
        let options = PluginOptions { offline: true, recovery_path: None, ..self.options.clone() };
        let manager = Self::build(options, Some(Arc::downgrade(self)))?;
        *manager.catalog.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = self.catalog();
        Ok(manager)
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn is_offline(&self) -> bool {
        self.options.offline
    }

    /// Changes [`PluginOptions::min_deadline`] for the instances created from now on. A test
    /// that plays live through a host it did not build the manager for gives every block all
    /// the time it needs, so that a busy machine never makes a plug-in miss a block (and the
    /// notes and controllers in it).
    pub fn set_min_deadline(&self, deadline: Duration) {
        self.min_deadline_ns.store(deadline.as_nanos().min(u128::from(u64::MAX)) as u64, Ordering::Release);
    }

    pub fn recovery(&self) -> &Arc<RecoveryStore> {
        &self.recovery
    }

    /// The catalogue of installed plug-ins: finds the file of a descriptor that names none.
    pub fn set_catalog(&self, catalog: Arc<Catalog>) {
        *self.catalog.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(catalog);
    }

    pub fn catalog(&self) -> Option<Arc<Catalog>> {
        self.catalog.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Adds a listener. Listeners are called without any of the manager's locks held and may
    /// call back into it.
    pub fn listen(&self, listener: Listener) {
        self.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(listener);
    }

    fn emit(&self, event: PluginEvent) {
        let listeners = self.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        for listener in listeners {
            listener(&event);
        }
    }

    /// Why no plug-in can be hosted, if none can.
    pub fn unavailable(&self) -> Option<&str> {
        self.supervisor.as_ref().err().map(String::as_str)
    }

    /// The grouping policy for instances created from now on.
    pub fn set_policy(&self, policy: GroupingPolicy) {
        if let Ok(supervisor) = &self.supervisor {
            supervisor.set_policy(policy);
        }
    }

    /// The process id of the container an instance lives in (0 when it is not running).
    pub fn container_pid(&self, hosted: &Hosted) -> u32 {
        let container = hosted.instance.container();
        self.supervisor.as_ref().ok().and_then(|supervisor| supervisor.containers().iter().find(|info| info.id == container).map(|info| info.pid)).unwrap_or(0)
    }

    // ---- lookups -------------------------------------------------------------------------

    pub fn hosted(&self, key: &str) -> Option<Arc<Hosted>> {
        self.inner().instances.get(key).cloned()
    }

    /// Every hosted instance, by key.
    pub fn all(&self) -> Vec<Arc<Hosted>> {
        let mut all: Vec<Arc<Hosted>> = self.inner().instances.values().cloned().collect();
        all.sort_by(|a, b| a.key.cmp(&b.key));
        all
    }

    pub fn lookup(&self, key: &str) -> Lookup {
        let inner = self.inner();
        match (inner.instances.get(key), inner.failures.get(key)) {
            (Some(hosted), _) => Lookup::Hosted(hosted.clone()),
            (None, Some(failure)) => Lookup::Unavailable(failure.reason.clone()),
            (None, None) => Lookup::Unknown,
        }
    }

    /// The instance for a descriptor of the reconciled project: by `instanceID` when it has
    /// one (the first track's instance of an instrument's chain effect), else by place.
    pub fn find(&self, owner: &str, instrument: bool, slot: usize, instance_id: Option<&str>) -> Option<Arc<Hosted>> {
        let inner = self.inner();
        let mut found: Vec<&Arc<Hosted>> = inner
            .instances
            .values()
            .filter(|hosted| {
                let placement = hosted.control().placement.clone();
                match instance_id.filter(|id| !id.is_empty()) {
                    Some(id) => placement.instance_id == id,
                    None => placement.owner == owner && placement.instrument == instrument && (instrument || placement.slot == slot),
                }
            })
            .collect();
        found.sort_by(|a, b| a.key.cmp(&b.key));
        found.first().map(|hosted| (*hosted).clone())
    }

    /// Why a wanted plug-in of the reconciled project is not hosted.
    pub fn failure(&self, key: &str) -> Option<String> {
        self.inner().failures.get(key).map(|failure| failure.reason.clone())
    }

    // ---- reconciliation ------------------------------------------------------------------

    fn identity_of(&self, insert: &InsertSpec, descriptor: Option<&PluginDescriptor>) -> Result<Identity, String> {
        let name = if insert.name.is_empty() { insert.component.clone() } else { insert.name.clone() };
        let format_hint = descriptor.and_then(|descriptor| descriptor.format.as_deref());
        let Some((format, id)) = parse_component(&insert.component, format_hint) else {
            return Err(format!("{name} names a plug-in format this version does not host ({}).", insert.component));
        };
        let mut identity = Identity {
            format,
            id,
            path: descriptor.and_then(|descriptor| descriptor.bundle_path.clone()).unwrap_or_default(),
            name: insert.name.clone(),
            manufacturer: descriptor.and_then(|descriptor| descriptor.manufacturer.clone()).unwrap_or_default(),
        };
        if identity.format != "au" && (identity.path.is_empty() || !std::path::Path::new(&identity.path).exists()) {
            // The descriptor names no file, or one that moved: ask the catalogue.
            match self.catalog().and_then(|catalog| catalog.find(&identity.format, &identity.id)) {
                Some(found) => {
                    identity.path = found.path;
                    if identity.name.is_empty() {
                        identity.name = found.name;
                    }
                    if identity.manufacturer.is_empty() {
                        identity.manufacturer = found.vendor;
                    }
                }
                None => return Err(format!("{name} is not installed on this computer. Its settings are kept in the project.")),
            }
        }
        Ok(identity)
    }

    fn create(&self, identity: &Identity, key: &str, instrument: bool, state: Option<Vec<u8>>, sample_rate: f64, max_frames: usize) -> Result<Arc<Hosted>, String> {
        if let Some(reason) = self.recovery.held(identity) {
            return Err(reason);
        }
        let name = identity.display_name().to_string();
        let supervisor = self.supervisor.as_ref().map_err(|reason| format!("{name} cannot be loaded: {reason}."))?;
        let config = InstanceConfig {
            manufacturer: identity.manufacturer.clone(),
            name: name.clone(),
            options: identity.path.clone(),
            kind: if instrument { ClientKind::Instrument } else { ClientKind::Effect },
            sample_rate,
            max_frames: max_frames as u32,
            // An effect gets a stereo sidechain bus; the plug-in says whether it uses it.
            input_buses: if instrument { Vec::new() } else { vec![2, 2] },
            output_buses: vec![2],
            event_capacity: EVENT_CAPACITY as u32,
            out_event_capacity: 64,
            fallback: Fallback::Auto,
            max_dry_delay_frames: if instrument { 0 } else { 16_384 },
            initial_state: state,
            ..InstanceConfig::effect(&identity.format, &identity.id)
        };
        self.recovery.begin_load(identity);
        let crashed = |error: &ClientError| matches!(error, ClientError::ContainerDied | ClientError::InstanceLost | ClientError::Timeout(_));
        let fail = |error: ClientError, doing: &str| -> String {
            let text = match &error {
                ClientError::ContainerDied | ClientError::InstanceLost => format!("{name} stopped working while {doing}: its plug-in process ended."),
                ClientError::Timeout(_) => format!("{name} did not finish {doing} in time."),
                ClientError::Spawn(message) => format!("{name} could not be loaded: the plug-in container did not start ({message})."),
                other => format!("{name} could not be loaded: {other}"),
            };
            self.recovery.end_load(identity, crashed(&error).then_some(text.as_str()));
            text
        };
        let instance = supervisor.create_instance(config).map_err(|error| fail(error, "loading"))?;
        if self.options.offline {
            instance.set_offline(true).map_err(|error| fail(error, "loading"))?;
        }
        instance.activate().map_err(|error| fail(error, "starting"))?;
        let info = instance.info();
        // The plug-in says what it is called and who made it; the descriptor and the
        // catalogue only repeat that, when they know it.
        let mut named = identity.clone();
        if !info.name.is_empty() {
            named.name = info.name.clone();
        }
        if !info.vendor.is_empty() {
            named.manufacturer = info.vendor.clone();
        }
        let hosted = Arc::new(Hosted {
            key: key.to_string(),
            identity: named,
            instrument,
            sidechain: !instrument && info.inputs.get(1).is_some_and(|bus| bus.channels > 0),
            has_editor: info.flags & INFO_HAS_EDITOR != 0,
            sustain_pedal: info.flags & INFO_SUSTAIN_PEDAL != 0,
            planned_latency: AtomicU32::new(instance.latency()),
            instance,
            table: AtomicPtr::new(Box::into_raw(Box::new(ParamTable::default()))),
            midi: Ring::new(256),
            midi_senders: Mutex::new(()),
            pedal_generation: AtomicU64::new(next_pedal_generation()),
            automated: AutomatedSet::new(),
            base_requested: AtomicBool::new(false),
            returns_pending: AtomicU32::new(0),
            offline: self.options.offline,
            min_deadline_ns: self.min_deadline_ns.load(Ordering::Acquire),
            control: Mutex::new(HostedControl::default()),
        });
        if let Err(error) = hosted.refresh_parameters() {
            let lost = hosted.lost().is_some();
            self.recovery.end_load(identity, lost.then_some(error.as_str()));
            return Err(error);
        }
        self.recovery.end_load(identity, None);
        Ok(hosted)
    }

    /// Makes the hosted instances match `project` (see the module documentation). Problems are
    /// announced as [`PluginEvent::LoadFailed`] and kept for [`PluginManager::lookup`]; a
    /// project is never refused because of a plug-in. `force_states` loads every descriptor's
    /// state again (a project was opened or replaced).
    ///
    /// Returns the keys whose processor the mixer must make anew: instances created or
    /// replaced, and those whose latency changed with their state.
    pub fn reconcile(&self, project: &Project, sample_rate: f64, max_frames: usize, force_states: bool) -> Vec<String> {
        // A project that describes no mixer is refused by the publication this is part of.
        let Ok(spec) = GraphSpec::from_project(project) else { return Vec::new() };
        let descriptors = project.plugin_list();
        let mut desired: Vec<Desired<'_>> = Vec::new();
        for strip in &spec.strips {
            for insert in strip.inserts.iter().filter(|insert| insert.builtin_id().is_none() && !insert.component.is_empty() && !insert.component.starts_with("test:")) {
                let descriptor = descriptors.iter().find(|descriptor| {
                    if !insert.instance_id.is_empty() {
                        return descriptor.instance_id.as_deref() == Some(insert.instance_id.as_str());
                    }
                    let instrument = descriptor.kind() == PluginKind::Instrument;
                    instrument == insert.instrument.is_some() && descriptor.owner.as_deref() == Some(strip.id.as_str()) && (instrument || descriptor.slot() == insert.slot as i64)
                });
                desired.push(Desired { insert, strip: &strip.id, descriptor });
            }
        }
        if desired.is_empty() && self.inner().instances.is_empty() {
            let mut inner = self.inner();
            inner.failures.clear();
            inner.sample_rate = sample_rate;
            inner.max_frames = max_frames;
            return Vec::new();
        }
        let _loading = self.loading.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut events = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        // What is no longer wanted goes first, so a replaced plug-in frees its place.
        let reconfigure = {
            let mut inner = self.inner();
            let reconfigure = !inner.instances.is_empty() && (inner.sample_rate != sample_rate || inner.max_frames != max_frames);
            inner.sample_rate = sample_rate;
            inner.max_frames = max_frames;
            let gone: Vec<String> = inner.instances.keys().filter(|key| !desired.iter().any(|wanted| wanted.insert.key == **key)).cloned().collect();
            for key in gone {
                Self::bury(&mut inner, &key, &self.recovery);
            }
            inner.failures.retain(|key, _| desired.iter().any(|wanted| wanted.insert.key == *key));
            reconfigure
        };
        for wanted in &desired {
            let insert = wanted.insert;
            let instrument = insert.instrument.is_some();
            let placement = Placement {
                owner: wanted.descriptor.and_then(|descriptor| descriptor.owner.clone()).unwrap_or_else(|| wanted.strip.to_string()),
                strip: wanted.strip.to_string(),
                instrument,
                slot: insert.slot,
                instance_id: insert.instance_id.clone(),
            };
            let name = if insert.name.is_empty() { insert.component.clone() } else { insert.name.clone() };
            let signature = format!("{}|{}|{}", insert.component, wanted.descriptor.and_then(|descriptor| descriptor.bundle_path.as_deref()).unwrap_or_default(), insert.state.as_deref().map_or(0, str::len));
            let existing = self.hosted(&insert.key);
            let identity = match self.identity_of(insert, wanted.descriptor) {
                Ok(identity) => identity,
                Err(reason) => {
                    // A plug-in that is not installed keeps an instance that is (same key): no.
                    let mut inner = self.inner();
                    Self::bury(&mut inner, &insert.key, &self.recovery);
                    let known = inner.failures.get(&insert.key).is_some_and(|failure| failure.signature == signature);
                    inner.failures.insert(insert.key.clone(), Failure { signature, reason: reason.clone(), name: name.clone() });
                    if !known {
                        events.push(PluginEvent::LoadFailed { key: insert.key.clone(), name, error: reason });
                    }
                    continue;
                }
            };
            if let Some(hosted) = existing.filter(|hosted| hosted.identity.format == identity.format && hosted.identity.id == identity.id && hosted.identity.path == identity.path && hosted.instrument == instrument) {
                hosted.control().placement = placement;
                if reconfigure && hosted.lost().is_none() {
                    if let Err(error) = hosted.instance.reconfigure(sample_rate, max_frames as u32) {
                        events.push(PluginEvent::LoadFailed { key: insert.key.clone(), name: name.clone(), error: hosted.explain(error) });
                    }
                    if hosted.planned_latency.swap(hosted.latency(), Ordering::Relaxed) != hosted.latency() {
                        changed.push(insert.key.clone());
                    }
                }
                if let Some(state) = insert.state.as_deref().filter(|state| !state.is_empty()) {
                    if hosted.lost().is_none() && (force_states || hosted.known_state().as_deref() != Some(state)) {
                        match hosted.load_state(state) {
                            Ok(()) => {
                                if hosted.planned_latency.swap(hosted.latency(), Ordering::Relaxed) != hosted.latency() {
                                    changed.push(insert.key.clone());
                                }
                            }
                            Err(error) => events.push(PluginEvent::LoadFailed { key: insert.key.clone(), name: name.clone(), error }),
                        }
                    }
                }
                continue;
            }
            // New, or another plug-in in the same place.
            changed.push(insert.key.clone());
            {
                let mut inner = self.inner();
                Self::bury(&mut inner, &insert.key, &self.recovery);
                // A failed load is not tried again for the same descriptor: each publication
                // would wait for it anew.
                if !force_states && inner.failures.get(&insert.key).is_some_and(|failure| failure.signature == signature) {
                    continue;
                }
            }
            // An offline copy starts from what the live instance holds now.
            let live_instance = self.live.as_ref().and_then(Weak::upgrade).and_then(|live| live.hosted(&insert.key));
            let live_state = live_instance.as_ref().and_then(|hosted| hosted.snapshot_state().ok().flatten());
            let state_text = live_state.or_else(|| insert.state.clone()).filter(|state| !state.is_empty());
            let state = state_text.as_deref().and_then(base64_decode).filter(|bytes| !bytes.is_empty());
            let created = self.create(&identity, &insert.key, instrument, state, sample_rate, max_frames);
            if let (Ok(hosted), Some(live)) = (&created, &live_instance) {
                // That state has the values lanes and step locks hold in the live instance at
                // this moment. The copy starts from the user's values, like a new playback.
                for (id, base) in live.held_base_values() {
                    if let Some(plain) = hosted.table().plain(id, base) {
                        let _ = hosted.set_parameter(id, plain);
                    }
                }
            }
            let mut inner = self.inner();
            match created {
                Ok(hosted) => {
                    {
                        let mut control = hosted.control();
                        control.placement = placement;
                        control.known_state = state_text;
                    }
                    inner.failures.remove(&insert.key);
                    inner.by_id.insert(hosted.instance.id(), insert.key.clone());
                    inner.instances.insert(insert.key.clone(), hosted);
                }
                Err(reason) => {
                    inner.failures.insert(insert.key.clone(), Failure { signature, reason: reason.clone(), name: name.clone() });
                    events.push(PluginEvent::LoadFailed { key: insert.key.clone(), name, error: reason });
                }
            }
        }
        drop(_loading);
        for event in events {
            self.emit(event);
        }
        changed
    }

    /// The engine runs at another sample rate or block size: every instance is reconfigured
    /// on the manager's thread, then the mixer is compiled again.
    pub fn request_format(&self, sample_rate: f64, max_frames: usize) {
        let mut inner = self.inner();
        if inner.sample_rate != sample_rate || inner.max_frames != max_frames {
            inner.wanted_format = Some((sample_rate, max_frames));
        }
    }

    fn apply_format(&self) {
        let Some((sample_rate, max_frames)) = self.inner().wanted_format.take() else { return };
        let _loading = self.loading.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        {
            let mut inner = self.inner();
            inner.sample_rate = sample_rate;
            inner.max_frames = max_frames;
        }
        let mut keys = Vec::new();
        for hosted in self.all() {
            if hosted.lost().is_none() && hosted.instance.reconfigure(sample_rate, max_frames as u32).is_ok() {
                hosted.planned_latency.store(hosted.latency(), Ordering::Relaxed);
            }
            keys.push(hosted.key.clone());
        }
        drop(_loading);
        if !keys.is_empty() {
            self.emit(PluginEvent::Graph { keys });
        }
    }

    fn bury(inner: &mut Inner, key: &str, recovery: &RecoveryStore) {
        if let Some(hosted) = inner.instances.remove(key) {
            inner.by_id.remove(&hosted.instance.id());
            if hosted.control().lost.is_some() {
                recovery.restored(&hosted.identity);
            }
            inner.graveyard.push(hosted);
        }
    }

    /// Forgets that a load failed, so the next reconciliation tries again.
    pub fn forget_failures(&self) {
        self.inner().failures.clear();
    }

    /// The plug-ins of the reconciled project that are not hosted: `(key, name, reason)`.
    pub fn failures(&self) -> Vec<(String, String, String)> {
        self.inner().failures.iter().map(|(key, failure)| (key.clone(), failure.name.clone(), failure.reason.clone())).collect()
    }

    // ---- recovery ------------------------------------------------------------------------

    /// Restores the lost instances of the plug-ins with these recovery keys (all lost ones
    /// when `keys` is `None`): the container is started again and each instance gets its last
    /// state back. Returns `(name, error)` for those that could not be restored.
    pub fn restore(&self, keys: Option<&[String]>) -> Vec<(String, String)> {
        let _loading = self.loading.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let lost: Vec<Arc<Hosted>> = self.all().into_iter().filter(|hosted| hosted.lost().is_some() && keys.is_none_or(|keys| keys.contains(&hosted.identity.key()))).collect();
        let mut errors = Vec::new();
        let mut changed = Vec::new();
        for hosted in lost {
            match hosted.instance.restore() {
                Ok(()) => {
                    hosted.control().lost = None;
                    hosted.pedals_reset();
                    self.recovery.restored(&hosted.identity);
                    let _ = hosted.instance.refresh_info();
                    let _ = hosted.refresh_parameters();
                    hosted.planned_latency.store(hosted.latency(), Ordering::Relaxed);
                    changed.push(hosted.key.clone());
                }
                Err(error) => errors.push((hosted.identity.display_name().to_string(), format!("could not be restored: {error}"))),
            }
        }
        drop(_loading);
        if !changed.is_empty() {
            self.emit(PluginEvent::Graph { keys: changed.clone() });
            for key in changed {
                self.emit(PluginEvent::Changed { key, what: "restored" });
            }
        }
        errors
    }

    /// All sound off in every instance: `reset` clears voices, delay lines and envelopes
    /// (on the manager's thread; the instances play their fallback for the moment that takes).
    pub fn panic(&self) {
        self.panic.store(true, Ordering::Release);
    }

    /// Ends every container. Instances still referred to become closed and play their fallback.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(supervisor) = &self.supervisor {
            supervisor.shutdown();
        }
    }

    // ---- the manager's thread ------------------------------------------------------------

    fn run(manager: Weak<PluginManager>, events: Receiver<HostEvent>, stop: Arc<AtomicBool>) {
        let mut pending: Vec<HostEvent> = Vec::new();
        while !stop.load(Ordering::Acquire) {
            match events.recv_timeout(Duration::from_millis(30)) {
                Ok(event) => pending.push(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            pending.extend(events.try_iter());
            let Some(manager) = manager.upgrade() else { break };
            manager.handle(std::mem::take(&mut pending));
            manager.apply_format();
            if manager.panic.swap(false, Ordering::AcqRel) {
                for hosted in manager.all() {
                    if hosted.lost().is_none() {
                        let _ = hosted.instance.reset();
                    }
                }
            }
            // Buried instances nothing refers to any more are destroyed here.
            let dead: Vec<Arc<Hosted>> = {
                let mut inner = manager.inner();
                let (dead, alive): (Vec<_>, Vec<_>) = std::mem::take(&mut inner.graveyard).into_iter().partition(|hosted| Arc::strong_count(hosted) == 1);
                inner.graveyard = alive;
                dead
            };
            drop(dead);
        }
    }

    fn handle(&self, events: Vec<HostEvent>) {
        // Editors and meters report values faster than anyone can read them: the last one counts.
        let mut values: Vec<(String, u64, f64)> = Vec::new();
        for event in events {
            let key_of = |instance: InstanceId| self.inner().by_id.get(&instance).cloned();
            match event {
                HostEvent::InstanceLost { instance, reason, .. } => {
                    let Some(hosted) = key_of(instance).and_then(|key| self.hosted(&key)) else { continue };
                    let name = hosted.identity.display_name().to_string();
                    let text = format!("{name} stopped working: {}. The track plays on without it; choose Review interrupted plug-ins to restore it with its settings.", crash_text(&reason));
                    let first = hosted.control().lost.replace(text.clone()).is_none();
                    if first {
                        self.recovery.lost(&hosted.identity, &text);
                        self.emit(PluginEvent::Lost { key: hosted.key.clone(), name, reason: text });
                    }
                }
                HostEvent::InstanceRestored { instance, .. } => {
                    if let Some(hosted) = key_of(instance).and_then(|key| self.hosted(&key)) {
                        if hosted.control().lost.take().is_some() {
                            self.recovery.restored(&hosted.identity);
                        }
                    }
                }
                HostEvent::RestartRequested { instance } => {
                    let Some(hosted) = key_of(instance).and_then(|key| self.hosted(&key)) else { continue };
                    if hosted.instance.deactivate().is_ok() && hosted.instance.activate().is_ok() {
                        let _ = hosted.instance.refresh_info();
                        self.latency_changed(&hosted);
                    }
                }
                HostEvent::LatencyChanged { instance, .. } => {
                    if let Some(hosted) = key_of(instance).and_then(|key| self.hosted(&key)) {
                        self.latency_changed(&hosted);
                    }
                }
                HostEvent::ParameterChanged { instance, id, value } => {
                    let Some(key) = key_of(instance) else { continue };
                    match values.iter_mut().find(|(known, address, _)| *known == key && *address == id) {
                        Some(entry) => entry.2 = value,
                        None => values.push((key, id, value)),
                    }
                }
                HostEvent::ParameterGesture { instance, id, begin } => {
                    if let Some(key) = key_of(instance) {
                        self.flush_values(&mut values);
                        self.emit(PluginEvent::Gesture { key, address: id, begin });
                    }
                }
                HostEvent::ParametersRescan { instance } => {
                    if let Some(hosted) = key_of(instance).and_then(|key| self.hosted(&key)) {
                        let _ = hosted.refresh_parameters();
                        self.emit(PluginEvent::Changed { key: hosted.key.clone(), what: "parameters" });
                    }
                }
                HostEvent::StateDirty { instance } => {
                    if let Some(key) = key_of(instance) {
                        self.emit(PluginEvent::Changed { key, what: "state" });
                    }
                }
                HostEvent::EditorClosed { instance } => {
                    if let Some(hosted) = key_of(instance).and_then(|key| self.hosted(&key)) {
                        hosted.control().editor_open = false;
                        self.emit(PluginEvent::Changed { key: hosted.key.clone(), what: "editor" });
                    }
                }
                HostEvent::Log { message, .. } if std::env::var_os("SUPERTRACKER_TRACE").is_some() => eprintln!("plug-in container: {message}"),
                _ => {}
            }
        }
        self.flush_values(&mut values);
    }

    fn flush_values(&self, values: &mut Vec<(String, u64, f64)>) {
        for (key, id, value) in values.drain(..) {
            let Some(hosted) = self.hosted(&key) else { continue };
            // Meters and other values the plug-in only reports are not edits.
            let parameters = hosted.control().parameters.clone();
            let reported_only = parameters.as_ref().and_then(|parameters| parameters.iter().find(|info| info.id == id)).is_some_and(|info| info.flags & (PARAM_READ_ONLY | PARAM_HIDDEN) != 0);
            let Some(normalized) = hosted.table().normalized(id, value) else { continue };
            if reported_only {
                continue;
            }
            hosted.remember_value(id, normalized);
            self.emit(PluginEvent::Parameter { key, address: id, normalized, value });
        }
    }

    fn latency_changed(&self, hosted: &Arc<Hosted>) {
        let latency = hosted.latency();
        if hosted.planned_latency.swap(latency, Ordering::Relaxed) != latency {
            self.emit(PluginEvent::Graph { keys: vec![hosted.key.clone()] });
            self.emit(PluginEvent::Changed { key: hosted.key.clone(), what: "latency" });
        }
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Events (notes and parameter changes) an instance accepts per block.
pub const EVENT_CAPACITY: usize = 2048;

/// The next value of [`Hosted::pedal_generation`]: never the same twice, across all instances.
fn next_pedal_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Whether a parameter can be automated, for the interface.
pub fn automatable(info: &ParameterInfo) -> bool {
    info.flags & PARAM_AUTOMATABLE != 0 && info.flags & PARAM_READ_ONLY == 0
}

/// macOS 14 and later: the host application must yield activation to the container's
/// process before that process can make its editor window key. A no-op elsewhere, and in a
/// host without a window. The AppKit call (`-[NSApplication yieldActivationToApplication:]`)
/// is made by the window layer, which installs it with [`set_activation_yielder`]; it has not
/// run on a Mac yet.
pub fn yield_activation(_container_pid: u32) {
    #[cfg(target_os = "macos")]
    {
        // st-app registers the AppKit call (see `set_activation_yielder`).
        if let Some(yielder) = ACTIVATION_YIELDER.get() {
            yielder(_container_pid);
        }
    }
}

type Yielder = Box<dyn Fn(u32) + Send + Sync>;
static ACTIVATION_YIELDER: std::sync::OnceLock<Yielder> = std::sync::OnceLock::new();

/// Installs what [`yield_activation`] calls (the window layer, on macOS).
pub fn set_activation_yielder(yielder: Yielder) {
    let _ = ACTIVATION_YIELDER.set(yielder);
}

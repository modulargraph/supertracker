//! The render plan: a mixer compiled for the audio thread.
//!
//! [`Compiler::compile`] runs on the control thread. It turns a [`GraphSpec`] into a [`Plan`]:
//! the graph's stages in a topological order as a flat list of operations, every buffer and
//! delay line allocated and touched, every automation destination resolved to an index. The
//! audio thread only ever walks that list ([`Plan::process`]); it never allocates, locks, looks
//! anything up by name, or frees.
//!
//! # The graph
//!
//! A strip is `input -> inserts -> pre-fader -> fader -> post-fader`. Its input sums the
//! strip's sources: the engine channel of a track, the track's live input
//! ([`super::spec::LineInputSpec`], read from its [`LineFeed`] once per block), the outputs of
//! strips routed into it, and sends. A live input is a source with latency (the hardware's
//! round trip), so the engine channel beside it and every other path are delayed to meet it.
//! A track's inserts are the chain of its assigned instrument followed by its own. Sends
//! and sidechains tap a strip before its inserts, before its fader or after it; a sidechain
//! ends in the auxiliary input of an insert. Every track also feeds the two built-in returns
//! (delay, reverb) after its fader, through its `delaySend` / `reverbSend`; the returns and the
//! engine's metronome bus end in the master's input. The master's post-fader signal is the
//! output.
//!
//! Gain law: a fader multiplies by its volume (0..2), 0 while muted. Pan is a balance: the
//! left channel is multiplied by `1 - pan` for a pan to the right, the right one by `1 + pan`
//! for a pan to the left, so the centre is unity on both. Routes multiply by their gain.
//!
//! Solo: with any strip soloed, an output, send or built-in send edge is audible only if it
//! lies upstream of a soloed strip's post-fader stage (what feeds it), downstream of a soloed
//! strip (the buses and returns it plays through, without unmuting their other inputs), or
//! upstream of a sidechain's source (a detector keeps hearing its key). Sidechain edges are
//! never gated. Solo closes edges; the stages before them still render, so meters, tails and
//! detectors keep running.
//!
//! Delay compensation is planned by [`super::pdc`]; edges that need it own a delay line.
//!
//! A strip can carry a tap ([`Compiler::set_taps`]): its pre-fader signal is copied, block by
//! block, into a [`StripTap`] for a recording that lines up with the song.
//!
//! # State that outlives a plan
//!
//! Every change of the mixer compiles a new plan. What must survive the swap lives in cells the
//! compiler keeps by identity and hands to each plan that needs them: insert processors (their
//! filters, tails and current parameters), the two send effects, and compensation delay lines of
//! unchanged length. A cell is an [`RtCell`]: only the plan the renderer has adopted touches its
//! contents, and the renderer adopts one plan at a time. Cells and retired plans are freed on
//! the control thread.

use super::automation::{EventQueue, Segment, BASE_VALUE, PARAM_GAIN, PARAM_PAN, PARAM_VOLUME};
use super::insert::{BlockInfo, InsertFactory, InsertProcessor, PassThrough, ProcessContext};
use super::meters::{block_peak, MeterBank};
use super::pdc::{self, DelayLine};
use super::sends::{DelayReturn, ReverbReturn};
use super::spec::{Destination, GraphSpec, RouteDestination, SendEffectsSpec, StripKind, INSERT_SLOTS};
use super::tap::{StripTap, TapBlock, NOT_PLAYING};
use crate::inputs::LineFeed;
use st_project::model::Tap;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use tracker_engine::engine::abi::{STEvent, ST_EVENT_ALL_NOTES_OFF, ST_EVENT_HOST_PARAMETER, ST_EVENT_NOTE_OFF, ST_EVENT_NOTE_ON, ST_EVENT_PARAMETER, ST_TARGET_DELAY, ST_TARGET_PAN, ST_TARGET_PLUGIN, ST_TARGET_REVERB, ST_TARGET_VOLUME};
use tracker_engine::engine::host::BlockView;

/// State owned by the render side but allocated and freed by the control thread.
pub struct RtCell<T>(UnsafeCell<T>);

// SAFETY: the content is reached only through `RtCell::get`, whose contract makes the render
// owner its single user; the control thread only moves the `Arc` around and drops it.
unsafe impl<T: Send> Sync for RtCell<T> {}

impl<T> RtCell<T> {
    pub fn new(value: T) -> RtCell<T> {
        RtCell(UnsafeCell::new(value))
    }

    /// # Safety
    /// Only the render owner may call this (the thread processing the one adopted plan), and it
    /// must not hold two references to the same cell at once.
    #[allow(clippy::mut_from_ref)]
    #[inline(always)]
    pub unsafe fn get(&self) -> &mut T {
        // SAFETY: per this function's contract.
        unsafe { &mut *self.0.get() }
    }
}

/// An insert processor shared by successive plans.
pub struct InsertCell {
    processor: RtCell<Box<dyn InsertProcessor>>,
    component: String,
    latency: u32,
    sidechain: bool,
    /// Why the insert passes its signal through instead of processing it.
    unavailable: Option<String>,
}

/// A compensation delay with a note of how long it has been fed silence.
pub struct EdgeDelay {
    line: DelayLine,
    quiet: usize,
}

/// A compensation delay shared by successive plans, with its length readable by the compiler.
pub struct DelayCell {
    frames: usize,
    state: RtCell<EdgeDelay>,
}

impl DelayCell {
    fn new(frames: usize) -> DelayCell {
        DelayCell { frames, state: RtCell::new(EdgeDelay { line: DelayLine::new(frames), quiet: usize::MAX / 2 }) }
    }
}

impl EdgeDelay {
    fn empty(&self) -> bool {
        self.quiet >= self.line.frames()
    }
}

/// The state of one insert as the interface sees it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertStatus {
    pub owner: String,
    pub slot: usize,
    pub instrument_chain: bool,
    #[serde(rename = "instanceID")]
    pub instance_id: String,
    pub component: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub latency_frames: u32,
    /// The track's instrument plug-in rather than an insert effect.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub instrument: bool,
    /// The insert reads a sidechain input when one is routed to it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sidechain: bool,
}

#[derive(Default)]
struct Buffer {
    left: Box<[f32]>,
    right: Box<[f32]>,
    /// The buffer's content is to be read as silence (and may be stale).
    silent: bool,
}

impl Buffer {
    fn new(frames: usize) -> Buffer {
        let mut buffer = Buffer { left: vec![0.0; frames].into_boxed_slice(), right: vec![0.0; frames].into_boxed_slice(), silent: true };
        // Touch every page now, so the audio thread never takes the first-use page fault.
        for channel in [&mut buffer.left, &mut buffer.right] {
            for sample in channel.iter_mut().step_by(512) {
                // SAFETY: a valid, exclusive reference; the volatile write only defeats elision.
                unsafe { std::ptr::write_volatile(sample, 0.0) };
            }
        }
        buffer
    }
}

#[derive(Clone, Copy)]
enum Source {
    Buffer(usize),
    EngineTrack(usize),
    /// The engine's return bus: the metronome and count-in clicks.
    EngineReturn,
    /// A live input (index into the plan's lines).
    Line(usize),
}

/// A live input of the plan: its feed and the block read from it.
struct PlanLine {
    feed: Arc<LineFeed>,
    block: Buffer,
}

/// A tap of the plan and how many frames the tapped signal lags the engine.
struct PlanTap {
    tap: Arc<StripTap>,
    arrival: u32,
}

struct Edge {
    source: Source,
    delay: Option<Arc<DelayCell>>,
    gain: f32,
    default_gain: f32,
    /// False while solo elsewhere silences this edge.
    open: bool,
    /// Automation target, or `u32::MAX`.
    target: u32,
}

struct StripState {
    volume: f32,
    pan: f32,
    default_volume: f32,
    default_pan: f32,
    mute: bool,
}

struct PlanInsert {
    cell: Arc<InsertCell>,
    bypass: bool,
    /// The values the insert starts from and returns to when automation ends.
    base: Vec<(u64, f64)>,
    target: u32,
    /// A bypassed insert that reports latency still delays by it, so the paths the plan
    /// aligned stay aligned.
    bypass_delay: Option<Arc<DelayCell>>,
}

enum Op {
    /// Sum the edges into a buffer.
    Sum { buffer: usize, edges: Range<usize> },
    Insert { buffer: usize, copy_from: Option<usize>, delay: Option<Arc<DelayCell>>, insert: usize, sidechain: Option<usize> },
    /// `tap`: copy `from` (the strip before its fader) to this tap of the plan first.
    Fader { from: usize, to: usize, strip: usize, tap: Option<usize> },
    Delay { buffer: usize },
    Reverb { buffer: usize },
}

const NO_TARGET: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct TrackTargets {
    strip: u32,
    delay_send: u32,
    reverb_send: u32,
    /// The track's own inserts by slot.
    inserts: [u32; INSERT_SLOTS],
    /// Index (in the plan's inserts) of the track's instrument plug-in, or `u32::MAX`.
    instrument: u32,
}

#[derive(Clone, Copy)]
enum DestinationTarget {
    None,
    Mix(u32),
    Route(u32),
    Insert(u32),
}

/// A compiled mixer. See the module documentation.
pub struct Plan {
    /// Counts the plans of one compiler; a newer plan has a larger serial.
    pub serial: u64,
    /// Identity of the host automation numbering this plan resolves.
    pub host_map: u64,
    /// Frames by which the output lags the engine (the largest arrival of the latency plan).
    pub latency_frames: u64,
    max_frames: usize,
    sample_rate: f64,
    ops: Vec<Op>,
    edges: Vec<Edge>,
    strips: Vec<StripState>,
    inserts: Vec<PlanInsert>,
    lines: Vec<PlanLine>,
    strip_taps: Vec<PlanTap>,
    /// Graph time of the block being processed, and the song frame the engine rendered there
    /// ([`NOT_PLAYING`] while the transport does not play): what a tap stamps its blocks with.
    block_start: u64,
    song_frame: i64,
    buffers: Vec<Buffer>,
    scratch: Buffer,
    events: EventQueue,
    target_offsets: Vec<u64>,
    track_targets: Vec<TrackTargets>,
    destinations: Vec<DestinationTarget>,
    master_buffer: usize,
    /// Per strip (in the spec's order): its post-fader buffer and that signal's arrival (how
    /// many frames it lags the engine). For offline stem capture, see [`Plan::strip_output`].
    taps: Vec<(usize, u64)>,
    meters: Arc<MeterBank>,
    delay: Arc<RtCell<DelayReturn>>,
    reverb: Arc<RtCell<ReverbReturn>>,
    effects: SendEffectsSpec,
    bpm: f64,
    block: BlockInfo,
}

/// Events a plan can hold per block and waiting for later blocks.
const EVENT_CAPACITY: usize = 16_384;

/// Compiles plans and keeps what successive plans share. Control thread only.
pub struct Compiler {
    factory: Arc<dyn InsertFactory>,
    sample_rate: f64,
    max_frames: usize,
    serial: u64,
    inserts: HashMap<String, Arc<InsertCell>>,
    delays: HashMap<String, Arc<DelayCell>>,
    delay: Arc<RtCell<DelayReturn>>,
    reverb: Arc<RtCell<ReverbReturn>>,
    report: Vec<InsertStatus>,
    unavailable_routes: Vec<(String, String)>,
    /// Automation target of each insert of the plan compiled last, by insert key.
    insert_targets: HashMap<String, u32>,
    /// Parameter values set live (not yet in the project's saved state), by insert key.
    live_values: HashMap<String, Vec<(u64, f64)>>,
    /// Taps by strip owner: every plan compiled while they are set carries them.
    taps: Vec<(String, Arc<StripTap>)>,
}

#[derive(Clone, Copy, PartialEq)]
enum EdgeKind {
    Native(usize),
    /// A track strip's live input (the strip's index).
    Line(usize),
    Metronome,
    Chain,
    Fader,
    Output(usize),
    Send(usize),
    Sidechain(usize),
    /// A track's send to a built-in return: (strip, 0 delay / 1 reverb).
    Builtin(usize, usize),
    SystemProcess,
    SystemOutput(usize),
}

impl Compiler {
    pub fn new(factory: Arc<dyn InsertFactory>, sample_rate: f64, max_frames: usize) -> Compiler {
        Compiler {
            factory,
            sample_rate,
            max_frames,
            serial: 0,
            inserts: HashMap::new(),
            delays: HashMap::new(),
            delay: Arc::new(RtCell::new(DelayReturn::new(sample_rate))),
            reverb: Arc::new(RtCell::new(ReverbReturn::new(sample_rate))),
            report: Vec::new(),
            unavailable_routes: Vec::new(),
            insert_targets: HashMap::new(),
            live_values: HashMap::new(),
            taps: Vec::new(),
        }
    }

    /// The taps the plans compiled from now on carry: each strip owner (`track:<id>`, ...) with
    /// the tap its pre-fader signal is copied to. A strip that does not exist has no tap. The
    /// plan in use is not changed: compile and publish one.
    pub fn set_taps(&mut self, taps: Vec<(String, Arc<StripTap>)>) {
        self.taps = taps;
    }

    /// The serial of the plan compiled last.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// Replaces the factory (the plug-in integration installs its own once it exists).
    /// Processors already made stay as they are.
    pub fn set_factory(&mut self, factory: Arc<dyn InsertFactory>) {
        self.factory = factory;
    }

    /// Forgets the processor kept for an insert, so that the next compile asks the factory
    /// again: the insert's latency, sidechain input or availability changed.
    pub fn invalidate_insert(&mut self, key: &str) {
        self.inserts.remove(key);
    }

    /// Remembers a parameter value set outside the project (a knob being turned) so that later
    /// plans start from it, and returns the insert's automation target in the plan compiled last.
    pub fn set_live_value(&mut self, key: &str, address: u64, normalized: f64) -> Option<u32> {
        let values = self.live_values.entry(key.to_string()).or_default();
        match values.iter_mut().find(|(known, _)| *known == address) {
            Some(entry) => entry.1 = normalized,
            None => values.push((address, normalized)),
        }
        self.insert_targets.get(key).copied()
    }

    /// Forgets the live values of one insert (a state was loaded into it).
    pub fn clear_live_values_of(&mut self, key: &str) {
        self.live_values.remove(key);
    }

    /// Forgets live values (the project's saved states are current again).
    pub fn clear_live_values(&mut self) {
        self.live_values.clear();
    }

    /// The base parameter values of an insert of the plan compiled last: `(address, normalized)`.
    pub fn insert_values(&self, insert: &super::spec::InsertSpec) -> Vec<(u64, f64)> {
        let mut base = self.factory.base_parameters(insert).unwrap_or_default();
        for (address, value) in self.live_values.get(&insert.key).into_iter().flatten() {
            match base.iter_mut().find(|(known, _)| known == address) {
                Some(entry) => entry.1 = *value,
                None => base.push((*address, *value)),
            }
        }
        base
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// The inserts of the plan compiled last.
    pub fn insert_report(&self) -> &[InsertStatus] {
        &self.report
    }

    /// Sidechains of the plan compiled last that could not be connected: `(route id, reason)`.
    pub fn unavailable_routes(&self) -> &[(String, String)] {
        &self.unavailable_routes
    }

    fn insert_cell(&mut self, insert: &super::spec::InsertSpec) -> Arc<InsertCell> {
        if let Some(cell) = self.inserts.get(&insert.key).filter(|cell| cell.component == insert.component) {
            return cell.clone();
        }
        let (mut processor, unavailable): (Box<dyn InsertProcessor>, _) = match self.factory.create(insert, self.sample_rate) {
            Ok(processor) => (processor, None),
            Err(reason) => (Box::new(PassThrough), Some(reason)),
        };
        processor.prepare(self.sample_rate, self.max_frames);
        let cell = Arc::new(InsertCell {
            latency: processor.latency(),
            sidechain: processor.accepts_sidechain(),
            processor: RtCell::new(processor),
            component: insert.component.clone(),
            unavailable,
        });
        self.inserts.insert(insert.key.clone(), cell.clone());
        cell
    }

    /// Compiles `spec`. Fails for routing that cannot be rendered (a loop, or more latency than
    /// ten seconds); the compiler's shared state is unchanged then.
    pub fn compile(&mut self, spec: &GraphSpec) -> Result<Box<Plan>, String> {
        let strip_index: HashMap<&str, usize> = spec.strips.iter().enumerate().map(|(index, strip)| (strip.id.as_str(), index)).collect();
        let master = spec.strips.len() - 1;

        // Inserts, in strip order then chain order.
        let mut inserts: Vec<PlanInsert> = Vec::new();
        let mut report = Vec::new();
        let mut strip_inserts: Vec<Range<usize>> = Vec::with_capacity(spec.strips.len());
        let mut live_keys: Vec<&str> = Vec::new();
        for strip in &spec.strips {
            let first = inserts.len();
            for insert in &strip.inserts {
                let cell = self.insert_cell(insert);
                live_keys.push(&insert.key);
                let base = if cell.unavailable.is_none() { self.insert_values(insert) } else { Vec::new() };
                report.push(InsertStatus {
                    owner: strip.id.clone(),
                    slot: insert.slot,
                    instrument_chain: insert.instrument_chain,
                    instance_id: insert.instance_id.clone(),
                    component: insert.component.clone(),
                    available: cell.unavailable.is_none(),
                    reason: cell.unavailable.clone(),
                    latency_frames: cell.latency,
                    instrument: insert.instrument.is_some(),
                    sidechain: cell.sidechain,
                });
                inserts.push(PlanInsert { cell, bypass: insert.bypass, base, target: NO_TARGET, bypass_delay: None });
            }
            strip_inserts.push(first..inserts.len());
        }

        // Nodes. Per strip: input, one per insert, pre-fader, post-fader.
        let mut latencies: Vec<u64> = Vec::new();
        let mut node = |latency: u64| -> usize {
            latencies.push(latency);
            latencies.len() - 1
        };
        struct StripNodes {
            input: usize,
            inserts: Range<usize>,
            pre: usize,
            post: usize,
            source: Option<usize>,
            /// The live input: a source as late as the hardware's round trip.
            line: Option<usize>,
        }
        let mut nodes: Vec<StripNodes> = Vec::with_capacity(spec.strips.len());
        // The live inputs, in strip order: (strip, feed).
        let mut lines: Vec<(usize, Arc<LineFeed>)> = Vec::new();
        for (index, strip) in spec.strips.iter().enumerate() {
            let source = matches!(strip.kind, StripKind::Track(_)).then(|| node(0));
            let line = strip.line_input.as_ref().filter(|_| source.is_some()).map(|input| {
                lines.push((index, input.feed.0.clone()));
                node(input.latency_frames as u64)
            });
            let input = node(0);
            let first = input + 1;
            for insert in strip_inserts[index].clone() {
                node(inserts[insert].cell.latency as u64);
            }
            let insert_nodes = first..first + strip_inserts[index].len();
            nodes.push(StripNodes { input, inserts: insert_nodes, pre: node(0), post: node(0), source, line });
        }
        let system = [(node(0), node(0)), (node(0), node(0))];
        let metronome = node(0);
        let node_count = latencies.len();

        // Edges.
        let mut graph_edges: Vec<(usize, usize)> = Vec::new();
        let mut kinds: Vec<EdgeKind> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut edge = |source: usize, destination: usize, kind: EdgeKind, name: String| {
            graph_edges.push((source, destination));
            kinds.push(kind);
            names.push(name);
        };
        let tap_node = |strip: usize, tap: Tap| match tap {
            Tap::PreInserts => nodes[strip].input,
            Tap::PreFader => nodes[strip].pre,
            Tap::PostFader => nodes[strip].post,
        };
        for (index, strip) in spec.strips.iter().enumerate() {
            let stages = &nodes[index];
            if let (Some(source), StripKind::Track(track)) = (stages.source, &strip.kind) {
                edge(source, stages.input, EdgeKind::Native(*track), format!("native:{}", strip.id));
            }
            if let Some(line) = stages.line {
                edge(line, stages.input, EdgeKind::Line(index), format!("line:{}", strip.id));
            }
            let mut previous = stages.input;
            for (insert_node, insert) in stages.inserts.clone().zip(&strip.inserts) {
                edge(previous, insert_node, EdgeKind::Chain, format!("insert:{}", insert.key));
                previous = insert_node;
            }
            edge(previous, stages.pre, EdgeKind::Chain, format!("preFader:{}", strip.id));
            edge(stages.pre, stages.post, EdgeKind::Fader, format!("fader:{}", strip.id));
            if let Some(destination) = strip.output.as_deref().and_then(|owner| strip_index.get(owner)).filter(|destination| **destination != index) {
                edge(stages.post, nodes[*destination].input, EdgeKind::Output(index), format!("output:{}", strip.id));
            }
            if matches!(strip.kind, StripKind::Track(_)) {
                edge(stages.post, system[0].0, EdgeKind::Builtin(index, 0), format!("builtin:delay:{}", strip.id));
                edge(stages.post, system[1].0, EdgeKind::Builtin(index, 1), format!("builtin:reverb:{}", strip.id));
            }
        }
        for (index, send) in spec.sends.iter().enumerate().filter(|(_, send)| send.enabled) {
            let RouteDestination::Strip(destination) = &send.destination else { continue };
            let (Some(source), Some(destination)) = (strip_index.get(send.source.as_str()), strip_index.get(destination.as_str())) else { continue };
            if *source != master {
                edge(tap_node(*source, send.tap), nodes[*destination].input, EdgeKind::Send(index), format!("send:{}", send.id));
            }
        }
        // Sidechains end in an insert node; `sidechain_of[insert]` says whether one arrives.
        let mut unavailable_routes = Vec::new();
        let mut keyed = vec![false; inserts.len()];
        for (index, sidechain) in spec.sidechains.iter().enumerate() {
            let RouteDestination::Insert(instance) = &sidechain.destination else { continue };
            let Some(source) = strip_index.get(sidechain.source.as_str()).filter(|source| **source != master) else {
                unavailable_routes.push((sidechain.id.clone(), "The sidechain source is unavailable.".to_string()));
                continue;
            };
            let found = spec.strips.iter().enumerate().find_map(|(strip, owner)| {
                let position = owner.inserts.iter().position(|insert| !insert.instance_id.is_empty() && insert.instance_id == *instance && !insert.instrument_chain)?;
                Some((strip, position))
            });
            let Some((strip, position)) = found else {
                unavailable_routes.push((sidechain.id.clone(), "The destination plug-in is unavailable.".to_string()));
                continue;
            };
            let insert = strip_inserts[strip].start + position;
            if !inserts[insert].cell.sidechain {
                unavailable_routes.push((sidechain.id.clone(), "The destination has no sidechain input.".to_string()));
                continue;
            }
            if sidechain.enabled {
                keyed[insert] = true;
                edge(tap_node(*source, sidechain.tap), nodes[strip].inserts.start + position, EdgeKind::Sidechain(index), format!("sidechain:{}", sidechain.id));
            }
        }
        for (which, (input, process)) in system.iter().enumerate() {
            edge(*input, *process, EdgeKind::SystemProcess, format!("system:{which}/processor"));
            edge(*process, nodes[master].input, EdgeKind::SystemOutput(which), format!("system:{which}/master"));
        }
        edge(metronome, nodes[master].input, EdgeKind::Metronome, "metronome".to_string());

        let capacity = (self.sample_rate * 10.0) as u64;
        let latency = pdc::plan(&latencies, &graph_edges, capacity)?;

        // Solo.
        let audible = solo_edges(spec, &nodes.iter().map(|stages| (stages.input, stages.pre, stages.post)).collect::<Vec<_>>(), &graph_edges, &kinds, node_count);

        // Buffers.
        let mut input_tapped = vec![false; spec.strips.len()];
        let mut pre_tapped = vec![false; spec.strips.len()];
        for (send, _) in spec.sends.iter().chain(&spec.sidechains).map(|route| (route, ())) {
            if let Some(source) = strip_index.get(send.source.as_str()) {
                match send.tap {
                    Tap::PreInserts => input_tapped[*source] = true,
                    Tap::PreFader => pre_tapped[*source] = true,
                    Tap::PostFader => {}
                }
            }
        }
        let mut buffer_count = 0usize;
        let mut buffer = || {
            buffer_count += 1;
            buffer_count - 1
        };
        // (input, work, post) per strip.
        let strip_buffers: Vec<(usize, usize, usize)> = (0..spec.strips.len())
            .map(|index| {
                let input = buffer();
                let work = if !strip_inserts[index].is_empty() && input_tapped[index] { buffer() } else { input };
                let post = if pre_tapped[index] || (input_tapped[index] && work == input) { buffer() } else { work };
                (input, work, post)
            })
            .collect();
        let system_buffers = [buffer(), buffer()];
        let sidechain_buffers: Vec<Option<usize>> = keyed.iter().map(|keyed| keyed.then(&mut buffer)).collect();

        // Targets: strips, then automatable edges, then inserts.
        let strip_count = spec.strips.len();
        let mut target_offsets: Vec<u64> = nodes.iter().map(|stages| latency.input_arrival[stages.post]).collect();
        let mut edge_targets: Vec<u32> = vec![NO_TARGET; graph_edges.len()];
        for (index, kind) in kinds.iter().enumerate() {
            if matches!(kind, EdgeKind::Send(_) | EdgeKind::Sidechain(_) | EdgeKind::Builtin(..)) {
                edge_targets[index] = target_offsets.len() as u32;
                target_offsets.push(latency.input_arrival[graph_edges[index].1]);
            }
        }
        for (strip, range) in strip_inserts.iter().enumerate() {
            for (position, insert) in range.clone().enumerate() {
                inserts[insert].target = target_offsets.len() as u32;
                target_offsets.push(latency.input_arrival[nodes[strip].inserts.start + position]);
            }
        }

        // Operations, in the latency plan's topological order.
        let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        for (index, (_, destination)) in graph_edges.iter().enumerate() {
            incoming[*destination].push(index);
        }
        let mut node_role: HashMap<usize, (usize, usize)> = HashMap::new(); // node -> (strip, role): 0 input, 1 post, 2+k insert k
        for (strip, stages) in nodes.iter().enumerate() {
            node_role.insert(stages.input, (strip, 0));
            node_role.insert(stages.post, (strip, 1));
            for (position, insert_node) in stages.inserts.clone().enumerate() {
                node_role.insert(insert_node, (strip, 2 + position));
            }
        }
        let mut delays: HashMap<String, Arc<DelayCell>> = HashMap::new();
        for (strip, range) in strip_inserts.iter().enumerate() {
            for (insert, description) in range.clone().zip(&spec.strips[strip].inserts) {
                let frames = inserts[insert].cell.latency as usize;
                if description.bypass && frames > 0 {
                    let name = format!("bypass:{}", description.key);
                    let cell = match self.delays.get(&name) {
                        Some(cell) if cell.frames == frames => cell.clone(),
                        _ => Arc::new(DelayCell::new(frames)),
                    };
                    delays.insert(name, cell.clone());
                    inserts[insert].bypass_delay = Some(cell);
                }
            }
        }
        let mut delay_for = |index: usize| -> Option<Arc<DelayCell>> {
            let frames = latency.edge_delays[index] as usize;
            if frames == 0 {
                return None;
            }
            let name = &names[index];
            let cell = match self.delays.get(name) {
                Some(cell) if cell.frames == frames => cell.clone(),
                _ => Arc::new(DelayCell::new(frames)),
            };
            delays.insert(name.clone(), cell.clone());
            Some(cell)
        };
        let mut ops: Vec<Op> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut strip_taps: Vec<PlanTap> = Vec::new();
        let mut route_targets: HashMap<&str, u32> = HashMap::new();
        let mut builtin_targets = vec![[NO_TARGET; 2]; strip_count];
        let mut sum = |ops: &mut Vec<Op>, edges: &mut Vec<Edge>, buffer: usize, list: &[usize], delay_for: &mut dyn FnMut(usize) -> Option<Arc<DelayCell>>| {
            let first = edges.len();
            for &index in list {
                let (source, gain) = match kinds[index] {
                    EdgeKind::Native(track) => (Source::EngineTrack(track), 1.0),
                    EdgeKind::Line(strip) => {
                        let Some(line) = lines.iter().position(|(owner, _)| *owner == strip) else { continue };
                        (Source::Line(line), spec.strips[strip].line_input.as_ref().map_or(0.0, |input| input.gain))
                    }
                    EdgeKind::Metronome => (Source::EngineReturn, 1.0),
                    EdgeKind::Output(strip) => (Source::Buffer(strip_buffers[strip].2), 1.0),
                    EdgeKind::Send(send) | EdgeKind::Sidechain(send) => {
                        let route = if matches!(kinds[index], EdgeKind::Send(_)) { &spec.sends[send] } else { &spec.sidechains[send] };
                        let strip = strip_index[route.source.as_str()];
                        let buffers = strip_buffers[strip];
                        let buffer = match route.tap {
                            Tap::PreInserts => buffers.0,
                            Tap::PreFader => buffers.1,
                            Tap::PostFader => buffers.2,
                        };
                        route_targets.insert(route.id.as_str(), edge_targets[index]);
                        (Source::Buffer(buffer), route.gain)
                    }
                    EdgeKind::Builtin(strip, which) => {
                        builtin_targets[strip][which] = edge_targets[index];
                        (Source::Buffer(strip_buffers[strip].2), if which == 0 { spec.strips[strip].delay_send } else { spec.strips[strip].reverb_send })
                    }
                    EdgeKind::SystemOutput(which) => (Source::Buffer(system_buffers[which]), if which == 0 { spec.effects.delay_mix } else { spec.effects.reverb_mix }),
                    EdgeKind::Chain | EdgeKind::Fader | EdgeKind::SystemProcess => continue,
                };
                edges.push(Edge { source, delay: delay_for(index), gain, default_gain: gain, open: audible[index], target: edge_targets[index] });
            }
            ops.push(Op::Sum { buffer, edges: first..edges.len() });
        };
        for &current in &latency.order {
            if let Some(&(strip, role)) = node_role.get(&current) {
                let (input, work, post) = strip_buffers[strip];
                match role {
                    0 => sum(&mut ops, &mut edges, input, &incoming[current], &mut delay_for),
                    1 => {
                        let tap = self.taps.iter().find(|(owner, _)| *owner == spec.strips[strip].id).map(|(_, tap)| {
                            // What the strip's signal lags the engine by before its fader.
                            strip_taps.push(PlanTap { tap: tap.clone(), arrival: latency.arrival[nodes[strip].pre].min(u32::MAX as u64) as u32 });
                            strip_taps.len() - 1
                        });
                        ops.push(Op::Fader { from: work, to: post, strip, tap });
                    }
                    _ => {
                        let position = role - 2;
                        let insert = strip_inserts[strip].start + position;
                        let keys: Vec<usize> = incoming[current].iter().copied().filter(|index| matches!(kinds[*index], EdgeKind::Sidechain(_))).collect();
                        if let Some(buffer) = sidechain_buffers[insert] {
                            sum(&mut ops, &mut edges, buffer, &keys, &mut delay_for);
                        }
                        let chain = incoming[current].iter().copied().find(|index| kinds[*index] == EdgeKind::Chain);
                        ops.push(Op::Insert {
                            buffer: work,
                            copy_from: (position == 0 && work != input).then_some(input),
                            delay: chain.and_then(&mut delay_for),
                            insert,
                            sidechain: sidechain_buffers[insert],
                        });
                    }
                }
            } else if let Some(which) = system.iter().position(|(input, _)| *input == current) {
                sum(&mut ops, &mut edges, system_buffers[which], &incoming[current], &mut delay_for);
                ops.push(if which == 0 { Op::Delay { buffer: system_buffers[0] } } else { Op::Reverb { buffer: system_buffers[1] } });
            }
        }

        // Where engine events land.
        let track_count = spec.strips.iter().filter_map(|strip| if let StripKind::Track(track) = strip.kind { Some(track + 1) } else { None }).max().unwrap_or(0);
        let mut track_targets = vec![TrackTargets { strip: NO_TARGET, delay_send: NO_TARGET, reverb_send: NO_TARGET, inserts: [NO_TARGET; INSERT_SLOTS], instrument: NO_TARGET }; track_count];
        for (index, strip) in spec.strips.iter().enumerate() {
            let StripKind::Track(track) = strip.kind else { continue };
            let targets = &mut track_targets[track];
            targets.strip = index as u32;
            targets.delay_send = builtin_targets[index][0];
            targets.reverb_send = builtin_targets[index][1];
            for (insert, description) in strip_inserts[index].clone().zip(&strip.inserts) {
                if description.instrument.is_some() {
                    targets.instrument = insert as u32;
                } else if !description.instrument_chain && description.slot < INSERT_SLOTS {
                    targets.inserts[description.slot] = inserts[insert].target;
                }
            }
        }
        let insert_targets: HashMap<&str, u32> =
            spec.strips.iter().enumerate().flat_map(|(strip, owner)| owner.inserts.iter().zip(strip_inserts[strip].clone())).map(|(insert, index)| (insert.key.as_str(), inserts[index].target)).collect();
        let destinations = spec
            .destinations
            .iter()
            .map(|destination| match destination {
                Destination::Mix(owner) => strip_index.get(owner.as_str()).map_or(DestinationTarget::None, |strip| DestinationTarget::Mix(*strip as u32)),
                Destination::Route(id) => route_targets.get(id.as_str()).filter(|target| **target != NO_TARGET).map_or(DestinationTarget::None, |target| DestinationTarget::Route(*target)),
                Destination::Insert(key) => insert_targets.get(key.as_str()).map_or(DestinationTarget::None, |target| DestinationTarget::Insert(*target)),
            })
            .collect();

        // Everything succeeded: keep what the next compile may reuse, drop the rest.
        self.inserts.retain(|key, _| live_keys.contains(&key.as_str()));
        self.delays = delays;
        self.report = report;
        self.unavailable_routes = unavailable_routes;
        self.insert_targets = insert_targets.iter().map(|(key, target)| (key.to_string(), *target)).collect();
        self.live_values.retain(|key, _| live_keys.contains(&key.as_str()));
        self.serial += 1;
        Ok(Box::new(Plan {
            serial: self.serial,
            host_map: spec.host_map,
            latency_frames: latency.arrival[nodes[master].post],
            max_frames: self.max_frames,
            sample_rate: self.sample_rate,
            ops,
            edges,
            strips: spec.strips.iter().map(|strip| StripState { volume: strip.volume, pan: strip.pan, default_volume: strip.volume, default_pan: strip.pan, mute: strip.mute }).collect(),
            inserts,
            lines: lines.into_iter().map(|(_, feed)| PlanLine { feed, block: Buffer::new(self.max_frames) }).collect(),
            strip_taps,
            block_start: 0,
            song_frame: NOT_PLAYING,
            buffers: (0..buffer_count).map(|_| Buffer::new(self.max_frames)).collect(),
            scratch: Buffer::new(self.max_frames),
            events: EventQueue::new(target_offsets.len(), EVENT_CAPACITY),
            target_offsets,
            track_targets,
            destinations,
            master_buffer: strip_buffers[master].2,
            taps: (0..spec.strips.len()).map(|strip| (strip_buffers[strip].2, latency.arrival[nodes[strip].post])).collect(),
            meters: Arc::new(MeterBank::new(spec.strips.iter().map(|strip| strip.id.clone()).collect())),
            delay: self.delay.clone(),
            reverb: self.reverb.clone(),
            effects: spec.effects,
            bpm: spec.bpm,
            block: BlockInfo::default(),
        }))
    }
}

/// Which edges stay audible under the spec's solo state (all of them when nothing is soloed).
fn solo_edges(spec: &GraphSpec, stages: &[(usize, usize, usize)], edges: &[(usize, usize)], kinds: &[EdgeKind], node_count: usize) -> Vec<bool> {
    if !spec.strips.iter().any(|strip| strip.solo) {
        return vec![true; edges.len()];
    }
    let audio = |index: usize| !matches!(kinds[index], EdgeKind::Sidechain(_));
    let mut marked = vec![false; edges.len()];
    let mut upstream = vec![false; node_count];
    let mut downstream = vec![false; node_count];
    for (strip, (input, pre, post)) in spec.strips.iter().zip(stages) {
        if strip.solo {
            upstream[*post] = true;
            // Downstream starts at every stage of a soloed strip, so its pre-insert and
            // pre-fader sends stay audible like its post-fader ones.
            downstream[*input] = true;
            downstream[*pre] = true;
            downstream[*post] = true;
        }
    }
    let mut detectors = vec![false; node_count];
    for (index, (source, _)) in edges.iter().enumerate() {
        if !audio(index) {
            detectors[*source] = true;
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (index, (source, destination)) in edges.iter().enumerate() {
            if audio(index) && upstream[*destination] {
                marked[index] = true;
                changed |= !std::mem::replace(&mut upstream[*source], true);
            }
            if audio(index) && downstream[*source] {
                marked[index] = true;
                changed |= !std::mem::replace(&mut downstream[*destination], true);
            }
            // A detector keeps its whole computational ancestry, through sidechains too.
            if detectors[*destination] {
                marked[index] = true;
                changed |= !std::mem::replace(&mut detectors[*source], true);
            }
        }
    }
    // Only audible boundaries are gated; everything inside a strip always runs.
    kinds.iter().zip(marked).map(|(kind, marked)| marked || !matches!(kind, EdgeKind::Output(_) | EdgeKind::Send(_) | EdgeKind::Builtin(..))).collect()
}

/// `dest[from..to] += source[from..to] * gain`, treating a silent `dest` as zeros.
#[inline]
fn mix_into(dest: &mut Buffer, source: (&[f32], &[f32]), from: usize, to: usize, gain: f32, frames: usize) {
    if gain == 0.0 || from >= to {
        return;
    }
    let (source_l, source_r) = (&source.0[from..to], &source.1[from..to]);
    if dest.silent {
        dest.silent = false;
        if from == 0 && to == frames {
            if gain == 1.0 {
                dest.left[..frames].copy_from_slice(source_l);
                dest.right[..frames].copy_from_slice(source_r);
            } else {
                for (out, sample) in dest.left[..frames].iter_mut().zip(source_l) {
                    *out = *sample * gain;
                }
                for (out, sample) in dest.right[..frames].iter_mut().zip(source_r) {
                    *out = *sample * gain;
                }
            }
            return;
        }
        dest.left[..frames].fill(0.0);
        dest.right[..frames].fill(0.0);
    }
    for (out, sample) in dest.left[from..to].iter_mut().zip(source_l) {
        *out += *sample * gain;
    }
    for (out, sample) in dest.right[from..to].iter_mut().zip(source_r) {
        *out += *sample * gain;
    }
}

/// Delays `work` by the line's length in place; a silent buffer stays silent once the line is empty.
fn delay_in_place(delay: &mut EdgeDelay, work: &mut Buffer, scratch: &mut Buffer, frames: usize) {
    if !work.silent {
        scratch.left[..frames].copy_from_slice(&work.left[..frames]);
        scratch.right[..frames].copy_from_slice(&work.right[..frames]);
        delay.line.process(&scratch.left[..frames], &scratch.right[..frames], &mut work.left[..frames], &mut work.right[..frames]);
        delay.quiet = 0;
    } else if !delay.empty() {
        delay.line.process_silence(&mut work.left[..frames], &mut work.right[..frames]);
        delay.quiet = delay.quiet.saturating_add(frames);
        work.silent = false;
    }
}

impl Plan {
    /// The largest block [`Plan::process`] accepts.
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    pub fn meters(&self) -> &Arc<MeterBank> {
        &self.meters
    }

    /// The post-fader signal of strip `strip` (the spec's order) as the last [`Plan::process`]
    /// left it, and its arrival: how many frames it lags the engine. The signal is `None`
    /// while the strip is silent. An offline render captures stems here, after the fader and
    /// before the strip's output bus; the render thread owns the plan, so this is for the
    /// thread that drives the renderer.
    pub fn strip_output(&self, strip: usize) -> Option<(Option<(&[f32], &[f32])>, u64)> {
        let (buffer, arrival) = *self.taps.get(strip)?;
        let buffer = self.buffers.get(buffer)?;
        Some(((!buffer.silent).then_some((&buffer.left[..], &buffer.right[..])), arrival))
    }

    /// Automation events dropped by this plan's queue.
    pub fn dropped_events(&self) -> u64 {
        self.events.dropped()
    }

    /// Render owner, when the plan becomes the one in use: gives every insert its bypass state
    /// and base parameters.
    pub fn adopt(&mut self) {
        self.apply_insert_bases();
    }

    fn apply_insert_bases(&mut self) {
        for insert in &self.inserts {
            // SAFETY: the render owner is adopting this plan; no other reference to the cell's
            // processor exists (the previous plan is no longer processed).
            let processor = unsafe { insert.cell.processor.get() };
            processor.set_bypass(insert.bypass);
            for (address, value) in &insert.base {
                processor.base_parameter(*address, *value);
            }
        }
    }

    /// Render owner: playback started or moved (a seek). The engine returns its own
    /// automated targets to the project's values at those moments; the inserts return to
    /// their base parameters here, so a value a lane or a step lock left behind does not
    /// reach into the next stretch of playback. Lanes that apply from the new position
    /// follow as events of the same block. A stop is not such a moment: what rings out
    /// after it keeps the values the song left.
    pub fn playback_begins(&mut self) {
        self.apply_insert_bases();
    }

    /// Render owner: every automated value back to its default and waiting events forgotten.
    /// The engine re-sends what is still automated (`StatusHandle::refresh_automation`).
    pub fn reset_automation(&mut self) {
        for strip in &mut self.strips {
            strip.volume = strip.default_volume;
            strip.pan = strip.default_pan;
        }
        for edge in &mut self.edges {
            edge.gain = edge.default_gain;
        }
        self.apply_insert_bases();
        self.events.clear();
    }

    /// Render owner: silence every tail (send effects, inserts, compensation delays).
    pub fn reset_signal(&mut self) {
        // SAFETY: render owner; one reference per cell at a time.
        unsafe {
            self.delay.get().clear();
            self.reverb.get().clear();
            for insert in &self.inserts {
                insert.cell.processor.get().reset();
                if let Some(delay) = &insert.bypass_delay {
                    let delay = delay.state.get();
                    delay.line.clear();
                    delay.quiet = usize::MAX / 2;
                }
            }
            for edge in &self.edges {
                if let Some(delay) = &edge.delay {
                    let delay = delay.state.get();
                    delay.line.clear();
                    delay.quiet = usize::MAX / 2;
                }
            }
            for op in &self.ops {
                if let Op::Insert { delay: Some(delay), .. } = op {
                    let delay = delay.state.get();
                    delay.line.clear();
                    delay.quiet = usize::MAX / 2;
                }
            }
        }
    }

    /// Render owner: a parameter value from the control thread (a knob being turned), applied at
    /// the start of the current block and kept as the insert's base value.
    pub fn live_parameter(&mut self, target: u32, address: u32, value: f32, now: u64) {
        if let Some(insert) = self.inserts.iter_mut().find(|insert| insert.target == target) {
            if let Some(entry) = insert.base.iter_mut().find(|(known, _)| *known == address as u64) {
                entry.1 = value as f64;
            }
            self.events.schedule(now, target, address, value);
        }
    }

    /// Starts the block `start..start + frames` for automation.
    pub fn begin_block(&mut self, start: u64, frames: usize) {
        self.events.begin_block(start, frames);
        self.block_start = start;
    }

    /// Whether the plan stamps what it captures with the song position (it carries a tap), so
    /// that the renderer asks the engine for it.
    pub fn wants_song_frame(&self) -> bool {
        !self.strip_taps.is_empty()
    }

    /// The song frame the engine rendered at the start of the block about to be processed, or
    /// [`NOT_PLAYING`].
    pub fn set_song_frame(&mut self, song_frame: i64) {
        self.song_frame = song_frame;
    }

    /// The transport and render cycle of the block about to be processed (for plug-ins).
    pub fn set_block(&mut self, block: BlockInfo) {
        self.block = block;
    }

    /// Files one engine event of the block that starts at `start`. Events numbered for another
    /// destination map are ignored (the engine re-sends its lanes once it has the project that
    /// matches this plan).
    pub fn engine_event(&mut self, event: &STEvent, start: u64) {
        let time = start + event.frame_offset.max(0) as u64;
        let mut schedule = |target: u32, param: u32, value: f64| {
            if let Some(offset) = self.target_offsets.get(target as usize) {
                self.events.schedule(time + offset, target, param, value as f32);
            }
        };
        match event.type_ {
            // Notes of a track whose instrument is a plug-in go to that plug-in, at their frame.
            ST_EVENT_NOTE_ON | ST_EVENT_NOTE_OFF | ST_EVENT_ALL_NOTES_OFF => {
                let Some(insert) = self.track_targets.get(event.track.max(0) as usize).filter(|_| event.track >= 0).and_then(|targets| self.inserts.get(targets.instrument as usize)) else { return };
                let (note, velocity) = (event.note.clamp(0, 127) as u8, event.velocity.clamp(0, 127) as u8);
                let bytes = match event.type_ {
                    ST_EVENT_NOTE_ON if velocity > 0 => [0x90, note, velocity],
                    ST_EVENT_ALL_NOTES_OFF => [0xB0, 123, 0],
                    _ => [0x80, note, 0],
                };
                // SAFETY: render owner; no other reference to this processor exists here.
                unsafe { insert.cell.processor.get() }.midi(event.frame_offset.max(0) as u32, bytes, event.slot != 0);
            }
            ST_EVENT_PARAMETER => {
                let Some(targets) = self.track_targets.get(event.track.max(0) as usize).filter(|_| event.track >= 0) else { return };
                match event.target {
                    ST_TARGET_VOLUME => schedule(targets.strip, PARAM_VOLUME, event.value.clamp(0.0, 2.0)),
                    ST_TARGET_PAN => schedule(targets.strip, PARAM_PAN, event.value.clamp(-1.0, 1.0)),
                    ST_TARGET_DELAY => schedule(targets.delay_send, PARAM_GAIN, event.value.clamp(0.0, 1.0)),
                    ST_TARGET_REVERB => schedule(targets.reverb_send, PARAM_GAIN, event.value.clamp(0.0, 1.0)),
                    ST_TARGET_PLUGIN if (0..INSERT_SLOTS as i32).contains(&event.slot) && event.address <= u32::MAX as u64 => {
                        schedule(targets.inserts[event.slot as usize], event.address as u32, event.value.clamp(0.0, 1.0))
                    }
                    _ => {}
                }
            }
            ST_EVENT_HOST_PARAMETER if event.host_map == self.host_map => match self.destinations.get(event.target.max(0) as usize).filter(|_| event.target >= 0) {
                Some(DestinationTarget::Mix(strip)) if event.address == 0 => schedule(*strip, PARAM_VOLUME, event.value.clamp(0.0, 2.0)),
                Some(DestinationTarget::Mix(strip)) if event.address == 1 => schedule(*strip, PARAM_PAN, event.value.clamp(-1.0, 1.0)),
                Some(DestinationTarget::Route(target)) if event.address == 0 => schedule(*target, PARAM_GAIN, event.value.clamp(0.0, 2.0)),
                // A negative value is the project's "back to the base value" (`BASE_VALUE`).
                Some(DestinationTarget::Insert(target)) if event.address <= u32::MAX as u64 => schedule(*target, event.address as u32, if event.value < 0.0 { BASE_VALUE as f64 } else { event.value.min(1.0) }),
                _ => {}
            },
            _ => {}
        }
    }

    /// Renders `frames` frames (at most `max_frames`) from the engine block into `out`.
    pub fn process(&mut self, engine: &BlockView<'_>, frames: usize, out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = frames.min(self.max_frames).min(out_l.len()).min(out_r.len());
        let context = ProcessContext { sample_rate: self.sample_rate, tempo: self.bpm, block: self.block };
        let tap_block = TapBlock { time: self.block_start, frames: frames as u32, arrival: 0, song_frame: self.song_frame, silent: false };
        let Plan { ops, edges, strips, inserts, lines, strip_taps, buffers, scratch, events, meters, delay, reverb, effects, bpm, master_buffer, .. } = self;
        for op in ops.iter() {
            match op {
                Op::Sum { buffer, edges: range } => {
                    let mut dest = std::mem::take(&mut buffers[*buffer]);
                    dest.silent = true;
                    for edge in &mut edges[range.clone()] {
                        let source: Option<(&[f32], &[f32])> = match edge.source {
                            Source::EngineTrack(track) => engine.track(track),
                            Source::EngineReturn => Some(engine.return_bus()),
                            Source::Line(index) => {
                                // Each live input has one edge, so its feed is read once per block.
                                let line = &mut lines[index];
                                let heard = line.feed.read(&mut line.block.left[..frames], &mut line.block.right[..frames]);
                                heard.then_some((&line.block.left[..], &line.block.right[..]))
                            }
                            Source::Buffer(index) => {
                                let source = &buffers[index];
                                (!source.silent).then_some((&source.left[..], &source.right[..]))
                            }
                        };
                        let source = match (&edge.delay, source) {
                            (None, source) => source,
                            (Some(delay), source) => {
                                // SAFETY: render owner; this edge is the cell's only user in the plan.
                                let delay = unsafe { delay.state.get() };
                                match source {
                                    Some((left, right)) => {
                                        delay.line.process(&left[..frames], &right[..frames], &mut scratch.left[..frames], &mut scratch.right[..frames]);
                                        delay.quiet = 0;
                                        Some((&scratch.left[..], &scratch.right[..]))
                                    }
                                    None if delay.empty() => None,
                                    None => {
                                        delay.line.process_silence(&mut scratch.left[..frames], &mut scratch.right[..frames]);
                                        delay.quiet = delay.quiet.saturating_add(frames);
                                        Some((&scratch.left[..], &scratch.right[..]))
                                    }
                                }
                            }
                        };
                        let gate = if edge.open { 1.0 } else { 0.0 };
                        if edge.target != NO_TARGET && events.has_events(edge.target) {
                            for segment in events.segments(edge.target, frames) {
                                match segment {
                                    Segment::Event(event) => edge.gain = event.value,
                                    Segment::Run(from, to) => {
                                        if let Some(source) = source {
                                            mix_into(&mut dest, source, from, to, edge.gain * gate, frames);
                                        }
                                    }
                                }
                            }
                        } else if let Some(source) = source {
                            mix_into(&mut dest, source, 0, frames, edge.gain * gate, frames);
                        }
                    }
                    buffers[*buffer] = dest;
                }
                Op::Insert { buffer, copy_from, delay: chain_delay, insert, sidechain } => {
                    let mut work = std::mem::take(&mut buffers[*buffer]);
                    if let Some(from) = copy_from {
                        let source = &buffers[*from];
                        work.silent = source.silent;
                        if !source.silent {
                            work.left[..frames].copy_from_slice(&source.left[..frames]);
                            work.right[..frames].copy_from_slice(&source.right[..frames]);
                        }
                    }
                    if let Some(delay) = chain_delay {
                        // SAFETY: render owner; this op is the cell's only user in the plan.
                        delay_in_place(unsafe { delay.state.get() }, &mut work, scratch, frames);
                    }
                    let slot = &inserts[*insert];
                    // SAFETY: render owner; each insert cell appears once in a plan.
                    let processor = unsafe { slot.cell.processor.get() };
                    let key = sidechain.map(|index| &mut buffers[index]).map(|key| {
                        if key.silent {
                            key.left[..frames].fill(0.0);
                            key.right[..frames].fill(0.0);
                        }
                        (&key.left[..frames], &key.right[..frames])
                    });
                    if let Some(delay) = &slot.bypass_delay {
                        // SAFETY: render owner; this insert is the cell's only user in the plan.
                        delay_in_place(unsafe { delay.state.get() }, &mut work, scratch, frames);
                    }
                    let skip = slot.bypass_delay.is_some() || (work.silent && processor.is_idle());
                    if !skip && work.silent {
                        work.left[..frames].fill(0.0);
                        work.right[..frames].fill(0.0);
                        work.silent = false;
                    }
                    if processor.timed_parameters() {
                        // One call per block; the processor places the changes itself.
                        let mut at = 0u32;
                        for segment in events.segments(slot.target, frames) {
                            match segment {
                                Segment::Event(event) if event.value < 0.0 => {
                                    if let Some((address, base)) = slot.base.iter().find(|(address, _)| *address == event.param as u64) {
                                        processor.base_parameter_at(*address, *base, at);
                                    }
                                }
                                Segment::Event(event) => processor.parameter_at(event.param as u64, event.value as f64, at),
                                Segment::Run(_, to) => at = to as u32,
                            }
                        }
                        if !skip {
                            processor.process(&mut work.left[..frames], &mut work.right[..frames], key, &context);
                        }
                        buffers[*buffer] = work;
                        continue;
                    }
                    for segment in events.segments(slot.target, frames) {
                        match segment {
                            Segment::Event(event) if event.value < 0.0 => {
                                if let Some((address, base)) = slot.base.iter().find(|(address, _)| *address == event.param as u64) {
                                    processor.base_parameter(*address, *base);
                                }
                            }
                            Segment::Event(event) => processor.parameter(event.param as u64, event.value as f64),
                            Segment::Run(from, to) if !skip => {
                                processor.process(&mut work.left[from..to], &mut work.right[from..to], key.map(|(left, right)| (&left[from..to], &right[from..to])), &context);
                            }
                            Segment::Run(..) => {}
                        }
                    }
                    buffers[*buffer] = work;
                }
                Op::Fader { from, to, strip, tap } => {
                    if let Some(tap) = tap.and_then(|tap| strip_taps.get(tap)) {
                        let source = &buffers[*from];
                        tap.tap.capture(TapBlock { arrival: tap.arrival, ..tap_block }, (!source.silent).then_some((&source.left[..frames], &source.right[..frames])));
                    }
                    let mut post = std::mem::take(&mut buffers[*to]);
                    if from != to {
                        let source = &buffers[*from];
                        post.silent = source.silent;
                        if !source.silent {
                            post.left[..frames].copy_from_slice(&source.left[..frames]);
                            post.right[..frames].copy_from_slice(&source.right[..frames]);
                        }
                    }
                    let state = &mut strips[*strip];
                    let mut audible = false;
                    for segment in events.segments(*strip as u32, frames) {
                        match segment {
                            Segment::Event(event) if event.param == PARAM_VOLUME => state.volume = event.value,
                            Segment::Event(event) if event.param == PARAM_PAN => state.pan = event.value,
                            Segment::Event(_) => {}
                            Segment::Run(_, _) if post.silent => {}
                            Segment::Run(from, to) => {
                                let gain = if state.mute { 0.0 } else { state.volume };
                                let (left, right) = (&mut post.left[from..to], &mut post.right[from..to]);
                                if gain == 0.0 {
                                    left.fill(0.0);
                                    right.fill(0.0);
                                    continue;
                                }
                                audible = true;
                                let pan = state.pan.clamp(-1.0, 1.0);
                                let (left_factor, right_factor) = (if pan > 0.0 { 1.0 - pan } else { 1.0 }, if pan < 0.0 { 1.0 + pan } else { 1.0 });
                                // sample * gain, then * pan factor; unity factors are skipped.
                                if gain != 1.0 {
                                    left.iter_mut().chain(right.iter_mut()).for_each(|sample| *sample *= gain);
                                }
                                if left_factor != 1.0 {
                                    left.iter_mut().for_each(|sample| *sample *= left_factor);
                                }
                                if right_factor != 1.0 {
                                    right.iter_mut().for_each(|sample| *sample *= right_factor);
                                }
                            }
                        }
                    }
                    if audible {
                        meters.accumulate(*strip, block_peak(&post.left[..frames]), block_peak(&post.right[..frames]));
                    } else {
                        post.silent = true;
                    }
                    buffers[*to] = post;
                }
                Op::Delay { buffer } | Op::Reverb { buffer } => {
                    let wet = &mut buffers[*buffer];
                    // SAFETY: render owner; the two send effects appear once each in a plan.
                    let (delay, reverb) = unsafe { (delay.get(), reverb.get()) };
                    let is_delay = matches!(op, Op::Delay { .. });
                    if wet.silent && if is_delay { delay.is_idle() } else { reverb.is_idle() } {
                        continue;
                    }
                    if wet.silent {
                        wet.left[..frames].fill(0.0);
                        wet.right[..frames].fill(0.0);
                        wet.silent = false;
                    }
                    if is_delay {
                        delay.process(&mut wet.left[..frames], &mut wet.right[..frames], effects, *bpm);
                    } else {
                        reverb.process(&mut wet.left[..frames], &mut wet.right[..frames], effects);
                    }
                }
            }
        }
        let master = &buffers[*master_buffer];
        if master.silent {
            out_l[..frames].fill(0.0);
            out_r[..frames].fill(0.0);
        } else {
            out_l[..frames].copy_from_slice(&master.left[..frames]);
            out_r[..frames].copy_from_slice(&master.right[..frames]);
        }
    }
}

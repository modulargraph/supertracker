//! The audio half of an Audio Unit instance (macOS only): rendering through the unit's
//! render block on the instance's real-time thread.
//!
//! What runs on the real-time thread here only *invokes blocks* (C function calls): the
//! unit's render and schedule blocks, which the main thread fetched and retained before
//! allocating render resources, as `AUAudioUnit` documents. It sends no Objective-C
//! message, retains or releases nothing, creates no autorelease pool and allocates
//! nothing: every buffer is sized in `Plugin::activate`. The blocks this side supplies
//! (input pull, MIDI output, musical context, transport state) are created on the main
//! thread too and read plain memory shared through `Arc`s.

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use block2::{DynBlock, RcBlock};
use objc2::runtime::Bool;
use objc2_audio_toolbox::{
    kAudioUnitErr_NoConnection, kAudioUnitErr_TooManyFramesToProcess, AUAudioUnit, AUHostTransportStateFlags,
    AudioUnitRenderActionFlags,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};
use stpc_runtime::{
    Event, ProcessData, ProcessStatus, Processor, Transport, TRANSPORT_PLAYING, TRANSPORT_PPQ_VALID,
    TRANSPORT_TEMPO_VALID, TRANSPORT_TIME_SIG_VALID,
};

use super::events::{self, Scheduled, Slices, TransportTracker};
use super::layout::{self, ChannelMap, Source};
use super::objc::guard_quiet;

pub(super) type PullFn =
    dyn Fn(NonNull<AudioUnitRenderActionFlags>, NonNull<AudioTimeStamp>, u32, isize, NonNull<AudioBufferList>) -> i32;
pub(super) type RenderFn = dyn Fn(
    NonNull<AudioUnitRenderActionFlags>,
    NonNull<AudioTimeStamp>,
    u32,
    isize,
    NonNull<AudioBufferList>,
    *mut DynBlock<PullFn>,
) -> i32;
pub(super) type ScheduleParameterFn = dyn Fn(i64, u32, u64, f32);
pub(super) type ScheduleMidiFn = dyn Fn(i64, u8, isize, NonNull<u8>);
pub(super) type MidiOutFn = dyn Fn(i64, u8, isize, NonNull<u8>) -> i32;
pub(super) type MusicalContextFn = dyn Fn(*mut f64, *mut f64, *mut isize, *mut f64, *mut isize, *mut f64) -> Bool;
pub(super) type TransportStateFn = dyn Fn(*mut AUHostTransportStateFlags, *mut f64, *mut f64, *mut f64) -> Bool;

extern "C" {
    /// The host clock of audio time stamps (libSystem; a plain register read).
    fn mach_absolute_time() -> u64;
}

const BUFFERS_OFFSET: usize = std::mem::offset_of!(AudioBufferList, mBuffers);
const _: () = assert!(BUFFERS_OFFSET == 8 && std::mem::size_of::<AudioBuffer>() == 16);
const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();

// ---- host transport for the unit ----------------------------------------------------------

/// The transport of the block being rendered, published by the processor and read by the
/// unit through its musical-context and transport-state blocks (normally on the render
/// thread, from inside the render call; plain atomics make any other thread harmless).
#[derive(Default)]
pub(super) struct TransportShared {
    flags: AtomicU32,
    changed: AtomicBool,
    numerator: AtomicU32,
    denominator: AtomicU32,
    tempo: AtomicU64,
    sample_position: AtomicI64,
    position: AtomicU64,
    bar_start: AtomicU64,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    sample_rate: AtomicU64,
}

impl TransportShared {
    pub fn set_sample_rate(&self, sample_rate: f64) {
        self.sample_rate.store(sample_rate.to_bits(), Ordering::Relaxed);
    }

    fn publish(&self, transport: &Transport, changed: bool) {
        let relaxed = Ordering::Relaxed;
        self.flags.store(transport.flags, relaxed);
        self.changed.store(changed, relaxed);
        self.numerator.store(transport.time_sig_numerator as u32, relaxed);
        self.denominator.store(transport.time_sig_denominator as u32, relaxed);
        self.tempo.store(transport.tempo_bpm.to_bits(), relaxed);
        self.sample_position.store(transport.sample_position, relaxed);
        self.position.store(transport.ppq_position.to_bits(), relaxed);
        self.bar_start.store(transport.bar_start_ppq.to_bits(), relaxed);
        self.loop_start.store(transport.loop_start_ppq.to_bits(), relaxed);
        self.loop_end.store(transport.loop_end_ppq.to_bits(), relaxed);
    }

    /// `AUAudioUnit.musicalContextBlock`. Any of the out pointers may be null.
    pub fn musical_context_block(self: &Arc<Self>) -> RcBlock<MusicalContextFn> {
        let shared = self.clone();
        RcBlock::new(
            move |tempo: *mut f64,
                  numerator: *mut f64,
                  denominator: *mut isize,
                  beat: *mut f64,
                  to_next_beat: *mut isize,
                  downbeat: *mut f64| {
                let relaxed = Ordering::Relaxed;
                let load = |cell: &AtomicU64| f64::from_bits(cell.load(relaxed));
                let flags = shared.flags.load(relaxed);
                let finite = |value: f64, fallback: f64| if value.is_finite() { value } else { fallback };
                let bpm = if flags & TRANSPORT_TEMPO_VALID != 0 {
                    finite(load(&shared.tempo), 120.0).max(1.0)
                } else {
                    120.0
                };
                let (top, bottom) = if flags & TRANSPORT_TIME_SIG_VALID != 0 {
                    (shared.numerator.load(relaxed).max(1), shared.denominator.load(relaxed).max(1))
                } else {
                    (4, 4)
                };
                let valid = flags & TRANSPORT_PPQ_VALID != 0;
                let position = if valid { finite(load(&shared.position), 0.0) } else { 0.0 };
                let bar = if valid { finite(load(&shared.bar_start), 0.0) } else { 0.0 };
                // SAFETY: the unit passes null or valid pointers.
                unsafe {
                    if !tempo.is_null() {
                        *tempo = bpm;
                    }
                    if !numerator.is_null() {
                        *numerator = top as f64;
                    }
                    if !denominator.is_null() {
                        *denominator = bottom as isize;
                    }
                    if !beat.is_null() {
                        *beat = position;
                    }
                    if !to_next_beat.is_null() {
                        *to_next_beat = events::samples_to_next_beat(position, bpm, load(&shared.sample_rate)) as isize;
                    }
                    if !downbeat.is_null() {
                        *downbeat = bar;
                    }
                }
                Bool::YES
            },
        )
    }

    /// `AUAudioUnit.transportStateBlock`.
    pub fn transport_state_block(self: &Arc<Self>) -> RcBlock<TransportStateFn> {
        let shared = self.clone();
        RcBlock::new(
            move |flags: *mut AUHostTransportStateFlags,
                  sample: *mut f64,
                  cycle_start: *mut f64,
                  cycle_end: *mut f64| {
                let relaxed = Ordering::Relaxed;
                let load = |cell: &AtomicU64| {
                    let value = f64::from_bits(cell.load(relaxed));
                    if value.is_finite() {
                        value
                    } else {
                        0.0
                    }
                };
                // SAFETY: the unit passes null or valid pointers.
                unsafe {
                    if !flags.is_null() {
                        let bits = events::au_transport_flags(shared.flags.load(relaxed), shared.changed.load(relaxed));
                        *flags = AUHostTransportStateFlags(bits);
                    }
                    if !sample.is_null() {
                        *sample = shared.sample_position.load(relaxed) as f64;
                    }
                    if !cycle_start.is_null() {
                        *cycle_start = load(&shared.loop_start);
                    }
                    if !cycle_end.is_null() {
                        *cycle_end = load(&shared.loop_end);
                    }
                }
                Bool::YES
            },
        )
    }
}

// ---- MIDI from the unit -------------------------------------------------------------------

/// Collects the MIDI a unit emits through `MIDIOutputEventBlock` during a block.
pub(super) struct MidiOutShared {
    /// `pthread_self` of the thread that is inside `process` right now, else 0. Events
    /// from any other thread, or outside a block, are dropped: the list is not shared.
    thread: AtomicUsize,
    block_start: AtomicI64,
    frames: AtomicU32,
    count: Cell<usize>,
    overflow: Cell<bool>,
    events: Box<[UnsafeCell<Event>]>,
}

// SAFETY: `count`, `overflow` and `events` are only touched by the thread whose id is in
// `thread` (the real-time thread, between `begin` and `finish`).
unsafe impl Send for MidiOutShared {}
unsafe impl Sync for MidiOutShared {}

fn current_thread() -> usize {
    // SAFETY: no preconditions.
    unsafe { libc::pthread_self() as usize }
}

impl MidiOutShared {
    pub fn new(capacity: usize) -> Arc<MidiOutShared> {
        Arc::new(MidiOutShared {
            thread: AtomicUsize::new(0),
            block_start: AtomicI64::new(0),
            frames: AtomicU32::new(0),
            count: Cell::new(0),
            overflow: Cell::new(false),
            events: (0..capacity.max(1)).map(|_| UnsafeCell::new(Event::default())).collect(),
        })
    }

    /// `AUAudioUnit.MIDIOutputEventBlock`.
    pub fn block(self: &Arc<Self>) -> RcBlock<MidiOutFn> {
        let shared = self.clone();
        RcBlock::new(move |time: i64, cable: u8, length: isize, bytes: NonNull<u8>| -> i32 {
            if shared.thread.load(Ordering::Acquire) != current_thread() || length <= 0 {
                return 0;
            }
            // SAFETY: the unit passes `length` bytes; this is the collecting thread.
            unsafe {
                let mut rest = std::slice::from_raw_parts(bytes.as_ptr(), (length as usize).min(1024));
                let block_start = shared.block_start.load(Ordering::Relaxed);
                let frames = shared.frames.load(Ordering::Relaxed);
                // A packet may hold several complete messages.
                while let Some(len) = rest.first().and_then(|&status| events::midi_message_length(status)) {
                    if rest.len() < len {
                        break;
                    }
                    if let Some(event) = events::output_event(time, cable, &rest[..len], block_start, frames) {
                        let count = shared.count.get();
                        if count < shared.events.len() {
                            *shared.events[count].get() = event;
                            shared.count.set(count + 1);
                        } else {
                            shared.overflow.set(true);
                        }
                    }
                    rest = &rest[len..];
                }
            }
            0
        })
    }

    fn begin(&self, block_start: i64, frames: u32) {
        self.block_start.store(block_start, Ordering::Relaxed);
        self.frames.store(frames, Ordering::Relaxed);
        self.count.set(0);
        self.overflow.set(false);
        self.thread.store(current_thread(), Ordering::Release);
    }

    /// Ends collection and hands the block's events over in offset order.
    fn finish(&self, data: &mut ProcessData<'_>) {
        self.thread.store(0, Ordering::Release);
        let count = self.count.get().min(self.events.len());
        // SAFETY: collection has ended; this thread is the only user of the list.
        let events = unsafe { std::slice::from_raw_parts_mut(self.events[0].get(), count) };
        events::sort_by_offset(events);
        for event in events.iter() {
            data.push_event(*event);
        }
        if self.overflow.get() {
            // The collection list has the capacity of the block's out-event list, so that
            // list is full now: this push is refused, which is what makes the runtime
            // report "out events dropped".
            data.push_event(Event::midi(data.frames.saturating_sub(1) as u32, &[0xFE]));
        }
    }
}

// ---- input for the unit -------------------------------------------------------------------

struct InputBus {
    map: ChannelMap,
    /// The host's channel pointers for the slice being rendered (`map.host` of them).
    host: UnsafeCell<Vec<*const f32>>,
    /// `map.unit` channels of `max_frames` samples: where adapted channels are prepared
    /// when the unit wants pointers rather than a copy.
    staging: UnsafeCell<Vec<f32>>,
}

/// What the pull-input block reads. Written by the processor before each render call.
pub(super) struct PullShared {
    /// Indexed by the unit's input bus number.
    inputs: Vec<InputBus>,
    max_frames: usize,
    /// Frames of host input available in the current slice; 0 outside a render call.
    frames: Cell<usize>,
    /// Silence to point at when the unit asks for a bus this side has nothing for.
    spare: UnsafeCell<Vec<f32>>,
}

// SAFETY: the interior is only used by the thread that is inside `Processor::process`:
// the processor writes it, and the unit calls the pull block from within the render call
// on that same thread.
unsafe impl Send for PullShared {}
unsafe impl Sync for PullShared {}

impl PullShared {
    fn new(maps: &[ChannelMap], max_frames: usize) -> PullShared {
        PullShared {
            inputs: maps
                .iter()
                .map(|&map| InputBus {
                    map,
                    host: UnsafeCell::new(vec![std::ptr::null(); map.host]),
                    staging: UnsafeCell::new(vec![0.0; map.unit * max_frames]),
                })
                .collect(),
            max_frames,
            frames: Cell::new(0),
            spare: UnsafeCell::new(vec![0.0; max_frames]),
        }
    }

    /// Points the buses at the host's input for frames `start..start + frames` of the block.
    fn begin_slice(&self, data: &ProcessData<'_>, start: usize, frames: usize) {
        for (bus, input) in self.inputs.iter().enumerate() {
            // SAFETY: see the `Sync` note; nothing else reads `host` right now.
            let host = unsafe { &mut *input.host.get() };
            let pointers: &[*const f32] = if bus < data.input_bus_count() { data.input_ptrs(bus) } else { &[] };
            for (channel, slot) in host.iter_mut().enumerate() {
                // SAFETY: each channel holds at least `start + frames` samples.
                *slot = pointers.get(channel).map_or(std::ptr::null(), |pointer| unsafe { pointer.add(start) });
            }
        }
        self.frames.set(frames);
    }

    fn end_slice(&self) {
        self.frames.set(0);
    }

    /// The body of the pull-input block.
    ///
    /// # Safety
    /// `list` must be the buffer list the unit passes to its pull block.
    unsafe fn pull(&self, frame_count: u32, bus: isize, list: NonNull<AudioBufferList>) -> i32 {
        let wanted = frame_count as usize;
        if wanted > self.max_frames {
            return kAudioUnitErr_TooManyFramesToProcess;
        }
        let available = self.frames.get().min(wanted);
        let list = list.as_ptr();
        let count = (*list).mNumberBuffers as usize;
        let buffers = (list as *mut u8).add(BUFFERS_OFFSET) as *mut AudioBuffer;
        let input = usize::try_from(bus).ok().and_then(|bus| self.inputs.get(bus));
        for index in 0..count {
            let buffer = &mut *buffers.add(index);
            let source = input.map(|input| (input, input.map.input_source(index)));
            let host: &[*const f32] = match input {
                Some(input) => &*input.host.get(),
                None => &[],
            };
            if buffer.mData.is_null() {
                // The unit wants this side's memory. A channel that maps one to one can be
                // read in place from the host's buffer; anything else is prepared.
                let direct = match source {
                    Some((input, Source::Channel(channel))) if channel == index && input.map.input_is_direct(index) => {
                        host.get(channel).copied().filter(|pointer| !pointer.is_null() && available == wanted)
                    }
                    _ => None,
                };
                let pointer = match (direct, source) {
                    (Some(pointer), _) => pointer as *mut f32,
                    (None, Some((input, source))) if index < input.map.unit => {
                        let staging = (*input.staging.get()).as_mut_ptr().add(index * self.max_frames);
                        fill(staging, source, host, wanted, available);
                        staging
                    }
                    _ => {
                        let spare = (*self.spare.get()).as_mut_ptr();
                        std::ptr::write_bytes(spare, 0, wanted);
                        spare
                    }
                };
                buffer.mData = pointer as *mut c_void;
                buffer.mDataByteSize = (wanted * SAMPLE_BYTES) as u32;
            } else {
                // The unit supplied a buffer: copy into it (never more than it holds).
                let frames = wanted.min(buffer.mDataByteSize as usize / SAMPLE_BYTES);
                let source = source.map_or(Source::Silence, |(_, source)| source);
                fill(buffer.mData as *mut f32, source, host, frames, available.min(frames));
            }
        }
        // A bus the unit does not have according to its own bus array: say so. A bus that
        // merely has no host channels got silence above and is reported as fine (a unit
        // that fails its whole render over an unconnected sidechain would go silent).
        if input.is_none() {
            kAudioUnitErr_NoConnection
        } else {
            0
        }
    }

    /// The block handed to every render call.
    pub fn block(self: &Arc<Self>) -> RcBlock<PullFn> {
        let shared = self.clone();
        RcBlock::new(
            move |_flags: NonNull<AudioUnitRenderActionFlags>,
                  _timestamp: NonNull<AudioTimeStamp>,
                  frame_count: u32,
                  bus: isize,
                  list: NonNull<AudioBufferList>|
                  -> i32 {
                // SAFETY: called by the unit with its input buffer list.
                unsafe { shared.pull(frame_count, bus, list) }
            },
        )
    }
}

/// Writes `frames` samples to `destination`: the first `available` from `source`, the
/// rest silence.
///
/// # Safety
/// `destination` holds `frames` samples; the non-null host pointers `available` each.
unsafe fn fill(destination: *mut f32, source: Source, host: &[*const f32], frames: usize, available: usize) {
    let available = available.min(frames);
    match source {
        Source::Channel(channel) => match host.get(channel).copied().filter(|pointer| !pointer.is_null()) {
            // `copy`, not `copy_nonoverlapping`: a unit may hand back a pointer it was
            // given in an earlier cycle.
            Some(pointer) => std::ptr::copy(pointer, destination, available),
            None => std::ptr::write_bytes(destination, 0, available),
        },
        Source::Average => {
            std::ptr::write_bytes(destination, 0, available);
            let out = std::slice::from_raw_parts_mut(destination, available);
            let mut sources = 0;
            for &pointer in host.iter().filter(|pointer| !pointer.is_null()) {
                layout::accumulate(out, std::slice::from_raw_parts(pointer, available));
                sources += 1;
            }
            if sources > 1 {
                layout::scale(out, 1.0 / sources as f32);
            }
        }
        Source::Silence => std::ptr::write_bytes(destination, 0, available),
    }
    if frames > available {
        std::ptr::write_bytes(destination.add(available), 0, frames - available);
    }
}

// ---- the processor ------------------------------------------------------------------------

/// Counters the main thread reads to report render failures (the real-time thread cannot
/// log).
#[derive(Default)]
pub(super) struct RenderDiagnostics {
    pub failures: AtomicU32,
    pub last_status: AtomicI32,
}

struct OutputBus {
    /// The unit's (and the host's) output bus number.
    bus: usize,
    map: ChannelMap,
    /// `map.unit` channels of `max_frames` samples for channels that cannot render
    /// straight into the host's buffers.
    scratch: Vec<f32>,
    /// No slice of the current block produced audio on this bus.
    silent: bool,
}

/// Storage for an `AudioBufferList` with any number of buffers.
struct BufferList {
    words: Vec<u64>,
}

impl BufferList {
    fn new(max_buffers: usize) -> BufferList {
        BufferList { words: vec![0; 1 + 2 * max_buffers.max(1)] }
    }

    fn list(&mut self) -> *mut AudioBufferList {
        self.words.as_mut_ptr().cast()
    }

    /// # Safety
    /// `index` is below the capacity given to `new`.
    unsafe fn buffer(&mut self, index: usize) -> *mut AudioBuffer {
        (self.words.as_mut_ptr() as *mut u8).add(BUFFERS_OFFSET + index * std::mem::size_of::<AudioBuffer>()).cast()
    }
}

/// Everything `Plugin::activate` hands to the processor.
pub(super) struct ProcessorSetup {
    pub unit: *const AUAudioUnit,
    pub render: RcBlock<RenderFn>,
    pub schedule_parameter: Option<RcBlock<ScheduleParameterFn>>,
    pub schedule_midi: Option<RcBlock<ScheduleMidiFn>>,
    pub cables: u8,
    /// Time events as "immediate plus offset" instead of absolute sample times.
    pub immediate_events: bool,
    /// Sorted addresses of rampable parameters.
    pub rampable: Vec<u64>,
    /// One map per unit input bus.
    pub inputs: Vec<ChannelMap>,
    /// `(bus number, map)` of the output buses to render, bus 0 first.
    pub outputs: Vec<(usize, ChannelMap)>,
    pub max_frames: usize,
    /// The unit's `maximumFramesToRender` (blocks are rendered in slices of at most this).
    pub max_render: u32,
    pub transport: Arc<TransportShared>,
    pub midi_out: Option<Arc<MidiOutShared>>,
    pub diagnostics: Arc<RenderDiagnostics>,
}

pub(super) struct AuProcessor {
    unit: *const AUAudioUnit,
    render: RcBlock<RenderFn>,
    schedule_parameter: Option<RcBlock<ScheduleParameterFn>>,
    schedule_midi: Option<RcBlock<ScheduleMidiFn>>,
    pull: RcBlock<PullFn>,
    pull_shared: Arc<PullShared>,
    cables: u8,
    immediate_events: bool,
    rampable: Vec<u64>,
    outputs: Vec<OutputBus>,
    buffers: BufferList,
    max_frames: usize,
    max_render: u32,
    /// Sample time of the next frame to render: the clock of this instance's timestamps.
    sample_time: i64,
    tracker: TransportTracker,
    transport: Arc<TransportShared>,
    midi_out: Option<Arc<MidiOutShared>>,
    diagnostics: Arc<RenderDiagnostics>,
}

// SAFETY: the processor is used by one thread at a time (the runtime's gate). The blocks
// are only invoked, never retained or released, off the main thread: the processor is
// created in `Plugin::activate` and dropped in `Plugin::deactivate`, both on the main
// thread. `unit` is only messaged in `reset`, on the main thread, and the plug-in that
// owns it outlives the processor.
unsafe impl Send for AuProcessor {}

impl AuProcessor {
    pub fn new(setup: ProcessorSetup) -> AuProcessor {
        let pull_shared = Arc::new(PullShared::new(&setup.inputs, setup.max_frames));
        let max_buffers = setup.outputs.iter().map(|(_, map)| map.unit).max().unwrap_or(1);
        AuProcessor {
            unit: setup.unit,
            render: setup.render,
            schedule_parameter: setup.schedule_parameter,
            schedule_midi: setup.schedule_midi,
            pull: pull_shared.block(),
            pull_shared,
            cables: setup.cables,
            immediate_events: setup.immediate_events,
            rampable: setup.rampable,
            outputs: setup
                .outputs
                .into_iter()
                .map(|(bus, map)| OutputBus { bus, map, scratch: vec![0.0; map.unit * setup.max_frames], silent: true })
                .collect(),
            buffers: BufferList::new(max_buffers),
            max_frames: setup.max_frames,
            max_render: setup.max_render.max(1),
            sample_time: 0,
            tracker: TransportTracker::default(),
            transport: setup.transport,
            midi_out: setup.midi_out,
            diagnostics: setup.diagnostics,
        }
    }

    fn schedule(&self, event: &Event, block_start: i64, frames: u32, not_before: i64) {
        let target = events::Target {
            accepts_midi: self.schedule_midi.is_some(),
            cables: self.cables.max(1),
            rampable: &self.rampable,
        };
        // `not_before` is the time stamp of the render call that follows: an event is never
        // scheduled in the past.
        let when = |time: i64| {
            let time = time.max(not_before);
            if self.immediate_events {
                events::immediate_time(time, not_before)
            } else {
                time
            }
        };
        match events::translate(event, block_start, frames, &target) {
            Some(Scheduled::Parameter { time, ramp, address, value }) => {
                if let Some(block) = &self.schedule_parameter {
                    block.call((when(time), ramp, address, value));
                }
            }
            Some(Scheduled::Midi { time, cable, len, bytes }) => {
                if let Some(block) = &self.schedule_midi {
                    block.call((when(time), cable, len as isize, NonNull::from(&bytes[0])));
                }
            }
            None => {}
        }
    }
}

/// Renders one output bus for one slice.
///
/// # Safety
/// Real-time thread, inside `process`; `start + frames` is within the block.
#[allow(clippy::too_many_arguments)]
unsafe fn render_bus(
    render: &RcBlock<RenderFn>,
    pull: &RcBlock<PullFn>,
    buffers: &mut BufferList,
    output: &mut OutputBus,
    data: &ProcessData<'_>,
    timestamp: &AudioTimeStamp,
    start: usize,
    frames: usize,
    max_frames: usize,
) -> i32 {
    let map = output.map;
    let host: &[*mut f32] = if output.bus < data.output_bus_count() { data.output_ptrs(output.bus) } else { &[] };
    let host_channels = host.len().min(map.host);
    let bytes = (frames * SAMPLE_BYTES) as u32;

    let list = buffers.list();
    (*list).mNumberBuffers = map.unit as u32;
    for channel in 0..map.unit {
        let direct = host.get(channel).filter(|_| map.output_is_direct(channel) && channel < host_channels);
        let pointer = match direct {
            Some(pointer) => pointer.add(start),
            None => output.scratch.as_mut_ptr().add(channel * max_frames),
        };
        buffers.buffer(channel).write(AudioBuffer { mNumberChannels: 1, mDataByteSize: bytes, mData: pointer.cast() });
    }

    let mut flags = AudioUnitRenderActionFlags(0);
    let mut time = *timestamp;
    let status = render.call((
        NonNull::from(&mut flags),
        NonNull::from(&mut time),
        frames as u32,
        output.bus as isize,
        NonNull::new_unchecked(list),
        RcBlock::as_ptr(pull),
    ));
    if status != 0 {
        return status;
    }

    // The unit may have rendered somewhere else (in place in its input, or in its own
    // buffers) and replaced the pointers: take the audio from where it says it is.
    let returned = ((*list).mNumberBuffers as usize).min(map.unit);
    let produced = |buffers: &mut BufferList, channel: usize| -> (*const f32, usize) {
        if channel >= returned {
            return (std::ptr::null(), 0);
        }
        let buffer = buffers.buffer(channel).read();
        (buffer.mData as *const f32, frames.min(buffer.mDataByteSize as usize / SAMPLE_BYTES))
    };
    let silent = flags.contains(AudioUnitRenderActionFlags::UnitRenderAction_OutputIsSilence);
    for (channel, &pointer) in host.iter().enumerate().take(host_channels) {
        let destination = pointer.add(start);
        if silent {
            // The flag promises silence, not that the buffers were cleared.
            std::ptr::write_bytes(destination, 0, frames);
            continue;
        }
        match map.output_source(channel) {
            Source::Channel(unit_channel) => {
                let (source, valid) = produced(buffers, unit_channel);
                if source.is_null() {
                    std::ptr::write_bytes(destination, 0, frames);
                } else {
                    if !std::ptr::eq(source, destination) {
                        std::ptr::copy(source, destination, valid);
                    }
                    if valid < frames {
                        std::ptr::write_bytes(destination.add(valid), 0, frames - valid);
                    }
                }
            }
            Source::Average => {
                // The destination is never one of the unit's buffers here (no channel of
                // an averaged bus renders directly).
                std::ptr::write_bytes(destination, 0, frames);
                let out = std::slice::from_raw_parts_mut(destination, frames);
                for unit_channel in 0..map.unit {
                    let (source, valid) = produced(buffers, unit_channel);
                    if !source.is_null() {
                        layout::accumulate(&mut out[..valid], std::slice::from_raw_parts(source, valid));
                    }
                }
                if map.unit > 1 {
                    layout::scale(out, 1.0 / map.unit as f32);
                }
            }
            Source::Silence => {}
        }
    }
    if !silent {
        output.silent = false;
    }
    0
}

impl Processor for AuProcessor {
    fn process(&mut self, data: &mut ProcessData<'_>) -> ProcessStatus {
        let frames = data.frames;
        if frames == 0 {
            return ProcessStatus::Ok;
        }
        if frames > self.max_frames {
            return ProcessStatus::Failed;
        }
        let transport = data.transport;
        let playing = transport.flags & TRANSPORT_PLAYING != 0;
        let changed = self.tracker.advance(playing, transport.sample_position, frames as u32, data.discontinuity);
        self.transport.publish(&transport, changed);

        let block_start = self.sample_time;
        if let Some(midi_out) = &self.midi_out {
            midi_out.begin(block_start, frames as u32);
        }
        for output in &mut self.outputs {
            output.silent = true;
        }

        let mut failure = 0;
        let mut slices = Slices::new(frames as u32, self.max_render);
        while let Some(slice) = slices.next() {
            let (start, count) = (slice.start as usize, slice.frames as usize);
            let slice_time = block_start + start as i64;
            // Events of this slice, scheduled at their absolute sample times right
            // before the render call that covers them.
            for event in slices.take_events(data.events, slice) {
                self.schedule(event, block_start, frames as u32, slice_time);
            }
            self.pull_shared.begin_slice(data, start, count);
            // SAFETY: plain data; all-zero is a valid time stamp.
            let mut timestamp: AudioTimeStamp = unsafe { std::mem::zeroed() };
            timestamp.mSampleTime = slice_time as f64;
            // SAFETY: no preconditions; does not block.
            timestamp.mHostTime = unsafe { mach_absolute_time() };
            timestamp.mRateScalar = 1.0;
            timestamp.mFlags = AudioTimeStampFlags::SampleTimeValid | AudioTimeStampFlags::HostTimeValid;
            for output in &mut self.outputs {
                // SAFETY: real-time thread; the slice lies within the block; buffers were
                // sized for `max_frames` and for the widest bus in `new`.
                let status = unsafe {
                    render_bus(
                        &self.render,
                        &self.pull,
                        &mut self.buffers,
                        output,
                        data,
                        &timestamp,
                        start,
                        count,
                        self.max_frames,
                    )
                };
                if status != 0 {
                    failure = status;
                    break;
                }
            }
            self.pull_shared.end_slice();
            if failure != 0 {
                break;
            }
        }
        // The clock advances by the whole block even after a failure.
        self.sample_time = block_start + frames as i64;

        if let Some(midi_out) = &self.midi_out {
            midi_out.finish(data);
        }
        if failure != 0 {
            self.diagnostics.last_status.store(failure, Ordering::Relaxed);
            self.diagnostics.failures.fetch_add(1, Ordering::Relaxed);
            return ProcessStatus::Failed;
        }

        // Channels nothing was rendered to: buses the unit does not have, channels beyond
        // its width, and buses it declared silent for the whole block.
        let mut silence = 0u64;
        for bus in 0..data.output_bus_count() {
            let first = data.output_flat_index(bus);
            let rendered = self.outputs.iter().find(|output| output.bus == bus);
            for channel in 0..data.output_channels(bus) {
                let audible = rendered
                    .is_some_and(|output| !output.silent && output.map.output_source(channel) != Source::Silence);
                if !audible && first + channel < 64 {
                    silence |= 1 << (first + channel);
                }
            }
        }
        data.output_silence = silence;
        ProcessStatus::Ok
    }

    fn reset(&mut self) {
        // Main thread, with processing excluded.
        // SAFETY: the unit outlives the processor; `reset` is a plain message.
        guard_quiet("being reset", || unsafe { (*self.unit).reset() });
        self.tracker.reset();
    }
}

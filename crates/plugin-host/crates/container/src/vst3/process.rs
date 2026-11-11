//! The audio half of a VST3 instance: the block's parameter queues and event lists as COM
//! objects, the translation of host events and transport into VST3 terms, and the
//! [`Processor`] the runtime's real-time thread calls. Nothing here allocates after
//! activation.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use stpc_runtime::transport::monotonic_ns;
use stpc_runtime::{
    Event, HostHandle, Notification, ProcessData, ProcessStatus, Processor, Transport, EVENT_FLAG_SEQUENCED,
    EVENT_MIDI, EVENT_PARAM, TRANSPORT_LOOPING, TRANSPORT_PLAYING, TRANSPORT_PPQ_VALID, TRANSPORT_RECORDING,
    TRANSPORT_TEMPO_VALID, TRANSPORT_TIME_SIG_VALID,
};
use vst3::Steinberg::Vst::{
    self as vst, AudioBusBuffers, IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentTrait, IEventList,
    IEventListTrait, IParamValueQueue, IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait, ParamID,
    ParamValue, ProcessContext,
};
use vst3::Steinberg::{kInvalidArgument, kNotImplemented, kResultFalse, kResultOk, tresult};
use vst3::{Class, ComPtr, ComWrapper};

pub const NO_PARAMETER: u32 = vst::kNoParamId;
/// MIDI controller numbers of `IMidiMapping`: 0..=127 control changes, then channel
/// pressure and pitch bend.
pub const MIDI_CONTROLLERS: usize = 130;
const CONTROLLER_AFTERTOUCH: usize = 128;
const CONTROLLER_PITCH_BEND: usize = 129;

const K_REALTIME: i32 = vst::ProcessModes_::kRealtime as i32;
pub const K_OFFLINE: i32 = vst::ProcessModes_::kOffline as i32;
const K_SAMPLE_32: i32 = vst::SymbolicSampleSizes_::kSample32 as i32;

const NOTE_ON: u16 = vst::Event_::EventTypes_::kNoteOnEvent as u16;
const NOTE_OFF: u16 = vst::Event_::EventTypes_::kNoteOffEvent as u16;
const POLY_PRESSURE: u16 = vst::Event_::EventTypes_::kPolyPressureEvent as u16;
const LEGACY_MIDI_OUT: u16 = vst::Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
const IS_LIVE: u16 = vst::Event_::EventFlags_::kIsLive as u16;

// ---- lock-free single-producer single-consumer ring ---------------------------------------------

/// Carries `(parameter id, tag, value)` from one thread to another.
pub struct Ring {
    slots: Vec<(AtomicU64, AtomicU64)>,
    head: AtomicU32,
    tail: AtomicU32,
}

impl Ring {
    pub fn new(capacity: u32) -> Ring {
        Ring {
            slots: (0..capacity).map(|_| (AtomicU64::new(0), AtomicU64::new(0))).collect(),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    fn capacity(&self) -> u32 {
        self.slots.len() as u32
    }

    /// Producer side. False if the ring is full.
    pub fn push(&self, id: u32, tag: u32, value: f64) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail.wrapping_sub(self.head.load(Ordering::Acquire)) >= self.capacity() {
            return false;
        }
        let slot = &self.slots[(tail % self.capacity()) as usize];
        slot.0.store(id as u64 | (tag as u64) << 32, Ordering::Relaxed);
        slot.1.store(value.to_bits(), Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side: the oldest entry, left in place.
    pub fn peek(&self) -> Option<(u32, u32, f64)> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        let slot = &self.slots[(head % self.capacity()) as usize];
        let meta = slot.0.load(Ordering::Relaxed);
        Some((meta as u32, (meta >> 32) as u32, f64::from_bits(slot.1.load(Ordering::Relaxed))))
    }

    /// Consumer side: drops the entry `peek` returned.
    pub fn advance(&self) {
        self.head.store(self.head.load(Ordering::Relaxed).wrapping_add(1), Ordering::Release);
    }

    pub fn pop(&self) -> Option<(u32, u32, f64)> {
        let entry = self.peek()?;
        self.advance();
        Some(entry)
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }
}

// ---- what the main thread, the real-time thread and the component handler share ----------------

/// `to_main` tag: the controller must learn the value (host automation it did not see).
pub const SYNC_ONLY: u32 = 0;
/// `to_main` tag: the controller must learn the value and so must the host application
/// (the plug-in changed it: an output parameter, or MIDI mapped to a parameter).
pub const SYNC_NOTIFY: u32 = 1;

pub struct Shared {
    pub handle: HostHandle,
    pub main_thread: std::thread::ThreadId,
    /// Untimed parameter changes (host UI, plug-in editor) on their way to the processor.
    /// Produced on the main thread; consumed by whoever runs `process` (the real-time
    /// thread, or the main thread while processing is excluded). The tag is the epoch.
    pub to_processor: Ring,
    /// Entries pushed before the current epoch are stale (a state was loaded since).
    pub epoch: AtomicU32,
    /// Edits that could not go into `to_processor` directly: made on a foreign thread, or
    /// while the ring was full. Moved over by `Plugin::idle`.
    pub overflow: Mutex<Vec<(u32, f64)>>,
    /// Values the edit controller has to be told, from the real-time thread.
    pub to_main: Ring,
    pub sync_requested: AtomicBool,
    /// `IComponentHandler::restartComponent` flags not yet handled.
    pub restart_flags: AtomicU32,
    /// `kRealtime` or `kOffline`, for the blocks' `processMode`.
    pub process_mode: AtomicI32,
    /// Latency last reported to the host.
    pub latency: AtomicU32,
    /// Parameter id per MIDI channel and controller (`NO_PARAMETER`: not mapped).
    pub midi_map: Vec<AtomicU32>,
    /// The program change parameter and its step count (`NO_PARAMETER`: none).
    pub program_parameter: AtomicU32,
    pub program_steps: AtomicI32,
    /// Events and parameter points that did not fit into a block.
    pub dropped: AtomicU64,
}

impl Shared {
    pub fn new(handle: HostHandle) -> Shared {
        Shared {
            handle,
            main_thread: std::thread::current().id(),
            to_processor: Ring::new(4096),
            epoch: AtomicU32::new(0),
            overflow: Mutex::new(Vec::new()),
            to_main: Ring::new(4096),
            sync_requested: AtomicBool::new(false),
            restart_flags: AtomicU32::new(0),
            process_mode: AtomicI32::new(K_REALTIME),
            latency: AtomicU32::new(0),
            midi_map: (0..16 * MIDI_CONTROLLERS).map(|_| AtomicU32::new(NO_PARAMETER)).collect(),
            program_parameter: AtomicU32::new(NO_PARAMETER),
            program_steps: AtomicI32::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Main thread: queues a change for the processor. False if the ring is full.
    pub fn send_to_processor(&self, id: u32, value: f64) -> bool {
        self.to_processor.push(id, self.epoch.load(Ordering::Relaxed), value)
    }

    /// `IComponentHandler::performEdit`: the user moved a control in the plug-in's editor.
    pub fn edit(&self, id: u32, value: f64) {
        let on_main = std::thread::current().id() == self.main_thread;
        if !on_main || !self.send_to_processor(id, value) {
            self.overflow.lock().unwrap().push((id, value));
            self.handle.request_main_callback();
        }
        self.handle.notify(Notification::ParamValue { id: id as u64, value });
    }

    /// Anything waiting to be handed to the processor?
    pub fn has_pending(&self) -> bool {
        !self.to_processor.is_empty() || !self.overflow.lock().unwrap().is_empty()
    }

    /// Reads the processor's latency and reports it if it differs from the last report.
    ///
    /// # Safety
    /// Main thread, or with the processor otherwise idle.
    pub unsafe fn report_latency(&self, processor: &ComPtr<IAudioProcessor>) {
        let frames = processor.getLatencySamples();
        if self.latency.swap(frames, Ordering::AcqRel) != frames {
            self.handle.notify(Notification::LatencyChanged { frames });
        }
    }

    /// The parameter the plug-in maps a MIDI controller of `channel` to (`NO_PARAMETER`: none).
    pub fn mapped_parameter(&self, channel: u8, controller: usize) -> u32 {
        self.midi_map[channel as usize * MIDI_CONTROLLERS + controller].load(Ordering::Relaxed)
    }

    fn to_main(&self, id: u32, tag: u32, value: f64) {
        if !self.to_main.push(id, tag, value) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---- IParamValueQueue / IParameterChanges --------------------------------------------------------

/// Where the points of an input queue came from, i.e. who still has to hear of them.
const FROM_QUEUE: u8 = 0;
const FROM_HOST: u8 = 1;
const FROM_MIDI: u8 = 2;

struct QueueInner {
    id: u32,
    origin: u8,
    /// (sample offset, value), ascending offsets. Capacity fixed at creation.
    points: Vec<(i32, f64)>,
}

/// One parameter's points of a block.
pub struct ParamQueue {
    inner: UnsafeCell<QueueInner>,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl ParamQueue {
    #[allow(clippy::mut_from_ref)]
    fn inner(&self) -> &mut QueueInner {
        // SAFETY: a block's queues are used by one thread at a time: the thread running
        // `process`, and the plug-in only from inside that call.
        unsafe { &mut *self.inner.get() }
    }

    /// Inserts in offset order; a point at an existing offset replaces it. When the queue
    /// is full the last point is overwritten, so the final value always arrives.
    fn add(&self, offset: i32, value: f64) -> i32 {
        let points = &mut self.inner().points;
        let place = points.iter().position(|point| point.0 >= offset).unwrap_or(points.len());
        if place < points.len() && points[place].0 == offset {
            points[place].1 = value;
            place as i32
        } else if points.len() == points.capacity() {
            let last = points.len() - 1;
            points[last] = (offset.max(points[last].0), value);
            last as i32
        } else {
            points.insert(place, (offset, value));
            place as i32
        }
    }

    pub fn id(&self) -> u32 {
        self.inner().id
    }

    pub fn last(&self) -> Option<f64> {
        self.inner().points.last().map(|point| point.1)
    }
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.inner().id
    }

    unsafe fn getPointCount(&self) -> i32 {
        self.inner().points.len() as i32
    }

    unsafe fn getPoint(&self, index: i32, sample_offset: *mut i32, value: *mut ParamValue) -> tresult {
        match self.inner().points.get(index as usize) {
            Some(point) if index >= 0 && !sample_offset.is_null() && !value.is_null() => {
                *sample_offset = point.0;
                *value = point.1;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addPoint(&self, sample_offset: i32, value: ParamValue, index: *mut i32) -> tresult {
        let place = self.add(sample_offset, value);
        if !index.is_null() {
            *index = place;
        }
        kResultOk
    }
}

struct ChangesInner {
    queues: Vec<ComWrapper<ParamQueue>>,
    pointers: Vec<*mut IParamValueQueue>,
    used: usize,
}

/// The parameter changes of a block, in either direction. All storage is allocated up
/// front: `queues` parameters, `points` points each.
pub struct ParamChanges {
    inner: UnsafeCell<ChangesInner>,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl ParamChanges {
    pub fn new(queues: usize, points: usize) -> ComWrapper<ParamChanges> {
        let queues: Vec<ComWrapper<ParamQueue>> = (0..queues)
            .map(|_| {
                ComWrapper::new(ParamQueue {
                    inner: UnsafeCell::new(QueueInner {
                        id: 0,
                        origin: FROM_QUEUE,
                        points: Vec::with_capacity(points),
                    }),
                })
            })
            .collect();
        let pointers = queues.iter().map(|queue| queue.as_com_ref::<IParamValueQueue>().unwrap().as_ptr()).collect();
        ComWrapper::new(ParamChanges { inner: UnsafeCell::new(ChangesInner { queues, pointers, used: 0 }) })
    }

    #[allow(clippy::mut_from_ref)]
    fn inner(&self) -> &mut ChangesInner {
        // SAFETY: as for `ParamQueue::inner`.
        unsafe { &mut *self.inner.get() }
    }

    pub fn clear(&self) {
        self.inner().used = 0;
    }

    pub fn used(&self) -> usize {
        self.inner().used
    }

    pub fn queue(&self, index: usize) -> &ParamQueue {
        &self.inner().queues[index]
    }

    /// The queue of parameter `id`, started if this is its first point. `None`: more
    /// parameters changed in this block than there are queues.
    fn queue_for(&self, id: u32) -> Option<usize> {
        let inner = self.inner();
        if let Some(index) = inner.queues[..inner.used].iter().position(|queue| queue.inner().id == id) {
            return Some(index);
        }
        if inner.used == inner.queues.len() {
            return None;
        }
        let index = inner.used;
        let queue = inner.queues[index].inner();
        queue.id = id;
        queue.origin = FROM_QUEUE;
        queue.points.clear();
        inner.used += 1;
        Some(index)
    }

    fn add(&self, id: u32, offset: i32, value: f64, origin: u8) -> bool {
        match self.queue_for(id) {
            Some(index) => {
                let queue = self.queue(index);
                queue.add(offset, value);
                queue.inner().origin = queue.inner().origin.max(origin);
                true
            }
            None => false,
        }
    }

    pub fn as_ptr(this: &ComWrapper<ParamChanges>) -> *mut IParameterChanges {
        this.as_com_ref::<IParameterChanges>().unwrap().as_ptr()
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        self.inner().used as i32
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        let inner = self.inner();
        if index >= 0 && (index as usize) < inner.used {
            inner.pointers[index as usize]
        } else {
            std::ptr::null_mut()
        }
    }

    unsafe fn addParameterData(&self, id: *const ParamID, index: *mut i32) -> *mut IParamValueQueue {
        if id.is_null() {
            return std::ptr::null_mut();
        }
        match self.queue_for(*id) {
            Some(place) => {
                if !index.is_null() {
                    *index = place as i32;
                }
                self.inner().pointers[place]
            }
            None => std::ptr::null_mut(),
        }
    }
}

// ---- IEventList -----------------------------------------------------------------------------------

/// The events of a block, in either direction. Fixed capacity.
pub struct EventList {
    events: UnsafeCell<Vec<vst::Event>>,
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl EventList {
    pub fn new(capacity: usize) -> ComWrapper<EventList> {
        ComWrapper::new(EventList { events: UnsafeCell::new(Vec::with_capacity(capacity.max(1))) })
    }

    #[allow(clippy::mut_from_ref)]
    fn events(&self) -> &mut Vec<vst::Event> {
        // SAFETY: as for `ParamQueue::inner`.
        unsafe { &mut *self.events.get() }
    }

    pub fn clear(&self) {
        self.events().clear();
    }

    fn push(&self, event: vst::Event) -> bool {
        let events = self.events();
        if events.len() == events.capacity() {
            return false;
        }
        events.push(event);
        true
    }

    pub fn as_ptr(this: &ComWrapper<EventList>) -> *mut IEventList {
        this.as_com_ref::<IEventList>().unwrap().as_ptr()
    }
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> i32 {
        self.events().len() as i32
    }

    unsafe fn getEvent(&self, index: i32, event: *mut vst::Event) -> tresult {
        match self.events().get(index as usize) {
            Some(found) if index >= 0 && !event.is_null() => {
                *event = *found;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, event: *mut vst::Event) -> tresult {
        if event.is_null() {
            kInvalidArgument
        } else if self.push(*event) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

// ---- zero-sample parameter flush ---------------------------------------------------------------

/// The objects of a block without audio, with which pending parameter changes are handed
/// to a processor that is not being called by the real-time thread.
pub struct Flusher {
    changes: ComWrapper<ParamChanges>,
    out_changes: ComWrapper<ParamChanges>,
    events: ComWrapper<EventList>,
    out_events: ComWrapper<EventList>,
}

/// Moves pending untimed changes into a block's queues (at offset 0). Returns how many.
fn take_pending(shared: &Shared, changes: &ParamChanges) -> usize {
    let epoch = shared.epoch.load(Ordering::Acquire);
    let mut taken = 0;
    while let Some((id, tag, value)) = shared.to_processor.peek() {
        if tag == epoch {
            if !changes.add(id, 0, value, FROM_QUEUE) {
                break; // stays queued for the next block
            }
            taken += 1;
        }
        shared.to_processor.advance();
    }
    taken
}

impl Flusher {
    pub fn new(parameter_count: usize) -> Flusher {
        let queues = parameter_count.clamp(8, 512);
        Flusher {
            changes: ParamChanges::new(queues, 16),
            out_changes: ParamChanges::new(queues, 16),
            events: EventList::new(1),
            out_events: EventList::new(64),
        }
    }

    /// Delivers everything in `shared.to_processor` through `process` calls of zero
    /// samples (the VST3 parameter flush). Returns the output parameter values the
    /// plug-in produced meanwhile.
    ///
    /// # Safety
    /// The component must be active and processing, and nobody else may be calling
    /// `process` (main thread with processing excluded, or an instance without a
    /// processor).
    pub unsafe fn run(&self, processor: &ComPtr<IAudioProcessor>, shared: &Shared) -> Vec<(u32, f64)> {
        let mut produced = Vec::new();
        for _ in 0..64 {
            self.changes.clear();
            self.out_changes.clear();
            self.events.clear();
            self.out_events.clear();
            if take_pending(shared, &self.changes) == 0 {
                break;
            }
            let mut data = vst::ProcessData {
                processMode: shared.process_mode.load(Ordering::Relaxed),
                symbolicSampleSize: K_SAMPLE_32,
                numSamples: 0,
                numInputs: 0,
                numOutputs: 0,
                inputs: std::ptr::null_mut(),
                outputs: std::ptr::null_mut(),
                inputParameterChanges: ParamChanges::as_ptr(&self.changes),
                outputParameterChanges: ParamChanges::as_ptr(&self.out_changes),
                inputEvents: EventList::as_ptr(&self.events),
                outputEvents: EventList::as_ptr(&self.out_events),
                processContext: std::ptr::null_mut(),
            };
            processor.process(&mut data);
            for index in 0..self.out_changes.used() {
                let queue = self.out_changes.queue(index);
                if let Some(value) = queue.last() {
                    produced.push((queue.id(), value));
                }
            }
        }
        produced
    }
}

// ---- the processor ----------------------------------------------------------------------------

/// One audio bus of the plug-in and the host bus it is fed from / feeds.
#[derive(Clone, Debug)]
pub struct BusMap {
    /// Index of the host bus, if the host allotted one.
    pub host: Option<usize>,
    /// Channels of the plug-in's bus.
    pub channels: usize,
}

struct Bus {
    host: Option<usize>,
    pointers: Vec<*mut f32>,
    /// `channels * max_frames` samples for channels the host did not allot.
    scratch: Vec<f32>,
}

#[derive(Clone, Copy, Default)]
struct HeldNote {
    active: bool,
    id: i32,
    bus: i32,
}

pub struct ProcessorConfig {
    pub component: ComPtr<IComponent>,
    pub processor: ComPtr<IAudioProcessor>,
    pub shared: Arc<Shared>,
    pub sample_rate: f64,
    pub max_frames: usize,
    pub inputs: Vec<BusMap>,
    pub outputs: Vec<BusMap>,
    pub event_inputs: usize,
    pub parameter_count: usize,
    pub event_capacity: usize,
    pub out_event_capacity: usize,
}

pub struct Vst3Processor {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    shared: Arc<Shared>,
    sample_rate: f64,
    max_frames: usize,
    changes: ComWrapper<ParamChanges>,
    out_changes: ComWrapper<ParamChanges>,
    events: ComWrapper<EventList>,
    out_events: ComWrapper<EventList>,
    inputs: Vec<Bus>,
    outputs: Vec<Bus>,
    in_buffers: Vec<AudioBusBuffers>,
    out_buffers: Vec<AudioBusBuffers>,
    event_inputs: usize,
    held: Vec<HeldNote>,
    held_count: usize,
    /// Samples processed since activation, for `continousTimeSamples`.
    continuous: i64,
    /// A restart of the component failed; nothing can be processed until reactivation.
    broken: bool,
}

// SAFETY: the processor is used by one thread at a time (the runtime's gate). The COM
// pointers refer to the plug-in, which the `Vst3Plugin` keeps alive beyond the processor,
// and VST3 requires `process` to be callable from a thread other than the UI thread.
unsafe impl Send for Vst3Processor {}

fn empty_buffers() -> AudioBusBuffers {
    AudioBusBuffers {
        numChannels: 0,
        silenceFlags: 0,
        __field0: vst::AudioBusBuffers__type0 { channelBuffers32: std::ptr::null_mut() },
    }
}

impl Vst3Processor {
    pub fn new(config: ProcessorConfig) -> Vst3Processor {
        let buses = |maps: &[BusMap]| -> Vec<Bus> {
            maps.iter()
                .map(|map| Bus {
                    host: map.host,
                    pointers: vec![std::ptr::null_mut(); map.channels],
                    scratch: vec![0.0; map.channels * config.max_frames],
                })
                .collect()
        };
        let queues = config.parameter_count.clamp(8, 512);
        Vst3Processor {
            inputs: buses(&config.inputs),
            outputs: buses(&config.outputs),
            in_buffers: vec![empty_buffers(); config.inputs.len()],
            out_buffers: vec![empty_buffers(); config.outputs.len()],
            changes: ParamChanges::new(queues, 128),
            out_changes: ParamChanges::new(queues, 16),
            // Room for the block's events plus a note-off for every held note.
            events: EventList::new(config.event_capacity + 256),
            out_events: EventList::new(config.out_event_capacity.max(16)),
            event_inputs: config.event_inputs,
            held: vec![HeldNote::default(); 16 * 128],
            held_count: 0,
            continuous: 0,
            broken: false,
            component: config.component,
            processor: config.processor,
            shared: config.shared,
            sample_rate: config.sample_rate,
            max_frames: config.max_frames,
        }
    }

    fn add_parameter(&mut self, id: u32, offset: u32, value: f64, origin: u8) {
        if id == NO_PARAMETER || !value.is_finite() {
            return;
        }
        if !self.changes.add(id, offset as i32, value.clamp(0.0, 1.0), origin) {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn add_event(&mut self, event: vst::Event) {
        if !self.events.push(event) {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Note-offs for the notes the plug-in was told to play and not yet to stop.
    fn release_held(&mut self, offset: u32, channel: Option<u8>) {
        if self.held_count == 0 {
            return;
        }
        for index in 0..self.held.len() {
            let note = self.held[index];
            let (note_channel, pitch) = ((index / 128) as u8, (index % 128) as i16);
            if !note.active || channel.is_some_and(|channel| channel != note_channel) {
                continue;
            }
            // SAFETY: an all-zero Event is valid; the union field written matches the type.
            let mut event: vst::Event = unsafe { std::mem::zeroed() };
            event.busIndex = note.bus;
            event.sampleOffset = offset as i32;
            event.r#type = NOTE_OFF;
            event.__field0.noteOff =
                vst::NoteOffEvent { channel: note_channel as i16, pitch, velocity: 0.0, noteId: note.id, tuning: 0.0 };
            self.add_event(event);
            self.held[index].active = false;
            self.held_count -= 1;
        }
    }

    /// Translates one host event into VST3 events and parameter points.
    fn add_host_event(&mut self, event: &Event) {
        if event.kind == EVENT_PARAM {
            if event.id <= u32::MAX as u64 {
                self.add_parameter(event.id as u32, event.offset, event.value, FROM_HOST);
            }
            return;
        }
        if event.kind != EVENT_MIDI {
            return;
        }
        let bytes = event.midi_bytes();
        let Some(&status) = bytes.first() else { return };
        let (kind, channel) = (status & 0xF0, status & 0x0F);
        let data1 = bytes.get(1).copied().unwrap_or(0) & 0x7F;
        let data2 = bytes.get(2).copied().unwrap_or(0) & 0x7F;
        let note_id = if event.aux == u32::MAX { -1 } else { event.aux as i32 };
        let bus = if (event.port as usize) < self.event_inputs { event.port as i32 } else { 0 };
        // SAFETY: an all-zero Event is valid; each union field written matches the type set.
        let mut out: vst::Event = unsafe { std::mem::zeroed() };
        out.busIndex = bus;
        out.sampleOffset = event.offset as i32;
        out.flags = if event.flags & EVENT_FLAG_SEQUENCED != 0 { 0 } else { IS_LIVE };
        let slot = channel as usize * 128 + data1 as usize;
        match kind {
            0x90 if data2 > 0 => {
                if self.event_inputs == 0 {
                    return;
                }
                out.r#type = NOTE_ON;
                out.__field0.noteOn = vst::NoteOnEvent {
                    channel: channel as i16,
                    pitch: data1 as i16,
                    tuning: 0.0,
                    velocity: data2 as f32 / 127.0,
                    length: 0,
                    noteId: note_id,
                };
                if !self.held[slot].active {
                    self.held_count += 1;
                }
                self.held[slot] = HeldNote { active: true, id: note_id, bus };
                self.add_event(out);
            }
            0x80 | 0x90 => {
                if self.event_inputs == 0 {
                    return;
                }
                out.r#type = NOTE_OFF;
                out.__field0.noteOff = vst::NoteOffEvent {
                    channel: channel as i16,
                    pitch: data1 as i16,
                    velocity: data2 as f32 / 127.0,
                    noteId: note_id,
                    tuning: 0.0,
                };
                if self.held[slot].active {
                    self.held[slot].active = false;
                    self.held_count -= 1;
                }
                self.add_event(out);
            }
            0xA0 => {
                if self.event_inputs == 0 {
                    return;
                }
                out.r#type = POLY_PRESSURE;
                out.__field0.polyPressure = vst::PolyPressureEvent {
                    channel: channel as i16,
                    pitch: data1 as i16,
                    pressure: data2 as f32 / 127.0,
                    noteId: note_id,
                };
                self.add_event(out);
            }
            0xB0 => {
                // VST3 has no controller events: controllers reach the plug-in as the
                // parameters it maps them to.
                if data1 == 120 || data1 == 123 {
                    // All sound off / all notes off: not every plug-in maps these, so the
                    // notes it holds are released explicitly, and the sustain pedal with them.
                    self.release_held(event.offset, Some(channel));
                    let sustain = self.shared.mapped_parameter(channel, 64);
                    self.add_parameter(sustain, event.offset, 0.0, FROM_MIDI);
                }
                let parameter = self.shared.mapped_parameter(channel, data1 as usize);
                self.add_parameter(parameter, event.offset, data2 as f64 / 127.0, FROM_MIDI);
            }
            0xC0 => {
                let parameter = self.shared.program_parameter.load(Ordering::Relaxed);
                let steps = self.shared.program_steps.load(Ordering::Relaxed);
                if steps > 0 {
                    let value = (data1 as i32).min(steps) as f64 / steps as f64;
                    self.add_parameter(parameter, event.offset, value, FROM_MIDI);
                }
            }
            0xD0 => {
                let parameter = self.shared.mapped_parameter(channel, CONTROLLER_AFTERTOUCH);
                self.add_parameter(parameter, event.offset, data1 as f64 / 127.0, FROM_MIDI);
            }
            0xE0 => {
                let parameter = self.shared.mapped_parameter(channel, CONTROLLER_PITCH_BEND);
                let bend = data1 as u32 | (data2 as u32) << 7;
                self.add_parameter(parameter, event.offset, bend as f64 / 16383.0, FROM_MIDI);
            }
            _ => {}
        }
    }

    fn context(&self, transport: Option<&Transport>) -> ProcessContext {
        use vst::ProcessContext_::StatesAndFlags_ as f;
        // SAFETY: ProcessContext is plain data; all-zero is valid.
        let mut context: ProcessContext = unsafe { std::mem::zeroed() };
        context.sampleRate = self.sample_rate;
        context.tempo = 120.0;
        context.timeSigNumerator = 4;
        context.timeSigDenominator = 4;
        context.continousTimeSamples = self.continuous;
        context.systemTime = monotonic_ns() as i64;
        let mut state = f::kContTimeValid | f::kSystemTimeValid;
        if let Some(t) = transport {
            context.projectTimeSamples = t.sample_position;
            for (ours, theirs) in [(TRANSPORT_PLAYING, f::kPlaying), (TRANSPORT_RECORDING, f::kRecording)] {
                if t.flags & ours != 0 {
                    state |= theirs;
                }
            }
            if t.flags & TRANSPORT_TEMPO_VALID != 0 && t.tempo_bpm.is_finite() && t.tempo_bpm > 0.0 {
                context.tempo = t.tempo_bpm;
                state |= f::kTempoValid;
            }
            if t.flags & TRANSPORT_TIME_SIG_VALID != 0 && t.time_sig_numerator > 0 && t.time_sig_denominator > 0 {
                context.timeSigNumerator = t.time_sig_numerator as i32;
                context.timeSigDenominator = t.time_sig_denominator as i32;
                state |= f::kTimeSigValid;
            }
            let musical = |value: f64| value.is_finite() && value.abs() < 1e12;
            if t.flags & TRANSPORT_PPQ_VALID != 0 && musical(t.ppq_position) {
                context.projectTimeMusic = t.ppq_position;
                state |= f::kProjectTimeMusicValid;
                if musical(t.bar_start_ppq) && t.bar_start_ppq <= t.ppq_position {
                    context.barPositionMusic = t.bar_start_ppq;
                    state |= f::kBarPositionValid;
                }
                if musical(t.loop_start_ppq) && musical(t.loop_end_ppq) && t.loop_start_ppq < t.loop_end_ppq {
                    context.cycleStartMusic = t.loop_start_ppq;
                    context.cycleEndMusic = t.loop_end_ppq;
                    state |= f::kCycleValid;
                    if t.flags & TRANSPORT_LOOPING != 0 {
                        state |= f::kCycleActive;
                    }
                }
            }
        }
        context.state = state;
        context
    }

    /// Hands the prepared block to the plug-in.
    unsafe fn call(&mut self, frames: usize, transport: Option<&Transport>) -> bool {
        let mut context = self.context(transport);
        let mut data = vst::ProcessData {
            processMode: self.shared.process_mode.load(Ordering::Relaxed),
            symbolicSampleSize: K_SAMPLE_32,
            numSamples: frames as i32,
            numInputs: self.in_buffers.len() as i32,
            numOutputs: self.out_buffers.len() as i32,
            inputs: if self.in_buffers.is_empty() { std::ptr::null_mut() } else { self.in_buffers.as_mut_ptr() },
            outputs: if self.out_buffers.is_empty() { std::ptr::null_mut() } else { self.out_buffers.as_mut_ptr() },
            inputParameterChanges: ParamChanges::as_ptr(&self.changes),
            outputParameterChanges: ParamChanges::as_ptr(&self.out_changes),
            inputEvents: EventList::as_ptr(&self.events),
            outputEvents: EventList::as_ptr(&self.out_events),
            processContext: &mut context,
        };
        self.continuous += frames as i64;
        self.processor.process(&mut data) == kResultOk
    }

    /// Points every channel of every bus at scratch memory (a block nobody listens to).
    fn use_scratch(&mut self, frames: usize) {
        let max_frames = self.max_frames;
        for (buses, buffers, input) in
            [(&mut self.inputs, &mut self.in_buffers, true), (&mut self.outputs, &mut self.out_buffers, false)]
        {
            for (bus, buffer) in buses.iter_mut().zip(buffers.iter_mut()) {
                let channels = bus.pointers.len();
                for (channel, pointer) in bus.pointers.iter_mut().enumerate() {
                    let scratch = &mut bus.scratch[channel * max_frames..channel * max_frames + frames];
                    scratch.fill(0.0);
                    *pointer = scratch.as_mut_ptr();
                }
                *buffer = AudioBusBuffers {
                    numChannels: channels as i32,
                    silenceFlags: if input { silence_mask(channels) } else { 0 },
                    __field0: vst::AudioBusBuffers__type0 { channelBuffers32: bus.pointers.as_mut_ptr() },
                };
            }
        }
    }

    /// What the controller and the host application still have to hear of this block.
    fn publish(&mut self, data: Option<&mut ProcessData<'_>>) {
        let mut wake = false;
        for index in 0..self.changes.used() {
            let queue = self.changes.queue(index);
            let origin = queue.inner().origin;
            if let (true, Some(value)) = (origin != FROM_QUEUE, queue.last()) {
                self.shared.to_main(queue.id(), if origin == FROM_MIDI { SYNC_NOTIFY } else { SYNC_ONLY }, value);
                wake = true;
            }
        }
        for index in 0..self.out_changes.used() {
            let queue = self.out_changes.queue(index);
            if let Some(value) = queue.last() {
                self.shared.to_main(queue.id(), SYNC_NOTIFY, value);
                wake = true;
            }
        }
        if wake && !self.shared.sync_requested.swap(true, Ordering::AcqRel) {
            self.shared.handle.request_main_callback();
        }
        if let Some(data) = data {
            for event in self.out_events.events().iter() {
                if let Some(midi) = midi_from_event(event) {
                    data.push_event(midi);
                }
            }
        }
    }
}

fn silence_mask(channels: usize) -> u64 {
    if channels >= 64 {
        u64::MAX
    } else {
        (1u64 << channels) - 1
    }
}

/// An event from the plug-in as MIDI for the host, if it has a MIDI form.
fn midi_from_event(event: &vst::Event) -> Option<Event> {
    let offset = event.sampleOffset.max(0) as u32;
    let unit = |value: f32| (value.clamp(0.0, 1.0) * 127.0).round() as u8;
    // SAFETY: the union field read is the one the event type names.
    let mut midi = unsafe {
        match event.r#type {
            NOTE_ON => {
                let note = event.__field0.noteOn;
                let mut midi = Event::midi(
                    offset,
                    &[0x90 | (note.channel & 15) as u8, (note.pitch & 127) as u8, unit(note.velocity).max(1)],
                );
                midi.aux = note.noteId as u32;
                midi
            }
            NOTE_OFF => {
                let note = event.__field0.noteOff;
                let mut midi = Event::midi(
                    offset,
                    &[0x80 | (note.channel & 15) as u8, (note.pitch & 127) as u8, unit(note.velocity)],
                );
                midi.aux = note.noteId as u32;
                midi
            }
            POLY_PRESSURE => {
                let pressure = event.__field0.polyPressure;
                Event::midi(
                    offset,
                    &[0xA0 | (pressure.channel & 15) as u8, (pressure.pitch & 127) as u8, unit(pressure.pressure)],
                )
            }
            LEGACY_MIDI_OUT => {
                let cc = event.__field0.midiCCOut;
                let channel = (cc.channel & 15) as u8;
                let (value, value2) = ((cc.value & 127) as u8, (cc.value2 & 127) as u8);
                match cc.controlNumber {
                    number @ 0..=127 => Event::midi(offset, &[0xB0 | channel, number, value]),
                    128 => Event::midi(offset, &[0xD0 | channel, value]),
                    129 => Event::midi(offset, &[0xE0 | channel, value, value2]),
                    130 => Event::midi(offset, &[0xC0 | channel, value]),
                    131 => Event::midi(offset, &[0xA0 | channel, value, value2]),
                    _ => return None,
                }
            }
            _ => return None,
        }
    };
    midi.port = event.busIndex.clamp(0, 255) as u8;
    Some(midi)
}

impl Processor for Vst3Processor {
    fn process(&mut self, data: &mut ProcessData<'_>) -> ProcessStatus {
        let frames = data.frames;
        if frames > self.max_frames || self.broken {
            return ProcessStatus::Failed;
        }
        self.changes.clear();
        self.out_changes.clear();
        self.events.clear();
        self.out_events.clear();
        // Untimed changes from the main thread first (offset 0), then the block's events.
        take_pending(&self.shared, &self.changes);
        for index in 0..data.events.len() {
            let event = data.events[index];
            self.add_host_event(&event);
        }

        // Audio: region channels where the host allotted them, scratch elsewhere.
        let max_frames = self.max_frames;
        for (bus, buffer) in self.inputs.iter_mut().zip(self.in_buffers.iter_mut()) {
            let host_channels = bus.host.map_or(0, |host| data.input_channels(host));
            let mut silence = 0u64;
            for (channel, pointer) in bus.pointers.iter_mut().enumerate() {
                let bit = 1u64.checked_shl(channel as u32).unwrap_or(0);
                if channel < host_channels {
                    let host = bus.host.unwrap();
                    *pointer = data.input_ptrs(host)[channel] as *mut f32;
                    let flat = data.input_flat_index(host) + channel;
                    if flat < 64 && data.input_silence & (1 << flat) != 0 {
                        silence |= bit;
                    }
                } else {
                    // A plug-in may process in place: the silence is renewed for every block.
                    let scratch = &mut bus.scratch[channel * max_frames..channel * max_frames + frames];
                    scratch.fill(0.0);
                    *pointer = scratch.as_mut_ptr();
                    silence |= bit;
                }
            }
            *buffer = AudioBusBuffers {
                numChannels: bus.pointers.len() as i32,
                silenceFlags: silence,
                __field0: vst::AudioBusBuffers__type0 { channelBuffers32: bus.pointers.as_mut_ptr() },
            };
        }
        for (bus, buffer) in self.outputs.iter_mut().zip(self.out_buffers.iter_mut()) {
            let host_channels = bus.host.map_or(0, |host| data.output_channels(host));
            for (channel, pointer) in bus.pointers.iter_mut().enumerate() {
                *pointer = if channel < host_channels {
                    data.output_ptrs(bus.host.unwrap())[channel]
                } else {
                    // SAFETY: the scratch holds `max_frames` samples per channel.
                    unsafe { bus.scratch.as_mut_ptr().add(channel * max_frames) }
                };
            }
            *buffer = AudioBusBuffers {
                numChannels: bus.pointers.len() as i32,
                silenceFlags: 0,
                __field0: vst::AudioBusBuffers__type0 { channelBuffers32: bus.pointers.as_mut_ptr() },
            };
        }

        // SAFETY: the component is active and processing (`Vst3Plugin::activate`), this is
        // the only thread in `process`, and every buffer holds at least `frames` samples.
        let ok = unsafe { self.call(frames, Some(&data.transport)) };
        if !ok {
            return ProcessStatus::Failed;
        }
        // Channels the plug-in declares silent need not have been written by it.
        for (bus, buffer) in self.outputs.iter().zip(self.out_buffers.iter()) {
            let Some(host) = bus.host else { continue };
            let host_channels = data.output_channels(host).min(bus.pointers.len());
            for channel in 0..host_channels.min(64) {
                if buffer.silenceFlags & (1 << channel) != 0 {
                    data.output(host, channel).fill(0.0);
                    let flat = data.output_flat_index(host) + channel;
                    if flat < 64 {
                        data.output_silence |= 1 << flat;
                    }
                }
            }
        }
        self.publish(Some(data));
        ProcessStatus::Ok
    }

    /// Releases held notes (a one-sample block nobody hears: stopping the processing is
    /// not a MIDI panic for every plug-in), then restarts the component, which is how a
    /// VST3 plug-in is told to clear its delay lines and voices.
    fn reset(&mut self) {
        // SAFETY: main thread, with the real-time thread excluded; the component is active.
        unsafe {
            if self.held_count > 0 && !self.broken {
                self.changes.clear();
                self.out_changes.clear();
                self.events.clear();
                self.out_events.clear();
                self.release_held(0, None);
                self.use_scratch(1);
                self.call(1, None);
                self.publish(None);
            }
            self.processor.setProcessing(0);
            self.component.setActive(0);
            self.broken = self.component.setActive(1) != kResultOk;
            let started = self.processor.setProcessing(1);
            self.broken |= started != kResultOk && started != kNotImplemented;
            self.held.fill(HeldNote::default());
            self.held_count = 0;
            if !self.broken {
                self.shared.report_latency(&self.processor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_first_in_first_out_and_bounded() {
        let ring = Ring::new(4);
        assert!(ring.is_empty() && ring.pop().is_none());
        for index in 0..4 {
            assert!(ring.push(index, 7, index as f64 * 0.5));
        }
        assert!(!ring.push(9, 7, 0.0), "full");
        assert_eq!(ring.peek(), Some((0, 7, 0.0)));
        assert_eq!(ring.pop(), Some((0, 7, 0.0)));
        assert!(ring.push(4, 8, 2.0));
        let rest: Vec<_> = std::iter::from_fn(|| ring.pop()).collect();
        assert_eq!(rest, [(1, 7, 0.5), (2, 7, 1.0), (3, 7, 1.5), (4, 8, 2.0)]);
        assert!(ring.is_empty());
    }

    #[test]
    fn queues_keep_points_sorted_and_the_last_value_when_full() {
        let changes = ParamChanges::new(2, 3);
        assert!(changes.add(5, 10, 0.1, FROM_HOST));
        assert!(changes.add(5, 4, 0.2, FROM_QUEUE));
        assert!(changes.add(5, 10, 0.3, FROM_QUEUE), "same offset replaces");
        assert!(changes.add(9, 0, 1.0, FROM_MIDI));
        assert!(!changes.add(11, 0, 1.0, FROM_QUEUE), "no queue left");
        assert_eq!(changes.used(), 2);
        let queue = changes.queue(0);
        assert_eq!((queue.id(), queue.inner().origin), (5, FROM_HOST));
        assert_eq!(queue.inner().points, [(4, 0.2), (10, 0.3)]);
        changes.add(5, 20, 0.4, FROM_QUEUE);
        changes.add(5, 30, 0.5, FROM_QUEUE);
        assert_eq!(queue.inner().points, [(4, 0.2), (10, 0.3), (30, 0.5)], "full: the last point is overwritten");
        assert_eq!(queue.last(), Some(0.5));
        // The same through the COM interface, as a plug-in uses an output queue.
        unsafe {
            let raw = ParamChanges::as_ptr(&changes);
            let changes_ref = vst3::ComRef::from_raw(raw).unwrap();
            assert_eq!(changes_ref.getParameterCount(), 2);
            let queue = vst3::ComRef::from_raw(changes_ref.getParameterData(1)).unwrap();
            assert_eq!((queue.getParameterId(), queue.getPointCount()), (9, 1));
            let (mut offset, mut value) = (0, 0.0);
            assert_eq!(queue.getPoint(0, &mut offset, &mut value), kResultOk);
            assert_eq!((offset, value), (0, 1.0));
            assert_eq!(queue.getPoint(1, &mut offset, &mut value), kInvalidArgument);
            assert!(changes_ref.getParameterData(2).is_null());
        }
        changes.clear();
        assert_eq!(changes.used(), 0);
        assert!(changes.add(11, 0, 1.0, FROM_QUEUE));
        assert_eq!(changes.queue(0).inner().points, [(0, 1.0)], "a reused queue starts empty");
    }

    #[test]
    fn plugin_events_become_midi() {
        let mut event: vst::Event = unsafe { std::mem::zeroed() };
        event.sampleOffset = 17;
        event.busIndex = 1;
        event.r#type = NOTE_ON;
        event.__field0.noteOn =
            vst::NoteOnEvent { channel: 3, pitch: 60, tuning: 0.0, velocity: 0.001, length: 0, noteId: 42 };
        let midi = midi_from_event(&event).unwrap();
        assert_eq!((midi.offset, midi.port, midi.aux), (17, 1, 42));
        assert_eq!(midi.midi_bytes(), [0x93, 60, 1], "a note-on never has velocity 0");
        event.r#type = LEGACY_MIDI_OUT;
        event.__field0.midiCCOut =
            vst::LegacyMIDICCOutEvent { controlNumber: 129, channel: 2, value: 0x7F, value2: 0x3F };
        assert_eq!(midi_from_event(&event).unwrap().midi_bytes(), [0xE2, 0x7F, 0x3F]);
        event.__field0.midiCCOut = vst::LegacyMIDICCOutEvent { controlNumber: 7, channel: 0, value: 100, value2: 0 };
        assert_eq!(midi_from_event(&event).unwrap().midi_bytes(), [0xB0, 7, 100]);
        event.r#type = vst::Event_::EventTypes_::kDataEvent as u16;
        assert!(midi_from_event(&event).is_none());
    }
}

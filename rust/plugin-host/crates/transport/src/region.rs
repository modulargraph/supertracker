//! The per-instance shared-memory region.
//!
//! ```text
//! +--------------------------------------------------------------+ 0
//! | Header (4096 bytes)                                          |
//! |   static block    magic, version, sizes (host, written once) |
//! |   request block   req seq + block parameters   (host writes) |
//! |   response block  rsp seq + block results (container writes) |
//! |   queue indices   notification queue head/tail               |
//! +--------------------------------------------------------------+
//! | events        [Event; event_capacity]        host -> plug-in |
//! | out events    [Event; out_event_capacity]    plug-in -> host |
//! | notifications [Cell; queue_capacity]         plug-in -> host |
//! | input channels   in_channels  x max_frames f32 (64-aligned)  |
//! | output channels  out_channels x max_frames f32 (64-aligned)  |
//! +--------------------------------------------------------------+ total_size
//! ```
//!
//! Ownership follows the handshake: after the host stores a new `req.seq` the request
//! block, the event list and the input channels belong to the container until it stores the
//! same number into `rsp.seq`; then the response block, the out events and the output
//! channels belong to the host until the next request. The notification queue is
//! independent of the handshake (multi-producer in the container, single consumer in the
//! host, lock-free).
//!
//! The host never trusts the region for memory safety: it keeps its own [`Layout`] and
//! clamps every count it reads, because a crashing plug-in can scribble over the region.

use std::cell::UnsafeCell;
use std::io;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::shm::{self, Mapping};
use crate::{MAX_BUSES, MAX_CHANNELS, MAX_FRAMES_LIMIT, PROTOCOL_VERSION};

/// "STPCRGN" + layout revision.
pub const REGION_MAGIC: u64 = 0x5354_5043_5247_4E01;
pub const HEADER_SIZE: usize = 4096;

/// `rsp.seq` value the host writes itself once the container is dead, to release a waiter.
pub const SEQ_ABORT: u32 = u32::MAX;

// ---- events ---------------------------------------------------------------------------

pub const EVENT_MIDI: u8 = 1;
/// Parameter change; `value` is in the range reported by the parameter's metadata.
pub const EVENT_PARAM: u8 = 2;

/// `Event::flags`: the note comes from the sequencer (timeline), not from live input.
pub const EVENT_FLAG_SEQUENCED: u8 = 1;

/// One timed event of a block, in either direction. `offset` is the sample offset within
/// the block; lists are sorted by offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Event {
    pub offset: u32,
    pub kind: u8,
    /// MIDI: number of valid bytes in `data` (1..=3).
    pub len: u8,
    /// MIDI: port / cable.
    pub port: u8,
    pub flags: u8,
    /// Parameter id.
    pub id: u64,
    /// Parameter value.
    pub value: f64,
    /// MIDI bytes.
    pub data: [u8; 4],
    /// Parameter: ramp length in frames (0: jump). MIDI: note id, or `u32::MAX` for none.
    pub aux: u32,
}

const _: () = assert!(std::mem::size_of::<Event>() == 32);

impl Event {
    pub fn midi(offset: u32, bytes: &[u8]) -> Event {
        let mut data = [0u8; 4];
        let len = bytes.len().min(3);
        data[..len].copy_from_slice(&bytes[..len]);
        Event { offset, kind: EVENT_MIDI, len: len as u8, data, aux: u32::MAX, ..Event::default() }
    }
    pub fn note_on(offset: u32, channel: u8, note: u8, velocity: u8) -> Event {
        Event::midi(offset, &[0x90 | (channel & 15), note & 127, velocity & 127])
    }
    pub fn note_off(offset: u32, channel: u8, note: u8) -> Event {
        Event::midi(offset, &[0x80 | (channel & 15), note & 127, 0])
    }
    pub fn param(offset: u32, id: u64, value: f64) -> Event {
        Event { offset, kind: EVENT_PARAM, id, value, ..Event::default() }
    }
    pub fn midi_bytes(&self) -> &[u8] {
        &self.data[..(self.len as usize).min(3)]
    }
}

// ---- transport ------------------------------------------------------------------------

pub const TRANSPORT_PLAYING: u32 = 1;
pub const TRANSPORT_RECORDING: u32 = 2;
pub const TRANSPORT_LOOPING: u32 = 4;
pub const TRANSPORT_TEMPO_VALID: u32 = 8;
pub const TRANSPORT_TIME_SIG_VALID: u32 = 16;
pub const TRANSPORT_PPQ_VALID: u32 = 32;

/// Host transport at the start of a block.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Transport {
    pub flags: u32,
    pub time_sig_numerator: u16,
    pub time_sig_denominator: u16,
    pub tempo_bpm: f64,
    /// Project position of the first frame of the block, in samples.
    pub sample_position: i64,
    /// Musical position in quarter notes.
    pub ppq_position: f64,
    pub bar_start_ppq: f64,
    pub loop_start_ppq: f64,
    pub loop_end_ppq: f64,
}

// ---- block flags ----------------------------------------------------------------------

/// Request flag: offline (faster or slower than real time) rendering.
pub const REQ_OFFLINE: u32 = 1;
/// Request flag: blocks were dropped before this one (deadline misses); held notes and
/// envelopes may be stale.
pub const REQ_DISCONTINUITY: u32 = 2;

/// Response flag: the plug-in processed the block.
pub const RSP_PROCESSED: u32 = 1;
/// Response flag: the instance is not active; nothing was processed.
pub const RSP_INACTIVE: u32 = 2;
/// Response flag: a main-thread operation holds the instance; nothing was processed.
pub const RSP_BUSY: u32 = 4;
/// Response flag: the plug-in reported a processing error; outputs are silent.
pub const RSP_ERROR: u32 = 8;
/// Response flag: more out events were produced than fit.
pub const RSP_OUT_EVENTS_DROPPED: u32 = 16;

// ---- notifications --------------------------------------------------------------------

/// What the plug-in side reports through the notification queue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Notification {
    /// The user grabbed a control in the plug-in's editor.
    ParamGestureBegin {
        id: u64,
    },
    /// A parameter was changed by the plug-in itself (editor, MIDI learn, program change).
    ParamValue {
        id: u64,
        value: f64,
    },
    ParamGestureEnd {
        id: u64,
    },
    LatencyChanged {
        frames: u32,
    },
    TailChanged {
        frames: u64,
    },
    /// Parameter metadata and/or all values changed (preset load inside the editor).
    ParamsRescan,
    /// The state changed in a way no parameter reports.
    StateDirty,
    /// The user closed the editor window.
    EditorClosed,
    EditorResized {
        width: u32,
        height: u32,
    },
    /// The plug-in asks to be deactivated and activated again (e.g. to change its latency).
    RestartRequested,
}

impl Notification {
    fn pack(self) -> (u32, u64, u64, u64) {
        match self {
            Notification::ParamGestureBegin { id } => (1, id, 0, 0),
            Notification::ParamValue { id, value } => (2, id, value.to_bits(), 0),
            Notification::ParamGestureEnd { id } => (3, id, 0, 0),
            Notification::LatencyChanged { frames } => (4, 0, 0, frames as u64),
            Notification::TailChanged { frames } => (5, 0, 0, frames),
            Notification::ParamsRescan => (6, 0, 0, 0),
            Notification::StateDirty => (7, 0, 0, 0),
            Notification::EditorClosed => (8, 0, 0, 0),
            Notification::EditorResized { width, height } => (9, 0, 0, ((width as u64) << 32) | height as u64),
            Notification::RestartRequested => (10, 0, 0, 0),
        }
    }
    fn unpack(kind: u32, id: u64, value: u64, aux: u64) -> Option<Notification> {
        Some(match kind {
            1 => Notification::ParamGestureBegin { id },
            2 => Notification::ParamValue { id, value: f64::from_bits(value) },
            3 => Notification::ParamGestureEnd { id },
            4 => Notification::LatencyChanged { frames: aux.min(u32::MAX as u64) as u32 },
            5 => Notification::TailChanged { frames: aux },
            6 => Notification::ParamsRescan,
            7 => Notification::StateDirty,
            8 => Notification::EditorClosed,
            9 => Notification::EditorResized { width: (aux >> 32) as u32, height: aux as u32 },
            10 => Notification::RestartRequested,
            _ => return None,
        })
    }
}

#[repr(C)]
struct Cell {
    seq: AtomicU32,
    kind: AtomicU32,
    id: AtomicU64,
    value: AtomicU64,
    aux: AtomicU64,
}
const _: () = assert!(std::mem::size_of::<Cell>() == 32);

// ---- header ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct StaticBlock {
    pub magic: AtomicU64,
    pub version: AtomicU32,
    pub header_size: AtomicU32,
    pub total_size: AtomicU64,
    pub max_frames: AtomicU32,
    pub in_channels: AtomicU32,
    pub out_channels: AtomicU32,
    pub event_capacity: AtomicU32,
    pub out_event_capacity: AtomicU32,
    pub queue_capacity: AtomicU32,
}

/// Written by the host before it publishes `seq`.
#[repr(C, align(64))]
pub struct RequestBlock {
    /// Block number; the container sleeps on this word.
    pub seq: AtomicU32,
    pub frames: AtomicU32,
    pub flags: AtomicU32,
    pub num_events: AtomicU32,
    /// Bit n set: input channel n is all zeros.
    pub input_silence: AtomicU64,
    transport: UnsafeCell<Transport>,
}

/// Written by the container before it publishes `seq`.
#[repr(C, align(64))]
pub struct ResponseBlock {
    /// Last completed block number; the host sleeps on this word.
    pub seq: AtomicU32,
    pub flags: AtomicU32,
    pub num_out_events: AtomicU32,
    _reserved: AtomicU32,
    /// Bit n set: output channel n is all zeros.
    pub output_silence: AtomicU64,
    /// Time the plug-in spent in its process call, in nanoseconds.
    pub process_ns: AtomicU64,
}

#[repr(C, align(64))]
struct QueueProducers {
    enqueue: AtomicU32,
    dropped: AtomicU32,
}

#[repr(C, align(64))]
struct QueueConsumer {
    dequeue: AtomicU32,
}

#[repr(C)]
pub struct Header {
    pub fixed: StaticBlock,
    pub req: RequestBlock,
    pub rsp: ResponseBlock,
    producers: QueueProducers,
    consumer: QueueConsumer,
}

const _: () = assert!(std::mem::size_of::<Header>() <= HEADER_SIZE);

// SAFETY: every field is atomic except `transport`, whose access follows the handshake.
unsafe impl Sync for Header {}

impl RequestBlock {
    /// Host side, before publishing `seq`.
    ///
    /// # Safety
    /// The caller must own the request block (no request in flight).
    pub unsafe fn set_transport(&self, transport: &Transport) {
        *self.transport.get() = *transport;
    }
    /// Container side, while processing the block.
    ///
    /// # Safety
    /// The caller must own the request block (request received, response not yet published).
    pub unsafe fn transport(&self) -> Transport {
        *self.transport.get()
    }
}

// ---- layout ---------------------------------------------------------------------------

/// What both sides need to agree on to lay a region out. Travels in `CreateInstance`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionConfig {
    pub max_frames: u32,
    /// Channel count of each input bus (bus 0 is the main input; later buses are sidechains).
    pub in_buses: Vec<u32>,
    pub out_buses: Vec<u32>,
    pub event_capacity: u32,
    pub out_event_capacity: u32,
    /// Notification queue capacity; a power of two.
    pub queue_capacity: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub total_size: usize,
    pub max_frames: usize,
    pub in_channels: usize,
    pub out_channels: usize,
    pub event_capacity: usize,
    pub out_event_capacity: usize,
    pub queue_capacity: usize,
    events_offset: usize,
    out_events_offset: usize,
    queue_offset: usize,
    inputs_offset: usize,
    outputs_offset: usize,
    channel_stride: usize,
}

#[derive(Debug)]
pub enum RegionError {
    Config(String),
    Io(io::Error),
    /// The mapped header does not describe the expected region (wrong version or layout).
    Mismatch(String),
}

impl std::fmt::Display for RegionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegionError::Config(msg) => write!(f, "invalid region configuration: {msg}"),
            RegionError::Io(err) => write!(f, "shared memory: {err}"),
            RegionError::Mismatch(msg) => write!(f, "shared region mismatch: {msg}"),
        }
    }
}
impl std::error::Error for RegionError {}

fn round_up(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

impl Layout {
    pub fn new(cfg: &RegionConfig) -> Result<Layout, RegionError> {
        let bad = |msg: &str| Err(RegionError::Config(msg.to_string()));
        if cfg.max_frames == 0 || cfg.max_frames > MAX_FRAMES_LIMIT {
            return bad("max_frames out of range");
        }
        if cfg.in_buses.len() > MAX_BUSES || cfg.out_buses.len() > MAX_BUSES {
            return bad("too many buses");
        }
        let in_channels: u64 = cfg.in_buses.iter().map(|&c| c as u64).sum();
        let out_channels: u64 = cfg.out_buses.iter().map(|&c| c as u64).sum();
        if in_channels > MAX_CHANNELS as u64 || out_channels > MAX_CHANNELS as u64 {
            return bad("too many channels");
        }
        if cfg.event_capacity > 1 << 20 || cfg.out_event_capacity > 1 << 20 {
            return bad("event capacity too large");
        }
        if !cfg.queue_capacity.is_power_of_two() || cfg.queue_capacity < 16 || cfg.queue_capacity > 1 << 20 {
            return bad("queue capacity must be a power of two in 16..=2^20");
        }
        let channel_stride = round_up(cfg.max_frames as usize * 4, 64);
        let events_offset = HEADER_SIZE;
        let out_events_offset = events_offset + round_up(cfg.event_capacity as usize * 32, 64);
        let queue_offset = out_events_offset + round_up(cfg.out_event_capacity as usize * 32, 64);
        let inputs_offset = queue_offset + cfg.queue_capacity as usize * 32;
        let outputs_offset = inputs_offset + in_channels as usize * channel_stride;
        let end = outputs_offset + out_channels as usize * channel_stride;
        Ok(Layout {
            // 16 KiB: the page size of Apple silicon; a multiple of the 4 KiB page elsewhere.
            total_size: round_up(end, 16384),
            max_frames: cfg.max_frames as usize,
            in_channels: in_channels as usize,
            out_channels: out_channels as usize,
            event_capacity: cfg.event_capacity as usize,
            out_event_capacity: cfg.out_event_capacity as usize,
            queue_capacity: cfg.queue_capacity as usize,
            events_offset,
            out_events_offset,
            queue_offset,
            inputs_offset,
            outputs_offset,
            channel_stride,
        })
    }
}

// ---- region ---------------------------------------------------------------------------

/// A mapped region. Both processes hold one over the same memory.
pub struct Region {
    map: Mapping,
    layout: Layout,
}

impl Region {
    /// Host side: creates the shared object, writes the static header and initialises the
    /// queue. The returned descriptor is sent to the container and can then be closed.
    pub fn create(cfg: &RegionConfig) -> Result<(Region, OwnedFd), RegionError> {
        let layout = Layout::new(cfg)?;
        let (fd, map) = shm::create(layout.total_size).map_err(RegionError::Io)?;
        let region = Region { map, layout };
        let fixed = &region.header().fixed;
        fixed.version.store(PROTOCOL_VERSION, Ordering::Relaxed);
        fixed.header_size.store(HEADER_SIZE as u32, Ordering::Relaxed);
        fixed.total_size.store(region.layout.total_size as u64, Ordering::Relaxed);
        fixed.max_frames.store(region.layout.max_frames as u32, Ordering::Relaxed);
        fixed.in_channels.store(region.layout.in_channels as u32, Ordering::Relaxed);
        fixed.out_channels.store(region.layout.out_channels as u32, Ordering::Relaxed);
        fixed.event_capacity.store(region.layout.event_capacity as u32, Ordering::Relaxed);
        fixed.out_event_capacity.store(region.layout.out_event_capacity as u32, Ordering::Relaxed);
        fixed.queue_capacity.store(region.layout.queue_capacity as u32, Ordering::Relaxed);
        for index in 0..region.layout.queue_capacity {
            region.cell(index).seq.store(index as u32, Ordering::Relaxed);
        }
        fixed.magic.store(REGION_MAGIC, Ordering::SeqCst);
        Ok((region, fd))
    }

    /// Container side: maps the object and checks that its header matches `cfg` and this
    /// build's protocol version.
    pub fn attach(fd: BorrowedFd<'_>, cfg: &RegionConfig) -> Result<Region, RegionError> {
        let layout = Layout::new(cfg)?;
        let map = shm::attach(fd, layout.total_size).map_err(RegionError::Io)?;
        let region = Region { map, layout };
        let fixed = &region.header().fixed;
        let mismatch = |what: &str, got: u64, want: u64| {
            Err(RegionError::Mismatch(format!("{what}: region has {got:#x}, this build expects {want:#x}")))
        };
        if fixed.magic.load(Ordering::SeqCst) != REGION_MAGIC {
            return mismatch("magic", fixed.magic.load(Ordering::SeqCst), REGION_MAGIC);
        }
        let version = fixed.version.load(Ordering::Relaxed);
        if version != PROTOCOL_VERSION {
            return mismatch("protocol version", version as u64, PROTOCOL_VERSION as u64);
        }
        let l = &region.layout;
        let checks: [(&str, u64, u64); 8] = [
            ("header size", fixed.header_size.load(Ordering::Relaxed) as u64, HEADER_SIZE as u64),
            ("total size", fixed.total_size.load(Ordering::Relaxed), l.total_size as u64),
            ("max frames", fixed.max_frames.load(Ordering::Relaxed) as u64, l.max_frames as u64),
            ("input channels", fixed.in_channels.load(Ordering::Relaxed) as u64, l.in_channels as u64),
            ("output channels", fixed.out_channels.load(Ordering::Relaxed) as u64, l.out_channels as u64),
            ("event capacity", fixed.event_capacity.load(Ordering::Relaxed) as u64, l.event_capacity as u64),
            (
                "out event capacity",
                fixed.out_event_capacity.load(Ordering::Relaxed) as u64,
                l.out_event_capacity as u64,
            ),
            ("queue capacity", fixed.queue_capacity.load(Ordering::Relaxed) as u64, l.queue_capacity as u64),
        ];
        for (what, got, want) in checks {
            if got != want {
                return mismatch(what, got, want);
            }
        }
        Ok(region)
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn header(&self) -> &Header {
        // SAFETY: the mapping is at least HEADER_SIZE bytes, page aligned, zero-initialised
        // (all-zero is a valid Header), and lives as long as `self`.
        unsafe { &*self.map.as_ptr().cast::<Header>() }
    }

    /// Start of input channel `channel` (`max_frames` samples). Panics if out of range.
    #[inline]
    pub fn input(&self, channel: usize) -> *mut f32 {
        assert!(channel < self.layout.in_channels);
        // SAFETY: inside the mapping by construction of the layout.
        unsafe { self.map.as_ptr().add(self.layout.inputs_offset + channel * self.layout.channel_stride).cast() }
    }

    /// Start of output channel `channel` (`max_frames` samples). Panics if out of range.
    #[inline]
    pub fn output(&self, channel: usize) -> *mut f32 {
        assert!(channel < self.layout.out_channels);
        // SAFETY: inside the mapping by construction of the layout.
        unsafe { self.map.as_ptr().add(self.layout.outputs_offset + channel * self.layout.channel_stride).cast() }
    }

    /// The host -> plug-in event list (`event_capacity` entries).
    #[inline]
    pub fn events(&self) -> *mut Event {
        // SAFETY: inside the mapping; the offset is 64-aligned.
        unsafe { self.map.as_ptr().add(self.layout.events_offset).cast() }
    }

    /// The plug-in -> host event list (`out_event_capacity` entries).
    #[inline]
    pub fn out_events(&self) -> *mut Event {
        // SAFETY: inside the mapping; the offset is 64-aligned.
        unsafe { self.map.as_ptr().add(self.layout.out_events_offset).cast() }
    }

    #[inline]
    fn cell(&self, index: usize) -> &Cell {
        debug_assert!(index < self.layout.queue_capacity);
        // SAFETY: inside the mapping; cells are all-atomic, so shared references are fine.
        unsafe { &*self.map.as_ptr().add(self.layout.queue_offset + index * 32).cast::<Cell>() }
    }

    /// Container side: appends a notification. Lock-free, real-time safe, callable from
    /// any thread. Returns false (and counts the drop) when the queue is full.
    pub fn push_notification(&self, notification: Notification) -> bool {
        let header = self.header();
        let mask = self.layout.queue_capacity as u32 - 1;
        let mut pos = header.producers.enqueue.load(Ordering::Relaxed);
        let cell = loop {
            let cell = self.cell((pos & mask) as usize);
            let seq = cell.seq.load(Ordering::Acquire);
            let diff = seq.wrapping_sub(pos) as i32;
            if diff == 0 {
                match header.producers.enqueue.compare_exchange_weak(
                    pos,
                    pos.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break cell,
                    Err(current) => pos = current,
                }
            } else if diff < 0 {
                header.producers.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                pos = header.producers.enqueue.load(Ordering::Relaxed);
            }
        };
        let (kind, id, value, aux) = notification.pack();
        cell.kind.store(kind, Ordering::Relaxed);
        cell.id.store(id, Ordering::Relaxed);
        cell.value.store(value, Ordering::Relaxed);
        cell.aux.store(aux, Ordering::Relaxed);
        cell.seq.store(pos.wrapping_add(1), Ordering::Release);
        true
    }

    /// Host side (one consumer thread): takes the next notification. Memory safe whatever
    /// the region contains; unknown records are skipped.
    pub fn pop_notification(&self) -> Option<Notification> {
        let header = self.header();
        let mask = self.layout.queue_capacity as u32 - 1;
        // At most one lap, so a corrupted queue cannot keep the consumer spinning.
        for _ in 0..self.layout.queue_capacity {
            let pos = header.consumer.dequeue.load(Ordering::Relaxed);
            let cell = self.cell((pos & mask) as usize);
            if cell.seq.load(Ordering::Acquire) != pos.wrapping_add(1) {
                return None;
            }
            header.consumer.dequeue.store(pos.wrapping_add(1), Ordering::Relaxed);
            let record = Notification::unpack(
                cell.kind.load(Ordering::Relaxed),
                cell.id.load(Ordering::Relaxed),
                cell.value.load(Ordering::Relaxed),
                cell.aux.load(Ordering::Relaxed),
            );
            cell.seq.store(pos.wrapping_add(mask).wrapping_add(1), Ordering::Release);
            if record.is_some() {
                return record;
            }
        }
        None
    }

    /// Number of notifications dropped because the queue was full.
    pub fn dropped_notifications(&self) -> u32 {
        self.header().producers.dropped.load(Ordering::Relaxed)
    }
}

/// Next block number after `seq`, skipping 0 (the initial value) and [`SEQ_ABORT`].
#[inline]
pub fn next_seq(seq: u32) -> u32 {
    match seq.wrapping_add(1) {
        0 | SEQ_ABORT => 1,
        next => next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;
    use std::sync::Arc;

    fn config() -> RegionConfig {
        RegionConfig {
            max_frames: 512,
            in_buses: vec![2, 2],
            out_buses: vec![2],
            event_capacity: 64,
            out_event_capacity: 16,
            queue_capacity: 64,
        }
    }

    #[test]
    fn layout_is_disjoint_and_aligned() {
        let layout = Layout::new(&config()).unwrap();
        assert_eq!(layout.in_channels, 4);
        assert_eq!(layout.total_size % 16384, 0);
        assert!(layout.outputs_offset + layout.out_channels * layout.channel_stride <= layout.total_size);
        assert_eq!(layout.inputs_offset % 64, 0);
        assert!(Layout::new(&RegionConfig { queue_capacity: 100, ..config() }).is_err());
        assert!(Layout::new(&RegionConfig { max_frames: 0, ..config() }).is_err());
        assert!(Layout::new(&RegionConfig { in_buses: vec![40, 40], ..config() }).is_err());
    }

    #[test]
    fn attach_checks_layout_and_version() {
        let (region, fd) = Region::create(&config()).unwrap();
        let other = Region::attach(fd.as_fd(), &config()).unwrap();
        // SAFETY: channel 3 exists and holds 512 samples in both mappings.
        unsafe {
            *region.input(3).add(511) = 0.25;
            assert_eq!(*other.input(3).add(511), 0.25);
        }
        let wrong = RegionConfig { out_event_capacity: 8, ..config() };
        assert!(matches!(Region::attach(fd.as_fd(), &wrong), Err(RegionError::Mismatch(_))));
        region.header().fixed.version.store(PROTOCOL_VERSION + 1, Ordering::Relaxed);
        let err = Region::attach(fd.as_fd(), &config()).err().unwrap();
        assert!(err.to_string().contains("protocol version"), "{err}");
    }

    #[test]
    fn notification_queue_round_trip_overflow_and_wrap() {
        let (region, _fd) = Region::create(&RegionConfig { queue_capacity: 16, ..config() }).unwrap();
        assert_eq!(region.pop_notification(), None);
        for lap in 0..10u64 {
            for i in 0..16u64 {
                assert!(region.push_notification(Notification::ParamValue { id: lap * 100 + i, value: i as f64 }));
            }
            assert!(!region.push_notification(Notification::StateDirty));
            for i in 0..16u64 {
                assert_eq!(
                    region.pop_notification(),
                    Some(Notification::ParamValue { id: lap * 100 + i, value: i as f64 })
                );
            }
            assert_eq!(region.pop_notification(), None);
        }
        assert_eq!(region.dropped_notifications(), 10);
        let all = [
            Notification::ParamGestureBegin { id: 3 },
            Notification::ParamGestureEnd { id: 3 },
            Notification::LatencyChanged { frames: 77 },
            Notification::TailChanged { frames: u64::MAX },
            Notification::ParamsRescan,
            Notification::StateDirty,
            Notification::EditorClosed,
            Notification::EditorResized { width: 640, height: 480 },
            Notification::RestartRequested,
        ];
        for n in all {
            assert!(region.push_notification(n));
            assert_eq!(region.pop_notification(), Some(n));
        }
    }

    #[test]
    fn notification_queue_many_producers() {
        let (region, _fd) = Region::create(&RegionConfig { queue_capacity: 1024, ..config() }).unwrap();
        let region = Arc::new(region);
        let producers: Vec<_> = (0..4u64)
            .map(|p| {
                let region = region.clone();
                std::thread::spawn(move || {
                    for i in 0..5000u64 {
                        while !region.push_notification(Notification::ParamValue { id: p, value: i as f64 }) {
                            std::thread::yield_now();
                        }
                    }
                })
            })
            .collect();
        let mut next = [0u64; 4];
        let mut received = 0;
        while received < 20000 {
            match region.pop_notification() {
                Some(Notification::ParamValue { id, value }) => {
                    assert_eq!(value as u64, next[id as usize], "per-producer order");
                    next[id as usize] += 1;
                    received += 1;
                }
                Some(other) => panic!("unexpected {other:?}"),
                None => std::thread::yield_now(),
            }
        }
        for p in producers {
            p.join().unwrap();
        }
    }

    #[test]
    fn sequence_numbers_skip_reserved_values() {
        assert_eq!(next_seq(0), 1);
        assert_eq!(next_seq(5), 6);
        assert_eq!(next_seq(u32::MAX - 1), 1);
        assert_eq!(next_seq(u32::MAX), 1);
    }
}
